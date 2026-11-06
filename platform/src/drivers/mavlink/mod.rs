mod bx20;
mod profile;
mod link;

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use mavlink::dialects::common::{CameraZoomType, MavCmd, MavMessage, MavResult, ParamAck};
use tracing::info;

use self::link::{CameraId, Command, Link, LinkError};
use self::profile::CameraProfile;
use super::Driver;
use crate::error::{ConnectError, DriverError};
use crate::model::{Action, Identity, Kind, Property, Sensor, Value, vocabulary};
use crate::uri::{CameraUri, UriError};


/// The MAVLink message ids we request (MAVLink common message set).
const CAMERA_INFORMATION: u32 = 259;
const CAMERA_SETTINGS: u32 = 260;
const VIDEO_STREAM_STATUS: u32 = 270;

/// How long to wait for the camera's first heartbeat before calling it "no answer".
const FIRST_HEARTBEAT: Duration = Duration::from_secs(3);

/// The thermal palette parameter both MAVLink cameras share, and its values by index
/// (BX-20 ICD 7, VLR-10 note 6). The names are the README's palette names.
const PALETTE_PARAM: &str = "IR_PALETTE";
const PALETTES: [&str; 4] = ["white_hot", "black_hot", "ironbow", "rainbow"];


pub struct MavlinkDriver {
    link: Link,
    profile: Box<dyn CameraProfile>,
    identity: Identity,
}

impl MavlinkDriver {
    pub async fn connect(uri: &CameraUri) -> Result<MavlinkDriver, ConnectError>{
        info!("This is a camera that use MavLink");
        /// a wrong uri is fatal, we need to restart 
        /// we create a function and store it to fatal so we can reuse it 
        let fatal = |error: UriError| ConnectError::Fatal(error.to_string());
    
        uri.accept_only(&["profile", "system_id", "component_id"]).map_err(fatal)?;

        let wanted_profile = uri.option("profile", "auto".to_string()).map_err(fatal)?;
        info!("User didn't add a profile - we run in {:#?}", wanted_profile);

        let camera = CameraId {
            // maybe I can set 1 as a const 
            system_id: uri.option("system_id", 1).map_err(fatal)?,
            component_id: uri.option("component_id", 100).map_err(fatal)?,
        };
        // Turns "127.0.0.1" + 14580 (or a host name) into a socket address.
        let address = tokio::net::lookup_host((uri.host.as_str(), uri.port))
            .await
            .ok()
            .and_then(|mut addresses| addresses.next())
            .ok_or_else(|| ConnectError::Fatal(format!("can't resolve host `{}`", uri.host)))?;

        let link = Link::open(address, camera)
            .await
            .map_err(|error| ConnectError::Fatal(format!("Can't open a UDP socket: {error}")))?;

        link.wait_for_heartbeat(FIRST_HEARTBEAT)
            .await
            .map_err(|_| ConnectError::NoAnswer(format!("no heartbeat from {address}")))?;

        let messages = link
            .request_message(CAMERA_INFORMATION, 1)
            .await
            .map_err(|_| ConnectError::NoAnswer("no CAMERA_INFORMATION from the camera".into()))?;

        let Some(MavMessage::CAMERA_INFORMATION(info)) = messages.into_iter().next() else {
            return Err(ConnectError::NoAnswer("no CAMERA_INFORMATION from the camera".into()));
        };

        let vendor = text(&info.vendor_name);
        let model = text(&info.model_name);

        let profile = if wanted_profile == "auto" {
            profile::detect(&vendor, &model).ok_or_else(|| {
                ConnectError::Fatal(format!(
                    "unknown MAVLink camera `{vendor} {model}`; known profiles: {}; \
                     force one with ?profile=",
                    profile::names()
                ))
            })?
        } else {
            profile::named(&wanted_profile).ok_or_else(|| {
                ConnectError::Fatal(format!(
                    "unknown profile `{wanted_profile}`; known profiles: {}",
                    profile::names()
                ))
            })?
        };

        info!(
            "connected to {vendor} {model} at {address}, profile {}",
            profile.name()
        );

        let identity = Identity {
            kind: "camera".into(),
            firmware: Some(profile.firmware(info.firmware_version)),
            vendor,
            model,
            driver: "mavlink".into(),
            profile: Some(profile.name().into()),
            uri: uri.text.clone(),
        };

        Ok(MavlinkDriver {
            link,
            profile,
            identity,
        })
    }

