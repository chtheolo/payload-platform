mod bx20;

use crate::error::{ConnectError};


const CAMERA_INFORMATION: u32 = 259;


pub struct MavlinkDriver {
    link: Link,
    profile: Box<dyn CameraProfile>,
    identity: Identity,
}

impl MavlinkDriver {
    pub async fn connect(uri: &CameraUri) -> Result<MavlinkDriver, ConnectError>{

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