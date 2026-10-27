//! The camera independent payload model
//! 
//! identity + connection + properties + actions
//! 

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