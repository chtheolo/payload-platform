//! A MAVLink 2 connection to one camera over UDP.

use std::time::{Duration, Instant};
use std::net::SocketAddr;

// use tokio::sync::broadcast;
// use tokio::sync::broadcast::error:RecvError;
use tokio::sync::broadcast{self, error:RecvError};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep, timeout};
use tracing::{debug, warn};
use mavlink::peek_reader::PeekReader;
use mavlink::{MavHeader, Message, read_any_msg, write_v2_msg};
use mavlink::dialects::common::{
    COMMAND_LONG_DATA, HEARTBEAT_DATA, MavAutopilot, MavCmd, MavMessage, MavModeFlag,
    MavParamExtType, MavResult, MavState, MavType, PARAM_EXT_REQUEST_READ_DATA,
    PARAM_EXT_SET_DATA, ParamAck,
};

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

        let outbox = OutBox {
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

    pub async fn request_message(
        &self,
        message_id: u32,
        count: usize,
    ) -> Result<Vec
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

impl OutBox {
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

async fn send_heartbeats(outbox: OutBox) {
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
                sleep(Duration::from_millis(100).await);
                continue;
            }
        };
        for (header, message) = in decode(&buffer[..length]) {
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

fn decode(datagram: &[u8]) -> Vec(MavHeader, MavMessage)> {
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
            rr(MessageReadError::Parse(error)) => debug!("skipping a MAVLink frame: {error}"),
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
