use anyhow::Result;
use serde_json::Value;
use std::fmt::Write;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

#[derive(Clone, Debug)]
pub struct MetricLabels {
    pub mac: String,
    pub guid: String,
    pub sensor: String,
    pub sensor_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct CollectTelemetry {
    pub started_unix_ms: i64,
    pub finished_unix_ms: i64,
    pub duration_seconds: f64,
    pub current_success: bool,
    pub current_duration_seconds: Option<f64>,
    pub history_success: bool,
    pub history_duration_seconds: Option<f64>,
    pub history_retries_total: u64,
    pub history_records_total: usize,
}

#[derive(Clone, Debug)]
pub struct CollectReport {
    pub labels: MetricLabels,
    pub current: Option<Value>,
    pub history: Vec<Value>,
    pub telemetry: CollectTelemetry,
}

pub fn render_collect_metrics(report: &CollectReport) -> Result<String> {
    let mut out = String::new();
    let base_labels = label_string(&[
        ("mac", report.labels.mac.as_str()),
        ("guid", report.labels.guid.as_str()),
        ("sensor", report.labels.sensor.as_str()),
        ("sensor_name", report.labels.sensor_name.as_str()),
    ]);

    let finish_ts = report.telemetry.finished_unix_ms;

    append_help(
        &mut out,
        "temtop_collect_success",
        "Whether the collection run completed successfully.",
    );
    append_gauge(
        &mut out,
        "temtop_collect_success",
        &base_labels,
        if report.telemetry.current_success && report.telemetry.history_success {
            1.0
        } else {
            0.0
        },
        Some(finish_ts),
    );

    append_help(
        &mut out,
        "temtop_collect_started_unix_seconds",
        "Unix start time of the collection run.",
    );
    append_gauge(
        &mut out,
        "temtop_collect_started_unix_seconds",
        &base_labels,
        report.telemetry.started_unix_ms as f64 / 1000.0,
        Some(finish_ts),
    );

    append_help(
        &mut out,
        "temtop_collect_duration_seconds",
        "Wall-clock duration of the collection run.",
    );
    append_gauge(
        &mut out,
        "temtop_collect_duration_seconds",
        &base_labels,
        report.telemetry.duration_seconds,
        Some(finish_ts),
    );

    append_help(
        &mut out,
        "temtop_collect_phase_success",
        "Whether an individual collection phase succeeded.",
    );
    append_phase_metric(
        &mut out,
        "temtop_collect_phase_success",
        &base_labels,
        "current",
        report.telemetry.current_success,
        Some(finish_ts),
    );
    append_phase_metric(
        &mut out,
        "temtop_collect_phase_success",
        &base_labels,
        "history",
        report.telemetry.history_success,
        Some(finish_ts),
    );

    append_help(
        &mut out,
        "temtop_collect_phase_duration_seconds",
        "Wall-clock duration of an individual collection phase.",
    );
    if let Some(value) = report.telemetry.current_duration_seconds {
        append_phase_gauge(
            &mut out,
            "temtop_collect_phase_duration_seconds",
            &base_labels,
            "current",
            value,
            Some(finish_ts),
        );
    }
    if let Some(value) = report.telemetry.history_duration_seconds {
        append_phase_gauge(
            &mut out,
            "temtop_collect_phase_duration_seconds",
            &base_labels,
            "history",
            value,
            Some(finish_ts),
        );
    }

    append_help(
        &mut out,
        "temtop_collect_history_retries_total",
        "Number of history chunk retries performed during the run.",
    );
    append_gauge(
        &mut out,
        "temtop_collect_history_retries_total",
        &base_labels,
        report.telemetry.history_retries_total as f64,
        Some(finish_ts),
    );

    append_help(
        &mut out,
        "temtop_collect_history_records_total",
        "Number of history records returned during the run.",
    );
    append_gauge(
        &mut out,
        "temtop_collect_history_records_total",
        &base_labels,
        report.telemetry.history_records_total as f64,
        Some(finish_ts),
    );

    if let Some(current) = &report.current {
        append_sample_metric(
            &mut out,
            "temtop_sensor_pm25_ug_m3",
            "Current PM2.5 reading from the sensor.",
            &base_labels,
            current,
            "pm25_ug_m3",
            None,
            Some(finish_ts),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_temperature_celsius",
            "Current temperature reading from the sensor.",
            &base_labels,
            current,
            "temperature_c",
            Some(("temperature_unit", "C")),
            Some(finish_ts),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_humidity_ratio",
            "Current relative humidity reading from the sensor.",
            &base_labels,
            current,
            "humidity_rh",
            None,
            Some(finish_ts),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_aqi",
            "Current air quality index reported by the sensor.",
            &base_labels,
            current,
            "aqi",
            None,
            Some(finish_ts),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_co2_ppm",
            "Current CO2 reading from the sensor.",
            &base_labels,
            current,
            "co2_ppm",
            None,
            Some(finish_ts),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_tvoc_ppb",
            "Current TVOC reading from the sensor.",
            &base_labels,
            current,
            "tvoc_ppb",
            None,
            Some(finish_ts),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_battery_percent",
            "Current battery percentage reported by the sensor.",
            &base_labels,
            current,
            "battery",
            None,
            Some(finish_ts),
        );
    }

    append_help(
        &mut out,
        "temtop_sensor_history_temperature_celsius",
        "Historical temperature readings reported by the sensor.",
    );
    append_help(
        &mut out,
        "temtop_sensor_history_humidity_ratio",
        "Historical humidity readings reported by the sensor.",
    );
    append_help(
        &mut out,
        "temtop_sensor_history_co2_ppm",
        "Historical CO2 readings reported by the sensor.",
    );

    for record in &report.history {
        let Some(timestamp) = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_local_timestamp_ms)
        else {
            continue;
        };

        append_sample_metric(
            &mut out,
            "temtop_sensor_history_temperature_celsius",
            "",
            &base_labels,
            record,
            "temperature_c",
            Some(("temperature_unit", "C")),
            Some(timestamp),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_history_humidity_ratio",
            "",
            &base_labels,
            record,
            "humidity_rh",
            None,
            Some(timestamp),
        );
        append_sample_metric(
            &mut out,
            "temtop_sensor_history_co2_ppm",
            "",
            &base_labels,
            record,
            "co2_ppm",
            None,
            Some(timestamp),
        );
    }

