//! Command handling of the simulated pod. Write commands are silent; read commands get a
//! reply from the addressed module (`M` lens, `D` system) to the requester.

use tracing::debug;

use super::{
    fields::{decimal, format_signed_hex4, unsigned_hex2},
    frame::{Control, Frame},
};
use crate::payload::{Payload, ThermalPalette, VideoSource};

const MAX_ZOOM_FACTOR: f32 = 30.0;
const LENS: u8 = b'M';
const SYSTEM: u8 = b'D';
/// Zoom position reported at full zoom; the lens reports 0 at 1x.
const MAX_ZOOM_POSITION: f32 = 16384.0;
/// `IMG` palette codes, by value.
const PALETTES: [(u8, ThermalPalette); 10] = [
    (0x00, ThermalPalette::WhiteHot),
    (0x01, ThermalPalette::Lava),
    (0x02, ThermalPalette::Ironbow),
    (0x03, ThermalPalette::HotIron),
    (0x04, ThermalPalette::Medical),
    (0x05, ThermalPalette::Arctic),
    (0x06, ThermalPalette::Rainbow),
    (0x07, ThermalPalette::RainbowHighContrast),
    (0x08, ThermalPalette::RedHot),
    (0x09, ThermalPalette::BlackHot),
];
/// `PIP` codes for the full-frame sources; the picture-in-picture codes 01 and 02 are ignored.
const SOURCES: [(u8, VideoSource); 2] =
    [(0x00, VideoSource::Visible), (0x03, VideoSource::Thermal)];
const NEXT: u8 = 0x0A;
const PREVIOUS: u8 = 0x0B;

pub struct Pod {
    pub payload: Payload,
}

impl Pod {
    pub fn new() -> Self {
        Self {
            payload: Payload::new(MAX_ZOOM_FACTOR),
        }
    }

    /// Applies a frame and returns the reply, if the command has one.
    pub fn handle(&mut self, frame: &Frame) -> Option<Frame> {
        let reply_data = match frame.destination {
            LENS => self.handle_lens(frame),
            SYSTEM => self.handle_system(frame),
            _ => ignore(frame),
        };
        let data = reply_data?;
        Some(Frame {
            source: frame.destination,
            destination: frame.source,
            control: Control::Read,
            identifier: frame.identifier,
            data: data.into_bytes(),
        })
    }

    fn handle_lens(&mut self, frame: &Frame) -> Option<String> {
        let zoom = &mut self.payload.zoom;
        match (&frame.identifier, frame.control) {
            (b"ZOM", Control::Read) => Some(format_signed_hex4(self.zoom_position())),
            (b"MUL", Control::Write) => {
                let tenths = decimal(frame.data.get(..4)?)?;
                zoom.set_factor(tenths as f32 / 10.0);
                None
            }
            (b"MUL", Control::Read) => {
                Some(format!("{:04}", (zoom.factor() * 10.0).round() as u32))
            }
            _ => ignore(frame),
        }
    }

    fn handle_system(&mut self, frame: &Frame) -> Option<String> {
        match (&frame.identifier, frame.control) {
            (b"PIP", Control::Write) => {
                self.payload.source =
                    select(&SOURCES, self.payload.source, unsigned_hex2(&frame.data)?)?;
                None
            }
            (b"PIP", Control::Read) => {
                Some(format!("{:02X}", code_of(&SOURCES, self.payload.source)))
            }
            (b"IMG", Control::Write) => {
                self.payload.palette =
                    select(&PALETTES, self.payload.palette, unsigned_hex2(&frame.data)?)?;
                None
            }
            (b"IMG", Control::Read) => {
                Some(format!("{:02X}", code_of(&PALETTES, self.payload.palette)))
            }
            _ => ignore(frame),
        }
    }

    fn zoom_position(&self) -> i16 {
        (self.payload.zoom.fraction() * MAX_ZOOM_POSITION).round() as i16
    }
}

fn ignore(frame: &Frame) -> Option<String> {
    debug!(
        "ignoring {} {:?} to {}: not simulated",
        frame.identifier(),
        frame.control,
        char::from(frame.destination)
    );
    None
}

