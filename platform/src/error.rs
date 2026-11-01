//! - The platform's errors in two layers:
//! 
//! - `DriverError` and `ConnectError`: what driver reports to core
//! - `PAyloadError`: what the GCS receives and `api.rs` decides which HTTP
//! status each one gets

use thiserror::Error;


/// Why a driver couldn't connect. The core retries the first kind and gives up on the second.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The camera didn't answer. It may still be starting up, so trying again makes sense.
    #[error("{0}")]
    NoAnswer(String),
    /// The URI, or the camera itself, is wrong. Trying again can't help.
    #[error("{0}")]
    Fatal(String),
}

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("`{0}` can't be changed")]
    ReadOnly(String),
    // Rust: in thiserror messages, `.options` refers to the variant's `options` field.
    #[error("`{value}` is not one of: {}", .options.join(", "))]
    UnsupportedValue {
        property: String,
        value: String,
        options: Vec<String>,
    },
    #[error("`{property}` must be between {minimum} and {maximum}")]
    OutOfRange {
        property: String,
        minimum: f64,
        maximum: f64,
    },
    #[error("the camera refused: {0}")]
    Rejected(String),
    #[error("the camera isn't connected")]
    PayloadOffline,
    #[error("too many commands are waiting, or the camera asked to retry; try again shortly")]
    Busy,
    #[error("the camera didn't confirm in time; read the value before retrying")]
    Timeout,
    #[error("{0}")]
    Internal(String),
}

impl PayloadError {
    /// The machine-readable `code` from the README's error table.
    pub fn code(&self) -> &'static str {
        match self {
            PayloadError::InvalidRequest(_) => "invalid_request",
            PayloadError::NotFound(_) => "not_found",
            PayloadError::ReadOnly(_) => "read_only",
            PayloadError::UnsupportedValue { .. } => "unsupported_value",
            PayloadError::OutOfRange { .. } => "out_of_range",
            PayloadError::Rejected(_) => "rejected",
            PayloadError::PayloadOffline => "payload_offline",
            PayloadError::Busy => "busy",
            PayloadError::Timeout => "timeout",
            PayloadError::Internal(_) => "internal",
        }
    }
}