/// The Contract : "what any driver must do" and list of known drivers
/// 

mod mavlink;

use crate::error::{ConnectError, DriverError};
use crate::uri::{CameraUri};
use crate::model::{Property, Action, Value, Identity};

use std::time::Duration;
use std::collections::{BTreeMap};

pub trait Driver: Send {
    /// who the camera is
    fn identity(&self) -> Identity;

    /// the camera's properties with their descriptors 
    fn properties(&self) -> BTreeMap<String, Property>;

    /// The camera's actions
    fn actions(&self) -> BTreeMap<String, Action>;

    /// How logn ago the camera was last heard, or 'None' if it never was.
    fn last_heard(&self) -> Option<Duration>;

    // /// Reads every property from the camera. The core calls this to poll.
    // async fn read(&mut self) -> Result<BTreeMap<String, Value>, DriverError>;

    // /// Sets a property and returns the value the camera reports afterwards.
    // async fn write(&mut self, property: &str, value: &Value) -> Result<Value, DriverError>;

    // /// Runs an action and returns the new values of the properties it changed.
    // async fn invoke(
    //     &mut self,
    //     action: &str,
    //     arguments: &BTreeMap<String, Value>,
    // ) -> Result<BTreeMap<String, Value>, DriverError>;
}

pub async fn connect(uri: &CameraUri) -> Result<Box<dyn Driver>, ConnectError> {
    match uri.scheme.as_str() {
        "mavlink" => Ok(Box::new(mavlink::MavlinkDriver::connect(uri).await?)),
        other => Err(ConnectError::Fatal(format!(
            "unknown camera URI scheme `{other}`; known schemes: mavlink"
        ))),
    }

}