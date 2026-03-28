pub mod c1plus;
pub mod scan;

use anyhow::{anyhow, Result};
use btleplug::api::Characteristic;
use btleplug::platform::Peripheral;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::ble::{connect_and_find_chars, first_adapter, resolve_device, TargetFilter};
#[derive(Parser, Debug)]
#[command(name = "temtop-sensor")]
#[command(about = "Temtop BLE CLI with scan, inspect, params, current, live and history support")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Scan(ScanCmd),
    C1plus(c1plus::C1PlusCli),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Csv,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum StreamFormat {
    Text,
    Jsonl,
    Csv,
    Prometheus,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum HistoryFormat {
    Text,
    Json,
    Jsonl,
    Csv,
    Prometheus,
}

#[derive(Args, Debug)]
pub struct ScanCmd {
    #[arg(long, default_value_t = 10)]
    pub seconds: u64,

    #[arg(long)]
    pub show_all: bool,

    #[arg(long, value_enum, default_value_t = OutputFormat::Csv)]
    pub format: OutputFormat,
}

#[derive(Args, Debug, Clone)]
pub struct TargetArgs {
    #[arg(
        long,
        help = "BLE MAC address. Optional if the target can be found by GUID or profile scan."
    )]
    pub address: Option<String>,

    #[arg(
        long,
        help = "Device GUID. Optional when it can be inferred from the advertisement name, such as C1+_<guid>."
    )]
    pub guid: Option<String>,

    #[arg(long, default_value_t = 15)]
    pub scan_timeout_secs: u64,

    #[arg(long, default_value_t = 10)]
    pub timeout_secs: u64,

    #[arg(long)]
    pub verbose: bool,
}

pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_jsonl<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

pub fn target_filter(target: &TargetArgs, sensor: &str) -> Result<TargetFilter> {
    Ok(TargetFilter {
        address: target.address.clone(),
        guid: target.guid.clone(),
        sensor: Some(sensor.to_string()),
        scan_timeout_secs: target.scan_timeout_secs,
        require_inferred_guid: false,
    })
}

pub fn resolved_guid(target: &TargetArgs, discovered_guid: Option<String>) -> Result<String> {
    target
        .guid
        .clone()
        .or(discovered_guid)
        .ok_or_else(|| {
            anyhow!(
                "GUID could not be inferred from the advertisement; pass --guid explicitly or run scan first"
            )
        })
}

pub struct ConnectedTarget {
    pub peripheral: Peripheral,
    pub address: String,
    pub name: Option<String>,
    pub guid: String,
    pub write_char: Characteristic,
    pub notify_char: Characteristic,
}

pub async fn connect_target(target: &TargetArgs, sensor: &str) -> Result<ConnectedTarget> {
    let adapter = first_adapter().await?;
    let mut filter = target_filter(target, sensor)?;
    filter.require_inferred_guid = target.guid.is_none();
    let (peripheral, discovered, profile) = resolve_device(&adapter, &filter).await?;
    let guid = resolved_guid(target, discovered.guid)?;
    let (write_char, notify_char) = connect_and_find_chars(&peripheral, profile).await?;

    Ok(ConnectedTarget {
        peripheral,
        address: discovered.address,
        name: discovered.name,
        guid,
        write_char,
        notify_char,
    })
}
