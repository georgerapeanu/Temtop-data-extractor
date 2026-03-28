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
pub struct HistoricalTelemetry {
    pub started_unix_ms: i64,
    pub finished_unix_ms: i64,
    pub duration_seconds: f64,
    pub success: bool,
    pub retries_total: u64,
    pub records_total: usize,
}

#[derive(Clone, Debug, Default)]
pub struct LiveTelemetry {
    pub finished_unix_ms: i64,
    pub duration_seconds: f64,
    pub success: bool,
    pub events_total: usize,
    pub errors_total: u64,
}

pub fn render_historical_metrics(
    labels: &MetricLabels,
    records: &[Value],
    telemetry: &HistoricalTelemetry,
) -> String {
    let mut out = String::new();
    let base_labels = base_labels(labels);
    let finish_ts = telemetry.finished_unix_ms;

    append_help(
        &mut out,
        "temtop_sensor_historical_success",
        "Whether the history collection run completed successfully.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_historical_success",
        &base_labels,
        bool_to_f64(telemetry.success),
        Some(finish_ts),
    );
    append_help(
        &mut out,
        "temtop_sensor_historical_started_unix_seconds",
        "Unix start time of the history collection run.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_historical_started_unix_seconds",
        &base_labels,
        telemetry.started_unix_ms as f64 / 1000.0,
        Some(finish_ts),
    );
    append_help(
        &mut out,
        "temtop_sensor_historical_duration_seconds",
        "Wall-clock duration of the history collection run.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_historical_duration_seconds",
        &base_labels,
        telemetry.duration_seconds,
        Some(finish_ts),
    );
    append_help(
        &mut out,
        "temtop_sensor_historical_retries_total",
        "Number of history chunk retries performed during the run.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_historical_retries_total",
        &base_labels,
        telemetry.retries_total as f64,
        Some(finish_ts),
    );
    append_help(
        &mut out,
        "temtop_sensor_historical_records_total",
        "Number of history records returned during the run.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_historical_records_total",
        &base_labels,
        telemetry.records_total as f64,
        Some(finish_ts),
    );

    append_help(
        &mut out,
        "temtop_sensor_historical_temperature_celsius",
        "Historical temperature readings reported by the sensor.",
    );
    append_help(
        &mut out,
        "temtop_sensor_historical_humidity_ratio",
        "Historical humidity readings reported by the sensor.",
    );
    append_help(
        &mut out,
        "temtop_sensor_historical_co2_ppm",
        "Historical CO2 readings reported by the sensor.",
    );

    for record in records {
        let Some(timestamp_ms) = record_timestamp_ms(record).or(Some(finish_ts)) else {
            continue;
        };
        append_field_metric(
            &mut out,
            "temtop_sensor_historical_temperature_celsius",
            &base_labels,
            record,
            "temperature_c",
            Some(("temperature_unit", "C")),
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_historical_humidity_ratio",
            &base_labels,
            record,
            "humidity_rh",
            None,
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_historical_co2_ppm",
            &base_labels,
            record,
            "co2_ppm",
            None,
            Some(timestamp_ms),
        );
    }

    out
}

pub fn render_live_metrics(
    labels: &MetricLabels,
    sample: Option<&Value>,
    telemetry: &LiveTelemetry,
) -> String {
    let mut out = String::new();
    let base_labels = base_labels(labels);
    let timestamp_ms = sample
        .and_then(record_timestamp_ms)
        .unwrap_or(telemetry.finished_unix_ms);

    append_help(
        &mut out,
        "temtop_sensor_live_success",
        "Whether the most recent live sample succeeded.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_live_success",
        &base_labels,
        bool_to_f64(telemetry.success),
        Some(timestamp_ms),
    );
    append_help(
        &mut out,
        "temtop_sensor_live_duration_seconds",
        "Wall-clock duration of the most recent live sample handling.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_live_duration_seconds",
        &base_labels,
        telemetry.duration_seconds,
        Some(timestamp_ms),
    );
    append_help(
        &mut out,
        "temtop_sensor_live_events_total",
        "Number of live events emitted by the process so far.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_live_events_total",
        &base_labels,
        telemetry.events_total as f64,
        Some(timestamp_ms),
    );
    append_help(
        &mut out,
        "temtop_sensor_live_errors_total",
        "Number of live sample or poll errors seen by the process so far.",
    );
    append_gauge(
        &mut out,
        "temtop_sensor_live_errors_total",
        &base_labels,
        telemetry.errors_total as f64,
        Some(timestamp_ms),
    );

    if let Some(sample) = sample {
        append_help(
            &mut out,
            "temtop_sensor_live_pm25_ug_m3",
            "Current PM2.5 reading from the sensor.",
        );
        append_help(
            &mut out,
            "temtop_sensor_live_temperature_celsius",
            "Current temperature reading from the sensor.",
        );
        append_help(
            &mut out,
            "temtop_sensor_live_humidity_ratio",
            "Current relative humidity reading from the sensor.",
        );
        append_help(
            &mut out,
            "temtop_sensor_live_aqi",
            "Current air quality index reported by the sensor.",
        );
        append_help(
            &mut out,
            "temtop_sensor_live_co2_ppm",
            "Current CO2 reading from the sensor.",
        );
        append_help(
            &mut out,
            "temtop_sensor_live_tvoc_ppb",
            "Current TVOC reading from the sensor.",
        );
        append_help(
            &mut out,
            "temtop_sensor_live_battery_percent",
            "Current battery percentage reported by the sensor.",
        );

        append_field_metric(
            &mut out,
            "temtop_sensor_live_pm25_ug_m3",
            &base_labels,
            sample,
            "pm25_ug_m3",
            None,
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_live_temperature_celsius",
            &base_labels,
            sample,
            "temperature_c",
            Some(("temperature_unit", "C")),
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_live_humidity_ratio",
            &base_labels,
            sample,
            "humidity_rh",
            None,
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_live_aqi",
            &base_labels,
            sample,
            "aqi",
            None,
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_live_co2_ppm",
            &base_labels,
            sample,
            "co2_ppm",
            None,
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_live_tvoc_ppb",
            &base_labels,
            sample,
            "tvoc_ppb",
            None,
            Some(timestamp_ms),
        );
        append_field_metric(
            &mut out,
            "temtop_sensor_live_battery_percent",
            &base_labels,
            sample,
            "battery",
            None,
            Some(timestamp_ms),
        );
    }

    out
}

