use anyhow::{anyhow, bail, Result};
use btleplug::api::Peripheral as _;
use clap::{Args, Subcommand, ValueEnum};
use futures::StreamExt;
use serde::Serialize;
use serde_json::json;
use std::future::Future;
use std::io::{self, Write};
use std::time::Instant;
use tokio::time::{interval, Duration};

use crate::ble::{
    disable_all_notifications, drain_notifications, first_adapter, format_frame_hex, request_once,
    resolve_device, wait_for_command,
};
use crate::output::prometheus::{
    render_collect_metrics, unix_timestamp_ms_now, CollectReport, CollectTelemetry, MetricLabels,
};
use crate::protocol::build_request;
use crate::sensor::c1_plus::C1_PLUS;
use crate::sensor::SensorProfile;

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
    Collect(C1PlusCollectCmd),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum CollectFormat {
    Prometheus,
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

#[derive(Args, Debug)]
pub struct C1PlusCollectCmd {
    #[command(flatten)]
    pub target: TargetArgs,

    #[arg(
        long,
        help = "Human-friendly sensor name included in metrics labels and logs."
    )]
    pub sensor_name: Option<String>,

    #[arg(long, value_enum, default_value_t = CollectFormat::Prometheus)]
    pub format: CollectFormat,
}

pub async fn run(cli: C1PlusCli) -> Result<()> {
    match cli.command {
        C1PlusCommand::Inspect(cmd) => inspect(cmd).await,
        C1PlusCommand::Params(cmd) => params(cmd).await,
        C1PlusCommand::Current(cmd) => current(cmd).await,
        C1PlusCommand::History(cmd) => history(cmd).await,
        C1PlusCommand::Live(cmd) => live(cmd).await,
        C1PlusCommand::Collect(cmd) => collect(cmd).await,
    }
}

#[derive(Debug)]
struct FetchOutcome<T> {
    value: Option<T>,
    success: bool,
    duration_seconds: f64,
    retries: u64,
    error: Option<anyhow::Error>,
}

#[derive(Debug)]
struct HistoryData {
    session: serde_json::Value,
    records: Vec<serde_json::Value>,
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

fn log_event(level: &str, event: &str, labels: Option<&MetricLabels>, extra: serde_json::Value) {
    let mut payload = serde_json::Map::new();
    payload.insert("level".to_string(), json!(level));
    payload.insert("event".to_string(), json!(event));

    if let Some(labels) = labels {
        payload.insert("sensor".to_string(), json!(labels.sensor));
        payload.insert("guid".to_string(), json!(labels.guid));
        payload.insert("sensor_name".to_string(), json!(labels.sensor_name));
        payload.insert("mac".to_string(), json!(labels.mac));
    }

    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            payload.insert(key.clone(), value.clone());
        }
    }

    eprintln!("{}", serde_json::Value::Object(payload));
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

async fn fetch_current_json(
    connected: &super::ConnectedTarget,
    target: &TargetArgs,
) -> FetchOutcome<serde_json::Value> {
    let started = Instant::now();
    let frame = match build_request(C1_PLUS.cmd_get_current(), &connected.guid, &[]) {
        Ok(frame) => frame,
        Err(err) => {
            return FetchOutcome {
                value: None,
                success: false,
                duration_seconds: started.elapsed().as_secs_f64(),
                retries: 0,
                error: Some(err),
            };
        }
    };

    match disconnect_on_sigint(
        &connected.peripheral,
        request_once(
            &connected.peripheral,
            &connected.write_char,
            &connected.notify_char,
            &frame,
            C1_PLUS.cmd_get_current(),
            &connected.guid,
            target.timeout_secs,
            target.verbose,
        ),
    )
    .await
    {
        Ok(response) => match C1_PLUS.parse_current(&response) {
            Ok(sample) => FetchOutcome {
                value: Some(sample.render_json_value()),
                success: true,
                duration_seconds: started.elapsed().as_secs_f64(),
                retries: 0,
                error: None,
            },
            Err(err) => FetchOutcome {
                value: None,
                success: false,
                duration_seconds: started.elapsed().as_secs_f64(),
                retries: 0,
                error: Some(err),
            },
        },
        Err(err) => FetchOutcome {
            value: None,
            success: false,
            duration_seconds: started.elapsed().as_secs_f64(),
            retries: 0,
            error: Some(err),
        },
    }
}

