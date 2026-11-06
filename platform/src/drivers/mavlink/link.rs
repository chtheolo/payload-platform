//! A MAVLink 2 connection to one camera over UDP.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mavlink::dialects::common::{
    COMMAND_LONG_DATA, HEARTBEAT_DATA, MavAutopilot, MavCmd, MavMessage, MavModeFlag,
    MavParamExtType, MavResult, MavState, MavType, PARAM_EXT_REQUEST_READ_DATA,
    PARAM_EXT_SET_DATA, ParamAck,
};
use mavlink::error::MessageReadError;
use mavlink::peek_reader::PeekReader;
use mavlink::{MavHeader, Message, read_any_msg, write_v2_msg};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep, timeout};
use tracing::{debug, warn};

/// Our own MAVLink address. We are on the same drone as the camera, so we use its system id;
/// component 191 is MAV_COMP_ID_ONBORAD_COMPUTER
const OUR_SYSTEM_ID: u8 = 1;
const OUR_COMPONENT_ID: u8 = 191;


const HEARTBEAT_PERIOD: Duration = Duration::from_secs(1);
/// How long one attempt waits for its answer, and how many attempts a repeatable request gets.
const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(500);
const ATTEMPTS: u8 = 3;

/// The camera's MAVLink address.
#[derive(Debug, Clone, Copy)]
pub struct CameraId {
    pub system_id: u8,
    pub component_id: u8,
}

/// A MAVLink command ready to send: which command, and its seven parameters.
// It's our own small description of on Mavlink command we want to send to the camera
#[derive(Debug, Clone, Copy)]
pub struct Command {
    /// which command - e.g. MAV_CMD_REQUEST_MESSAGE=512
    pub id: MavCmd,
    /// its 7 param numbers in every command
    /// 1 - id of the message - 259=CAMERA_INFORMATION
    /// 2-6 - extra options(which stream, which index) - 0.0 not used
    /// 7 - where to send the reply:0=default , 2=bcast - 0.0 reply back to us 
    pub params: [f32; 7],
    /// Whether sending it twice does no harm. Only such commands are resent when the ACK
    /// doesn't come: resending "zoom one step in" after a lost ACK could zoom in twice.
    pub repeatable: bool,
}

impl Command {
    /// A command that sets an absolute state, so it may be resent.
    pub fn repeatable(id: MavCmd, params: [f32; 7]) -> Command {
        Command {
            id,
            params,
            repeatable: true,
        }
    }

    /// A command that must be sent at most once.
    pub fn once(id: MavCmd, params: [f32; 7]) -> Command {
        Command {
            id,
            params,
            repeatable: false,
        }
    }
}


#[derive(Debug, Error)]
pub enum LinkError {
    #[error("no answer from the camera")]
    Timeout,
    #[error("the camera answered {0:?}")]
    Refused(MavResult),
}

pub struct Link {
    outbox: Outbox,
    camera: CameraId,
    /// Every message from the camera is published here
    incoming: broadcast::Sender<MavMessage>,
    /// when the camera was last heard, and the receive() task writes it
    /// one task writes, the receive(), and the other reads, the payload.core()
    last_heard: Arc<Mutex<Option<Instant>>>, 
    tasks: Vec<JoinHandle<()>>,
}

impl Link {
    /// Opens a UDP socket towards the camera and starts the two background tasks.
    // io::Result<Link> is a shorthand for Result<Link, std::io::Error>
    pub async fn open(address: SocketAddr, camera: CameraId) -> io::Result<Link> {
        let local = if address.is_ipv4() {"0.0.0.0:0"} else {"[::]:0"};
        let socket = UdpSocket::bind(local).await?;
        socket.connect(address).await?;

        let outbox = Outbox {
            socket: Arc::new(socket),
            sequence: Arc::new(AtomicU8::new(0)),
        };

        /// broadcasting the messages to every subscriber. 64 is the size of our buffer
        /// number of messages
        let (incoming, _) = broadcast::channel(64);
        let last_heard = Arc::new(Mutex::new(None));

        let tasks = vec![
            tokio::spawn(send_heartbeats(outbox.clone())),
            tokio::spawn(receive(
                outbox.socket.clone(),
                camera,
                incoming.clone(),
                last_heard.clone(),
            ))
        ];
        Ok(Link {
            outbox,
            camera,
            incoming,
            last_heard,
            tasks,
        })
    }