pub fn unix_timestamp_ms_now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
}

fn base_labels(labels: &MetricLabels) -> String {
    [
        ("mac", labels.mac.as_str()),
        ("guid", labels.guid.as_str()),
        ("sensor", labels.sensor.as_str()),
        ("sensor_name", labels.sensor_name.as_str()),
    ]
    .into_iter()
    .map(|(key, value)| format!(r#"{key}="{}""#, escape_label_value(value)))
    .collect::<Vec<_>>()
    .join(",")
}

fn append_field_metric(
    out: &mut String,
    metric: &str,
    base_labels: &str,
    value: &Value,
    key: &str,
    extra_label: Option<(&str, &str)>,
    timestamp_ms: Option<i64>,
) {
    let Some(value) = value.get(key).and_then(Value::as_f64) else {
        return;
    };
    let labels = match extra_label {
        Some((key, value)) => format!(r#"{base_labels},{key}="{}""#, escape_label_value(value)),
        None => base_labels.to_string(),
    };
    append_gauge(out, metric, &labels, value, timestamp_ms);
}

fn append_help(out: &mut String, metric: &str, help: &str) {
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

fn escape_label_value(value: &str) -> String {
    value.replace('\\', r"\\").replace('"', r#"\""#)
}

fn record_timestamp_ms(record: &Value) -> Option<i64> {
    let timestamp = record.get("timestamp")?.as_str()?;
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    let primitive = PrimitiveDateTime::parse(timestamp, &format).ok()?;
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    Some((primitive.assume_offset(offset).unix_timestamp_nanos() / 1_000_000) as i64)
}

fn bool_to_f64(value: bool) -> f64 {
    if value {
        1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        render_historical_metrics, render_live_metrics, HistoricalTelemetry, LiveTelemetry,
        MetricLabels,
    };
    use serde_json::json;

    #[test]
    fn renders_historical_metrics() {
        let rendered = render_historical_metrics(
            &MetricLabels {
                mac: "AA:BB".to_string(),
                guid: "123".to_string(),
                sensor: "c1plus".to_string(),
                sensor_name: "office".to_string(),
            },
            &[json!({
                "timestamp": "2026-03-28 12:00:00",
                "temperature_c": 21.0,
                "humidity_rh": 40.0,
                "co2_ppm": 700
            })],
            &HistoricalTelemetry {
                started_unix_ms: 1,
                finished_unix_ms: 2,
                duration_seconds: 1.0,
                success: true,
                retries_total: 2,
                records_total: 1,
            },
        );
        assert!(rendered.contains("temtop_sensor_historical_success"));
        assert!(rendered.contains("temtop_sensor_historical_co2_ppm"));
    }

    #[test]
    fn renders_live_metrics() {
        let rendered = render_live_metrics(
            &MetricLabels {
                mac: "AA:BB".to_string(),
                guid: "123".to_string(),
                sensor: "c1plus".to_string(),
                sensor_name: "office".to_string(),
            },
            Some(&json!({
                "timestamp": "2026-03-28 12:00:00",
                "pm25_ug_m3": 12.0,
                "temperature_c": 21.0,
                "humidity_rh": 40.0,
                "aqi": 10,
                "co2_ppm": 700,
                "tvoc_ppb": 80,
                "battery": 90
            })),
            &LiveTelemetry {
                finished_unix_ms: 2,
                duration_seconds: 0.4,
                success: true,
                events_total: 1,
                errors_total: 0,
            },
        );
        assert!(rendered.contains("temtop_sensor_live_success"));
        assert!(rendered.contains("temtop_sensor_live_pm25_ug_m3"));
    }
}
