//! Where the logs go: always to the terminal, and with `--log-dir` also to files on the drone,
//! one per day, keeping the last week.
//!
//! On a real drone the platform would run as a service, and the system's journal (systemd's
//! journald) could keep the terminal output instead. `--log-dir` is for when there is no such
//! journal, or when a copy should go somewhere else, such as a removable card.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// How many daily files to keep. Older ones are deleted, so the logs can't fill the storage.
const KEEP_DAYS: usize = 7;

/// Keeps the file writer running until `main` ends (see `start`).
///
/// Rust: the field is never read, but holding it is its whole job. The `_` at the start of its
/// name tells the compiler that's on purpose.
pub struct Logs {
    _file_writer: Option<WorkerGuard>,
}

/// Starts logging: INFO and above by default, more with e.g. `RUST_LOG=debug`. With a
/// `directory`, the same lines also go to `platform.<date>.log` files in it.
pub fn start(directory: Option<&Path>) -> Result<Logs, String> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Rust: an `Option` of a layer is a layer too, and `None` does nothing. That lets the one
    // chain of `.with(...)` below cover both cases, with files and without.
    let (file_layer, file_writer) = match directory {
        Some(directory) => {
            // The folder is created if it doesn't exist yet.
            let files = Builder::new()
                .rotation(Rotation::DAILY)
                .filename_prefix("platform")
                .filename_suffix("log")
                .max_log_files(KEEP_DAYS)
                .build(directory)
                .map_err(|error| format!("can't write logs to {}: {error}", directory.display()))?;
            // Writing to a file can be slow, for example on an SD card. `non_blocking` hands each
            // line to a background thread, so logging never makes the platform wait. The guard
            // stops that thread, after writing what's still waiting, when it's dropped.
            let (writer, guard) = tracing_appender::non_blocking(files);
            // No colour codes in the files.
            let layer = fmt::layer().with_writer(writer).with_ansi(false);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(file_layer)
        .init();
    Ok(Logs {
        _file_writer: file_writer,
    })
}
