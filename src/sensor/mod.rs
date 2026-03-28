pub mod c1_plus;

use serde_json::Value;
use uuid::Uuid;

pub trait CommandOutput: Sync {
    fn render_text(&self) -> String;
    fn render_csv_header(&self) -> &'static str;
    fn render_csv_row(&self) -> String;
    fn render_json_value(&self) -> Value;
}

pub trait HistoryRecordOutput: Sync {
    fn render_text_row(&self) -> String;
    fn render_csv_row(&self) -> String;
    fn render_json_value(&self) -> Value;
}

/// Model-specific behavior for a Temtop BLE sensor family.
///
/// Shared transport logic lives outside the profile. The profile is responsible
/// for saying how a model is identified and how its payloads are decoded.
pub trait SensorProfile: Sync {
    /// Stable internal id used by the CLI, for example `c1plus`.
    fn id(&self) -> &'static str;
    /// Primary GATT service used by this model.
    fn service_uuid(&self) -> Uuid;
    /// Notification characteristic used for async replies and live data.
    fn notify_uuid(&self) -> Uuid;
    /// Write characteristic used for command requests.
    fn write_uuid(&self) -> Uuid;
    /// Returns true if an advertised local name matches this model's naming
    /// convention closely enough to select the profile during scanning.
    ///
    /// For example, the current C1+ implementation matches names beginning with
    /// `C1+_`, because the app and live captures both show advertisements of the
    /// form `C1+_<20-digit-guid>`.
    fn matches_name(&self, name: &str) -> bool;
    /// Extracts the device GUID from an advertised local name when possible.
    fn infer_guid(&self, name: &str) -> Option<String>;
}

/// All sensor profiles compiled into this binary.
pub fn all_profiles() -> [&'static dyn SensorProfile; 1] {
    [&c1_plus::C1_PLUS]
}

/// Detects a profile from an advertised local name.
pub fn detect_profile(name: Option<&str>) -> Option<&'static dyn SensorProfile> {
    let name = name?;
    all_profiles()
        .into_iter()
        .find(|profile| profile.matches_name(name))
}

/// Resolves a profile by its stable CLI id.
pub fn profile_by_id(id: &str) -> Option<&'static dyn SensorProfile> {
    all_profiles().into_iter().find(|profile| profile.id() == id)
}
