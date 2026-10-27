//! Camera protocol pieces both MAVLink simulators share.

use mavlink::dialects::common::{CameraZoomType, MavResult};
use num_traits::FromPrimitive;

use super::command::Command;
use crate::payload::zoom::Zoom;

/// Size of one `ZOOM_TYPE_STEP` step, as a change in zoom factor.
const ZOOM_STEP_FACTOR: f32 = 1.0;

pub fn fixed_text<const N: usize>(text: &str) -> [u8; N] {
    let mut bytes = [0u8; N];
    for (slot, byte) in bytes.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    bytes
}

/// Horizontal field of view of a lens with `wide_hfov_deg` at 1x, zoomed to `zoom_factor`.
pub fn zoomed_hfov_deg(wide_hfov_deg: f32, zoom_factor: f32) -> f32 {
    let half_width = (wide_hfov_deg.to_radians() / 2.0).tan() / zoom_factor;
    (2.0 * half_width.atan()).to_degrees()
}

/// Applies `MAV_CMD_SET_CAMERA_ZOOM` for the zoom types in `supported`.
pub fn set_camera_zoom(
    zoom: &mut Zoom,
    command: &Command,
    supported: &[CameraZoomType],
) -> MavResult {
    let zoom_type = CameraZoomType::from_f32(command.param(1));
    let value = command.param(2);
    let Some(zoom_type) = zoom_type.filter(|zoom_type| supported.contains(zoom_type)) else {
        return MavResult::MAV_RESULT_DENIED;
    };
    if !value.is_finite() {
        return MavResult::MAV_RESULT_DENIED;
    }
    match zoom_type {
        CameraZoomType::ZOOM_TYPE_STEP if value != 0.0 => {
            zoom.set_factor(zoom.factor() + value.signum() * ZOOM_STEP_FACTOR)
        }
        CameraZoomType::ZOOM_TYPE_STEP => {}
        CameraZoomType::ZOOM_TYPE_RANGE if (0.0..=100.0).contains(&value) => {
            zoom.set_fraction(value / 100.0)
        }
        _ => return MavResult::MAV_RESULT_DENIED,
    }
    MavResult::MAV_RESULT_ACCEPTED
}

#[cfg(test)]
mod tests {
    use mavlink::dialects::common::MavCmd;

    use super::*;

    const STEP_AND_RANGE: [CameraZoomType; 2] = [
        CameraZoomType::ZOOM_TYPE_STEP,
        CameraZoomType::ZOOM_TYPE_RANGE,
    ];

    fn zoom_command(zoom_type: CameraZoomType, value: f32) -> Command {
        Command {
            id: MavCmd::MAV_CMD_SET_CAMERA_ZOOM,
            params: [zoom_type as u32 as f32, value, 0.0, f32::NAN, 0.0, 0.0, 0.0],
        }
    }

    fn zoom(zoom: &mut Zoom, zoom_type: CameraZoomType, value: f32) -> MavResult {
        set_camera_zoom(zoom, &zoom_command(zoom_type, value), &STEP_AND_RANGE)
    }

    #[test]
    fn range_zoom_maps_percentage_to_factor() {
        let mut lens = Zoom::new(11.0);
        let result = zoom(&mut lens, CameraZoomType::ZOOM_TYPE_RANGE, 50.0);
        assert_eq!(result, MavResult::MAV_RESULT_ACCEPTED);
        assert_eq!(lens.factor(), 6.0);
    }

    #[test]
    fn step_zoom_moves_one_step_in_the_direction_of_the_value() {
        let mut lens = Zoom::new(11.0);
        zoom(&mut lens, CameraZoomType::ZOOM_TYPE_STEP, 3.0);
        zoom(&mut lens, CameraZoomType::ZOOM_TYPE_STEP, 1.0);
        zoom(&mut lens, CameraZoomType::ZOOM_TYPE_STEP, -1.0);
        zoom(&mut lens, CameraZoomType::ZOOM_TYPE_STEP, 0.0);
        assert_eq!(lens.factor(), 2.0);
    }

    #[test]
    fn unsupported_zoom_type_is_denied() {
        let mut lens = Zoom::new(11.0);
        let command = zoom_command(CameraZoomType::ZOOM_TYPE_RANGE, 50.0);
        let result = set_camera_zoom(&mut lens, &command, &[CameraZoomType::ZOOM_TYPE_STEP]);
        assert_eq!(result, MavResult::MAV_RESULT_DENIED);
    }

    #[test]
    fn continuous_zoom_is_denied() {
        let mut lens = Zoom::new(11.0);
        let result = zoom(&mut lens, CameraZoomType::ZOOM_TYPE_CONTINUOUS, 1.0);
        assert_eq!(result, MavResult::MAV_RESULT_DENIED);
    }

    #[test]
    fn out_of_range_percentage_is_denied() {
        let mut lens = Zoom::new(11.0);
        let result = zoom(&mut lens, CameraZoomType::ZOOM_TYPE_RANGE, 150.0);
        assert_eq!(result, MavResult::MAV_RESULT_DENIED);
    }

    #[test]
    fn hfov_narrows_with_zoom() {
        assert!((zoomed_hfov_deg(60.0, 1.0) - 60.0).abs() < 1e-3);
        assert!(zoomed_hfov_deg(60.0, 4.0) < 20.0);
    }
}
