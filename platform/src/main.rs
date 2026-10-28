mod uri;

// use tokio::net::TcpListener;
use std::process::ExitCode;
use clap::Parser;
use std::net::SocketAddr;
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
    Ok(())
}