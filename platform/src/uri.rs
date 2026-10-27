//! The camera URI given with `--camera or -c `
//! ```text
//! <scheme>://<host>:<port>[?<option>=<value>[&...]]
//! ```
//! 
//! The scheme picks the driver. Host, port and options belong to that driver.

use std::collections::BTreeMap;

pub struct CameraUri {
    pub text: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub options: BTreeMap<String, String>
}