//! The BX-20: one video stream whose source is switched with `MAV_CMD_SET_CAMERA_SOURCE`.

use mavlink::dialects::common::{
    CAMERA_INFORMATION_DATA, CAMERA_SETTINGS_DATA, CameraCapFlags, CameraMode, CameraSource,
    CameraZoomType, MavCmd, MavMessage, VIDEO_STREAM_INFORMATION_DATA, VIDEO_STREAM_STATUS_DATA,
    VideoStreamEncoding, VideoStreamStatusFlags, VideoStreamType,
};
use num_traits::FromPrimitive;

use super::{
    camera::{fixed_text, set_camera_zoom, zoomed_hfov_deg},
    command::{Command, CommandOutcome},
    session::CameraProfile,
};
use crate::payload::{Payload, VideoSource};

pub const MAX_ZOOM_FACTOR: f32 = 20.0;

const CAMERA_SETTINGS_ID: u32 = 260;
const VIDEO_STREAM_INFORMATION_ID: u32 = 269;
const VIDEO_STREAM_STATUS_ID: u32 = 270;
const STREAM_ID: u8 = 1;
const VISIBLE_WIDE_HFOV_DEG: f32 = 62.0;
/// The thermal sensor has a fixed lens, so its field of view does not follow the zoom.
const THERMAL_HFOV_DEG: f32 = 32.0;

pub struct Bx20Camera {
    component_id: u8,
}

impl Bx20Camera {
    pub fn new(component_id: u8) -> Self {
        Self { component_id }
    }

    fn set_camera_source(&self, payload: &mut Payload, command: &Command) -> CommandOutcome {
        let device_id = command.param(1);
        if device_id != 0.0 && device_id != f32::from(self.component_id) {
            return CommandOutcome::denied();
        }
        let primary = CameraSource::from_f32(command.param(2));
        let secondary = CameraSource::from_f32(command.param(3));
        let Some(source) = video_source(primary, secondary) else {
            return CommandOutcome::denied();
        };
        payload.source = source;
        CommandOutcome::accepted()
    }
}

impl CameraProfile for Bx20Camera {
    fn camera_information(&self, time_boot_ms: u32) -> MavMessage {
        MavMessage::CAMERA_INFORMATION(CAMERA_INFORMATION_DATA {
            time_boot_ms,
            firmware_version: 0x0001_0000,
            focal_length: 4.3,
            sensor_size_h: 6.2,
            sensor_size_v: 3.5,
            flags: CameraCapFlags::CAMERA_CAP_FLAGS_HAS_VIDEO_STREAM
                | CameraCapFlags::CAMERA_CAP_FLAGS_HAS_BASIC_ZOOM,
            resolution_h: 1920,
            resolution_v: 1080,
            cam_definition_version: 0,
            vendor_name: fixed_text("Brightmere"),
            model_name: fixed_text("BX-20"),
            lens_id: 0,
            cam_definition_uri: "".into(),
            gimbal_device_id: 0,
            camera_device_id: 0,
        })
    }

    fn handle_command(&mut self, payload: &mut Payload, command: &Command) -> CommandOutcome {
        match command.id {
            MavCmd::MAV_CMD_SET_CAMERA_ZOOM => {
                let supported = [
                    CameraZoomType::ZOOM_TYPE_STEP,
                    CameraZoomType::ZOOM_TYPE_RANGE,
                ];
                CommandOutcome::result(set_camera_zoom(&mut payload.zoom, command, &supported))
            }
            MavCmd::MAV_CMD_SET_CAMERA_SOURCE => self.set_camera_source(payload, command),
            _ => CommandOutcome::unsupported(),
        }
    }

    fn requested_message(
        &self,
        payload: &Payload,
        message_id: u32,
        time_boot_ms: u32,
    ) -> Option<Vec<MavMessage>> {
        match message_id {
            CAMERA_SETTINGS_ID => Some(vec![camera_settings(payload, time_boot_ms)]),
            VIDEO_STREAM_INFORMATION_ID => Some(vec![stream_information(payload)]),
            VIDEO_STREAM_STATUS_ID => Some(vec![stream_status(payload)]),
            _ => None,
        }
    }
}

/// A single full-frame source; picture-in-picture is not available.
fn video_source(
    primary: Option<CameraSource>,
    secondary: Option<CameraSource>,
) -> Option<VideoSource> {
    use CameraSource::{CAMERA_SOURCE_DEFAULT, CAMERA_SOURCE_IR, CAMERA_SOURCE_RGB};
    match (primary?, secondary?) {
        (CAMERA_SOURCE_DEFAULT | CAMERA_SOURCE_RGB, CAMERA_SOURCE_DEFAULT) => {
            Some(VideoSource::Visible)
        }
        (CAMERA_SOURCE_IR, CAMERA_SOURCE_DEFAULT) => Some(VideoSource::Thermal),
        _ => None,
    }
}