    /// How long ago anything arrived from the camera; `None` if nothing has arrived yet.
    pub fn last_heard(&self) -> Option<Duration> {
        // Rust: `lock()` fails only if another thread panicked while holding the lock.
        let last = *self.last_heard.lock().expect("last_heard lock poisoned");
        last.map(|instant| instant.elapsed())
    }

    pub async fn wait_for_heartbeat(&self, limit: Duration) -> Result<(), LinkError> {
        let mut messages = self.incoming.subscribe();
        let heartbeat = async {
            while let Some(message) = next(&mut messages).await {
                if matches!(message, MavMessage::HEARTBEAT(_)) {
                    return true;
                }
            }
            false
        };
        match timeout(limit, heartbeat).await {
            Ok(true) => Ok(()),
            _ => Err(LinkError::Timeout),
        }
    }

    /// Sends a command and returns the result from its COMMAND_ACK.
    ///
    /// A repeatable command gets up to `ATTEMPTS` tries. Each resend increments the
    /// `confirmation` field, as the MAVLink command protocol asks, so the camera can tell a
    /// resend from a new command.
    pub async fn command(&self, command: &Command) -> Result<MavResult, LinkError> {
        let attempts = if command.repeatable { ATTEMPTS } else { 1 };
        self.ask(
            attempts,
            |attempt| self.command_long(command, attempt),
            |incoming| match incoming {
                // IN_PROGRESS isn't the final answer; keep waiting for that.
                MavMessage::COMMAND_ACK(ack)
                    if ack.command == command.id
                        && ack.result != MavResult::MAV_RESULT_IN_PROGRESS =>
                {
                    Some(ack.result)
                }
                _ => None,
            },
        )
        .await
    }

