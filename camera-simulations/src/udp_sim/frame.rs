//! ASCII frame format of the UDP protocol:
//! `#tp` | source | destination | length | control | identifier(3) | data(length) | checksum(2).
//!
//! The length is one hex character. `#TP` frames always carry two data characters. The
//! checksum is the byte sum of everything before it, modulo 256, as two uppercase hex digits.

use thiserror::Error;

const HEADER_VARIABLE: &[u8; 3] = b"#tp";
const HEADER_FIXED: &[u8; 3] = b"#TP";
const FIXED_DATA_LENGTH: usize = 2;
/// Bytes of a frame apart from its data: header, addresses, length, control, identifier, checksum.
const FRAME_OVERHEAD: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Read,
    Write,
}

impl Control {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'r' => Some(Self::Read),
            b'w' => Some(Self::Write),
            _ => None,
        }
    }

    fn as_byte(self) -> u8 {
        match self {
            Self::Read => b'r',
            Self::Write => b'w',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub source: u8,
    pub destination: u8,
    pub control: Control,
    pub identifier: [u8; 3],
    pub data: Vec<u8>,
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum FrameError {
    #[error("unsupported frame header {0:?}")]
    UnsupportedHeader(String),
    #[error("frame is truncated")]
    Truncated,
    #[error("invalid length character {0:?}")]
    InvalidLength(char),
    #[error("fixed-length frame with {0} data characters")]
    FixedLengthMismatch(usize),
    #[error("invalid control character {0:?}")]
    InvalidControl(char),
    #[error("checksum mismatch: frame says {found}, computed {computed:02X}")]
    ChecksumMismatch { found: String, computed: u8 },
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = HEADER_VARIABLE.to_vec();
        bytes.push(self.source);
        bytes.push(self.destination);
        bytes.push(hex_digit(self.data.len()));
        bytes.push(self.control.as_byte());
        bytes.extend_from_slice(&self.identifier);
        bytes.extend_from_slice(&self.data);
        bytes.extend(format!("{:02X}", checksum(&bytes)).bytes());
        bytes
    }

    pub fn identifier(&self) -> &str {
        std::str::from_utf8(&self.identifier).unwrap_or("???")
    }
}

/// Parses every frame in a datagram. Bytes before a `#` are skipped; a malformed frame
/// yields an error and parsing resumes at the next `#`.
pub fn parse_datagram(datagram: &[u8]) -> Vec<Result<Frame, FrameError>> {
    let mut results = Vec::new();
    let mut rest = datagram;
    while let Some(start) = rest.iter().position(|byte| *byte == b'#') {
        rest = rest.get(start..).unwrap_or_default();
        match parse_frame(rest) {
            Ok((frame, length)) => {
                results.push(Ok(frame));
                rest = rest.get(length..).unwrap_or_default();
            }
            Err(e) => {
                results.push(Err(e));
                rest = rest.get(1..).unwrap_or_default();
            }
        }
    }
    results
}

fn parse_frame(bytes: &[u8]) -> Result<(Frame, usize), FrameError> {
    let header = bytes.get(..3).ok_or(FrameError::Truncated)?;
    let is_fixed = header == HEADER_FIXED;
    if header != HEADER_VARIABLE && !is_fixed {
        return Err(FrameError::UnsupportedHeader(
            String::from_utf8_lossy(header).into_owned(),
        ));
    }
    let [source, destination, length_char, control_char] = *fixed::<4>(bytes, 3)?;
    let data_length =
        hex_value(length_char).ok_or(FrameError::InvalidLength(char::from(length_char)))?;
    if is_fixed && data_length != FIXED_DATA_LENGTH {
        return Err(FrameError::FixedLengthMismatch(data_length));
    }
    let control = Control::from_byte(control_char)
        .ok_or(FrameError::InvalidControl(char::from(control_char)))?;
    let identifier = *fixed::<3>(bytes, 7)?;
    let frame_length = FRAME_OVERHEAD + data_length;
    let data = bytes
        .get(10..10 + data_length)
        .ok_or(FrameError::Truncated)?
        .to_vec();
    let checksum_chars = bytes
        .get(10 + data_length..frame_length)
        .ok_or(FrameError::Truncated)?;
    verify_checksum(
        bytes.get(..10 + data_length).unwrap_or_default(),
        checksum_chars,
    )?;
    let frame = Frame {
        source,
        destination,
        control,
        identifier,
        data,
    };
    Ok((frame, frame_length))
}

fn fixed<const N: usize>(bytes: &[u8], offset: usize) -> Result<&[u8; N], FrameError> {
    bytes
        .get(offset..offset + N)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(FrameError::Truncated)
}

fn verify_checksum(covered: &[u8], checksum_chars: &[u8]) -> Result<(), FrameError> {
    let computed = checksum(covered);
    let found = String::from_utf8_lossy(checksum_chars).into_owned();
    if u8::from_str_radix(&found, 16).ok() == Some(computed) {
        return Ok(());
    }
    Err(FrameError::ChecksumMismatch { found, computed })
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

fn hex_value(character: u8) -> Option<usize> {
    char::from(character)
        .to_digit(16)
        .map(|digit| digit as usize)
}

fn hex_digit(value: usize) -> u8 {
    b"0123456789ABCDEF"
        .get(value & 0xF)
        .copied()
        .unwrap_or(b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(text: &str) -> Result<Frame, FrameError> {
        let mut results = parse_datagram(text.as_bytes());
        assert_eq!(results.len(), 1, "expected one frame in {text}");
        results.remove(0)
    }

    #[test]
    fn parses_zoom_in_example_from_the_protocol_document() {
        let frame = parse_one("#tpPM2wZMC0299").unwrap();
        assert_eq!(frame.source, b'P');
        assert_eq!(frame.destination, b'M');
        assert_eq!(frame.control, Control::Write);
        assert_eq!(frame.identifier(), "ZMC");
        assert_eq!(frame.data, b"02");
    }

    #[test]
    fn parses_fixed_length_and_long_frames_from_the_protocol_document() {
        parse_one("#TPUG2wPTZ006A").unwrap();
        let frame = parse_one("#tpUG6wGAYEF073288").unwrap();
        assert_eq!(frame.data, b"EF0732");
    }

    #[test]
    fn parses_twelve_character_data() {
        let frame = parse_one("#tpUGCwGAMEC783203E800CE").unwrap();
        assert_eq!(frame.data, b"EC783203E800");
    }

    #[test]
    fn rejects_bad_checksum() {
        assert!(matches!(
            parse_one("#tpPM2wZMC0200"),
            Err(FrameError::ChecksumMismatch { computed: 0x99, .. })
        ));
    }

    #[test]
    fn rejects_fixed_header_with_other_length() {
        assert_eq!(
            parse_one("#TPUD4wXXX0000"),
            Err(FrameError::FixedLengthMismatch(4))
        );
    }

    #[test]
    fn parses_consecutive_frames_and_skips_noise() {
        let results = parse_datagram(b"noise#tpPM2wZMC0299#tpPM2wZMC0097");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(Result::is_ok));
    }

    #[test]
    fn truncated_frame_is_an_error() {
        assert_eq!(parse_one("#tpPM2wZMC02"), Err(FrameError::Truncated));
    }

    #[test]
    fn encode_matches_the_protocol_document() {
        let frame = Frame {
            source: b'P',
            destination: b'M',
            control: Control::Read,
            identifier: *b"ZOM",
            data: b"00".to_vec(),
        };
        assert_eq!(frame.encode(), b"#tpPM2rZOM009E");
    }
}