/// Zoom level as a percentage of the zoom range, as the camera protocol defines it.
fn camera_settings(payload: &Payload, time_boot_ms: u32) -> MavMessage {
    MavMessage::CAMERA_SETTINGS(CAMERA_SETTINGS_DATA {
        time_boot_ms,
        mode_id: CameraMode::CAMERA_MODE_VIDEO,
        zoomLevel: payload.zoom.fraction() * 100.0,
        focusLevel: f32::NAN,
        camera_device_id: 0,
    })
}

fn stream_flags(payload: &Payload) -> VideoStreamStatusFlags {
    let mut flags = VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_RUNNING;
    if payload.source == VideoSource::Thermal {
        flags |= VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_THERMAL;
    }
    flags
}

fn stream_hfov_deg(payload: &Payload) -> u16 {
    match payload.source {
        VideoSource::Thermal => THERMAL_HFOV_DEG as u16,
        VideoSource::Visible => {
            zoomed_hfov_deg(VISIBLE_WIDE_HFOV_DEG, payload.zoom.factor()).round() as u16
        }
    }
}

fn stream_information(payload: &Payload) -> MavMessage {
    MavMessage::VIDEO_STREAM_INFORMATION(VIDEO_STREAM_INFORMATION_DATA {
        framerate: 30.0,
        bitrate: 4_000_000,
        flags: stream_flags(payload),
        resolution_h: 1920,
        resolution_v: 1080,
        rotation: 0,
        hfov: stream_hfov_deg(payload),
        stream_id: STREAM_ID,
        count: 1,
        mavtype: VideoStreamType::VIDEO_STREAM_TYPE_RTSP,
        name: "main".into(),
        uri: "rtsp://127.0.0.1:8554/main".into(),
        encoding: VideoStreamEncoding::VIDEO_STREAM_ENCODING_H264,
        camera_device_id: 0,
    })
}

fn stream_status(payload: &Payload) -> MavMessage {
    MavMessage::VIDEO_STREAM_STATUS(VIDEO_STREAM_STATUS_DATA {
        framerate: 30.0,
        bitrate: 4_000_000,
        flags: stream_flags(payload),
        resolution_h: 1920,
        resolution_v: 1080,
        rotation: 0,
        hfov: stream_hfov_deg(payload),
        stream_id: STREAM_ID,
        camera_device_id: 0,
    })
}

#[cfg(test)]
mod tests {
    use mavlink::dialects::common::MavResult;

    use super::*;

    fn set_source(primary: f32, secondary: f32) -> Command {
        Command {
            id: MavCmd::MAV_CMD_SET_CAMERA_SOURCE,
            params: [0.0, primary, secondary, 0.0, 0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn camera_source_switches_to_thermal() {
        let mut camera = Bx20Camera::new(100);
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);

        let outcome = camera.handle_command(&mut payload, &set_source(2.0, 0.0));

        assert_eq!(outcome.result, MavResult::MAV_RESULT_ACCEPTED);
        assert_eq!(payload.source, VideoSource::Thermal);
    }

    #[test]
    fn picture_in_picture_is_denied() {
        let mut camera = Bx20Camera::new(100);
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);

        let outcome = camera.handle_command(&mut payload, &set_source(2.0, 1.0));

        assert_eq!(outcome.result, MavResult::MAV_RESULT_DENIED);
        assert_eq!(payload.source, VideoSource::Visible);
    }

    #[test]
    fn zoom_has_no_follow_up_message() {
        let mut camera = Bx20Camera::new(100);
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);
        let zoom = Command {
            id: MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
            params: [2.0, 25.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };

        let outcome = camera.handle_command(&mut payload, &zoom);

        assert_eq!(outcome.result, MavResult::MAV_RESULT_ACCEPTED);
        assert!(outcome.follow_up.is_empty());
    }

    #[test]
    fn camera_settings_report_zoom_in_percent() {
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);
        payload.zoom.set_fraction(0.25);

        let MavMessage::CAMERA_SETTINGS(settings) = camera_settings(&payload, 0) else {
            unreachable!("camera_settings builds CAMERA_SETTINGS");
        };

        assert_eq!(settings.zoomLevel, 25.0);
    }

    #[test]
    fn thermal_stream_reports_fixed_field_of_view() {
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);
        payload.zoom.set_factor(10.0);
        payload.source = VideoSource::Thermal;

        let MavMessage::VIDEO_STREAM_STATUS(status) = stream_status(&payload) else {
            unreachable!("stream_status builds VIDEO_STREAM_STATUS");
        };

        assert_eq!(status.hfov, 32);
        assert!(
            status
                .flags
                .contains(VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_THERMAL)
        );
    }
}
