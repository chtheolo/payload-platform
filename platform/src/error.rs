//! - The platform's errors in two layers:
//! 
//! - `DriverError` and `ConnectError`: what driver reports to core
//! - `PAyloadError`: what the GCS receives and `api.rs` decides which HTTP
//! status each one gets

use thiserror::Error;

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