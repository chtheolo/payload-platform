//! The thermal palette as the `IR_PALETTE` camera parameter (`MAV_PARAM_EXT_TYPE_UINT8`) over
//! the extended parameter protocol.

use mavlink::dialects::common::{
    MavMessage, MavParamExtType, PARAM_EXT_ACK_DATA, PARAM_EXT_REQUEST_READ_DATA,
    PARAM_EXT_SET_DATA, PARAM_EXT_VALUE_DATA, ParamAck,
};

use crate::payload::{Payload, ThermalPalette};

const PARAM_ID: &str = "IR_PALETTE";
/// `IR_PALETTE` values, by index.
const PALETTES: [ThermalPalette; 4] = [
    ThermalPalette::WhiteHot,
    ThermalPalette::BlackHot,
    ThermalPalette::Ironbow,
    ThermalPalette::Rainbow,
];

/// Replies to an extended parameter message.
pub fn handle(payload: &mut Payload, message: &MavMessage) -> Vec<MavMessage> {
    match message {
        MavMessage::PARAM_EXT_REQUEST_LIST(_) => vec![value_message(payload)],
        MavMessage::PARAM_EXT_REQUEST_READ(data) if requests_palette(data) => {
            vec![value_message(payload)]
        }
        MavMessage::PARAM_EXT_SET(data) if param_id(&data.param_id) == PARAM_ID => {
            vec![set(payload, data)]
        }
        _ => Vec::new(),
    }
}

fn set(payload: &mut Payload, data: &PARAM_EXT_SET_DATA) -> MavMessage {
    let requested = PALETTES.get(usize::from(data.param_value[0])).copied();
    let result = match (data.param_type, requested) {
        (MavParamExtType::MAV_PARAM_EXT_TYPE_UINT8, Some(palette)) => {
            payload.palette = palette;
            ParamAck::PARAM_ACK_ACCEPTED
        }
        _ => ParamAck::PARAM_ACK_VALUE_UNSUPPORTED,
    };
    MavMessage::PARAM_EXT_ACK(PARAM_EXT_ACK_DATA {
        param_id: data.param_id,
        param_value: palette_value(payload.palette).into(),
        param_type: MavParamExtType::MAV_PARAM_EXT_TYPE_UINT8,
        param_result: result,
    })
}

fn param_id(raw: &[u8; 16]) -> &str {
    let length = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    std::str::from_utf8(raw.get(..length).unwrap_or_default()).unwrap_or_default()
}

fn requests_palette(data: &PARAM_EXT_REQUEST_READ_DATA) -> bool {
    match data.param_index {
        -1 => param_id(&data.param_id) == PARAM_ID,
        index => index == 0,
    }
}

fn palette_value(palette: ThermalPalette) -> [u8; 128] {
    let mut value = [0u8; 128];
    value[0] = PALETTES
        .iter()
        .position(|known| *known == palette)
        .unwrap_or(0) as u8;
    value
}

fn value_message(payload: &Payload) -> MavMessage {
    MavMessage::PARAM_EXT_VALUE(PARAM_EXT_VALUE_DATA {
        param_count: 1,
        param_index: 0,
        param_id: PARAM_ID.into(),
        param_value: palette_value(payload.palette).into(),
        param_type: MavParamExtType::MAV_PARAM_EXT_TYPE_UINT8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette_set(value: u8, param_type: MavParamExtType) -> MavMessage {
        let mut param_value = [0u8; 128];
        param_value[0] = value;
        MavMessage::PARAM_EXT_SET(PARAM_EXT_SET_DATA {
            target_system: 1,
            target_component: 100,
            param_id: PARAM_ID.into(),
            param_value: param_value.into(),
            param_type,
        })
    }

    #[test]
    fn palette_is_set_through_the_parameter() {
        let mut payload = Payload::new(10.0);

        let replies = handle(
            &mut payload,
            &palette_set(2, MavParamExtType::MAV_PARAM_EXT_TYPE_UINT8),
        );

        let [MavMessage::PARAM_EXT_ACK(ack)] = replies.as_slice() else {
            panic!("expected PARAM_EXT_ACK, got {replies:?}");
        };
        assert_eq!(ack.param_result, ParamAck::PARAM_ACK_ACCEPTED);
        assert_eq!(payload.palette, ThermalPalette::Ironbow);
    }

    #[test]
    fn palette_with_wrong_type_is_rejected_with_current_value() {
        let mut payload = Payload::new(10.0);

        let replies = handle(
            &mut payload,
            &palette_set(2, MavParamExtType::MAV_PARAM_EXT_TYPE_REAL32),
        );

        let [MavMessage::PARAM_EXT_ACK(ack)] = replies.as_slice() else {
            panic!("expected PARAM_EXT_ACK, got {replies:?}");
        };
        assert_eq!(ack.param_result, ParamAck::PARAM_ACK_VALUE_UNSUPPORTED);
        assert_eq!(ack.param_value[0], 0);
        assert_eq!(payload.palette, ThermalPalette::WhiteHot);
    }

    #[test]
    fn unknown_palette_index_is_rejected() {
        let mut payload = Payload::new(10.0);

        let replies = handle(
            &mut payload,
            &palette_set(9, MavParamExtType::MAV_PARAM_EXT_TYPE_UINT8),
        );

        let [MavMessage::PARAM_EXT_ACK(ack)] = replies.as_slice() else {
            panic!("expected PARAM_EXT_ACK, got {replies:?}");
        };
        assert_eq!(ack.param_result, ParamAck::PARAM_ACK_VALUE_UNSUPPORTED);
    }
}
