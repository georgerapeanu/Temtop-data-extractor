use anyhow::{anyhow, bail, Result};
use btleplug::api::Peripheral as _;
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde::Serialize;
use std::future::Future;
use std::io::{self, Write};
use tokio::time::{interval, Duration};

use crate::ble::{
    disable_all_notifications, drain_notifications, first_adapter, format_frame_hex, request_once,
    resolve_device, wait_for_command,
};
use crate::protocol::build_request;
use crate::sensor::c1_plus::C1_PLUS;
use crate::sensor::SensorProfile;
use serde_json::json;

use super::{
    connect_target, print_json, print_jsonl, target_filter, HistoryFormat, OutputFormat,
    StreamFormat, TargetArgs,
};

#[derive(Args, Debug)]
pub struct C1PlusCli {
    #[command(subcommand)]
    pub command: C1PlusCommand,
}

#[derive(Subcommand, Debug)]
pub enum C1PlusCommand {
    Inspect(C1PlusInspectCmd),
    Params(C1PlusOutputCmd),
    Current(C1PlusOutputCmd),
    History(C1PlusHistoryCmd),
    Live(C1PlusLiveCmd),
}

#[derive(Args, Debug)]
pub struct C1PlusInspectCmd {
    #[command(flatten)]
    pub target: TargetArgs,

    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

#[derive(Args, Debug)]
pub struct C1PlusOutputCmd {
    #[command(flatten)]
    pub target: TargetArgs,

    #[arg(long, value_enum, default_value_t = OutputFormat::Csv)]
    pub format: OutputFormat,
}

#[derive(Args, Debug)]
pub struct C1PlusHistoryCmd {
    #[command(flatten)]
    pub target: TargetArgs,

    #[arg(long, value_enum, default_value_t = HistoryFormat::Csv)]
    pub format: HistoryFormat,
}

#[derive(Args, Debug)]
pub struct C1PlusLiveCmd {
    #[command(flatten)]
    pub target: TargetArgs,

    #[arg(long, default_value_t = 10)]
    pub poll_secs: u64,

    #[arg(long, value_enum, default_value_t = StreamFormat::Csv)]
    pub format: StreamFormat,

