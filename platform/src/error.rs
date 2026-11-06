//! The platform's errors, in two layers:
//!
//! - `DriverError` and `ConnectError`: what a driver reports to the core;
//! - `PayloadError`: what the GCS receives (README, "Errors"). `api.rs` decides which HTTP
//!   status each one gets, so this file stays free of HTTP.

use serde_json::json;
use thiserror::Error;

/// What can go wrong while a driver talks to its camera.
///
/// Rust: `#[derive(Error)]` (from thiserror) implements `std::error::Error`, and each
/// `#[error("...")]` becomes the text that `to_string()` returns.
#[derive(Debug, Error)]
pub enum DriverError {
    #[error("the camera refused: {0}")]
    Rejected(String),
    #[error("the camera asked to retry later")]
    Busy,
    #[error("no confirmation from the camera in time")]
    Timeout,
    #[error("{0}")]
    Internal(String),
}

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

/// The errors the GCS can receive. Each variant is one `code` of the README's error table.
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
            PayloadError::InvalidRequest(_) => "invalid_request",               // we ingonre what ever string someone pass
            PayloadError::NotFound(_) => "not_found",
            PayloadError::ReadOnly(_) => "read_only",
            PayloadError::UnsupportedValue { .. } => "unsupported_value",       // { .. } struct variant that we don't care about the fields
            PayloadError::OutOfRange { .. } => "out_of_range",
            PayloadError::Rejected(_) => "rejected",
            PayloadError::PayloadOffline => "payload_offline",
            PayloadError::Busy => "busy",
            PayloadError::Timeout => "timeout",
            PayloadError::Internal(_) => "internal",
        }
    }

    /// Extra fields that help the GCS react, such as the allowed range.
    pub fn details(&self) -> Option<serde_json::Value> {
        match self {
            PayloadError::UnsupportedValue {
                property, options, ..
            } => Some(json!({ "property": property, "options": options })),
            PayloadError::OutOfRange {
                property,
                minimum,
                maximum,
            } => Some(json!({ "property": property, "minimum": minimum, "maximum": maximum })),
            _ => None,
        }
    }
}

/// Rust: implementing `From` lets the `?` operator turn a `DriverError` into a
/// `PayloadError` by itself.
impl From<DriverError> for PayloadError {
    fn from(error: DriverError) -> PayloadError {
        match error {
            DriverError::Rejected(reason) => PayloadError::Rejected(reason),
            DriverError::Busy => PayloadError::Busy,
            DriverError::Timeout => PayloadError::Timeout,
            DriverError::Internal(reason) => PayloadError::Internal(reason),
        }
    }
}
