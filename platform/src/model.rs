//! The camera independent payload model
//! 
//! identity + connection + properties + actions
//! 

use std::time::SystemTime;
use std::collections::{BTreeMap};

use serde::{Deserialize, Serialize, Serializer};

// The ids of the camera. Not enum, cause future camera drivers might add new ids.
pub mod vocabulary {
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
        Sensor::ALL.into_iter().find(|sensor| sensor.as_str() == name)
    }
}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Number(f64),
    Text(String),
}

/// Rounds `value` to a multiple of `resolution`: 4.04 becomes 4.0 for a resolution of 0.1.
///
/// Dividing by `1 / resolution`, instead of multiplying by `resolution`, avoids
/// floating-point results such as 0.30000000000000004.
pub fn round_to(value: f64, resolution: f64) -> f64 {
    let steps_per_unit = 1.0 / resolution;
    (value * steps_per_unit).round() / steps_per_unit
}

/// Who the camera is
#[derive(Debug, Clone, Serialize)]
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

/// What kind of values a property, or an action argument, accepts 
/// 
/// zoom : Kind::Number = unit, min 1, max 30, resolution 0.1
/// sensor: Kind::Enum = "visible", "thermal"
/// palette: Kind::Enum = "whiete_hot", "black_hot", "ironbow"
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    Number {
        unit: String,
        minimum: f64,
        maximum: f64,
        resolution: f64,
    },
    Enum {
        options: Vec<String>,
    },
}

// Keep a state of a value and when it was read 
#[derive(Debug, Clone, Serialize)]
pub struct Reading {
    pub value: Value,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub parameters: BTreeMap<String, Kind>
}

#[derive(Debug, Clone, Serialize)]
pub struct Property {
    #[serde(flatten)]
    ///what values is accepts
    pub kind: Kind,
    /// can the GCS change it ?
    pub writable: bool,
    ///again null until the camera has been read for the first time
    pub value: Option<Value>,
    /// when did we read that value 
    pub observed_at: Option<Timestamp>,
}

impl Property {
    /// A property the camera has before its value has been read.
    /// Basically, we know what it is, but we haven't read its value yet
    /// by convention ww create a constructor by returning a initialized Property
    pub fn unread(kind: Kind, writable: bool) -> Property {
        Property {
            kind,
            writable,
            value: None,
            observed_at: None,
        }
    }
}

/// A snapshot of the camera the GCS can see
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub identity: Option<Identity>,         // who
    pub connection: Connection,             // alive?
    pub properties: BTreeMap<String, Property>, // state
    pub actions: BTreeMap<String, Action>   // actions to do 
}

impl Snapshot {
    pub fn connecting() -> Snapshot {
        Snapshot {
            identity: None,
            connection: Connection {
                status: ConnectionStatus::Connecting,
                last_seen: None,
            },
            properties: BTreeMap::new(),
            actions: BTreeMap::new(),
        }
    }
}