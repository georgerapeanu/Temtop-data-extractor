use anyhow::Result;
use std::io::{self, Write};

use crate::ble::{first_adapter, scan_devices};

use super::{print_json, OutputFormat, ScanCmd};

pub async fn run(cmd: ScanCmd) -> Result<()> {
    let adapter = first_adapter().await?;
    let devices = scan_devices(&adapter, cmd.seconds).await?;
    let devices: Vec<_> = if cmd.show_all {
        devices
    } else {
        devices.into_iter().filter(|dev| dev.sensor.is_some()).collect()
    };

    if cmd.format == OutputFormat::Json {
        print_json(&devices)?;
        return Ok(());
    }

    if devices.is_empty() && !cmd.show_all {
        println!("no Temtop devices detected");
        println!("retry with --show-all to inspect every nearby BLE advertisement");
        return Ok(());
    }

    match cmd.format {
        OutputFormat::Csv => {
            let mut out = io::stdout().lock();
            writeln!(out, "address,sensor,guid,name")?;
            for dev in devices {
                writeln!(
                    out,
                    "{},{},{},{}",
                    dev.address,
                    dev.sensor.unwrap_or_else(|| "-".to_string()),
                    dev.guid.unwrap_or_else(|| "-".to_string()),
                    dev.name.unwrap_or_else(|| "-".to_string())
                )?;
            }
        }
        OutputFormat::Text => {
            for dev in devices {
                println!(
                    "{}\t{}\t{}\t{}",
                    dev.address,
                    dev.sensor.unwrap_or_else(|| "-".to_string()),
                    dev.guid.unwrap_or_else(|| "-".to_string()),
                    dev.name.unwrap_or_else(|| "-".to_string())
                );
            }
        }
        OutputFormat::Json => unreachable!(),
    }
    Ok(())
}
