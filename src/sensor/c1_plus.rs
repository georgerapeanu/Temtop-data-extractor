use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::sensor::{CommandOutput, HistoryRecordOutput, SensorProfile};

pub struct C1PlusProfile;

pub static C1_PLUS: C1PlusProfile = C1PlusProfile;

// UUIDs and payload offsets come from the decompiled APK:
// app/com.elitech.environment.en/sources/defpackage/al0.java
// app/com.elitech.environment.en/sources/com/elitech/environment/en/device/DeviceDetailActivityC1Plus.java
//
// The BLE UUID constants are exposed directly in al0.java. The current/history field offsets are
// taken from DeviceDetailActivityC1Plus.sendRecordData(...) and the current-data parser reachable
// from onCharacteristicChanged(...).
impl C1PlusProfile {
    pub const CMD_GET_CURRENT: u8 = 0x80;
    pub const CMD_GET_PARAMS: u8 = 0x84;
    pub const CMD_GET_HISTORY_META: u8 = 0x87;
    pub const CMD_GET_HISTORY_CHUNK: u8 = 0x88;
    pub const CMD_REALTIME_NOTIFY: u8 = 0xe1;

    fn unit_string(byte: u8) -> &'static str {
        match byte {
            2 => "F",
            _ => "C",
        }
    }

    pub fn cmd_get_params(&self) -> u8 {
        Self::CMD_GET_PARAMS
    }

    pub fn cmd_get_current(&self) -> u8 {
        Self::CMD_GET_CURRENT
    }

    pub fn cmd_get_history_meta(&self) -> u8 {
        Self::CMD_GET_HISTORY_META
    }

    pub fn cmd_get_history_chunk(&self) -> u8 {
        Self::CMD_GET_HISTORY_CHUNK
    }

    pub fn cmd_realtime_notify(&self) -> u8 {
        Self::CMD_REALTIME_NOTIFY
    }

    pub fn parse_params(&self, frame: &[u8]) -> Result<Box<dyn CommandOutput>> {
        if frame.len() <= 26 {
            bail!("params frame too short");
        }
        Ok(Box::new(C1PlusParamsData {
            battery: frame[14],
            // The first 14 bytes are framing plus echoed GUID. DeviceDetailActivityC1Plus
            // reads the params payload starting at byte 14, so the field offsets below are
            // relative to the full response frame rather than a stripped payload view.
            temperature_unit: Self::unit_string(frame[15]).to_string(),
            log_interval_seconds: u16::from_be_bytes(frame[16..18].try_into().unwrap()),
            aqi_standard: frame[18],
            alarm_mode: frame[19],
            alarm_light_y: frame[20],
            alarm_light_r: frame[21],
            time_mode: frame[23],
            version_code: u16::from_be_bytes(frame[24..26].try_into().unwrap()),
            work_mode: frame[26],
        }))
    }

    pub fn parse_current(&self, frame: &[u8]) -> Result<Box<dyn CommandOutput>> {
        if frame.len() <= 35 {
            bail!("current frame too short");
        }
        // Current-data responses use the same framed packet shape as params:
        // header + command + echoed GUID, then a model-specific payload. The
        // offsets here are based on the full frame layout used by the app.
        let year = u16::from_be_bytes(frame[15..17].try_into().unwrap());
        Ok(Box::new(C1PlusCurrentData {
            timestamp: format!(
                "{year:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                frame[17], frame[18], frame[19], frame[20], frame[21]
            ),
            pm25_ug_m3: u16::from_be_bytes(frame[22..24].try_into().unwrap()) as f32 / 10.0,
            temperature_c: i16::from_be_bytes(frame[24..26].try_into().unwrap()) as f32 / 10.0,
            humidity_rh: u16::from_be_bytes(frame[26..28].try_into().unwrap()) as f32 / 10.0,
            aqi: u16::from_be_bytes(frame[28..30].try_into().unwrap()),
            co2_ppm: u16::from_be_bytes(frame[30..32].try_into().unwrap()),
            tvoc_ppb: u16::from_be_bytes(frame[32..34].try_into().unwrap()),
            battery: frame[34],
            temperature_unit: Self::unit_string(frame[35]).to_string(),
            alarm_mode: if frame.len() > 39 {
                Some(frame[39])
            } else {
                None
            },
        }))
    }

    pub fn parse_history_record(
        &self,
        record: &[u8],
        index: usize,
    ) -> Result<Box<dyn HistoryRecordOutput>> {
        if record.len() < 16 {
            bail!("record too short");
        }
        // For C1+, the app's history parser consumes year/month/day/hour/minute,
        // then temperature, humidity, and CO2 from each fixed-size record. Two
        // byte pairs inside the record are still unknown and are intentionally
        // ignored in the user-facing output.
        let year = u16::from_be_bytes(record[0..2].try_into().unwrap());
        Ok(Box::new(C1PlusHistoryRecord {
            index,
            timestamp: format!(
                "{year:04}-{:02}-{:02} {:02}:{:02}:00",
                record[2], record[3], record[4], record[5]
            ),
            temperature_c: i16::from_be_bytes(record[8..10].try_into().unwrap()) as f32 / 10.0,
            humidity_rh: u16::from_be_bytes(record[10..12].try_into().unwrap()) as f32 / 10.0,
            co2_ppm: u16::from_be_bytes(record[14..16].try_into().unwrap()),
        }))
    }
}

