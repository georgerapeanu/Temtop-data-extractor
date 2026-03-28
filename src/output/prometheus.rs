use serde_json::Value;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct Labels {
    pub mac: String,
    pub guid: String,
    pub sensor: String,
    pub sensor_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Telemetry {
    pub collector_up: bool,
    pub success_total: u64,
    pub failure_total: u64,
    pub last_success_unixtime: Option<u64>,
    pub last_failure_unixtime: Option<u64>,
    pub last_collection_duration_seconds: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub labels: Labels,
    pub sample: Option<Value>,
    pub telemetry: Telemetry,
}

pub fn render_metrics(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    let labels = format_labels(&snapshot.labels);

    metric_help(
        &mut out,
        "temtop_exporter_up",
        "Whether the exporter has a valid current sample cached.",
    );
    metric_gauge(
        &mut out,
        "temtop_exporter_up",
        &labels,
        snapshot.telemetry.collector_up as u8 as f64,
    );

    metric_help(
        &mut out,
        "temtop_exporter_collect_success_total",
        "Successful collection cycles since process start.",
    );
    metric_counter(
        &mut out,
        "temtop_exporter_collect_success_total",
        &labels,
        snapshot.telemetry.success_total as f64,
    );

    metric_help(
        &mut out,
        "temtop_exporter_collect_failure_total",
        "Failed collection cycles since process start.",
    );
    metric_counter(
        &mut out,
        "temtop_exporter_collect_failure_total",
        &labels,
        snapshot.telemetry.failure_total as f64,
    );

    if let Some(value) = snapshot.telemetry.last_success_unixtime {
        metric_help(
            &mut out,
            "temtop_exporter_last_success_unixtime",
            "Unix time of the most recent successful collection.",
        );
        metric_gauge(
            &mut out,
            "temtop_exporter_last_success_unixtime",
            &labels,
            value as f64,
        );
    }

    if let Some(value) = snapshot.telemetry.last_failure_unixtime {
        metric_help(
            &mut out,
            "temtop_exporter_last_failure_unixtime",
            "Unix time of the most recent failed collection.",
        );
        metric_gauge(
            &mut out,
            "temtop_exporter_last_failure_unixtime",
            &labels,
            value as f64,
        );
    }

    if let Some(value) = snapshot.telemetry.last_collection_duration_seconds {
        metric_help(
            &mut out,
            "temtop_exporter_last_collection_duration_seconds",
            "Wall-clock duration of the most recent collection.",
        );
        metric_gauge(
            &mut out,
            "temtop_exporter_last_collection_duration_seconds",
            &labels,
            value,
        );
    }

    if let Some(sample) = &snapshot.sample {
        push_sample_gauge(
            &mut out,
            "temtop_sensor_pm25_ug_m3",
            "Current PM2.5 measurement.",
            &labels,
            sample,
            "pm25_ug_m3",
        );
        push_sample_gauge(
            &mut out,
            "temtop_sensor_temperature_celsius",
            "Current temperature measurement in Celsius.",
            &labels,
            sample,
            "temperature_c",
        );
        push_sample_gauge(
            &mut out,
            "temtop_sensor_humidity_ratio",
            "Current relative humidity measurement.",
            &labels,
            sample,
            "humidity_rh",
        );
        push_sample_gauge(
            &mut out,
            "temtop_sensor_aqi",
            "Current air quality index reported by the sensor.",
            &labels,
            sample,
            "aqi",
        );
        push_sample_gauge(
            &mut out,
            "temtop_sensor_co2_ppm",
            "Current CO2 measurement in ppm.",
            &labels,
            sample,
            "co2_ppm",
        );
        push_sample_gauge(
            &mut out,
            "temtop_sensor_tvoc_ppb",
            "Current TVOC measurement in ppb.",
            &labels,
            sample,
            "tvoc_ppb",
        );
        push_sample_gauge(
            &mut out,
            "temtop_sensor_battery_percent",
            "Current battery percentage reported by the sensor.",
            &labels,
            sample,
            "battery",
        );
    }

    out
}

fn push_sample_gauge(
    out: &mut String,
    metric: &str,
    help: &str,
    labels: &str,
    sample: &Value,
    key: &str,
) {
    let Some(value) = sample.get(key).and_then(Value::as_f64) else {
        return;
    };
    metric_help(out, metric, help);
    metric_gauge(out, metric, labels, value);
}

fn metric_help(out: &mut String, metric: &str, help: &str) {
    let _ = writeln!(out, "# HELP {metric} {help}");
    let _ = writeln!(out, "# TYPE {metric} gauge");
}

fn metric_counter(out: &mut String, metric: &str, labels: &str, value: f64) {
    let _ = writeln!(out, "{metric}{{{labels}}} {value}");
}

fn metric_gauge(out: &mut String, metric: &str, labels: &str, value: f64) {
    let _ = writeln!(out, "{metric}{{{labels}}} {value}");
}

fn format_labels(labels: &Labels) -> String {
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

fn escape_label_value(value: &str) -> String {
    value.replace('\\', r"\\").replace('"', r#"\""#)
}

#[cfg(test)]
mod tests {
    use super::{render_metrics, Labels, Snapshot, Telemetry};
    use serde_json::json;

    #[test]
    fn renders_sensor_and_exporter_metrics() {
        let metrics = render_metrics(&Snapshot {
            labels: Labels {
                mac: "AA:BB:CC:DD:EE:FF".to_string(),
                guid: "90158797465673526885".to_string(),
                sensor: "c1plus".to_string(),
                sensor_name: "living_room".to_string(),
            },
            sample: Some(json!({
                "pm25_ug_m3": 12.3,
                "temperature_c": 21.4,
                "humidity_rh": 45.1,
                "aqi": 18,
                "co2_ppm": 712,
                "tvoc_ppb": 123,
                "battery": 88
            })),
            telemetry: Telemetry {
                collector_up: true,
                success_total: 4,
                failure_total: 1,
                last_success_unixtime: Some(1_743_120_000),
                last_failure_unixtime: Some(1_743_119_900),
                last_collection_duration_seconds: Some(1.25),
            },
        });

        assert!(metrics.contains("temtop_sensor_pm25_ug_m3"));
        assert!(metrics.contains("sensor_name=\"living_room\""));
        assert!(metrics.contains("temtop_exporter_collect_success_total"));
        assert!(metrics.contains("temtop_exporter_last_collection_duration_seconds"));
    }
}