    /// Sends one command and turns the camera's COMMAND_ACK into success or an error
    async fn run(&self, command: Command) -> Result<(), DriverError> {
        match self.link.command(&command).await? {
            MavResult::MAV_RESULT_ACCEPTED => Ok(()),
            MavResult::MAV_RESULT_TEMPORARILY_REJECTED => Err(DriverError::Busy),
            // The profile sent something this camera can't do: our bug, not the GCS's.
            MavResult::MAV_RESULT_UNSUPPORTED => Err(DriverError::Internal(format!(
                "the camera doesn't support {:?}",
                command.id
            ))),
            other => Err(DriverError::Rejected(format!(
                "{:?} answered {other:?}",
                command.id
            ))),
        }
    }

    async fn read_zoom(&self) -> Result<f64, DriverError> {
        let messages = self.link.request_message(CAMERA_SETTINGS, 1).await?;
        let Some(MavMessage::CAMERA_SETTINGS(settings)) = messages.first() else {
            return Err(DriverError::Internal("expected CAMERA_SETTINGS".into()));
        };
        let factor = self.profile.zoom_from_settings(settings);
        Ok(round_to(factor, self.profile.zoom_range().resolution))
    }

    async fn read_sensor(&self) -> Result<Sensor, DriverError> {
        let count = self.profile.stream_count();
        let messages = self.link.request_message(VIDEO_STREAM_STATUS, count).await?;
        // Rust: `filter_map` keeps the `Some`s and drops the `None`s, mapping as it goes.
        let streams: Vec<_> = messages
            .iter()
            .filter_map(|message| match message {
                MavMessage::VIDEO_STREAM_STATUS(status) => Some(status.clone()),
                _ => None,
            })
            .collect();
        self.profile
            .sensor_from_streams(&streams)
            .ok_or_else(|| DriverError::Internal("expected VIDEO_STREAM_STATUS".into()))
    }

    async fn read_palette(&self) -> Result<&'static str, DriverError> {
        let index = self.link.read_param(PALETTE_PARAM).await?;
        palette_name(index)
    }
}

#[async_trait]
impl Driver for MavlinkDriver {
    fn identity(&self) -> Identity {
        self.identity.clone()
    }

    fn properties(&self) -> BTreeMap<String, Property> {
        let zoom = self.profile.zoom_range();
        let zoom_kind = Kind::Number {
            unit: "x".into(),
            minimum: zoom.minimum,
            maximum: zoom.maximum,
            resolution: zoom.resolution,
        };
        let sensor_kind = Kind::Enum {
            options: Sensor::ALL.iter().map(|sensor| sensor.as_str().into()).collect(),
        };
        let palette_kind = Kind::Enum {
            options: PALETTES.iter().map(|palette| palette.to_string()).collect(),
        };
        BTreeMap::from([
            (vocabulary::ZOOM.into(), Property::unread(zoom_kind, true)),
            (vocabulary::SENSOR.into(), Property::unread(sensor_kind, true)),
            (vocabulary::THERMAL_PALETTE.into(), Property::unread(palette_kind, true)),
        ])
    }

    fn actions(&self) -> BTreeMap<String, Action> {
        let direction = Kind::Enum {
            options: vec!["in".into(), "out".into()],
        };
        let zoom_step = Action {
            parameters: BTreeMap::from([(vocabulary::DIRECTION.into(), direction)]),
        };
        BTreeMap::from([(vocabulary::ZOOM_STEP.into(), zoom_step)])
    }

    fn last_heard(&self) -> Option<Duration> {
        self.link.last_heard()
    }

