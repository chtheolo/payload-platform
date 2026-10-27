//! The I/O loop shared by both MAVLink simulators: it owns the socket and the payload model, and
//! hands camera-specific behaviour to a [`CameraProfile`].

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use mavlink::{
    MavHeader,
    dialects::common::{
        COMMAND_ACK_DATA, HEARTBEAT_DATA, MavAutopilot, MavCmd, MavMessage, MavModeFlag, MavState,
        MavType,
    },
};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

use super::{
    codec,
    command::{Command, CommandOutcome},
    palette_param,
};
use crate::{error::SimError, link::Link, payload::Payload};

/// MAVLink components are expected to heartbeat at 1 Hz.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(1);
const CAMERA_INFORMATION_ID: u32 = 259;

/// Camera behaviour that differs between the simulators.
pub trait CameraProfile {
    fn camera_information(&self, time_boot_ms: u32) -> MavMessage;

    /// Commands other than `MAV_CMD_REQUEST_MESSAGE`; unknown commands are unsupported.
    fn handle_command(&mut self, payload: &mut Payload, command: &Command) -> CommandOutcome;

    /// Reply to `MAV_CMD_REQUEST_MESSAGE`, or `None` when the message is not provided.
    fn requested_message(
        &self,
        payload: &Payload,
        message_id: u32,
        time_boot_ms: u32,
    ) -> Option<Vec<MavMessage>>;
}

#[derive(Debug, Clone, Copy)]
pub struct MavlinkOptions {
    pub port: u16,
    pub system_id: u8,
    pub component_id: u8,
}

struct Session<P> {
    link: Link,
    options: MavlinkOptions,
    profile: P,
    payload: Payload,
    peers: Vec<SocketAddr>,
    sequence: u8,
    boot: Instant,
}

pub async fn run<P: CameraProfile>(
    profile: P,
    max_zoom_factor: f32,
    options: MavlinkOptions,
) -> Result<(), SimError> {
    let link = Link::bind(options.port)
        .await
        .map_err(|source| SimError::Bind {
            port: options.port,
            source,
        })?;
    info!(
        "listening for MAVLink on UDP port {} as system {} component {}",
        options.port, options.system_id, options.component_id
    );
    serve(link, profile, max_zoom_factor, options).await;
    Ok(())
}

/// Runs the simulator on an already bound link until Ctrl-C.
async fn serve<P: CameraProfile>(
    link: Link,
    profile: P,
    max_zoom_factor: f32,
    options: MavlinkOptions,
) {
    let mut session = Session {
        link,
        options,
        profile,
        payload: Payload::new(max_zoom_factor),
        peers: Vec::new(),
        sequence: 0,
        boot: Instant::now(),
    };
    session.run().await;
}

impl<P: CameraProfile> Session<P> {
    async fn run(&mut self) {
        let mut heartbeat = interval(HEARTBEAT_PERIOD);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);
        let mut buffer = [0u8; 2048];

        loop {
            tokio::select! {
                _ = &mut shutdown => return,
                received = self.link.recv(&mut buffer) => match received {
                    Ok((length, sender)) => {
                        let datagram = buffer.get(..length).unwrap_or_default().to_vec();
                        self.handle_datagram(&datagram, sender).await;
                    }
                    Err(e) => warn!("failed to receive: {e}"),
                },
                _ = heartbeat.tick() => {
                    for peer in self.peers.clone() {
                        self.send(vec![heartbeat_message()], peer).await;
                    }
                }
            }
        }
    }

    async fn handle_datagram(&mut self, datagram: &[u8], sender: SocketAddr) {
        if !self.peers.contains(&sender) {
            info!("new MAVLink peer {sender}");
            self.peers.push(sender);
        }
        for (header, message) in codec::decode(datagram) {
            let replies = self.handle_message(header, message);
            self.send(replies, sender).await;
        }
    }

    fn handle_message(&mut self, header: MavHeader, message: MavMessage) -> Vec<MavMessage> {
        if !self.is_addressed_to_us(&message) {
            return Vec::new();
        }
        match &message {
            MavMessage::COMMAND_LONG(data) => self.handle_command(header, &Command::from(data)),
            MavMessage::COMMAND_INT(data) => self.handle_command(header, &Command::from(data)),
            other => palette_param::handle(&mut self.payload, other),
        }
    }

    fn handle_command(&mut self, sender: MavHeader, command: &Command) -> Vec<MavMessage> {
        let outcome = match command.id {
            MavCmd::MAV_CMD_REQUEST_MESSAGE => self.request_message(command),
            _ => self.profile.handle_command(&mut self.payload, command),
        };
        info!(
            "{:?} from {}/{}: {:?}",
            command.id, sender.system_id, sender.component_id, outcome.result
        );
        let ack = MavMessage::COMMAND_ACK(COMMAND_ACK_DATA {
            command: command.id,
            result: outcome.result,
            target_system: sender.system_id,
            target_component: sender.component_id,
            ..Default::default()
        });
        let mut replies = vec![ack];
        replies.extend(outcome.follow_up);
        replies
    }

    fn request_message(&self, command: &Command) -> CommandOutcome {
        let message_id = command.param(1) as u32;
        let time_boot_ms = self.time_boot_ms();
        let messages = match message_id {
            CAMERA_INFORMATION_ID => Some(vec![self.profile.camera_information(time_boot_ms)]),
            _ => self
                .profile
                .requested_message(&self.payload, message_id, time_boot_ms),
        };
        let Some(messages) = messages else {
            return CommandOutcome::denied();
        };
        messages
            .into_iter()
            .fold(CommandOutcome::accepted(), CommandOutcome::with_follow_up)
    }

    fn is_addressed_to_us(&self, message: &MavMessage) -> bool {
        let Some((system_id, component_id)) = target_of(message) else {
            return true;
        };
        let system_matches = system_id == 0 || system_id == self.options.system_id;
        let component_matches = component_id == 0 || component_id == self.options.component_id;
        system_matches && component_matches
    }

    async fn send(&mut self, messages: Vec<MavMessage>, destination: SocketAddr) {
        for message in messages {
            let header = MavHeader {
                system_id: self.options.system_id,
                component_id: self.options.component_id,
                sequence: self.sequence,
            };
            self.sequence = self.sequence.wrapping_add(1);
            debug!("sending {} to {destination}", message_name(&message));
            if let Some(datagram) = codec::encode(header, &message) {
                self.link.send(datagram, destination).await;
            }
        }
    }

    fn time_boot_ms(&self) -> u32 {
        u32::try_from(self.boot.elapsed().as_millis()).unwrap_or(u32::MAX)
    }
}

