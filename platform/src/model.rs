//! The camera independent payload model
//! 
//! identity + connection + properties + actions
//! 

use std::time:SystemTime;

// The ids of the camera. Not enum, cause future camera drivers might add new ids.
pub mod ids {
    pub const ZOOM: &str = "zoom";
    pub const SENSOR: &str = "sensor";
    pub const THERMAL_PALETTE: &str = "thermal_palette";
    pub const ZOOM_STEP: &str = "zoom_step";
    /// The argument of `zoom_step`: "in" or "out".
    pub const DIRECTION: &str = "direction";
}

/// Which sensor feeds the video. At the API it becomes a text of "visible" or "thermal"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Visible,
    Thermal
}

impl Sensor {
    pub const ALL: [Sensor; 2] = [Sensor::Visible, Sensor::Thermal];

    pub fn as_str(self) -> &'static str {
        match self {
            Sensor::Visible => "visible",
            Sensor::Thermal => "thermal",
        }
    }

    pub fn from_name(name: &str) -> Option<Sensor> {
        Sensor::All.into_iter().find(|sensor| sensor.as_str() == name)
    }

}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Number(f64),
    Value(String),
}

/// Who the camera is
pub struct Identity {
    pub kind: String,
    pub vendor: String,
    pub model: String,
    /// It can be 'None' if camera cna't report it
    pub firmware: Option<String>,
    pub driver: String,
    /// the per model profile the driver uses, for drivers that have profiles
    pub profile: Option<String>,
    /// the camera URI the platform was started with
    pub uri: String,
}

// Wrapping the SystemTime in our Timestamp(new type) so we serialize it in a nice format
// readable for the end user in GCS. it's tuple struct
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timestamp (pub SystemTime);

impl Timestamp {
    pub fn now() -> Timestamp {
        Timestamp(SystemTime::now())
    }
}


impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&humantime::format_rfc3339_millis(self.0))
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all="snake_case")]
pub enum ConnectionStatus {
    Connecting,
    Online,
    Offline,
}

#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    pub status: ConnectionStatus,
    // when the camera was last heard from, null if it is the first time
    pub last_seen: Option<Timestamp>,
}


#[derive(Debug, Clone, Serialize)]
pub struct Reading {
    pub value: Value,
    pub observed_at: Timestamp,
}