/// Picks the option for a code: its own code, or `NEXT`/`PREVIOUS` relative to `current`.
fn select<T: Copy + PartialEq>(options: &[(u8, T)], current: T, code: u8) -> Option<T> {
    let current_index = options
        .iter()
        .position(|(_, option)| *option == current)
        .unwrap_or(0);
    let index = match code {
        NEXT => (current_index + 1) % options.len(),
        PREVIOUS => (current_index + options.len() - 1) % options.len(),
        code => options.iter().position(|(known, _)| *known == code)?,
    };
    options.get(index).map(|(_, option)| *option)
}

fn code_of<T: PartialEq>(options: &[(u8, T)], value: T) -> u8 {
    options
        .iter()
        .find(|(_, option)| *option == value)
        .map_or(0, |(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(destination: u8, identifier: &[u8; 3], data: &str) -> Frame {
        Frame {
            source: b'P',
            destination,
            control: Control::Write,
            identifier: *identifier,
            data: data.as_bytes().to_vec(),
        }
    }

    fn read(destination: u8, identifier: &[u8; 3]) -> Frame {
        Frame {
            control: Control::Read,
            ..write(destination, identifier, "00")
        }
    }

    #[test]
    fn write_commands_have_no_reply() {
        let mut pod = Pod::new();
        assert_eq!(pod.handle(&write(LENS, b"MUL", "0155")), None);
    }

    #[test]
    fn zoom_read_replies_from_the_lens_to_the_requester() {
        let mut pod = Pod::new();
        pod.handle(&write(LENS, b"MUL", "0155"));

        let reply = pod.handle(&read(LENS, b"MUL")).unwrap();

        assert_eq!(reply.source, LENS);
        assert_eq!(reply.destination, b'P');
        assert_eq!(reply.control, Control::Read);
        assert_eq!(reply.data, b"0155");
    }

    #[test]
    fn zoom_position_runs_up_to_full_zoom() {
        let mut pod = Pod::new();
        pod.handle(&write(LENS, b"MUL", "0300"));

        assert_eq!(pod.handle(&read(LENS, b"ZOM")).unwrap().data, b"4000");
    }

    #[test]
    fn continuous_zoom_is_ignored() {
        let mut pod = Pod::new();
        pod.handle(&write(LENS, b"ZMC", "02"));

        assert_eq!(pod.payload.zoom.factor(), 1.0);
    }

    #[test]
    fn gimbal_commands_are_ignored() {
        let mut pod = Pod::new();
        assert_eq!(pod.handle(&read(b'G', b"GAC")), None);
    }

    #[test]
    fn palette_next_wraps_around() {
        let mut pod = Pod::new();
        pod.handle(&write(SYSTEM, b"IMG", "09"));
        pod.handle(&write(SYSTEM, b"IMG", "0A"));

        assert_eq!(pod.payload.palette, ThermalPalette::WhiteHot);
    }

    #[test]
    fn unknown_palette_code_is_ignored() {
        let mut pod = Pod::new();
        pod.handle(&write(SYSTEM, b"IMG", "0F"));

        assert_eq!(pod.payload.palette, ThermalPalette::WhiteHot);
    }

    #[test]
    fn source_switches_to_thermal() {
        let mut pod = Pod::new();
        pod.handle(&write(SYSTEM, b"PIP", "03"));

        assert_eq!(pod.payload.source, VideoSource::Thermal);
        assert_eq!(pod.handle(&read(SYSTEM, b"PIP")).unwrap().data, b"03");
    }

    #[test]
    fn picture_in_picture_codes_are_ignored() {
        let mut pod = Pod::new();
        pod.handle(&write(SYSTEM, b"PIP", "02"));

        assert_eq!(pod.payload.source, VideoSource::Visible);
    }

    #[test]
    fn next_source_toggles_between_visible_and_thermal() {
        let mut pod = Pod::new();
        pod.handle(&write(SYSTEM, b"PIP", "0A"));
        assert_eq!(pod.payload.source, VideoSource::Thermal);

        pod.handle(&write(SYSTEM, b"PIP", "0A"));
        assert_eq!(pod.payload.source, VideoSource::Visible);
    }
}