#[derive(Clone, Debug, Serialize)]
struct C1PlusParamsData {
    battery: u8,
    temperature_unit: String,
    log_interval_seconds: u16,
    aqi_standard: u8,
    alarm_mode: u8,
    alarm_light_y: u8,
    alarm_light_r: u8,
    time_mode: u8,
    version_code: u16,
    work_mode: u8,
}

impl CommandOutput for C1PlusParamsData {
    fn render_text(&self) -> String {
        format!("{self:#?}")
    }

    fn render_csv_header(&self) -> &'static str {
        "battery,temperature_unit,log_interval_seconds,aqi_standard,alarm_mode,alarm_light_y,alarm_light_r,time_mode,version_code,work_mode"
    }

    fn render_csv_row(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{}",
            self.battery,
            self.temperature_unit,
            self.log_interval_seconds,
            self.aqi_standard,
            self.alarm_mode,
            self.alarm_light_y,
            self.alarm_light_r,
            self.time_mode,
            self.version_code,
            self.work_mode
        )
    }

    fn render_json_value(&self) -> serde_json::Value {
        json!(self)
    }
}

#[derive(Clone, Debug, Serialize)]
struct C1PlusCurrentData {
    timestamp: String,
    pm25_ug_m3: f32,
    temperature_c: f32,
    humidity_rh: f32,
    aqi: u16,
    co2_ppm: u16,
    tvoc_ppb: u16,
    battery: u8,
    temperature_unit: String,
    alarm_mode: Option<u8>,
}

impl CommandOutput for C1PlusCurrentData {
    fn render_text(&self) -> String {
        format!("{self:#?}")
    }

    fn render_csv_header(&self) -> &'static str {
        "timestamp,pm25_ug_m3,temperature_c,humidity_rh,aqi,co2_ppm,tvoc_ppb,battery,temperature_unit,alarm_mode"
    }

    fn render_csv_row(&self) -> String {
        let alarm_mode = self
            .alarm_mode
            .map(|value| value.to_string())
            .unwrap_or_default();
        format!(
            "{},{:.1},{:.1},{:.1},{},{},{},{},{},{}",
            self.timestamp,
            self.pm25_ug_m3,
            self.temperature_c,
            self.humidity_rh,
            self.aqi,
            self.co2_ppm,
            self.tvoc_ppb,
            self.battery,
            self.temperature_unit,
            alarm_mode
        )
    }

    fn render_json_value(&self) -> serde_json::Value {
        json!(self)
    }
}

#[derive(Clone, Debug, Serialize)]
struct C1PlusHistoryRecord {
    index: usize,
    timestamp: String,
    temperature_c: f32,
    humidity_rh: f32,
    co2_ppm: u16,
}

impl HistoryRecordOutput for C1PlusHistoryRecord {
    fn render_text_row(&self) -> String {
        format!(
            "{}\t{}\t{:.1}\t{:.1}\t{}",
            self.index, self.timestamp, self.temperature_c, self.humidity_rh, self.co2_ppm
        )
    }

    fn render_csv_row(&self) -> String {
        format!(
            "{},{},{:.1},{:.1},{}",
            self.index, self.timestamp, self.temperature_c, self.humidity_rh, self.co2_ppm
        )
    }

    fn render_json_value(&self) -> serde_json::Value {
        json!(self)
    }
}

