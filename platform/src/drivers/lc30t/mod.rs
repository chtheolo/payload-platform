//! The Lucen LC-30T driver: the pod's own ASCII protocol over UDP, as described in
//! specs/LC-30T_Protocol_V1.1.3.pdf ("protocol" in the comments, so "protocol 4.1" is its
//! section 4.1).
//!
//! - `frame.rs` turns frames into text and back;
//! - `link.rs` sends and receives them over UDP, at the pace the vendor asks for.
//!
//! The big difference from MAVLink: the pod never confirms anything. A write gets no answer at
//! all (protocol 3.3), and a value it can't use is silently ignored (4.3). So every write here
//! is followed by a read of the same setting, and only a read-back equal to the request counts
//! as success (README, "How camera answers map to these codes").

mod frame;
mod link;

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use tracing::info;

use self::frame::{LENS, SYSTEM};
use self::link::Link;
use super::{Driver, resolve};
use crate::error::{ConnectError, DriverError};
use crate::model::{Action, Identity, Kind, Property, Sensor, Value, vocabulary};
use crate::uri::{CameraUri, UriError};

/// The settings this driver uses, by identifier: the zoom on the lens (protocol 4.1), and the
/// image mode and the palette on the system module (4.2).
const ZOOM: &str = "MUL";
const IMAGE_MODE: &str = "PIP";
const PALETTE: &str = "IMG";

/// MUL counts in tenths of 1x, from 0010 to 0300: 1x to 30x (protocol 4.1).
const MIN_TENTHS: i32 = 10;
const MAX_TENTHS: i32 = 300;
/// One `zoom_step` is 1x (README, "Camera vocabulary").
const STEP_TENTHS: i32 = 10;

/// The PIP codes of the two full-frame images (protocol 4.2). 01 and 02 are reserved.
const VISIBLE: &str = "00";
const THERMAL: &str = "03";

/// The palette names (README, "Palette names") with their IMG codes (protocol 4.3). The four
/// that the MAVLink cameras also have come first, in the same order.
const PALETTES: [(&str, &str); 10] = [
    ("white_hot", "00"),
    ("black_hot", "09"),
    ("ironbow", "02"),
    ("rainbow", "06"),
    ("lava", "01"),
    ("hot_iron", "03"),
    ("medical", "04"),
    ("arctic", "05"),
    ("rainbow_high_contrast", "07"),
    ("red_hot", "08"),
];

/// The gap between two frames that the vendor recommends (protocol 6.2). The URI option
/// `interval_ms` changes it (README, "Camera URI").
const DEFAULT_INTERVAL_MS: u64 = 50;

/// How many times a write is sent before a read-back that still differs counts as a refusal.
const WRITE_ATTEMPTS: u8 = 3;

pub struct Lc30tDriver {
    link: Link,
    identity: Identity,
}

impl Lc30tDriver {
    /// Opens the link and waits for the pod's first answer.
    pub async fn connect(uri: &CameraUri) -> Result<Lc30tDriver, ConnectError> {
        // A wrong URI is fatal: trying again can't fix it.
        let fatal = |error: UriError| ConnectError::Fatal(error.to_string());
        uri.accept_only(&["interval_ms"]).map_err(fatal)?;
        let interval_ms = uri
            .option("interval_ms", DEFAULT_INTERVAL_MS)
            .map_err(fatal)?;

        let address = resolve(uri).await?;
        let link = Link::open(address, Duration::from_millis(interval_ms))
            .await
            .map_err(|error| ConnectError::Fatal(format!("can't open a UDP socket: {error}")))?;
        // The pod may not answer for about 8 s after power-on (protocol 6.1), so silence here
        // is worth retrying. Any answer proves it's there; reading the zoom is as good as any.
        link.read(LENS, ZOOM).await.map_err(|_| {
            ConnectError::NoAnswer(format!("no answer from the LC-30T at {address}"))
        })?;
        info!("connected to Lucen LC-30T at {address}");

        // The protocol has no way to ask the pod who it is, so the driver fills the identity
        // in itself, and the firmware stays unknown (README, "Identity").
        let identity = Identity {
            kind: "camera".into(),
            vendor: "Lucen".into(),
            model: "LC-30T".into(),
            firmware: None,
            driver: "lc30t".into(),
            profile: None,
            uri: uri.text.clone(),
        };
        Ok(Lc30tDriver { link, identity })
    }