async fn fetch_history_json(
    connected: &super::ConnectedTarget,
    target: &TargetArgs,
    labels: &MetricLabels,
) -> FetchOutcome<HistoryData> {
    let started = Instant::now();
    let mut retries = 0u64;

    let result = disconnect_on_sigint(&connected.peripheral, async {
        let peripheral = &connected.peripheral;
        let mut notifications = peripheral.notifications().await?;
        peripheral.subscribe(&connected.notify_char).await?;
        let drained = drain_notifications(&mut notifications, 250, target.verbose).await?;
        if target.verbose && drained > 0 {
            eprintln!("drained {drained} queued notifications before history request");
        }

        let meta_req = build_request(C1_PLUS.cmd_get_history_meta(), &connected.guid, &[1])?;
        write_request(peripheral, &connected.write_char, &meta_req, target.verbose).await?;
        let meta = wait_for_command(
            &mut notifications,
            C1_PLUS.cmd_get_history_meta(),
            &connected.guid,
            target.timeout_secs,
            target.verbose,
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
            let chunk_req = build_request(C1_PLUS.cmd_get_history_chunk(), &connected.guid, &[1])?;
            write_request(
                peripheral,
                &connected.write_char,
                &chunk_req,
                target.verbose,
            )
            .await?;

            let chunk = match wait_for_command(
                &mut notifications,
                C1_PLUS.cmd_get_history_chunk(),
                &connected.guid,
                target.timeout_secs,
                target.verbose,
            )
            .await
            {
                Ok(chunk) => {
                    retries_left = 3;
                    chunk
                }
                Err(err) => {
                    retries += 1;
                    retries_left -= 1;
                    log_event(
                        "warn",
                        "history_chunk_retry",
                        Some(labels),
                        json!({
                            "retries_total": retries,
                            "retries_left": retries_left.max(0),
                            "error": err.to_string(),
                        }),
                    );
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
                records.push(
                    C1_PLUS
                        .parse_history_record(raw_record, records.len() + 1)?
                        .render_json_value(),
                );
                if records.len() >= total_count as usize {
                    break;
                }
            }
            if records.len() == before {
                retries += 1;
                retries_left -= 1;
                log_event(
                    "warn",
                    "history_chunk_empty_retry",
                    Some(labels),
                    json!({
                        "retries_total": retries,
                        "retries_left": retries_left.max(0),
                    }),
                );
                if retries_left < 0 {
                    bail!("history chunk produced no records after multiple retries");
                }
                continue;
            }
        }

        let _ = peripheral.unsubscribe(&connected.notify_char).await;
        Ok(HistoryData {
            session: session_json,
            records,
        })
    })
    .await;

    match result {
        Ok(value) => FetchOutcome {
            success: true,
            duration_seconds: started.elapsed().as_secs_f64(),
            retries,
            value: Some(value),
            error: None,
        },
        Err(err) => FetchOutcome {
            success: false,
            duration_seconds: started.elapsed().as_secs_f64(),
            retries,
            value: None,
            error: Some(err),
        },
    }
}

async fn history(cmd: C1PlusHistoryCmd) -> Result<()> {
    let connected = connect_target(&cmd.target, C1_PLUS.id()).await?;
    let labels = MetricLabels {
        mac: connected.address.clone(),
        guid: connected.guid.clone(),
        sensor: C1_PLUS.id().to_string(),
        sensor_name: connected
            .name
            .clone()
            .unwrap_or_else(|| connected.guid.clone()),
    };

    with_disconnect(&connected.peripheral, async {
        let history_result = fetch_history_json(&connected, &cmd.target, &labels).await;
        let history = history_result.value.ok_or_else(|| {
            history_result
                .error
                .unwrap_or_else(|| anyhow!("history collection failed"))
        })?;
        let records: Vec<_> = history
            .records
            .iter()
            .map(|record| {
                Box::new(JsonHistoryRecord(record.clone()))
                    as Box<dyn crate::sensor::HistoryRecordOutput>
            })
            .collect();
        print_history_output(cmd.format, history.session, &records)
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

async fn collect(cmd: C1PlusCollectCmd) -> Result<()> {
    let started_unix_ms = unix_timestamp_ms_now();
    let run_started = Instant::now();
    let provisional_labels = MetricLabels {
        mac: cmd
            .target
            .address
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        guid: cmd
            .target
            .guid
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        sensor: C1_PLUS.id().to_string(),
        sensor_name: cmd
            .sensor_name
            .clone()
            .or_else(|| cmd.target.guid.clone())
            .or_else(|| cmd.target.address.clone())
            .unwrap_or_else(|| "unknown".to_string()),
    };

    let connected = match connect_target(&cmd.target, C1_PLUS.id()).await {
        Ok(connected) => connected,
        Err(err) => {
            let report = CollectReport {
                labels: provisional_labels.clone(),
                current: None,
                history: Vec::new(),
                telemetry: CollectTelemetry {
                    started_unix_ms,
                    finished_unix_ms: unix_timestamp_ms_now(),
                    duration_seconds: run_started.elapsed().as_secs_f64(),
                    current_success: false,
                    current_duration_seconds: None,
                    history_success: false,
                    history_duration_seconds: None,
                    history_retries_total: 0,
                    history_records_total: 0,
                },
            };
            log_event(
                "error",
                "collect_connect_failed",
                Some(&provisional_labels),
                json!({
                    "error": err.to_string(),
                }),
            );
            match cmd.format {
                CollectFormat::Prometheus => {
                    print!("{}", render_collect_metrics(&report)?);
                }
            }
            return Err(err);
        }
    };
    let labels = MetricLabels {
        mac: connected.address.clone(),
        guid: connected.guid.clone(),
        sensor: C1_PLUS.id().to_string(),
        sensor_name: cmd
            .sensor_name
            .clone()
            .or_else(|| connected.name.clone())
            .unwrap_or_else(|| connected.guid.clone()),
    };

    log_event(
        "info",
        "collect_started",
        Some(&labels),
        json!({
            "command": "c1plus collect",
        }),
    );

    let (current, history) = with_disconnect(&connected.peripheral, async {
        log_event(
            "info",
            "collect_phase_started",
            Some(&labels),
            json!({ "phase": "current" }),
        );
        let current = fetch_current_json(&connected, &cmd.target).await;
        if let Some(err) = current.error.as_ref() {
            log_event(
                "error",
                "collect_phase_failed",
                Some(&labels),
                json!({
                    "phase": "current",
                    "duration_seconds": current.duration_seconds,
                    "error": err.to_string(),
                }),
            );
        } else {
            log_event(
                "info",
                "collect_phase_succeeded",
                Some(&labels),
                json!({
                    "phase": "current",
                    "duration_seconds": current.duration_seconds,
                }),
            );
        }

        log_event(
            "info",
            "collect_phase_started",
            Some(&labels),
            json!({ "phase": "history" }),
        );
        let history = fetch_history_json(&connected, &cmd.target, &labels).await;
        if let Some(err) = history.error.as_ref() {
            log_event(
                "error",
                "collect_phase_failed",
                Some(&labels),
                json!({
                    "phase": "history",
                    "duration_seconds": history.duration_seconds,
                    "retries_total": history.retries,
                    "error": err.to_string(),
                }),
            );
        } else {
            let records_total = history
                .value
                .as_ref()
                .map(|value| value.records.len())
                .unwrap_or(0);
            log_event(
                "info",
                "collect_phase_succeeded",
                Some(&labels),
                json!({
                    "phase": "history",
                    "duration_seconds": history.duration_seconds,
                    "retries_total": history.retries,
                    "records_total": records_total,
                }),
            );
        }

        Ok((current, history))
    })
    .await?;

    let finished_unix_ms = unix_timestamp_ms_now();
    let report = CollectReport {
        labels: labels.clone(),
        current: current.value.clone(),
        history: history
            .value
            .as_ref()
            .map(|value| value.records.clone())
            .unwrap_or_default(),
        telemetry: CollectTelemetry {
            started_unix_ms,
            finished_unix_ms,
            duration_seconds: run_started.elapsed().as_secs_f64(),
            current_success: current.success,
            current_duration_seconds: Some(current.duration_seconds),
            history_success: history.success,
            history_duration_seconds: Some(history.duration_seconds),
            history_retries_total: history.retries,
            history_records_total: history
                .value
                .as_ref()
                .map(|value| value.records.len())
                .unwrap_or(0),
        },
    };

    match cmd.format {
        CollectFormat::Prometheus => {
            print!("{}", render_collect_metrics(&report)?);
        }
    }

    log_event(
        if report.telemetry.current_success && report.telemetry.history_success {
            "info"
        } else {
            "error"
        },
        "collect_finished",
        Some(&labels),
        json!({
            "success": report.telemetry.current_success && report.telemetry.history_success,
            "duration_seconds": report.telemetry.duration_seconds,
            "history_retries_total": report.telemetry.history_retries_total,
            "history_records_total": report.telemetry.history_records_total,
            "history_session": history.value.as_ref().map(|value| value.session.clone()),
        }),
    );

    if let Some(err) = current.error {
        bail!(err);
    }
    if let Some(err) = history.error {
        bail!(err);
    }
    Ok(())
}

#[derive(Clone)]
struct JsonHistoryRecord(serde_json::Value);

impl crate::sensor::HistoryRecordOutput for JsonHistoryRecord {
    fn render_text_row(&self) -> String {
        self.0.to_string()
    }

    fn render_csv_row(&self) -> String {
        format!(
            "{},{},{},{},{}",
            self.0["index"].as_u64().unwrap_or(0),
            self.0["timestamp"].as_str().unwrap_or(""),
            self.0["temperature_c"].as_f64().unwrap_or(0.0),
            self.0["humidity_rh"].as_f64().unwrap_or(0.0),
            self.0["co2_ppm"].as_u64().unwrap_or(0)
        )
    }

    fn render_json_value(&self) -> serde_json::Value {
        self.0.clone()
    }
}
