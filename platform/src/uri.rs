//! The camera URI given with `--camera or -c `
//! ```text
//! <scheme>://<host>:<port>[?<option>=<value>[&...]]
//! ```
//! 
//! The scheme picks the driver. Host, port and options belong to that driver.

use std::collections::BTreeMap;
use std::str::FromStr;

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
        // url crate handles the tricky parts for us, ports, IPv 
        let url = Url::parse(text).map_err(|_| UriError::Invalid(text.to_string()))?;
        let host = url.host_str().ok_or(UriError::MissingAddress)?.to_string();
        let port = url.port().ok_or(UriError::MissingAddress)?;

        // into_owned() copies the values into the entries of the map, so map can live 
        // on each own 
        let options = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();

        Ok(
            CameraUri{
                text: text.to_string(),
                scheme: url.scheme().to_string(),
                host,
                port,
                options,
            }
        )
    }

    /// fails on any options that is not included. Protected by typos too.
    /// `?profile=bx20` correct
    /// `?prfile=bx20` wrong 
    pub fn accept_only(&self, accepted: &[&str]) -> Result<(), UriError> {
        match self.options.keys().find(|option| !accepted.contains(&option.as_str())) {
            Some(option) => Err(UriError::UnknownOption{
                option: option.clone(),
                accepted: accepted.join(", "),
            }),
            None => Ok(()),
        }
    }

    /// Return the value of the option converted to the type the caller wants
    /// FromStr means any type that can be parsed from text
    pub fn option<T: FromStr>(&self, option: &str, default: T) -> Result<T, UriError> {
        match self.options.get(option){
            None => Ok(default),
            Some(value) => value.parse().map_err(|_| UriError::BadValue {
                option: option.to_string(),
                value: value.clone(),
            }),
        }
    }
}

// Rust: `#[cfg(test)]` compiles this module only for `cargo test`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_mavlink_uri() {
        let uri = CameraUri::parse("mavlink://127.0.0.1:14580?profile=bx20").unwrap();
        assert_eq!(uri.scheme, "mavlink");
        assert_eq!(uri.host, "127.0.0.1");
        assert_eq!(uri.port, 14580);
        assert_eq!(uri.option("profile", "auto".to_string()).unwrap(), "bx20");
        assert_eq!(uri.option("system_id", 1u8).unwrap(), 1);
    }

    #[test]
    fn a_port_is_required() {
        assert!(matches!(
            CameraUri::parse("mavlink://127.0.0.1"),
            Err(UriError::MissingAddress)
        ));
    }

    #[test]
    fn unknown_options_are_refused() {
        let uri = CameraUri::parse("mavlink://127.0.0.1:14580?profle=bx20").unwrap();
        assert!(uri.accept_only(&["profile"]).is_err());
    }
}
