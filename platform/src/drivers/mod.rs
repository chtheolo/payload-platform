//! The driver contract (README, "Driver contract") and the list of known drivers.
//!
//! A driver knows one camera protocol. The core only ever talks to a driver through the
//! `Driver` trait, so supporting a new camera means writing a driver, not changing the core.

mod lc30t;
// Rust: this module has the same name as the `mavlink` crate. Inside this file, `mavlink::`
// means our module; to reach the crate from here you would write `::mavlink::`.
mod mavlink;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;

use crate::error::{ConnectError, DriverError};
use crate::model::{Action, Identity, Property, Value};
use crate::uri::CameraUri;

/// Everything the core needs from a driver.
///
/// The core promises a driver that calls never overlap, and that every value has already
/// been checked against the driver's own descriptors. A driver promises that success means
/// the camera confirmed the change.
///
/// Rust: a trait is an interface. `Send` means a driver can move between threads, which tokio
/// needs because a task may run on any of its threads.
///
/// Rust: a plain `async fn` in a trait can't yet be called through `Box<dyn Driver>`. The
/// `#[async_trait]` macro rewrites each async method to return a boxed future, which can.
#[async_trait]
pub trait Driver: Send {
    /// Who the camera is.
    fn identity(&self) -> Identity;

    /// The camera's properties with their descriptors. Their values come from `read`.
    fn properties(&self) -> BTreeMap<String, Property>;

    /// The camera's actions.
    fn actions(&self) -> BTreeMap<String, Action>;

    /// How long ago the camera was last heard from, or `None` if it never was. The core
    /// turns this into online or offline with one rule for every driver.
    fn last_heard(&self) -> Option<Duration>;

    /// Reads every property from the camera. The core calls this to poll.
    async fn read(&mut self) -> Result<BTreeMap<String, Value>, DriverError>;

    /// Sets a property and returns the value the camera reports afterwards.
    async fn write(&mut self, property: &str, value: &Value) -> Result<Value, DriverError>;

    /// Runs an action and returns the new values of the properties it changed.
    async fn invoke(
        &mut self,
        action: &str,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, DriverError>;
}

/// Picks the driver for the URI's scheme and connects it to the camera.
///
/// Rust: `Box<dyn Driver>` means "some type that implements `Driver`", decided at run time.
/// The core never needs to know which driver it got.
pub async fn connect(uri: &CameraUri) -> Result<Box<dyn Driver>, ConnectError> {
    match uri.scheme.as_str() {
        "mavlink" => mavlink::connect(uri).await,
        "lc30t" => Ok(Box::new(lc30t::Lc30tDriver::connect(uri).await?)),
        other => Err(ConnectError::Fatal(format!(
            "unknown camera URI scheme `{other}`; known schemes: mavlink, lc30t"
        ))),
    }
}

/// Turns the URI's host and port into a socket address, looking the host up if it's a name.
/// Both drivers talk UDP to a host and port, so they share this.
///
/// Rust: there's no `pub` here. A private item is visible in its own module and in every
/// module inside it, so `mavlink` and `lc30t` can call it, but `payload.rs` can't.
async fn resolve(uri: &CameraUri) -> Result<SocketAddr, ConnectError> {
    tokio::net::lookup_host((uri.host.as_str(), uri.port))
        .await
        .ok()
        .and_then(|mut addresses| addresses.next())
        .ok_or_else(|| ConnectError::Fatal(format!("can't resolve host `{}`", uri.host)))
}
