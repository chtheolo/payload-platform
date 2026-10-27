mod error;
mod link;
mod mavlink_sim;
mod payload;
mod udp_sim;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing::{Level, error};

use error::SimError;
use mavlink_sim::MavlinkOptions;

const BX20_PORT: u16 = 14580;
const VLR10_PORT: u16 = 14581;
const LC30T_PORT: u16 = 9003;
const SYSTEM_ID: u8 = 1;
/// MAV_COMP_ID_CAMERA.
const COMPONENT_ID: u8 = 100;

/// Simulated EO/IR cameras, each speaking its vendor's protocol over UDP.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    simulator: Simulator,
}

#[derive(Subcommand)]
enum Simulator {
    /// Brightmere BX-20 (MAVLink, UDP port 14580).
    Bx20,
    /// Optronique Vallerin VLR-10 (MAVLink, UDP port 14581).
    Vlr10,
    /// Lucen LC-30T (UDP protocol, UDP port 9003).
    Lc30t,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    match run(cli.simulator).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(simulator: Simulator) -> Result<(), SimError> {
    match simulator {
        Simulator::Bx20 => mavlink_sim::run_bx20(mavlink_options(BX20_PORT)).await,
        Simulator::Vlr10 => mavlink_sim::run_vlr10(mavlink_options(VLR10_PORT)).await,
        Simulator::Lc30t => udp_sim::run(LC30T_PORT).await,
    }
}

fn mavlink_options(port: u16) -> MavlinkOptions {
    MavlinkOptions {
        port,
        system_id: SYSTEM_ID,
        component_id: COMPONENT_ID,
    }
}