fn target_of(message: &MavMessage) -> Option<(u8, u8)> {
    match message {
        MavMessage::COMMAND_LONG(data) => Some((data.target_system, data.target_component)),
        MavMessage::COMMAND_INT(data) => Some((data.target_system, data.target_component)),
        MavMessage::PARAM_EXT_SET(data) => Some((data.target_system, data.target_component)),
        MavMessage::PARAM_EXT_REQUEST_READ(data) => {
            Some((data.target_system, data.target_component))
        }
        MavMessage::PARAM_EXT_REQUEST_LIST(data) => {
            Some((data.target_system, data.target_component))
        }
        _ => None,
    }
}

fn heartbeat_message() -> MavMessage {
    MavMessage::HEARTBEAT(HEARTBEAT_DATA {
        custom_mode: 0,
        mavtype: MavType::MAV_TYPE_CAMERA,
        autopilot: MavAutopilot::MAV_AUTOPILOT_INVALID,
        base_mode: MavModeFlag::empty(),
        system_status: MavState::MAV_STATE_ACTIVE,
        mavlink_version: 3,
    })
}

fn message_name(message: &MavMessage) -> &'static str {
    use mavlink::Message;
    message.message_name()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use mavlink::dialects::common::{COMMAND_LONG_DATA, MavResult};
    use tokio::{net::UdpSocket, time::timeout};

    use super::*;
    use crate::mavlink_sim::{
        bx20::{self, Bx20Camera},
        vlr10::{self, Vlr10Camera},
    };

    const OPTIONS: MavlinkOptions = MavlinkOptions {
        port: 0,
        system_id: 1,
        component_id: 100,
    };
    /// Longer than the heartbeat period, so a missing message is a real failure.
    const RECEIVE_TIMEOUT: Duration = Duration::from_secs(2);
    const REQUEST_CAMERA_INFORMATION: [f32; 7] = [259.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    const REQUEST_CAMERA_SETTINGS: [f32; 7] = [260.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    struct Client {
        socket: UdpSocket,
        simulator: SocketAddr,
    }

    impl Client {
        async fn start<P: CameraProfile + Send + 'static>(
            profile: P,
            max_zoom_factor: f32,
        ) -> Self {
            let link = Link::bind(0).await.unwrap();
            let simulator =
                SocketAddr::from((Ipv4Addr::LOCALHOST, link.local_addr().unwrap().port()));
            tokio::spawn(serve(link, profile, max_zoom_factor, OPTIONS));
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            Self { socket, simulator }
        }

        async fn bx20() -> Self {
            Self::start(Bx20Camera::new(100), bx20::MAX_ZOOM_FACTOR).await
        }

        async fn send(&self, message: MavMessage) {
            let header = MavHeader {
                system_id: 255,
                component_id: 190,
                sequence: 0,
            };
            let datagram = codec::encode(header, &message).unwrap();
            self.socket
                .send_to(&datagram, self.simulator)
                .await
                .unwrap();
        }

        async fn command(&self, command: MavCmd, target_component: u8, params: [f32; 7]) {
            self.send(MavMessage::COMMAND_LONG(COMMAND_LONG_DATA {
                param1: params[0],
                param2: params[1],
                param3: params[2],
                param4: params[3],
                param5: params[4],
                param6: params[5],
                param7: params[6],
                command,
                target_system: 1,
                target_component,
                confirmation: 0,
            }))
            .await;
        }

        async fn wait_for<T>(
            &self,
            mut matches: impl FnMut(&MavMessage) -> Option<T>,
        ) -> Option<T> {
            let mut buffer = [0u8; 2048];
            let search = async {
                loop {
                    let (length, _) = self.socket.recv_from(&mut buffer).await.unwrap();
                    for (_, message) in codec::decode(&buffer[..length]) {
                        if let Some(found) = matches(&message) {
                            return found;
                        }
                    }
                }
            };
            timeout(RECEIVE_TIMEOUT, search).await.ok()
        }

        async fn wait_for_ack(&self, command: MavCmd) -> Option<MavResult> {
            self.wait_for(|message| match message {
                MavMessage::COMMAND_ACK(ack) if ack.command == command => Some(ack.result),
                _ => None,
            })
            .await
        }
    }

    #[tokio::test]
    async fn requested_camera_information_follows_the_ack() {
        let client = Client::bx20().await;

        client
            .command(
                MavCmd::MAV_CMD_REQUEST_MESSAGE,
                100,
                REQUEST_CAMERA_INFORMATION,
            )
            .await;

        let result = client.wait_for_ack(MavCmd::MAV_CMD_REQUEST_MESSAGE).await;
        assert_eq!(result, Some(MavResult::MAV_RESULT_ACCEPTED));
        let information = client
            .wait_for(|message| match message {
                MavMessage::CAMERA_INFORMATION(information) => Some(information.flags),
                _ => None,
            })
            .await;
        assert!(information.is_some());
    }

    #[tokio::test]
    async fn heartbeats_start_once_a_peer_has_been_heard() {
        let client = Client::bx20().await;

        client
            .send(MavMessage::HEARTBEAT(HEARTBEAT_DATA::default()))
            .await;

        let heartbeat = client
            .wait_for(|message| match message {
                MavMessage::HEARTBEAT(heartbeat) => Some(heartbeat.mavtype),
                _ => None,
            })
            .await;
        assert_eq!(heartbeat, Some(MavType::MAV_TYPE_CAMERA));
    }

    #[tokio::test]
    async fn commands_for_another_component_are_ignored() {
        let client = Client::bx20().await;

        client
            .command(
                MavCmd::MAV_CMD_REQUEST_MESSAGE,
                154,
                REQUEST_CAMERA_INFORMATION,
            )
            .await;

        assert_eq!(
            client.wait_for_ack(MavCmd::MAV_CMD_REQUEST_MESSAGE).await,
            None
        );
    }

    #[tokio::test]
    async fn unknown_commands_are_unsupported() {
        let client = Client::bx20().await;

        client
            .command(MavCmd::MAV_CMD_IMAGE_START_CAPTURE, 100, [0.0; 7])
            .await;

        let result = client
            .wait_for_ack(MavCmd::MAV_CMD_IMAGE_START_CAPTURE)
            .await;
        assert_eq!(result, Some(MavResult::MAV_RESULT_UNSUPPORTED));
    }

    #[tokio::test]
    async fn camera_settings_are_sent_only_on_request() {
        let client = Client::bx20().await;

        client
            .command(
                MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
                100,
                [2.0, 50.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            )
            .await;
        let unprompted = client
            .wait_for(|message| match message {
                MavMessage::CAMERA_SETTINGS(_) => Some(()),
                _ => None,
            })
            .await;
        assert_eq!(unprompted, None);

        client
            .command(
                MavCmd::MAV_CMD_REQUEST_MESSAGE,
                100,
                REQUEST_CAMERA_SETTINGS,
            )
            .await;
        let zoom_level = client
            .wait_for(|message| match message {
                MavMessage::CAMERA_SETTINGS(settings) => Some(settings.zoomLevel),
                _ => None,
            })
            .await;
        assert_eq!(zoom_level, Some(50.0));
    }

    #[tokio::test]
    async fn vlr10_takes_the_palette_parameter() {
        let client = Client::start(Vlr10Camera, vlr10::MAX_ZOOM_FACTOR).await;

        client
            .send(MavMessage::PARAM_EXT_REQUEST_LIST(
                mavlink::dialects::common::PARAM_EXT_REQUEST_LIST_DATA {
                    target_system: 1,
                    target_component: 100,
                },
            ))
            .await;

        let param_id = client
            .wait_for(|message| match message {
                MavMessage::PARAM_EXT_VALUE(value) => Some(value.param_id),
                _ => None,
            })
            .await;
        assert_eq!(param_id, Some("IR_PALETTE".into()));
    }
}
