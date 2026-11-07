//! What differs between MAVLink camera models (README, "Translation per camera").
//!
//! A profile only translates: it builds commands and reads values out of answers, and never
//! does I/O itself. The driver in `mod.rs` does all the talking. That keeps profiles small
//! and easy to test, and adding a camera model means writing one file and listing it in `all`.
 
use mavlink::dialects::common::{CAMERA_SETTINGS_DATA, VIDEO_STREAM_STATUS_DATA};


use super::bx20::Bx20;
use super::vlr10::Vlr10;

use crate::drivers::mavlink::link::Command;
use crate::model::Sensor;

/// A camera's zoom limits, as magnification factors.
#[derive(Debug, Clone, Copy)]
pub struct ZoomRange {
    pub minimum: f64,
    pub maximum: f64,
    /// The smallest zoom change offered to the GCS.
    pub resolution: f64,
}

/// Rust: `Send + Sync` lets a driver that holds a profile be used from any thread.
pub trait CameraProfile: Send + Sync {
    /// Short name, shown in the identity and accepted by `?profile=`.
    fn name(&self) -> &'static str;
    /// The names the camera reports in CAMERA_INFORMATION, used to detect the model.
    fn vendor(&self) -> &'static str;
    fn model(&self) -> &'static str;

    fn zoom_range(&self) -> ZoomRange;
    /// The zoom factor in CAMERA_SETTINGS. Models disagree on what `zoomLevel` means.
    fn zoom_from_settings(&self, settings: &CAMERA_SETTINGS_DATA) -> f64;

    /// The commands that move the zoom from `current` to `target`.
    /// we return a vecto cause different cameras have different number of commands
    // 1 for bx20 3 for vlr10
    fn zoom_to(&self, current: f64, target: f64) -> Vec<Command>;

    /// How many VIDEO_STREAM_STATUS messages one request returns: one per video stream.
    fn stream_count(&self) -> usize;
    fn sensor_from_streams(&self, streams: &[VIDEO_STREAM_STATUS_DATA]) -> Option<Sensor>;
    fn select_sensor(&self, sensor: Sensor) -> Command;

    // /// CAMERA_INFORMATION.firmware_version, decoded the way the vendor documents it.
    fn firmware(&self, raw: u32) -> String;
}

fn all() -> Vec<Box<dyn CameraProfile>>{
    vec![Box::new(Bx20), Box::new(Vlr10)]
}

/// the profile matching the vendor and model names from the CAMERA_INFORMATION
pub fn detect(vendor: &str, model: &str) -> Option<Box<dyn CameraProfile>> {
    all()
        .into_iter()
        .find(|profile| profile.vendor() == vendor && profile.model() == model)
}

/// return the camera profile that the user chose if it exists, otherwise None
pub fn named(name: &str) -> Option<Box<dyn CameraProfile>> {
    all()
        .into_iter()
        .find(|profile| profile.name() == name)
}

/// helper function that returns all the profile names in string , usually for debug messages
pub fn names() -> String {
    all()
        .iter()
        .map(|profile| profile.name())
        .collect::<Vec<_>>()    // the type is Vec<&'static str> but _ autofill it
        .join(", ")
}