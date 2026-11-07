//! The LC-30T's ASCII frames (protocol section 3). This file only turns frames into text and
//! back. It does no I/O, so it is easy to test against the examples in the vendor document.
//!
//! ```text
//! #tp P M 4 w MUL 0155 08      "zoom to 15.5x", from protocol section 5
//! │   │ │ │ │ │   │    └ checksum: the low 8 bits of the byte sum of everything before it
//! │   │ │ │ │ │   └ data: as many characters as the length says
//! │   │ │ │ │ └ identifier: what is read or written, e.g. MUL, the zoom
//! │   │ │ │ └ control: w (write) or r (read)
//! │   │ │ └ data length: one hex digit, 0 to F
//! │   │ └ target: who the frame is for, e.g. M, the lens
//! │   └ source: who sent it; we are P, the network host
//! └ header: #tp, or #TP for a frame that always carries 2 data characters
//! ```

use std::ops::Range;

use thiserror::Error;

/// Addresses (protocol 3.2). We are the network host; each module of the pod answers for itself.
pub const HOST: char = 'P';
pub const LENS: char = 'M';
pub const SYSTEM: char = 'D';

/// Characters in a frame besides its data: header 3, addresses 2, length 1, control 1,
/// identifier 3 and checksum 2.
const OVERHEAD: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Read,
    Write,
}

/// One frame, either one we send or one the pod sent.
///
/// Rust: `Clone` is required by the broadcast channel in `link.rs`, which gives every
/// subscriber its own copy of each frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub source: char,
    pub target: char,
    pub control: Control,
    pub identifier: String,
    pub data: String,
}

impl Frame {
    /// A read request. Its data is always "00" (protocol 3.3).
    pub fn read(target: char, identifier: &str) -> Frame {
        Frame::new(target, Control::Read, identifier, "00")
    }

    /// A write request. The pod never answers one (protocol 3.3).
    pub fn write(target: char, identifier: &str, data: &str) -> Frame {
        Frame::new(target, Control::Write, identifier, data)
    }

    fn new(target: char, control: Control, identifier: &str, data: &str) -> Frame {
        Frame {
            source: HOST,
            target,
            control,
            identifier: identifier.into(),
            data: data.into(),
        }
    }

    /// The frame as text, ready to send. We always use the `#tp` header, which fits any data.
    pub fn encode(&self) -> String {
        // The length is a single hex digit, so the data can't be longer than 15 characters.
        // Rust: `debug_assert!` checks this in debug builds and tests, and costs nothing in a
        // release build.
        debug_assert!(self.data.len() <= 0xF, "frame data too long: {}", self.data);
        let control = match self.control {
            Control::Read => 'r',
            Control::Write => 'w',
        };
        // Rust: `{:X}` writes a number in uppercase hex, so a length of 4 becomes "4".
        let mut text = format!(
            "#tp{}{}{:X}{control}{}{}",
            self.source,
            self.target,
            self.data.len(),
            self.identifier,
            self.data
        );
        let check = checksum(&text);
        // Rust: `{:02X}` is two uppercase hex digits, with a leading zero if needed: 8 is "08".
        text.push_str(&format!("{check:02X}"));
        text
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("the datagram isn't text")]
    NotText,
    #[error("unknown header `{0}`")]
    Header(String),
    #[error("the frame is cut short")]
    Truncated,
    #[error("`{0}` isn't a hex length")]
    Length(char),
    #[error("a #TP frame carries 2 data characters, not {0}")]
    FixedLength(usize),
    #[error("`{0}` isn't r or w")]
    Control(char),
    #[error("the checksum says {found}, but the frame adds up to {expected:02X}")]
    Checksum { found: String, expected: u8 },
}

/// Every frame in one datagram; a datagram may hold several (protocol 6.3). Anything before a
/// `#` is skipped, and after a broken frame decoding carries on at the next `#`, so one bad
/// frame doesn't hide the good ones after it.
pub fn decode(datagram: &[u8]) -> Vec<Result<Frame, FrameError>> {
    // Rust: `from_utf8` checks that the bytes are valid text before we use them as a `&str`.
    let Ok(mut rest) = std::str::from_utf8(datagram) else {
        return vec![Err(FrameError::NotText)];
    };
    let mut frames = Vec::new();
    while let Some(start) = rest.find('#') {
        rest = &rest[start..];
        match decode_one(rest) {
            Ok((frame, length)) => {
                frames.push(Ok(frame));
                rest = &rest[length..];
            }
            Err(error) => {
                frames.push(Err(error));
                rest = &rest[1..]; // skip this `#` and look for the next one
            }
        }
    }
    frames
}

