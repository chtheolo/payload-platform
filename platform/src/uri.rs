//! The camera URI given with `--camera or -c `
//! ```text
//! <scheme>://<host>:<port>[?<option>=<value>[&...]]
//! ```
//! 
//! The scheme picks the driver. Host, port and options belong to that driver.

use std::collections::BTreeMap;

use url::Url;
use thiserror::Error;


#[derive(Debug, Clone)]
pub struct CameraUri {
    pub text: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    //I am using BTreeMap to keep the order of the keys always the same
    pub options: BTreeMap<String, String>
}

#[derive(Debug, Error)]
pub enum UriError {
    #[error("`{0}` is not valid URI")]
    Invalid(String),
    #[error("The camera URI needs a host and a port, e.g. mavlink://127.0.0.1:14580")]
    MissingAddress,
    #[error("unknown option `{option}`; this driver accepts: {accepted}")]
    UnknownOption { option: String, accepted: String },
    #[error("option `{option}` has an invalid value `{value}`")]
    BadValue { option: String, value: String }
}

impl CameraUri {
    pub fn parse(text: &str) -> Result<CameraUri, UriError> {
        let url = Url::parse(text).map_err(|_| UriError::Invalid(text.to_string()))?;
        println!("{}", url);
        println!("{url:?}");

        Ok(
            CameraUri{
                text: text.to_string(),
                scheme: String::new(),
                host: String::from("mavlink"),
                port: 1000,
                options: BTreeMap::new(),
            }
        )
    }

    // pub fn accept_only(&self, accepted: &[&str]) -> Result<(), UriError> {
    //     match self.options.keys().
    // }
}