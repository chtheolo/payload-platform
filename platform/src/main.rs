//! The payload platform: runs on the drone, between one camera and the ground control station.
//!
//! Start-up: read the command line, start the payload core (which connects to the camera in
//! the background), then serve the HTTP API for the GCS until Ctrl-C.
//!
//! How the files fit together (README, "Architecture"):
//!
//! - `api.rs`      HTTP routes and JSON; knows nothing about cameras
//! - `payload.rs`  the payload core: validation, state cache, polling, command queue
//! - `drivers/`    the driver contract, and one driver per camera protocol
//! - `model.rs`    the camera-independent model the GCS sees
//! - `uri.rs`      the camera URI
//! - `error.rs`    every error, and what the GCS receives for each

mod api;
mod drivers;
mod error;
mod model;
mod payload;
mod uri;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::uri::CameraUri;


/// Translation layer between one camera payload and the ground control station.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The camera, e.g. mavlink://127.0.0.1:14580 (README, "Camera URI").
    #[arg(short, long)]
    camera: String,
    /// where the GCS API listens.
    #[arg(short, long, default_value="0.0.0.0:8080")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    // INFO by default. Otherwise, 'RUST_LOG=debug ...'
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!("{message}");
            ExitCode::FAILURE
        }
    }
}

// maybe here I can add an AppError enum that holds
// UriError, CameraError, NetworkError

async fn run(cli: Cli) -> Result<(), String> {
    let uri = CameraUri::parse(&cli.camera).map_err(|error| error.to_string())?;
    info!("Camera URI: {:?}", uri);
    info!("Camera URI: {:#?}", uri);
    let (handle, core_task) = payload::start(uri);

    let listener = TcpListener::bind(cli.listen)
        .await
        .map_err(|error| format!("can't listen on {}: {error}", cli.listen))?;
    info!("GCS API listening on http://{}", cli.listen);

    // here we pass to axum task the other end of the PayloadHandler, the one that posts Jobs to the queue
    // Jobs: mpsc::Sender<Job> AND the snapshot: watch::Receiver<Snapshot>
    let server = axum::serve(listener, api::router(handle)).with_graceful_shutdown(shutdown());

    // Run until the server stops (after Ctrl-C) or the core gives up (for example on an
    // unknown camera model, which retrying can't fix).
    tokio::select! {
        result = server.into_future() => result.map_err(|error| error.to_string()),
        result = core_task => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(crash) => Err(format!("the payload core crashed: {crash}")),
        },
    }
    // info!("Handle: {:#?}, core_task: {:?}", handle, core_task);
}

/// Completes when Ctrl-C is pressed, which tells the server to finish and stop.
async fn shutdown() {
    // `.ok()` ignores the error, which only happens if the Ctrl-C handler can't be installed.
    tokio::signal::ctrl_c().await.ok();
    info!("shutting down");
}