mod ble;
mod cli;
mod protocol;
mod sensor;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan(cmd) => crate::cli::scan::run(cmd).await,
        Command::C1plus(cmd) => crate::cli::c1plus::run(cmd).await,
    }
}
