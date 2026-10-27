use mavlink::{
    MavHeader, dialects::common::MavMessage, error::MessageReadError, peek_reader::PeekReader,
    read_any_msg, write_v2_msg,
};
use tracing::{debug, warn};

/// Decodes every MAVLink 1 or 2 frame in a datagram, skipping frames that fail to parse.
pub fn decode(datagram: &[u8]) -> Vec<(MavHeader, MavMessage)> {
    let mut reader = PeekReader::new(datagram);
    let mut messages = Vec::new();
    loop {
        match read_any_msg::<MavMessage, _>(&mut reader) {
            Ok(message) => messages.push(message),
            Err(MessageReadError::Parse(e)) => debug!("skipping MAVLink frame: {e}"),
            Err(MessageReadError::Io(_)) => return messages,
        }
    }
}

pub fn encode(header: MavHeader, message: &MavMessage) -> Option<Vec<u8>> {
    let mut datagram = Vec::new();
    match write_v2_msg(&mut datagram, header, message) {
        Ok(_) => Some(datagram),
        Err(e) => {
            warn!("failed to encode MAVLink message: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavlink::dialects::common::HEARTBEAT_DATA;

    #[test]
    fn decodes_every_frame_in_a_datagram() {
        let header = MavHeader {
            system_id: 255,
            component_id: 190,
            sequence: 7,
        };
        let heartbeat = MavMessage::HEARTBEAT(HEARTBEAT_DATA::default());
        let mut datagram = encode(header, &heartbeat).unwrap();
        datagram.extend(encode(header, &heartbeat).unwrap());

        let decoded = decode(&datagram);

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, header);
        assert_eq!(decoded[0].1, heartbeat);
    }

    #[test]
    fn ignores_garbage() {
        assert!(decode(b"not mavlink").is_empty());
    }
}
