use mavlink::dialects::common::{
    COMMAND_INT_DATA, COMMAND_LONG_DATA, MavCmd, MavMessage, MavResult,
};

/// A `COMMAND_LONG` or `COMMAND_INT`, normalised to seven float parameters.
#[derive(Debug, Clone, Copy)]
pub struct Command {
    pub id: MavCmd,
    pub params: [f32; 7],
}

impl Command {
    pub fn param(&self, index: usize) -> f32 {
        self.params
            .get(index.wrapping_sub(1))
            .copied()
            .unwrap_or(f32::NAN)
    }
}

impl From<&COMMAND_LONG_DATA> for Command {
    fn from(data: &COMMAND_LONG_DATA) -> Self {
        Self {
            id: data.command,
            params: [
                data.param1,
                data.param2,
                data.param3,
                data.param4,
                data.param5,
                data.param6,
                data.param7,
            ],
        }
    }
}

impl From<&COMMAND_INT_DATA> for Command {
    fn from(data: &COMMAND_INT_DATA) -> Self {
        Self {
            id: data.command,
            params: [
                data.param1,
                data.param2,
                data.param3,
                data.param4,
                data.x as f32,
                data.y as f32,
                data.z,
            ],
        }
    }
}

pub struct CommandOutcome {
    pub result: MavResult,
    /// Messages sent right after the `COMMAND_ACK`.
    pub follow_up: Vec<MavMessage>,
}

impl CommandOutcome {
    pub fn accepted() -> Self {
        Self::result(MavResult::MAV_RESULT_ACCEPTED)
    }

    pub fn denied() -> Self {
        Self::result(MavResult::MAV_RESULT_DENIED)
    }

    pub fn unsupported() -> Self {
        Self::result(MavResult::MAV_RESULT_UNSUPPORTED)
    }

    pub fn result(result: MavResult) -> Self {
        Self {
            result,
            follow_up: Vec::new(),
        }
    }

    pub fn with_follow_up(mut self, message: MavMessage) -> Self {
        self.follow_up.push(message);
        self
    }
}
