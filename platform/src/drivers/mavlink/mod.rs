//! MAVLink 2 over UDP, the `mavlink://` scheme (README, "Camera URI").
//!
//! - `link.rs` is MAVLink itself: heartbeats, commands and their ACKs, requesting messages,
//!   parameters. Apart from a few names, such as `CameraId`, nothing in it is about cameras,
//!   so any MAVLink device can use it.
//! - `camera/` is the MAVLink camera: zoom, sensor and palette, with one profile per model.
//!
//! A MAVLink gimbal would be a `gimbal/` folder next to `camera/`, sharing `link.rs`, and
//! `connect` would pick between them, for example by the device type in its HEARTBEAT. Gimbal
//! control is out of scope (README, "Scope"), so today a MAVLink device is always a camera.

mod camera;
mod link;

use super::Driver;
use crate::error::ConnectError;
use crate::uri::CameraUri;

/// Connects to the MAVLink device at `uri`.
pub async fn connect(uri: &CameraUri) -> Result<Box<dyn Driver>, ConnectError> {
    Ok(Box::new(camera::MavlinkCameraDriver::connect(uri).await?))
}