    /// Writes a setting, then reads it back (README, "Translation per camera").
    ///
    /// A write that got lost, or that the pod ignored, shows up as an old value in the
    /// read-back. So the write is sent again. That is safe because a write names the value it
    /// wants, not a change. Only a read-back that still differs after every attempt counts as
    /// a refusal.
    async fn set(&self, module: char, identifier: &str, data: &str) -> Result<String, DriverError> {
        let mut reported = String::new();
        for _ in 0..WRITE_ATTEMPTS {
            self.link.write(module, identifier, data).await;
            // No answer at all is a timeout: `?` returns it straight away.
            reported = self.link.read(module, identifier).await?;
            if reported == data {
                return Ok(reported);
            }
        }
        Err(DriverError::Rejected(format!(
            "asked for {identifier} {data}, but the camera reports {identifier} {reported}"
        )))
    }

    async fn read_zoom(&self) -> Result<i32, DriverError> {
        parse_tenths(&self.link.read(LENS, ZOOM).await?)
    }

    /// Sets the zoom, in tenths, and returns the zoom the pod reports afterwards.
    async fn set_zoom(&self, tenths: i32) -> Result<i32, DriverError> {
        // MUL data is always four digits: 0155 is 15.5x (protocol 4.1).
        // Rust: `{:04}` pads a number with zeros to four characters, so 155 becomes "0155".
        let reported = self.set(LENS, ZOOM, &format!("{tenths:04}")).await?;
        parse_tenths(&reported)
    }

    async fn read_sensor(&self) -> Result<Sensor, DriverError> {
        sensor(&self.link.read(SYSTEM, IMAGE_MODE).await?)
    }

    async fn read_palette(&self) -> Result<&'static str, DriverError> {
        palette_name(&self.link.read(SYSTEM, PALETTE).await?)
    }
}

#[async_trait]
impl Driver for Lc30tDriver {
    fn identity(&self) -> Identity {
        self.identity.clone()
    }

    fn properties(&self) -> BTreeMap<String, Property> {
        // The same ids and shapes as the MAVLink driver (README, "Camera vocabulary"), so a
        // GCS needs nothing new for this camera; only the range and the palette list differ.
        let zoom_kind = Kind::Number {
            unit: "x".into(),
            minimum: factor(MIN_TENTHS),
            maximum: factor(MAX_TENTHS),
            resolution: 0.1,
        };
        let sensor_kind = Kind::Enum {
            options: Sensor::ALL
                .iter()
                .map(|sensor| sensor.as_str().into())
                .collect(),
        };
        let palette_kind = Kind::Enum {
            options: PALETTES.iter().map(|(name, _)| name.to_string()).collect(),
        };
        BTreeMap::from([
            (vocabulary::ZOOM.into(), Property::unread(zoom_kind, true)),
            (
                vocabulary::SENSOR.into(),
                Property::unread(sensor_kind, true),
            ),
            (
                vocabulary::THERMAL_PALETTE.into(),
                Property::unread(palette_kind, true),
            ),
        ])
    }

    fn actions(&self) -> BTreeMap<String, Action> {
        let direction = Kind::Enum {
            options: vec!["in".into(), "out".into()],
        };
        let zoom_step = Action {
            parameters: BTreeMap::from([(vocabulary::DIRECTION.into(), direction)]),
        };
        BTreeMap::from([(vocabulary::ZOOM_STEP.into(), zoom_step)])
    }

    fn last_heard(&self) -> Option<Duration> {
        self.link.last_heard()
    }

    async fn read(&mut self) -> Result<BTreeMap<String, Value>, DriverError> {
        Ok(BTreeMap::from([
            (
                vocabulary::ZOOM.into(),
                Value::Number(factor(self.read_zoom().await?)),
            ),
            (
                vocabulary::SENSOR.into(),
                Value::Text(self.read_sensor().await?.as_str().into()),
            ),
            (
                vocabulary::THERMAL_PALETTE.into(),
                Value::Text(self.read_palette().await?.into()),
            ),
        ]))
    }

    async fn write(&mut self, property: &str, value: &Value) -> Result<Value, DriverError> {
        match (property, value) {
            (vocabulary::ZOOM, Value::Number(target)) => {
                // The core has already rounded the target to 0.1x, so this is a whole number
                // of tenths. Rust: `as i32` turns the float into a whole number.
                let reported = self.set_zoom((target * 10.0).round() as i32).await?;
                Ok(Value::Number(factor(reported)))
            }
            (vocabulary::SENSOR, Value::Text(name)) => {
                let code = match Sensor::from_name(name) {
                    Some(Sensor::Visible) => VISIBLE,
                    Some(Sensor::Thermal) => THERMAL,
                    None => return Err(DriverError::Internal(format!("unknown sensor `{name}`"))),
                };
                let reported = self.set(SYSTEM, IMAGE_MODE, code).await?;
                Ok(Value::Text(sensor(&reported)?.as_str().into()))
            }
            (vocabulary::THERMAL_PALETTE, Value::Text(name)) => {
                let Some(code) = palette_code(name) else {
                    return Err(DriverError::Internal(format!("unknown palette `{name}`")));
                };
                let reported = self.set(SYSTEM, PALETTE, code).await?;
                Ok(Value::Text(palette_name(&reported)?.into()))
            }
            _ => Err(DriverError::Internal(format!(
                "can't write {value:?} to `{property}`"
            ))),
        }
    }

