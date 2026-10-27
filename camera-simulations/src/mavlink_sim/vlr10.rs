//! The VLR-10: separate visible and thermal streams selected with
//! `MAV_CMD_VIDEO_START_STREAMING`, step zoom, and the zoom factor in `CAMERA_SETTINGS.zoomLevel`.

use mavlink::dialects::common::{
    CAMERA_INFORMATION_DATA, CAMERA_SETTINGS_DATA, CameraCapFlags, CameraMode, CameraZoomType,
    MavCmd, MavMessage, VIDEO_STREAM_INFORMATION_DATA, VIDEO_STREAM_STATUS_DATA,
    VideoStreamEncoding, VideoStreamStatusFlags, VideoStreamType,
};

use super::{
    camera::{fixed_text, set_camera_zoom, zoomed_hfov_deg},
    command::{Command, CommandOutcome},
    session::CameraProfile,
};
use crate::payload::{Payload, VideoSource};

pub const MAX_ZOOM_FACTOR: f32 = 10.0;

const CAMERA_SETTINGS_ID: u32 = 260;
const VIDEO_STREAM_INFORMATION_ID: u32 = 269;
const VIDEO_STREAM_STATUS_ID: u32 = 270;
const VISIBLE_STREAM_ID: u8 = 1;
const THERMAL_STREAM_ID: u8 = 2;
const VISIBLE_WIDE_HFOV_DEG: f32 = 58.0;
const THERMAL_WIDE_HFOV_DEG: f32 = 24.0;

pub struct Vlr10Camera;

impl Vlr10Camera {
    fn start_streaming(&self, payload: &mut Payload, command: &Command) -> CommandOutcome {
        match command.param(1) as u8 {
            VISIBLE_STREAM_ID => payload.source = VideoSource::Visible,
            THERMAL_STREAM_ID => payload.source = VideoSource::Thermal,
            _ => return CommandOutcome::denied(),
        }
        CommandOutcome::accepted()
    }
}

impl CameraProfile for Vlr10Camera {
    fn camera_information(&self, time_boot_ms: u32) -> MavMessage {
        MavMessage::CAMERA_INFORMATION(CAMERA_INFORMATION_DATA {
            time_boot_ms,
            firmware_version: 0x0203_0000,
            focal_length: 6.0,
            sensor_size_h: 7.6,
            sensor_size_v: 5.7,
            flags: CameraCapFlags::CAMERA_CAP_FLAGS_HAS_VIDEO_STREAM
                | CameraCapFlags::CAMERA_CAP_FLAGS_HAS_BASIC_ZOOM,
            resolution_h: 1280,
            resolution_v: 720,
            cam_definition_version: 0,
            vendor_name: fixed_text("Vallerin"),
            model_name: fixed_text("VLR-10"),
            lens_id: 0,
            cam_definition_uri: "".into(),
            gimbal_device_id: 0,
            camera_device_id: 0,
        })
    }

    fn handle_command(&mut self, payload: &mut Payload, command: &Command) -> CommandOutcome {
        match command.id {
            MavCmd::MAV_CMD_SET_CAMERA_ZOOM => {
                let supported = [CameraZoomType::ZOOM_TYPE_STEP];
                CommandOutcome::result(set_camera_zoom(&mut payload.zoom, command, &supported))
            }
            MavCmd::MAV_CMD_VIDEO_START_STREAMING => self.start_streaming(payload, command),
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
            VIDEO_STREAM_INFORMATION_ID => Some(
                stream_ids()
                    .map(|id| stream_information(payload, id))
                    .collect(),
            ),
            VIDEO_STREAM_STATUS_ID => {
                Some(stream_ids().map(|id| stream_status(payload, id)).collect())
            }
            _ => None,
        }
    }
}

fn stream_ids() -> impl Iterator<Item = u8> {
    [VISIBLE_STREAM_ID, THERMAL_STREAM_ID].into_iter()
}

fn active_stream_id(payload: &Payload) -> u8 {
    match payload.source {
        VideoSource::Visible => VISIBLE_STREAM_ID,
        VideoSource::Thermal => THERMAL_STREAM_ID,
    }
}

fn camera_settings(payload: &Payload, time_boot_ms: u32) -> MavMessage {
    MavMessage::CAMERA_SETTINGS(CAMERA_SETTINGS_DATA {
        time_boot_ms,
        mode_id: CameraMode::CAMERA_MODE_VIDEO,
        zoomLevel: payload.zoom.factor(),
        focusLevel: f32::NAN,
        camera_device_id: 0,
    })
}

fn stream_flags(payload: &Payload, stream_id: u8) -> VideoStreamStatusFlags {
    let mut flags = VideoStreamStatusFlags::empty();
    if stream_id == active_stream_id(payload) {
        flags |= VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_RUNNING;
    }
    if stream_id == THERMAL_STREAM_ID {
        flags |= VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_THERMAL;
    }
    flags
}

fn stream_resolution(stream_id: u8) -> (u16, u16) {
    if stream_id == THERMAL_STREAM_ID {
        (640, 512)
    } else {
        (1280, 720)
    }
}