    // This the function that we ask data from the camera 
    // cameras will not send their state, only heartbeats. So, if we want 
    // to know the zoom, the sensor or the camera's name, we have to ask.
    pub async fn request_message(
        &self,
        message_id: u32,
        count: usize,
    ) -> Result<Vec<MavMessage>, LinkError> {
        /**
         * MAV_CMD_REQUEST_MESSAGE (512) defines its 7 parameters like this in the MAVLink standard:
         * 1    |  ID of the message you want               | message_id(259=CAMERA_INFORMATION)
         * 2-6  | extra options(e.g. which streams)         | 0.0 not used
         * 7    | where to send reply: 0(default), 2(bcast) | 0.0 = us
         */
        let request = Command::repeatable(
            MavCmd::MAV_CMD_REQUEST_MESSAGE,
            [message_id as f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        for attempt in 0..ATTEMPTS {
            // fresh for every attempt, so answers to an earlier attempt not to be mixed 
            let mut found = Vec::new(); // the answers collected so far 
            let mut refused = None;     // set if the camera says "no"
            // pick: the filter that decides what incoming message means 
            let mut pick = |incoming: &MavMessage| {
                match incoming {
                    MavMessage::COMMAND_ACK(ack) if ack.command == request.id => {
                        // we only accept, just keep listening
                        let accepted = matches!(
                            ack.result,
                            MavResult::MAV_RESULT_ACCEPTED | MavResult::MAV_RESULT_IN_PROGRESS
                        );
                        // this means that the camera refused to answer us 
                        if !accepted {
                            refused = Some(ack.result);
                            return Some(());
                        }
                    }
                    message if message.message_id() == message_id => {
                        found.push(message.clone());
                        if found.len() == count {
                            return Some(());
                        }
                    }
                    _ => {} // ignore other messages, if e.g. it's a heartbeat we will fall here and ignore it 
                }
                None
            };
            self.attempt(&self.command_long(&request, attempt), &mut pick).await;
            if let Some(result) = refused {
                return Err(LinkError::Refused(result));
            }
            if found.len() == count {
                return Ok(found);
            }
        }
        Err(LinkError::Timeout)
    }

    /// Reads a uint8 camera parameter (PARAM_EXT_REQUEST_READ, answered by PARAM_EXT_VALUE).
    pub async fn read_param(&self, name: &str) -> Result<u8, LinkError> {
        let request = MavMessage::PARAM_EXT_REQUEST_READ(PARAM_EXT_REQUEST_READ_DATA {
            target_system: self.camera.system_id,
            target_component: self.camera.component_id,
            param_id: name.into(),
            // -1 means "look the parameter up by its name, not by an index".
            param_index: -1,
        });
        self.ask(
            ATTEMPTS,
            |_| request.clone(),
            |incoming| match incoming {
                MavMessage::PARAM_EXT_VALUE(value) if value.param_id.to_str() == Ok(name) => {
                    // A uint8 value travels in byte 0 of `param_value`.
                    Some(value.param_value[0])
                }
                _ => None,
            },
        )
        .await
    }

    /// Sets a uint8 camera parameter (PARAM_EXT_SET, answered by PARAM_EXT_ACK). Returns the
    /// camera's verdict and the value in effect afterwards, which the ACK always carries.
    pub async fn set_param(&self, name: &str, value: u8) -> Result<(ParamAck, u8), LinkError> {
        let mut bytes = [0u8; 128];
        bytes[0] = value;
        let request = MavMessage::PARAM_EXT_SET(PARAM_EXT_SET_DATA {
            target_system: self.camera.system_id,
            target_component: self.camera.component_id,
            param_id: name.into(),
            param_value: bytes.into(),
            param_type: MavParamExtType::MAV_PARAM_EXT_TYPE_UINT8,
        });
        // Setting a value can be repeated: setting it twice leaves the same state.
        self.ask(
            ATTEMPTS,
            |_| request.clone(),
            |incoming| match incoming {
                MavMessage::PARAM_EXT_ACK(ack)
                    if ack.param_id.to_str() == Ok(name)
                        && ack.param_result != ParamAck::PARAM_ACK_IN_PROGRESS =>
                {
                    Some((ack.param_result, ack.param_value[0]))
                }
                _ => None,
            },
        )
        .await
    }

    /// Up to `attempts` tries of: send this attempt's message, then wait for an answer.
    ///
    /// Rust: `impl Fn(u8) -> MavMessage` and `impl FnMut(&MavMessage) -> Option<T>` accept
    /// closures. `FnMut` means the closure may change variables it has captured.
    async fn ask<T>(
        &self,
        attempts: u8,
        message_for: impl Fn(u8) -> MavMessage,
        mut pick: impl FnMut(&MavMessage) -> Option<T>,
    ) -> Result<T, LinkError> {
        for attempt in 0..attempts {
            if let Some(answer) = self.attempt(&message_for(attempt), &mut pick).await {
                return Ok(answer);
            }
        }
        Err(LinkError::Timeout)
    }

    async fn attempt<T>(
        &self,
        message: &MavMessage,
        pick: &mut impl FnMut(&MavMessage) -> Option<T>,
    ) -> Option<T> {
        // subscribe before sending, avoiding missing quick responses
        let mut messages = self.incoming.subscribe();
        self.outbox.send(message).await;
        let answer = async {
            while let Some(incoming) = next(&mut messages).await {
                if let Some(answer) = pick(&incoming) {
                    return Some(answer);
                }
            }
            None
        };
        // from the answer() we return Option<T>, but timeout adds another layer
        // so we get Result<Option<T>, Elapsed>.
        // ok() turns the result into an Option. so Option<Option<T>>, 
        // flatten() will give us the Option<T>(merges) - Some(value) or None
        timeout(ATTEMPT_TIMEOUT, answer).await.ok().flatten()
    }

    /**
     * platform     | ---COMMAND_LONG(512,param1=259)---> | send me camera info
     */
    /// fun to create a command long MavMessage
    fn command_long(&self, command: &Command, confirmation: u8) -> MavMessage {
        // destructure in the 7 params
        let [param1, param2, param3, param4, param5, param6, param7] = command.params;
        MavMessage::COMMAND_LONG(COMMAND_LONG_DATA {
            param1,
            param2,
            param3,
            param4,
            param5,
            param6,
            param7,
            command: command.id,
            target_system: self.camera.system_id,
            target_component: self.camera.component_id,
            confirmation,
        })
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The sending side of the link, shared with the heartbeat task.
/// This struct basically holds our tools of how we send data to the camera
/// 1st is the socket: UDP connection to the camera
/// 2nd is the sequence counter: a number MacLink puts in every message's header.
/// Rust: `Arc` lets several tasks own the same socket. `AtomicU8` is a number several tasks
/// can increment safely without a lock.
#[derive(Clone)]
struct Outbox {
    socket: Arc<UdpSocket>,
    sequence: Arc<AtomicU8>,
}

impl Outbox {
    /// Encodes `message` as one Mavlink frame and we send it.
    async fn send(&self, message: &MavMessage) {
        let header = MavHeader {
            system_id: OUR_SYSTEM_ID,
            component_id: OUR_COMPONENT_ID,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
        };
        // empty byte buffer
        let mut datagram = Vec::new();
        // write_v2_msg() turns a message into bytes: it encodes one MAVLink message as a MAVLink 2 frame
        // the exact layout the camera expects
        if let Err(error) = write_v2_msg(&mut datagram, header, message) {
            warn!("can't encode {}: {error}", message.message_name());
            return;
        }

        if let Err(error) = self.socket.send(&datagram).await {
            // "Connection refused" mneas that the camera might not running 
            debug!("sending {} failed: {error}", message.message_name());
        }
    }
}

async fn send_heartbeats(outbox: Outbox) {
    /// we create the heartbeat mavLink message
    let heartbeat = MavMessage::HEARTBEAT(
        HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: MavType::MAV_TYPE_ONBOARD_CONTROLLER,
            autopilot: MavAutopilot::MAV_AUTOPILOT_INVALID,
            base_mode: MavModeFlag::empty(),
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        }
    );
    // sleep waits 1 s after the work, so the period drifts. sp better interval as it works every 0s,1s,..
    let mut ticks = interval(HEARTBEAT_PERIOD);
    loop {
        ticks.tick().await;
        outbox.send(&heartbeat).await;
    }
}

/// This is on of our main background tasks. receives datagram messages,
/// keeps the messages that come from the camera, keeps track of when 
/// the camera was last time heard, and publishes each message
async fn receive(
    socket: Arc<UdpSocket>,
    camera: CameraId,
    incoming: broadcast::Sender<MavMessage>,
    last_heard: Arc<Mutex<Option<Instant>>>,
) {
    let mut buffer = [0u8; 2048];
    loop {
        let length = match socket.recv(&mut buffer).await {
            Ok(length) => length,
            Err(error) => {
                /// this isn't fatal, the camera might not have started yet
                debug!("receiving failed: {error}");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        for (header, message) in decode(&buffer[..length]) {
            if header.system_id != camera.system_id || header.component_id != camera.component_id {
                // another mavlink component, not our camera 
                continue;
            }
            *last_heard.lock().expect("last_heard locked") = Some(Instant::now());
            // and lastly we publish the message to the incoming channel,so every task that 
            // is subscribed to that channel can read it
            let _ = incoming.send(message);
        }
    }
}

fn decode(datagram: &[u8]) -> Vec<(MavHeader, MavMessage)> {
    // wrap the datagram byte
    // PeekReader, doesn't read and move the bookmark like normal reading
    // it checks/wraps the whole thing that we want to read without moving the bookmark
    let mut reader = PeekReader::new(datagram);
    let mut messages = Vec::new();
    
    // the simulators always post one frame per datagram,
    // so the loop runs once and the Vec messages holds one
    //message
    // Although, we loop and use a Vec because there is a case 
    // that one datagram may contain several frames, back to back
    loop {
        match read_any_msg::<MavMessage, _>(&mut reader) {
            Ok(message) => messages.push(message),
            Err(MessageReadError::Parse(error)) => debug!("skipping a MAVLink frame: {error}"),
            // The reader ran out of bytes: we've seen the whole datagram.
            Err(MessageReadError::Io(_)) => return messages,
        }
    }
} 

/// The next message from the camera, or `None` once the link is gone.
///
/// Rust: a broadcast receiver reports `Lagged` when it fell behind and missed messages. For
/// us that only means "keep reading".
async fn next(messages: &mut broadcast::Receiver<MavMessage>) -> Option<MavMessage> {
    loop {
        match messages.recv().await {
            Ok(message) => return Some(message),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return None,
        }
    }
}