    async fn invoke(
        &mut self,
        action: &str,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, DriverError> {
        match action {
            vocabulary::ZOOM_STEP => {
                let change = match arguments.get(vocabulary::DIRECTION) {
                    Some(Value::Text(direction)) if direction == "in" => STEP_TENTHS,
                    Some(Value::Text(direction)) if direction == "out" => -STEP_TENTHS,
                    other => {
                        return Err(DriverError::Internal(format!(
                            "bad zoom direction {other:?}"
                        )));
                    }
                };
                // The pod has no step command (ZMC isn't supported, protocol 4.1), so a step is
                // a read followed by an absolute write. Unlike a MAVLink step, that write may
                // safely be sent again, because it names the target rather than the change.
                let current = self.read_zoom().await?;
                let target = step(current, change);
                let reported = if target == current {
                    current // already at the end of the range: nothing to send
                } else {
                    self.set_zoom(target).await?
                };
                Ok(BTreeMap::from([(
                    vocabulary::ZOOM.into(),
                    Value::Number(factor(reported)),
                )]))
            }
            _ => Err(DriverError::Internal(format!("unknown action `{action}`"))),
        }
    }
}

/// The zoom factor for a number of tenths: 155 is 15.5x.
fn factor(tenths: i32) -> f64 {
    f64::from(tenths) / 10.0
}

/// MUL data such as "0155" as a number of tenths.
fn parse_tenths(data: &str) -> Result<i32, DriverError> {
    data.parse()
        .map_err(|_| DriverError::Internal(format!("the camera reported the zoom as `{data}`")))
}

/// The zoom after one step in or out, in tenths. At either end of the range it stays where it
/// is, as it does on the MAVLink cameras.
fn step(current: i32, change: i32) -> i32 {
    (current + change).clamp(MIN_TENTHS, MAX_TENTHS)
}

/// The sensor for a PIP code (protocol 4.2).
fn sensor(code: &str) -> Result<Sensor, DriverError> {
    match code {
        VISIBLE => Ok(Sensor::Visible),
        THERMAL => Ok(Sensor::Thermal),
        // 01 and 02 are reserved, so the pod shouldn't report them.
        other => Err(DriverError::Internal(format!(
            "image mode `{other}` is neither visible nor thermal"
        ))),
    }
}

/// The palette name for an IMG code (protocol 4.3).
fn palette_name(code: &str) -> Result<&'static str, DriverError> {
    PALETTES
        .iter()
        .find(|(_, known)| *known == code)
        .map(|(name, _)| *name)
        .ok_or_else(|| DriverError::Internal(format!("unknown palette code `{code}`")))
}

/// The IMG code for a palette name.
fn palette_code(name: &str) -> Option<&'static str> {
    PALETTES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, code)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_counts_in_tenths() {
        // Protocol 4.1's example: 0155 is 15.5x.
        assert_eq!(parse_tenths("0155").unwrap(), 155);
        assert_eq!(factor(155), 15.5);
        assert!(parse_tenths("01x5").is_err());
    }

    #[test]
    fn a_step_stays_within_the_range() {
        assert_eq!(step(155, STEP_TENTHS), 165);
        assert_eq!(step(295, STEP_TENTHS), MAX_TENTHS);
        assert_eq!(step(MIN_TENTHS, -STEP_TENTHS), MIN_TENTHS);
    }

    #[test]
    fn image_modes_are_sensors() {
        assert_eq!(sensor("00").unwrap(), Sensor::Visible);
        assert_eq!(sensor("03").unwrap(), Sensor::Thermal);
        assert!(sensor("01").is_err()); // reserved
    }

    #[test]
    fn palettes_have_the_readme_codes() {
        assert_eq!(palette_code("black_hot"), Some("09"));
        assert_eq!(palette_code("rainbow"), Some("06"));
        assert_eq!(palette_code("sepia"), None);
        // Every name maps to a code and back again.
        for (name, code) in PALETTES {
            assert_eq!(palette_name(code).unwrap(), name);
        }
    }
}