/// The frame at the start of `text`, and how many characters it takes up.
fn decode_one(text: &str) -> Result<(Frame, usize), FrameError> {
    // Rust: `get` returns `None` instead of panicking when the text is too short, and `ok_or`
    // turns that into an error. A cut-off frame becomes an error, never a crash.
    let part = |range: Range<usize>| text.get(range).ok_or(FrameError::Truncated);
    let character = |index: usize| {
        text.as_bytes()
            .get(index)
            .map(|&byte| char::from(byte))
            .ok_or(FrameError::Truncated)
    };

    let fixed_length = match part(0..3)? {
        "#tp" => false,
        "#TP" => true,
        other => return Err(FrameError::Header(other.into())),
    };
    let source = character(3)?;
    let target = character(4)?;
    let length_digit = character(5)?;
    // Rust: `to_digit(16)` reads one hex digit: '4' gives 4, 'F' gives 15.
    let length = length_digit
        .to_digit(16)
        .ok_or(FrameError::Length(length_digit))? as usize;
    if fixed_length && length != 2 {
        return Err(FrameError::FixedLength(length));
    }
    let control = match character(6)? {
        'r' => Control::Read,
        'w' => Control::Write,
        other => return Err(FrameError::Control(other)),
    };
    let identifier = part(7..10)?;
    let data = part(10..10 + length)?;
    let found = part(10 + length..OVERHEAD + length)?;
    // The checksum covers everything before it, the header included (protocol 3.4).
    let expected = checksum(part(0..10 + length)?);
    if u8::from_str_radix(found, 16) != Ok(expected) {
        return Err(FrameError::Checksum {
            found: found.into(),
            expected,
        });
    }
    let frame = Frame {
        source,
        target,
        control,
        identifier: identifier.into(),
        data: data.into(),
    };
    Ok((frame, OVERHEAD + length))
}

/// The low 8 bits of the byte sum (protocol 3.4).
///
/// Rust: `wrapping_add` lets the `u8` sum roll over from 255 to 0, where `+` would stop with
/// an overflow error. Rolling over is exactly "keep the low 8 bits".
fn checksum(text: &str) -> u8 {
    text.bytes().fold(0u8, |sum, byte| sum.wrapping_add(byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a datagram that holds exactly one valid frame.
    fn decode_one_frame(text: &str) -> Frame {
        let mut frames = decode(text.as_bytes());
        assert_eq!(frames.len(), 1, "expected one frame in {text}");
        frames.remove(0).unwrap()
    }

    #[test]
    fn checksum_matches_the_worked_example() {
        // Protocol 3.4: "#tpPD2wIMG02" adds up to 0x383, so the checksum is 83.
        assert_eq!(checksum("#tpPD2wIMG02"), 0x83);
    }

    #[test]
    fn encodes_the_documented_requests() {
        // Every request in protocol section 5 that uses the #tp header.
        assert_eq!(
            Frame::write(LENS, "MUL", "0155").encode(),
            "#tpPM4wMUL015508"
        );
        assert_eq!(
            Frame::write(LENS, "MUL", "0010").encode(),
            "#tpPM4wMUL0010FE"
        );
        assert_eq!(Frame::read(LENS, "MUL").encode(), "#tpPM2rMUL0096");
        assert_eq!(Frame::read(LENS, "ZOM").encode(), "#tpPM2rZOM009E");
        assert_eq!(Frame::write(SYSTEM, "PIP", "03").encode(), "#tpPD2wPIP0390");
        assert_eq!(Frame::write(SYSTEM, "PIP", "00").encode(), "#tpPD2wPIP008D");
        assert_eq!(Frame::read(SYSTEM, "PIP").encode(), "#tpPD2rPIP0088");
        assert_eq!(Frame::write(SYSTEM, "IMG", "09").encode(), "#tpPD2wIMG098A");
        assert_eq!(Frame::read(SYSTEM, "IMG").encode(), "#tpPD2rIMG007C");
    }

    #[test]
    fn decodes_the_documented_replies() {
        let zoom = decode_one_frame("#tpMP4rMUL015503");
        assert_eq!(
            zoom,
            Frame {
                source: LENS,
                target: HOST,
                control: Control::Read,
                identifier: "MUL".into(),
                data: "0155".into(),
            }
        );
        assert_eq!(decode_one_frame("#tpMP4rZOM200002").data, "2000");
        assert_eq!(decode_one_frame("#tpDP2rPIP038B").data, "03");
        assert_eq!(decode_one_frame("#tpDP2rIMG0985").data, "09");
    }

    #[test]
    fn decodes_the_documented_fixed_length_frame() {
        // "Palette: next", the only #TP example in protocol section 5.
        let next = decode_one_frame("#TPPD2wIMG0A52");
        assert_eq!(next.control, Control::Write);
        assert_eq!(next.data, "0A");
    }

    #[test]
    fn refuses_a_wrong_checksum() {
        let frames = decode(b"#tpMP4rMUL015504");
        assert_eq!(
            frames,
            [Err(FrameError::Checksum {
                found: "04".into(),
                expected: 0x03
            })]
        );
    }

    #[test]
    fn refuses_a_cut_off_frame() {
        assert_eq!(decode(b"#tpMP4rMUL01"), [Err(FrameError::Truncated)]);
    }

    #[test]
    fn a_fixed_length_frame_carries_two_characters() {
        assert_eq!(
            decode(b"#TPPM4wMUL015508"),
            [Err(FrameError::FixedLength(4))]
        );
    }

    #[test]
    fn finds_every_frame_in_a_datagram() {
        // Noise first, then two good frames with a broken one between them.
        let frames = decode(b"noise#tpDP2rPIP038B#tpDP2rIMG0900#tpDP2rIMG0985");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].as_ref().unwrap().identifier, "PIP");
        assert!(frames[1].is_err());
        assert_eq!(frames[2].as_ref().unwrap().data, "09");
    }
}
