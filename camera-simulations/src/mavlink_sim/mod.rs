//! The MAVLink camera simulators: the BX-20 and the VLR-10.

mod bx20;
mod camera;
mod codec;
mod command;
mod palette_param;
mod session;
mod vlr10;

pub use session::MavlinkOptions;

use crate::error::SimError;

pub async fn run_bx20(options: MavlinkOptions) -> Result<(), SimError> {
    let camera = bx20::Bx20Camera::new(options.component_id);
    session::run(camera, bx20::MAX_ZOOM_FACTOR, options).await
}

pub async fn run_vlr10(options: MavlinkOptions) -> Result<(), SimError> {
    session::run(vlr10::Vlr10Camera, vlr10::MAX_ZOOM_FACTOR, options).await
}
