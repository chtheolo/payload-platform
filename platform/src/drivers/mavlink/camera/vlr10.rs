//! The Optronique Vallerin VLR-10, as described in specs/NT-0317_VLR-10_indice_D.pdf ("note"
//! in the comments, so "note 4" is its section 4).
//!
//! It speaks the same MAVLink as the BX-20, but differs in four places, and each one is a
//! method of the profile: what `zoomLevel` means, how to zoom, how to read and switch the
//! sensor, and how the firmware version is encoded.

use mavlink::dialects::common::{
    CAMERA_SETTINGS_DATA, CameraZoomType, MavCmd, VIDEO_STREAM_STATUS_DATA, VideoStreamStatusFlags,
};

use super::profile::{CameraProfile, ZoomRange};
use crate::drivers::mavlink::link::Command;
use crate::model::Sensor;

pub struct Vlr10;

/// 10x optical zoom (note 2).
const MAX_ZOOM: f64 = 10.0;

/// The video channels, by stream id (note 5).
const VISIBLE_STREAM: u8 = 1;
const THERMAL_STREAM: u8 = 2;

impl CameraProfile for Vlr10 {
    fn name(&self) -> &'static str {
        "vlr10"
    }

    fn vendor(&self) -> &'static str {
        "Vallerin"
    }

    fn model(&self) -> &'static str {
        "VLR-10"
    }

    fn zoom_range(&self) -> ZoomRange {
        // The VLR-10 can only move in steps of 1x (note 4), so 1x is also the resolution. The
        // core rounds a request such as 4.6 to 5 before it reaches this profile.
        ZoomRange {
            minimum: 1.0,
            maximum: MAX_ZOOM,
            resolution: 1.0,
        }
    }

    fn zoom_from_settings(&self, settings: &CAMERA_SETTINGS_DATA) -> f64 {
        // zoomLevel is the zoom factor itself, e.g. 4.0 for 4x (note 4). No conversion needed.
        f64::from(settings.zoomLevel)
    }

    fn zoom_to(&self, current: f64, target: f64) -> Vec<Command> {
        // There is no absolute zoom: only ZOOM_TYPE_STEP, one 1x step per command, with
        // param2 = 1 to zoom in and -1 to zoom out (note 4). So going from 4x to 7x is three
        // steps in. This is why `zoom_to` gets the current zoom and returns a list.
        let steps = (target - current).round() as i32;
        // Rust: `signum` is 1 for positive numbers, -1 for negative ones and 0 for zero.
        let direction = steps.signum() as f32;
        let zoom_type = CameraZoomType::ZOOM_TYPE_STEP as u32 as f32;
        // A step isn't repeatable: if its ACK got lost, resending it could step twice.
        let step = Command::once(
            MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
            [zoom_type, direction, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Rust: `vec![value; n]` makes a vector holding `n` copies of `value`.
        vec![step; steps.unsigned_abs() as usize]
    }

    fn stream_count(&self) -> usize {
        2 // one VIDEO_STREAM_STATUS per channel, visible and thermal (note 5)
    }

    fn sensor_from_streams(&self, streams: &[VIDEO_STREAM_STATUS_DATA]) -> Option<Sensor> {
        // Only one channel runs at a time, and it carries the RUNNING flag. The THERMAL flag is
        // always set on channel 2, running or not (note 5). So: find the running stream, then
        // check whether it is the thermal one. (On the BX-20, the THERMAL flag alone is enough.)
        let running = streams.iter().find(|stream| {
            stream
                .flags
                .contains(VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_RUNNING)
        })?;
        let thermal = running
            .flags
            .contains(VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_THERMAL);
        Some(if thermal {
            Sensor::Thermal
        } else {
            Sensor::Visible
        })
    }

    fn select_sensor(&self, sensor: Sensor) -> Command {
        // MAV_CMD_VIDEO_START_STREAMING with the channel in param1; the other channel stops by
        // itself (note 5). The VLR-10 answers SET_CAMERA_SOURCE, the BX-20's way, with
        // UNSUPPORTED. Starting the channel that already runs changes nothing, so it may be resent.
        let stream = match sensor {
            Sensor::Visible => VISIBLE_STREAM,
            Sensor::Thermal => THERMAL_STREAM,
        };
        Command::repeatable(
            MavCmd::MAV_CMD_VIDEO_START_STREAMING,
            [f32::from(stream), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
    }

    fn firmware(&self, raw: u32) -> String {
        // Note 7 documents the camera's 0x02030000 as "2.3": the major version in the top byte,
        // the minor in the next one. The two low bytes aren't documented, so they're left out.
        format!("{}.{}", raw >> 24, (raw >> 16) & 0xff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(zoom_level: f32) -> CAMERA_SETTINGS_DATA {
        CAMERA_SETTINGS_DATA {
            zoomLevel: zoom_level,
            ..Default::default()
        }
    }

    fn stream(stream_id: u8, flags: VideoStreamStatusFlags) -> VIDEO_STREAM_STATUS_DATA {
        VIDEO_STREAM_STATUS_DATA {
            stream_id,
            flags,
            ..Default::default()
        }
    }

    #[test]
    fn zoom_level_is_the_factor() {
        // Note 4's example: 4.0 means 4x.
        assert_eq!(Vlr10.zoom_from_settings(&settings(4.0)), 4.0);
    }

    #[test]
    fn zoom_to_sends_one_step_per_1x() {
        let commands = Vlr10.zoom_to(4.0, 7.0);
        assert_eq!(commands.len(), 3);
        for command in &commands {
            assert_eq!(command.id, MavCmd::MAV_CMD_SET_CAMERA_ZOOM);
            assert_eq!(command.params[0], 0.0); // ZOOM_TYPE_STEP
            assert_eq!(command.params[1], 1.0); // in
            assert!(!command.repeatable);
        }
        let out = Vlr10.zoom_to(7.0, 5.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].params[1], -1.0);
        assert!(Vlr10.zoom_to(5.0, 5.0).is_empty());
    }

    #[test]
    fn the_running_stream_is_the_sensor() {
        let running = VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_RUNNING;
        let thermal = VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_THERMAL;
        // At power-on channel 1 runs (note 5), while channel 2 still has its THERMAL flag.
        let visible = [stream(1, running), stream(2, thermal)];
        assert_eq!(Vlr10.sensor_from_streams(&visible), Some(Sensor::Visible));
        let infrared = [
            stream(1, VideoStreamStatusFlags::empty()),
            stream(2, running | thermal),
        ];
        assert_eq!(Vlr10.sensor_from_streams(&infrared), Some(Sensor::Thermal));
    }

    #[test]
    fn selecting_thermal_starts_channel_2() {
        let command = Vlr10.select_sensor(Sensor::Thermal);
        assert_eq!(command.id, MavCmd::MAV_CMD_VIDEO_START_STREAMING);
        assert_eq!(command.params[0], 2.0);
    }

    #[test]
    fn firmware_reads_as_the_note_says() {
        assert_eq!(Vlr10.firmware(0x0203_0000), "2.3");
    }
}
