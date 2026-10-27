use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SimError {
    #[error("failed to bind UDP port {port}: {source}")]
    Bind { port: u16, source: io::Error },
}