    async fn read(&mut self) -> Result<BTreeMap<String, Value>, DriverError> {
        Ok(BTreeMap::from([
            (vocabulary::ZOOM.into(), Value::Number(self.read_zoom().await?)),
            (
                vocabulary::SENSOR.into(),
                Value::Text(self.read_sensor().await?.as_str().into()),
            ),
            (
                vocabulary::THERMAL_PALETTE.into(),
                Value::Text(self.read_palette().await?.into()),
            ),
        ]))
    }

    async fn write(&mut self, property: &str, value: &Value) -> Result<Value, DriverError> {
        match (property, value) {
            (vocabulary::ZOOM, Value::Number(target)) => {
                let current = self.read_zoom().await?;
                for command in self.profile.zoom_to(current, *target) {
                    self.run(command).await?;
                }
                // Answer with what the camera reports now, not with what we asked for.
                Ok(Value::Number(self.read_zoom().await?))
            }
            (vocabulary::SENSOR, Value::Text(name)) => {
                let Some(sensor) = Sensor::from_name(name) else {
                    return Err(DriverError::Internal(format!("unknown sensor `{name}`")));
                };
                self.run(self.profile.select_sensor(sensor)).await?;
                Ok(Value::Text(self.read_sensor().await?.as_str().into()))
            }
            (vocabulary::THERMAL_PALETTE, Value::Text(name)) => {
                let Some(index) = PALETTES.iter().position(|palette| *palette == name.as_str())
                else {
                    return Err(DriverError::Internal(format!("unknown palette `{name}`")));
                };
                // The ACK carries the palette in effect afterwards (BX-20 ICD 7), so it already
                // is the read-back; no extra request needed.
                let (result, in_effect) = self.link.set_param(PALETTE_PARAM, index as u8).await?;
                if result != ParamAck::PARAM_ACK_ACCEPTED {
                    return Err(DriverError::Rejected(format!(
                        "{PALETTE_PARAM} answered {result:?}"
                    )));
                }
                Ok(Value::Text(palette_name(in_effect)?.into()))
            }
            _ => Err(DriverError::Internal(format!(
                "can't write {value:?} to `{property}`"
            ))),
        }
    }

    async fn invoke(
        &mut self,
        action: &str,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, DriverError> {
        match action {
            vocabulary::ZOOM_STEP => {
                let step = match arguments.get(vocabulary::DIRECTION) {
                    Some(Value::Text(direction)) if direction == "in" => 1.0,
                    Some(Value::Text(direction)) if direction == "out" => -1.0,
                    other => {
                        return Err(DriverError::Internal(format!(
                            "bad zoom direction {other:?}"
                        )));
                    }
                };
                // Both MAVLink cameras step natively, 1x per step, so this lives here rather
                // than in a profile. A step isn't repeatable: resending a lost one could zoom
                // twice, so it goes out once.
                let zoom_type = CameraZoomType::ZOOM_TYPE_STEP as u32 as f32;
                self.run(Command::once(
                    MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
                    [zoom_type, step, 0.0, 0.0, 0.0, 0.0, 0.0],
                ))
                .await?;
                Ok(BTreeMap::from([(
                    vocabulary::ZOOM.into(),
                    Value::Number(self.read_zoom().await?),
                )]))
            }
            _ => Err(DriverError::Internal(format!("unknown action `{action}`"))),
        }
    }
}

/// Rust: this `From` lets `?` turn a `LinkError` into a `DriverError` automatically.
impl From<LinkError> for DriverError {
    fn from(error: LinkError) -> DriverError {
        match error {
            LinkError::Timeout => DriverError::Timeout,
            LinkError::Refused(MavResult::MAV_RESULT_TEMPORARILY_REJECTED) => DriverError::Busy,
            LinkError::Refused(result) => {
                DriverError::Rejected(format!("the camera answered {result:?}"))
            }
        }
    }
}

fn palette_name(index: u8) -> Result<&'static str, DriverError> {
    // Rust: `.get()` returns `None` instead of panicking when the index is out of range.
    PALETTES
        .get(usize::from(index))
        .copied()
        .ok_or_else(|| DriverError::Internal(format!("unknown palette index {index}")))
}

/// Text from a fixed-size, zero-padded byte array such as CAMERA_INFORMATION.vendor_name.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}