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

    async fn run() {

    }

    async fn read_zoom() {

    }

    async fn read_sensor() {

    }

    async fn read_palette() {

    }
}

impl Driver for MavlinkDriver {
    fn identity(&self) -> Identity {
        todo!()
    }

    fn properties(&self) -> BTreeMap<String, Property> {
        todo!()
    }

    fn actions(&self) -> BTreeMap<String, Action> {
        todo!()
    }

    fn last_heard(&self) -> Option<Duration> {
        todo!()
    }
}

/// Text from a fixed-size, zero-padded byte array such as CAMERA_INFORMATION.vendor_name.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}