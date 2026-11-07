//! The Brightmere BX-20, as described in specs/Brightmere_BX-20_ICD_RevC.pdf.

use mavlink::dialects::common::{
    CAMERA_SETTINGS_DATA, CameraSource, CameraZoomType, MavCmd, VIDEO_STREAM_STATUS_DATA,
    VideoStreamStatusFlags,
};

use super::profile::{CameraProfile, ZoomRange};
use crate::drivers::mavlink::link::Command;
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
    /// So, here we are getting percentage and we calculate the zoom factor.
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
        // from official docs from mavlink crate
        //  enum CameraZoomType {
        //      ZOOM_TYPE_STEP = 0,          // "zoom in/out by N steps"
        //      ZOOM_TYPE_CONTINUOUS = 1,    // "keep zooming in/out until told to stop
        //      ZOOM_TYPE_RANGE = 2,         // "go to N % of the range"   ← what the BX-20 uses"
        //      ZOOM_TYPE_FOCAL_LENGTH = 3,  // "go to this focal length in mm"
        //      …
        // }
        // so we do first type cast for converting enum to int and then int to float
        //
        let zoom_type = CameraZoomType::ZOOM_TYPE_RANGE as u32 as f32;

        // return a vector cause we have of course to align with CameraProfile
        vec![Command::repeatable(
            MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
            [zoom_type, percent as f32, 0.0, 0.0, 0.0, 0.0, 0.0],
        )]
    }

    fn stream_count(&self) -> usize {
        1 // a single stream, "main" (ICD 6.3)
    }

    fn select_sensor(&self, sensor: Sensor) -> Command {
        // MAV_CMD_SET_CAMERA_SOURCE (ICD 5.3): param1 0 means "all cameras", param2 is the
        // primary source, and param3, the secondary source, must stay 0 (no picture-in-picture).
        let source = match sensor {
            Sensor::Visible => CameraSource::CAMERA_SOURCE_RGB,
            Sensor::Thermal => CameraSource::CAMERA_SOURCE_IR,
        };
        Command::repeatable(
            MavCmd::MAV_CMD_SET_CAMERA_SOURCE,
            [0.0, source as u32 as f32, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
    }

    fn sensor_from_streams(&self, streams: &[VIDEO_STREAM_STATUS_DATA]) -> Option<Sensor> {
        // The single stream carries the THERMAL flag while the thermal sensor feeds it (ICD 6.3).
        // Rust: `?` on an `Option` returns `None` early when there is no first stream.
        let stream = streams.first()?;
        let thermal = stream
            .flags
            .contains(VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_THERMAL);
        Some(if thermal {
            Sensor::Thermal
        } else {
            Sensor::Visible
        })
    }

    fn firmware(&self, raw: u32) -> String {
        // The ICD documents 0x00010000 as "1.0.0.0", which puts the major version in bits
        // 16-23. That matches neither MAVLink's documented layout nor the VLR-10's, which is
        // why each profile decodes the firmware version itself.
        // Rust: a closure stored in a variable, so the shifting isn't written out four times.
        let byte = |shift: u32| (raw >> shift) & 0xff;
        format!("{}.{}.{}.{}", byte(16), byte(8), byte(0), byte(24))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(zoom_level: f32) -> CAMERA_SETTINGS_DATA {
        // Rust: `..Default::default()` fills every field not named here with its default.
        CAMERA_SETTINGS_DATA {
            zoomLevel: zoom_level,
            ..Default::default()
        }
    }

    #[test]
    fn zoom_level_is_a_percentage_of_the_range() {
        // The ICD's start-up example: zoomLevel 50 means 10.5x.
        assert_eq!(Bx20.zoom_from_settings(&settings(50.0)), 10.5);
    }

    #[test]
    fn zoom_to_sends_a_range_percentage() {
        let commands = Bx20.zoom_to(1.0, 10.5);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].params[0], 2.0); // ZOOM_TYPE_RANGE
        assert_eq!(commands[0].params[1], 50.0);
    }

    #[test]
    fn firmware_reads_as_the_icd_says() {
        assert_eq!(Bx20.firmware(0x0001_0000), "1.0.0.0");
    }
}