    Ok(out)
}

fn append_sample_metric(
    out: &mut String,
    metric: &str,
    help: &str,
    base_labels: &str,
    sample: &Value,
    field: &str,
    extra_label: Option<(&str, &str)>,
    timestamp_ms: Option<i64>,
) {
    let Some(value) = sample.get(field).and_then(Value::as_f64) else {
        return;
    };
    if !help.is_empty() {
        append_help(out, metric, help);
    }
    let labels = match extra_label {
        Some((key, value)) => format!("{base_labels},{key}=\"{}\"", escape_label_value(value)),
        None => base_labels.to_string(),
    };
    append_gauge(out, metric, &labels, value, timestamp_ms);
}

fn append_phase_metric(
    out: &mut String,
    metric: &str,
    base_labels: &str,
    phase: &str,
    success: bool,
    timestamp_ms: Option<i64>,
) {
    append_phase_gauge(
        out,
        metric,
        base_labels,
        phase,
        if success { 1.0 } else { 0.0 },
        timestamp_ms,
    );
}

fn append_phase_gauge(
    out: &mut String,
    metric: &str,
    base_labels: &str,
    phase: &str,
    value: f64,
    timestamp_ms: Option<i64>,
) {
    let labels = format!("{base_labels},phase=\"{}\"", escape_label_value(phase));
    append_gauge(out, metric, &labels, value, timestamp_ms);
}

fn append_help(out: &mut String, metric: &str, help: &str) {
    if help.is_empty() {
        return;
    }
    let _ = writeln!(out, "# HELP {metric} {help}");
    let _ = writeln!(out, "# TYPE {metric} gauge");
}

fn append_gauge(
    out: &mut String,
    metric: &str,
    labels: &str,
    value: f64,
    timestamp_ms: Option<i64>,
) {
    match timestamp_ms {
        Some(timestamp_ms) => {
            let _ = writeln!(out, "{metric}{{{labels}}} {value} {timestamp_ms}");
        }
        None => {
            let _ = writeln!(out, "{metric}{{{labels}}} {value}");
        }
    }
}

fn label_string(labels: &[(&str, &str)]) -> String {
    labels
        .iter()
        .map(|(key, value)| format!(r#"{key}="{}""#, escape_label_value(value)))
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_label_value(value: &str) -> String {
    value.replace('\\', r"\\").replace('"', r#"\""#)
}

fn parse_local_timestamp_ms(value: &str) -> Option<i64> {
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    let primitive = PrimitiveDateTime::parse(value, &format).ok()?;
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let timestamp = primitive.assume_offset(offset).unix_timestamp_nanos() / 1_000_000;
    Some(timestamp as i64)
}

pub fn unix_timestamp_ms_now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::{render_collect_metrics, CollectReport, CollectTelemetry, MetricLabels};
    use serde_json::json;

    #[test]
    fn renders_collect_and_history_metrics() {
        let rendered = render_collect_metrics(&CollectReport {
            labels: MetricLabels {
                mac: "AA:BB:CC:DD:EE:FF".to_string(),
                guid: "90158797465673526885".to_string(),
                sensor: "c1plus".to_string(),
                sensor_name: "living_room".to_string(),
            },
            current: Some(json!({
                "pm25_ug_m3": 12.3,
                "temperature_c": 21.5,
                "humidity_rh": 44.1,
                "aqi": 18,
                "co2_ppm": 700,
                "tvoc_ppb": 120,
                "battery": 88
            })),
            history: vec![json!({
                "timestamp": "2026-03-28 12:00:00",
                "temperature_c": 20.1,
                "humidity_rh": 43.5,
                "co2_ppm": 650
            })],
            telemetry: CollectTelemetry {
                started_unix_ms: 1_743_160_000_000,
                finished_unix_ms: 1_743_160_001_500,
                duration_seconds: 1.5,
                current_success: true,
                current_duration_seconds: Some(0.4),
                history_success: true,
                history_duration_seconds: Some(1.1),
                history_retries_total: 2,
                history_records_total: 1,
            },
        })
        .unwrap();

        assert!(rendered.contains("temtop_collect_success"));
        assert!(rendered.contains("temtop_sensor_pm25_ug_m3"));
        assert!(rendered.contains("temtop_sensor_history_co2_ppm"));
        assert!(rendered.contains("sensor_name=\"living_room\""));
        assert!(rendered.contains("phase=\"history\""));
    }
}
