mod bx20;
mod profile;

use tracing::{error, info};

use self::profile::{CameraProfile};
use crate::uri::{UriError, CameraUri};
use crate::model::{Identity};
use crate::error::{ConnectError, DriverError};


/// The MAVLink message ids we request (MAVLink common message set).
const CAMERA_INFORMATION: u32 = 259;
const CAMERA_SETTINGS: u32 = 260;
const VIDEO_STREAM_STATUS: u32 = 270;


pub struct MavlinkDriver {
    // link: Link,
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
        let wanted_profile = uri.option("profile", "auto".to_string());
        info!("{:#?}", wanted_profile);
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