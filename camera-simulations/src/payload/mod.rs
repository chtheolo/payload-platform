//! Protocol-independent model of an EO/IR camera. Each simulator translates its wire protocol
//! into operations on this model.

pub mod zoom;

use zoom::Zoom;

/// What the operator sees in the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSource {
    Visible,
    Thermal,
}

/// Union of the palettes the simulated cameras offer; each protocol exposes its own subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalPalette {
    WhiteHot,
    BlackHot,
    Ironbow,
    HotIron,
    Lava,
    Medical,
    Arctic,
    Rainbow,
    RainbowHighContrast,
    RedHot,
}

#[derive(Debug, Clone)]
pub struct Payload {
    pub zoom: Zoom,
    pub source: VideoSource,
    pub palette: ThermalPalette,
}

impl Payload {
    pub fn new(max_zoom_factor: f32) -> Self {
        Self {
            zoom: Zoom::new(max_zoom_factor),
            source: VideoSource::Visible,
            palette: ThermalPalette::WhiteHot,
        }
    }
}
