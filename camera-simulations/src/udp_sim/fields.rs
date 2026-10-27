//! Field encodings used inside frame data: signed two's complement numbers as uppercase hex
//! characters, high digit first, and plain decimal digits.

pub fn unsigned_hex2(text: &[u8]) -> Option<u8> {
    let digits = std::str::from_utf8(text.get(..2)?).ok()?;
    u8::from_str_radix(digits, 16).ok()
}

pub fn decimal(text: &[u8]) -> Option<u32> {
    let digits = std::str::from_utf8(text).ok()?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub fn format_signed_hex4(value: i16) -> String {
    format!("{:04X}", value as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_examples_from_the_protocol_document() {
        assert_eq!(unsigned_hex2(b"0A"), Some(10));
        assert_eq!(decimal(b"0123"), Some(123));
    }

    #[test]
    fn rejects_malformed_fields() {
        assert_eq!(unsigned_hex2(b"G1"), None);
        assert_eq!(unsigned_hex2(b"1"), None);
        assert_eq!(decimal(b"12a"), None);
        assert_eq!(decimal(b"+12"), None);
    }

    #[test]
    fn formats_negative_values_as_twos_complement() {
        assert_eq!(format_signed_hex4(-76), "FFB4");
        assert_eq!(format_signed_hex4(50), "0032");
    }
}