fn stream_hfov_deg(payload: &Payload, stream_id: u8) -> u16 {
    let wide_hfov_deg = if stream_id == THERMAL_STREAM_ID {
        THERMAL_WIDE_HFOV_DEG
    } else {
        VISIBLE_WIDE_HFOV_DEG
    };
    zoomed_hfov_deg(wide_hfov_deg, payload.zoom.factor()).round() as u16
}

fn stream_information(payload: &Payload, stream_id: u8) -> MavMessage {
    let (resolution_h, resolution_v) = stream_resolution(stream_id);
    let name = if stream_id == THERMAL_STREAM_ID {
        "thermal"
    } else {
        "visible"
    };
    MavMessage::VIDEO_STREAM_INFORMATION(VIDEO_STREAM_INFORMATION_DATA {
        framerate: 25.0,
        bitrate: 2_500_000,
        flags: stream_flags(payload, stream_id),
        resolution_h,
        resolution_v,
        rotation: 0,
        hfov: stream_hfov_deg(payload, stream_id),
        stream_id,
        count: 2,
        mavtype: VideoStreamType::VIDEO_STREAM_TYPE_RTSP,
        name: name.into(),
        uri: format!("rtsp://127.0.0.1:8554/{name}").as_str().into(),
        encoding: VideoStreamEncoding::VIDEO_STREAM_ENCODING_H265,
        camera_device_id: 0,
    })
}

fn stream_status(payload: &Payload, stream_id: u8) -> MavMessage {
    let (resolution_h, resolution_v) = stream_resolution(stream_id);
    MavMessage::VIDEO_STREAM_STATUS(VIDEO_STREAM_STATUS_DATA {
        framerate: 25.0,
        bitrate: 2_500_000,
        flags: stream_flags(payload, stream_id),
        resolution_h,
        resolution_v,
        rotation: 0,
        hfov: stream_hfov_deg(payload, stream_id),
        stream_id,
        camera_device_id: 0,
    })
}

#[cfg(test)]
mod tests {
    use mavlink::dialects::common::MavResult;

    use super::*;

    fn command(id: MavCmd, param1: f32, param2: f32) -> Command {
        Command {
            id,
            params: [param1, param2, 0.0, 0.0, 0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn start_streaming_selects_the_thermal_stream() {
        let mut camera = Vlr10Camera;
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);

        let outcome = camera.handle_command(
            &mut payload,
            &command(MavCmd::MAV_CMD_VIDEO_START_STREAMING, 2.0, 0.0),
        );

        assert_eq!(outcome.result, MavResult::MAV_RESULT_ACCEPTED);
        assert_eq!(payload.source, VideoSource::Thermal);
    }

    #[test]
    fn camera_source_command_is_unsupported() {
        let mut camera = Vlr10Camera;
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);

        let outcome = camera.handle_command(
            &mut payload,
            &command(MavCmd::MAV_CMD_SET_CAMERA_SOURCE, 0.0, 2.0),
        );

        assert_eq!(outcome.result, MavResult::MAV_RESULT_UNSUPPORTED);
    }

    #[test]
    fn step_zoom_is_accepted() {
        let mut camera = Vlr10Camera;
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);

        let outcome = camera.handle_command(
            &mut payload,
            &command(MavCmd::MAV_CMD_SET_CAMERA_ZOOM, 0.0, 1.0),
        );

        assert_eq!(outcome.result, MavResult::MAV_RESULT_ACCEPTED);
        assert_eq!(payload.zoom.factor(), 2.0);
    }

    #[test]
    fn range_zoom_is_denied() {
        let mut camera = Vlr10Camera;
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);

        let outcome = camera.handle_command(
            &mut payload,
            &command(MavCmd::MAV_CMD_SET_CAMERA_ZOOM, 2.0, 50.0),
        );

        assert_eq!(outcome.result, MavResult::MAV_RESULT_DENIED);
    }

    #[test]
    fn camera_settings_report_zoom_factor() {
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);
        payload.zoom.set_factor(4.0);

        let MavMessage::CAMERA_SETTINGS(settings) = camera_settings(&payload, 0) else {
            unreachable!("camera_settings builds CAMERA_SETTINGS");
        };

        assert_eq!(settings.zoomLevel, 4.0);
    }

    #[test]
    fn both_fields_of_view_narrow_with_zoom() {
        let mut payload = Payload::new(MAX_ZOOM_FACTOR);
        assert_eq!(stream_hfov_deg(&payload, VISIBLE_STREAM_ID), 58);
        assert_eq!(stream_hfov_deg(&payload, THERMAL_STREAM_ID), 24);

        payload.zoom.set_factor(2.0);

        assert_eq!(stream_hfov_deg(&payload, VISIBLE_STREAM_ID), 31);
        assert_eq!(stream_hfov_deg(&payload, THERMAL_STREAM_ID), 12);
    }

    #[test]
    fn only_the_active_stream_is_running() {
        let payload = Payload::new(MAX_ZOOM_FACTOR);

        let running: Vec<bool> = stream_ids()
            .map(|id| {
                stream_flags(&payload, id)
                    .contains(VideoStreamStatusFlags::VIDEO_STREAM_STATUS_FLAGS_RUNNING)
            })
            .collect();

        assert_eq!(running, [true, false]);
    }
}
