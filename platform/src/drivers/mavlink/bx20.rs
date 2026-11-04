//! The Brightmere BX-20, as described in specs/Brightmere_BX-20_ICD_RevC.pdf.

use mavlink::dialects::common::{
    CAMERA_SETTINGS_DATA, CameraSource, CameraZoomType, MavCmd, VIDEO_STREAM_STATUS_DATA,
    VideoStreamStatusFlags,
};

use super::profile::{CameraProfile, ZoomRange};
use super::link::Command;
use crate::model::Sensor;

/// Rust: a struct with no fields (called unit struct). The profile needs no data, only behaviour.
pub struct Bx20;

const MAX_ZOOM: f64 = 20.0;

impl CameraProfile for Bx20 {
    fn name(&self) -> &'static str {
        "bx20"
    }

    fn vendor(&self) -> &'static str {
        "Brightmere"
    }

    fn model(&self) -> &'static str {
        "BX-20"
    }

    fn zoom_range(&self) -> ZoomRange {
        ZoomRange {
            minimum: 1.0,
            maximum: MAX_ZOOM,
            resolution: 0.1,
        }
    }

    /// the idea is that we are mapping the 0-100% to 1x-20x(hardware limits of our camera)
    ///percentage  0 ──────────── 50 ──────────── 100
    ///factor     1× ─────────── 10.5× ────────── 20×
    fn zoom_from_settings(&self, settings: &CAMERA_SETTINGS_DATA) ->f64 {
        1.0 + (MAX_ZOOM - 1.0) * f64::from(settings.zoomLevel) / 100.0
    }

    /// _current: where the zoom is now
    /// target: where GCS wants to 
    /// returns a list of comands, which the driver sends in order 
    fn zoom_to(&self, _current: f64, target: f64) -> Vec<Command> {
        let percent = (target - 1.0) / (MAX_ZOOM - 1.0)* 100.0;
        let zoom_type = CameraZoomType::ZOOM_TYPE_RANGE as u32 as f32;
        vec![Command::repeatable(
            MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
            [zoom_type, percent as f32, 0.0, 0.0, 0.0, 0.0, 0.0],
        )]
    }

    fn firmware(&self, raw: u32) -> String {
        // The ICD documents 0x00010000 as "1.0.0.0", which puts the major version in bits
        // 16-23. That matches neither MAVLink's documented layout nor the VLR-10's, which is
        // why each profile decodes the firmware version itself.
        // Rust: a closure stored in a variable, so the shifting isn't written out four times.
        let byte = |shift: u32| (raw >> shift) & 0xff;
        format!("{}.{}.{}.{}", byte(16), byte(8), byte(0), byte(24))
    }

    // fn stream_count(&self) -> usize {
    //     1 // a single stream, "main" (ICD 6.3)
    // }

    // fn sensor_from_streams(&self, streams: &)

}