    #[arg(long)]
    pub limit: Option<usize>,
}

pub async fn run(cli: C1PlusCli) -> Result<()> {
    match cli.command {
        C1PlusCommand::Inspect(cmd) => inspect(cmd).await,
        C1PlusCommand::Params(cmd) => params(cmd).await,
        C1PlusCommand::Current(cmd) => current(cmd).await,
        C1PlusCommand::History(cmd) => history(cmd).await,
        C1PlusCommand::Live(cmd) => live(cmd).await,
    }
}

async fn disconnect_on_sigint<T, F>(
    peripheral: &btleplug::platform::Peripheral,
    future: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::pin!(future);
    tokio::select! {
        result = future => result,
        _ = tokio::signal::ctrl_c() => {
            let _ = peripheral.disconnect().await;
            bail!("interrupted by SIGINT");
        }
    }
}

async fn with_disconnect<T, F>(peripheral: &btleplug::platform::Peripheral, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let result = future.await;
    let _ = peripheral.disconnect().await;
    result
}

fn parse_history_meta(meta: &[u8]) -> Result<(u16, u16)> {
    if meta.len() < 19 {
        bail!("history meta frame too short: {}", meta.len());
    }
    let ready = meta[14];
    if ready != 0x01 {
        bail!("history meta not ready: status=0x{ready:02x}");
    }
    let total_count = u16::from_be_bytes(
        meta[15..17]
            .try_into()
            .map_err(|_| anyhow!("history meta missing total_count"))?,
    );
    let record_len = u16::from_be_bytes(
        meta[17..19]
            .try_into()
            .map_err(|_| anyhow!("history meta missing record_len"))?,
    );
    if record_len == 0 {
        bail!("history meta reported record_len=0");
    }
    Ok((total_count, record_len))
}

fn print_command_output(
    format: OutputFormat,
    output: &dyn crate::sensor::CommandOutput,
) -> Result<()> {
    match format {
        OutputFormat::Json => print_json(&output.render_json_value())?,
        OutputFormat::Csv => {
            println!("{}", output.render_csv_header());
            println!("{}", output.render_csv_row());
        }
        OutputFormat::Text => println!("{}", output.render_text()),
    }
    Ok(())
}

async fn write_request(
    peripheral: &btleplug::platform::Peripheral,
    write_char: &btleplug::api::Characteristic,
    frame: &[u8],
    verbose: bool,
) -> Result<()> {
    if verbose {
        println!("write  -> {}", format_frame_hex(frame));
    }
    peripheral
        .write(write_char, frame, btleplug::api::WriteType::WithoutResponse)
        .await?;
    Ok(())
}

fn print_history_output(
    format: HistoryFormat,
    session_json: serde_json::Value,
    records: &[Box<dyn crate::sensor::HistoryRecordOutput>],
) -> Result<()> {
    match format {
        HistoryFormat::Text => {
            println!(
                "address: {}\nsensor: {}\nguid: {}\nrecords: {}\nrecord_len: {}",
                session_json["address"].as_str().unwrap_or(""),
                session_json["sensor"].as_str().unwrap_or(""),
                session_json["guid"].as_str().unwrap_or(""),
                session_json["total_count"].as_u64().unwrap_or(0),
                session_json["record_len"].as_u64().unwrap_or(0)
            );
            for record in records {
                println!("{}", record.render_text_row());
            }
        }
        HistoryFormat::Json => {
            let records_json: Vec<_> = records
                .iter()
                .map(|record| record.render_json_value())
                .collect();
            print_json(&json!({
                "session": session_json,
                "records": records_json,
            }))?;
        }
        HistoryFormat::Jsonl => {
            for record in records {
                print_jsonl(&record.render_json_value())?;
            }
        }
        HistoryFormat::Csv => {
            let mut out = io::stdout().lock();
            writeln!(out, "index,timestamp,temperature_c,humidity_rh,co2_ppm")?;
            for record in records {
                writeln!(out, "{}", record.render_csv_row())?;
            }
        }
    }
    Ok(())
}

async fn fetch_command_output(
    cmd: C1PlusOutputCmd,
    request_cmd: u8,
    body: &[u8],
    parser: impl FnOnce(&[u8]) -> Result<Box<dyn crate::sensor::CommandOutput>>,
) -> Result<()> {
    let target = cmd.target;
    let connected = connect_target(&target, C1_PLUS.id()).await?;
    with_disconnect(&connected.peripheral, async {
        let response = disconnect_on_sigint(
            &connected.peripheral,
            request_once(
                &connected.peripheral,
                &connected.write_char,
                &connected.notify_char,
                &build_request(request_cmd, &connected.guid, body)?,
                request_cmd,
                &connected.guid,
                target.timeout_secs,
                target.verbose,
            ),
        )
        .await?;
        let output = parser(&response)?;
        print_command_output(cmd.format, output.as_ref())
    })
    .await
}

async fn inspect(cmd: C1PlusInspectCmd) -> Result<()> {
    let target = cmd.target;
    let adapter = first_adapter().await?;
    let filter = target_filter(&target, C1_PLUS.id())?;
    let (peripheral, discovered, profile) = resolve_device(&adapter, &filter).await?;
    peripheral.connect().await?;
    with_disconnect(&peripheral, async {
        peripheral.discover_services().await?;
        disable_all_notifications(&peripheral).await;

        #[derive(Serialize)]
        struct CharInfo {
            uuid: String,
            properties: Vec<String>,
        }
        #[derive(Serialize)]
        struct ServiceInfo {
            uuid: String,
            characteristics: Vec<CharInfo>,
        }
        #[derive(Serialize)]
        struct InspectInfo {
            address: String,
            name: Option<String>,
            guid: Option<String>,
            sensor: String,
            expected_service_uuid: String,
            services: Vec<ServiceInfo>,
        }

        let info = InspectInfo {
            address: discovered.address,
            name: discovered.name,
            guid: discovered.guid,
            sensor: profile.id().to_string(),
            expected_service_uuid: profile.service_uuid().to_string(),
            services: peripheral
                .services()
                .iter()
                .map(|svc| ServiceInfo {
                    uuid: svc.uuid.to_string(),
                    characteristics: svc
                        .characteristics
                        .iter()
                        .map(|ch| CharInfo {
                            uuid: ch.uuid.to_string(),
                            properties: ch.properties.iter().map(|p| format!("{p:?}")).collect(),
                        })
                        .collect(),
                })
                .collect(),
        };

        if cmd.format == OutputFormat::Json {
            print_json(&info)?;
        } else {
            println!("address: {}", info.address);
            println!("sensor: {}", info.sensor);
            println!("expected service: {}", info.expected_service_uuid);
            if let Some(name) = info.name {
                println!("name: {name}");
            }
            if let Some(guid) = info.guid {
                println!("guid: {guid}");
            }
            for svc in info.services {
                println!("\nservice: {}", svc.uuid);
                for ch in svc.characteristics {
                    println!("  char: {} | props: {}", ch.uuid, ch.properties.join(", "));
                }
            }
        }

        Ok(())
    })
    .await
}

async fn params(cmd: C1PlusOutputCmd) -> Result<()> {
    fetch_command_output(cmd, C1_PLUS.cmd_get_params(), &[1], |response| {
        C1_PLUS.parse_params(response)
    })
    .await
}

async fn current(cmd: C1PlusOutputCmd) -> Result<()> {
    fetch_command_output(cmd, C1_PLUS.cmd_get_current(), &[], |response| {
        C1_PLUS.parse_current(response)
    })
    .await
}

async fn history(cmd: C1PlusHistoryCmd) -> Result<()> {
    let connected = connect_target(&cmd.target, C1_PLUS.id()).await?;
    with_disconnect(&connected.peripheral, async {
        let history_result = disconnect_on_sigint(&connected.peripheral, async {
            let peripheral = &connected.peripheral;
            let mut notifications = peripheral.notifications().await?;
            peripheral.subscribe(&connected.notify_char).await?;
            let drained = drain_notifications(&mut notifications, 250, cmd.target.verbose).await?;
            if cmd.target.verbose && drained > 0 {
                eprintln!("drained {drained} queued notifications before history request");
            }

            let meta_req = build_request(C1_PLUS.cmd_get_history_meta(), &connected.guid, &[1])?;
            write_request(
                peripheral,
                &connected.write_char,
                &meta_req,
                cmd.target.verbose,
            )
            .await?;
            let meta = wait_for_command(
                &mut notifications,
                C1_PLUS.cmd_get_history_meta(),
                &connected.guid,
                cmd.target.timeout_secs,
                cmd.target.verbose,
            )
            .await?;
            let (total_count, record_len) = parse_history_meta(&meta)?;
            let session_json = json!({
                "address": peripheral.address().to_string(),
                "guid": connected.guid.clone(),
                "sensor": C1_PLUS.id(),
                "total_count": total_count,
                "record_len": record_len,
            });

            let mut records = Vec::new();
            let mut retries_left = 3i32;
            while records.len() < total_count as usize {
                let chunk_req =
                    build_request(C1_PLUS.cmd_get_history_chunk(), &connected.guid, &[1])?;
                write_request(
                    peripheral,
                    &connected.write_char,
                    &chunk_req,
                    cmd.target.verbose,
                )
                .await?;

                let chunk = match wait_for_command(
                    &mut notifications,
                    C1_PLUS.cmd_get_history_chunk(),
                    &connected.guid,
                    cmd.target.timeout_secs,
                    cmd.target.verbose,
                )
                .await
                {
                    Ok(chunk) => {
                        retries_left = 3;
                        chunk
                    }
                    Err(err) => {
                        retries_left -= 1;
                        if retries_left < 0 {
                            let _ = peripheral.unsubscribe(&connected.notify_char).await;
                            return Err(err);
                        }
                        continue;
                    }
                };

                let payload = &chunk[17..chunk.len() - 1];
                if payload.len() % record_len as usize != 0 {
                    bail!(
                        "history payload size {} is not a multiple of record length {}",
                        payload.len(),
                        record_len
                    );
                }
                let before = records.len();
                for raw_record in payload.chunks_exact(record_len as usize) {
                    records.push(C1_PLUS.parse_history_record(raw_record, records.len() + 1)?);
                    if records.len() >= total_count as usize {
                        break;
                    }
                }
                if records.len() == before {
                    retries_left -= 1;
                    if retries_left < 0 {
                        bail!("history chunk produced no records after multiple retries");
                    }
                    if cmd.target.verbose {
                        eprintln!("history chunk produced no records; retrying");
                    }
                    continue;
                }
            }

            let _ = peripheral.unsubscribe(&connected.notify_char).await;
            Ok((session_json, records))
        })
        .await;

        let (session_json, records) = history_result?;
        print_history_output(cmd.format, session_json, &records)
    })
    .await
}

async fn live(cmd: C1PlusLiveCmd) -> Result<()> {
    let connected = connect_target(&cmd.target, C1_PLUS.id()).await?;
    with_disconnect(&connected.peripheral, async {
        let mut notifications = connected.peripheral.notifications().await?;
        connected
            .peripheral
            .subscribe(&connected.notify_char)
            .await?;

        if cmd.poll_secs > 0 {
            let initial = build_request(C1_PLUS.cmd_get_current(), &connected.guid, &[])?;
            if cmd.target.verbose {
                println!("write  -> {}", format_frame_hex(&initial));
            }
            connected
                .peripheral
                .write(
                    &connected.write_char,
                    &initial,
                    btleplug::api::WriteType::WithoutResponse,
                )
                .await?;
        }

        let mut ticker = if cmd.poll_secs > 0 {
            Some(interval(Duration::from_secs(cmd.poll_secs)))
        } else {
            None
        };
        let mut seen = 0usize;
        let mut wrote_csv_header = false;

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = async {
                    if let Some(ticker) = &mut ticker {
                        ticker.tick().await;
                    }
                }, if ticker.is_some() => {
                    let req = build_request(C1_PLUS.cmd_get_current(), &connected.guid, &[])?;
                    if cmd.target.verbose {
                        println!("write  -> {}", format_frame_hex(&req));
                    }
                    connected
                        .peripheral
                        .write(
                            &connected.write_char,
                            &req,
                            btleplug::api::WriteType::WithoutResponse,
                        )
                        .await?;
                }
                maybe_notification = notifications.next() => {
                    let Some(notification) = maybe_notification else { break; };
                    let frame = notification.value;
                    if cmd.target.verbose {
                        println!("notify <- {}", format_frame_hex(&frame));
                    }
                    if !crate::protocol::verify_response(&frame) {
                        continue;
                    }
                    match frame[3] {
                        received_cmd
                            if received_cmd == C1_PLUS.cmd_realtime_notify()
                                || received_cmd == C1_PLUS.cmd_get_current() =>
                        {
                            let sample = C1_PLUS.parse_current(&frame)?;
                            match cmd.format {
                                StreamFormat::Jsonl => print_jsonl(&sample.render_json_value())?,
                                StreamFormat::Csv => {
                                    if !wrote_csv_header {
                                        println!("{}", sample.render_csv_header());
                                        wrote_csv_header = true;
                                    }
                                    println!("{}", sample.render_csv_row());
                                }
                                StreamFormat::Text => println!("{}", sample.render_text()),
                            }
                            seen += 1;
                            if cmd.limit.is_some_and(|limit| seen >= limit) {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })
    .await
}