impl SensorProfile for C1PlusProfile {
    fn id(&self) -> &'static str {
        "c1plus"
    }

    fn service_uuid(&self) -> Uuid {
        // This UUID is not guessed. It comes from the C1+ app path and is
        // exposed as a constant in al0.java. Other Temtop models may use
        // different UUIDs, which is why UUID selection belongs to the profile.
        Uuid::parse_str("00010203-0405-0607-0809-0a0b0c0d1910").unwrap()
    }

    fn notify_uuid(&self) -> Uuid {
        Uuid::parse_str("00010203-0405-0607-0809-0a0b0c0d2b10").unwrap()
    }

    fn write_uuid(&self) -> Uuid {
        Uuid::parse_str("00010203-0405-0607-0809-0a0b0c0d2b11").unwrap()
    }

    fn matches_name(&self, name: &str) -> bool {
        name.starts_with("C1+_")
    }

    fn infer_guid(&self, name: &str) -> Option<String> {
        let guid = name.strip_prefix("C1+_")?;
        if guid.len() == 20 && guid.chars().all(|c| c.is_ascii_digit()) {
            Some(guid.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::C1_PLUS;
    use crate::sensor::SensorProfile;

    fn assert_json_float(json: &serde_json::Value, key: &str, expected: f64) {
        let actual = json[key].as_f64().unwrap();
        assert!(
            (actual - expected).abs() < 0.001,
            "{key}: expected {expected}, got {actual}"
        );
    }

    #[test]
    fn infers_guid_from_advertised_name() {
        assert_eq!(
            C1_PLUS.infer_guid("C1+_90158797465673526885"),
            Some("90158797465673526885".to_string())
        );
        assert_eq!(C1_PLUS.infer_guid("C1+_short"), None);
    }

    #[test]
    fn parses_params_frame() {
        let mut frame = vec![0u8; 27];
        frame[14] = 93;
        frame[15] = 2;
        frame[16] = 0x00;
        frame[17] = 0x3c;
        frame[18] = 1;
        frame[19] = 2;
        frame[20] = 3;
        frame[21] = 4;
        frame[23] = 1;
        frame[24] = 0x01;
        frame[25] = 0x02;
        frame[26] = 5;

        let params = C1_PLUS.parse_params(&frame).unwrap();
        let json = params.render_json_value();
        assert_eq!(json["battery"], 93);
        assert_eq!(json["temperature_unit"], "F");
        assert_eq!(json["log_interval_seconds"], 60);
        assert_eq!(json["aqi_standard"], 1);
        assert_eq!(json["alarm_mode"], 2);
        assert_eq!(json["alarm_light_y"], 3);
        assert_eq!(json["alarm_light_r"], 4);
        assert_eq!(json["time_mode"], 1);
        assert_eq!(json["version_code"], 0x0102);
        assert_eq!(json["work_mode"], 5);
    }

    #[test]
    fn parses_current_frame() {
        let mut frame = vec![0u8; 40];
        frame[15] = 0x07;
        frame[16] = 0xea;
        frame[17] = 3;
        frame[18] = 27;
        frame[19] = 21;
        frame[20] = 30;
        frame[21] = 45;
        frame[22] = 0x00;
        frame[23] = 0x7b;
        frame[24] = 0x00;
        frame[25] = 0xe9;
        frame[26] = 0x01;
        frame[27] = 0xc0;
        frame[28] = 0x00;
        frame[29] = 0x2a;
        frame[30] = 0x03;
        frame[31] = 0x7e;
        frame[32] = 0x01;
        frame[33] = 0xf4;
        frame[34] = 88;
        frame[35] = 0;
        frame[39] = 6;

        let current = C1_PLUS.parse_current(&frame).unwrap();
        let json = current.render_json_value();
        assert_eq!(json["timestamp"], "2026-03-27 21:30:45");
        assert_json_float(&json, "pm25_ug_m3", 12.3);
        assert_json_float(&json, "temperature_c", 23.3);
        assert_json_float(&json, "humidity_rh", 44.8);
        assert_eq!(json["aqi"], 42);
        assert_eq!(json["co2_ppm"], 894);
        assert_eq!(json["tvoc_ppb"], 500);
        assert_eq!(json["battery"], 88);
        assert_eq!(json["temperature_unit"], "C");
        assert_eq!(json["alarm_mode"], 6);
    }

    #[test]
    fn parses_history_record_from_live_capture() {
        let record = [
            0x07, 0xea, 0x03, 0x1b, 0x15, 0x1e, 0x00, 0x00, 0x00, 0xf2, 0x01, 0xb3, 0x00, 0x00,
            0x04, 0x67,
        ];

        let parsed = C1_PLUS.parse_history_record(&record, 2).unwrap();
        let json = parsed.render_json_value();
        assert_eq!(json["index"], 2);
        assert_eq!(json["timestamp"], "2026-03-27 21:30:00");
        assert_json_float(&json, "temperature_c", 24.2);
        assert_json_float(&json, "humidity_rh", 43.5);
        assert_eq!(json["co2_ppm"], 1127);
        let csv_line = parsed.render_csv_row();
        assert_eq!(csv_line, "2,2026-03-27 21:30:00,24.2,43.5,1127");
    }
}
