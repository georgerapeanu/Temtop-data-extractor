use anyhow::{anyhow, bail, Context, Result};
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    ValueNotification, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio::time::{sleep, timeout};

use crate::protocol::{response_matches_guid, verify_response};
use crate::sensor::{detect_profile, profile_by_id, SensorProfile};

#[derive(Clone, Debug, serde::Serialize)]
pub struct DiscoveredDevice {
    pub address: String,
    pub name: Option<String>,
    pub guid: Option<String>,
    pub sensor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TargetFilter {
    pub address: Option<String>,
    pub guid: Option<String>,
    pub sensor: Option<String>,
    pub scan_timeout_secs: u64,
    pub require_inferred_guid: bool,
}

pub async fn first_adapter() -> Result<Adapter> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapters found"))
}

pub async fn scan_devices(adapter: &Adapter, seconds: u64) -> Result<Vec<DiscoveredDevice>> {
    adapter.start_scan(ScanFilter::default()).await?;
    sleep(Duration::from_secs(seconds)).await;
    let peripherals = adapter.peripherals().await?;
    let mut devices = Vec::new();
    for peripheral in peripherals {
        let props = peripheral.properties().await?;
        let Some(props) = props else { continue };
        let profile = detect_profile(props.local_name.as_deref());
        let guid = props
            .local_name
            .as_deref()
            .and_then(|name| profile.and_then(|p| p.infer_guid(name)));
        devices.push(DiscoveredDevice {
            address: peripheral.address().to_string(),
            name: props.local_name,
            guid,
            sensor: profile.map(|p| p.id().to_string()),
        });
    }
    let _ = adapter.stop_scan().await;
    devices.sort_by(|a, b| a.address.cmp(&b.address));
    Ok(devices)
}

pub async fn resolve_device(
    adapter: &Adapter,
    target: &TargetFilter,
) -> Result<(Peripheral, DiscoveredDevice, &'static dyn SensorProfile)> {
    let deadline = Instant::now() + Duration::from_secs(target.scan_timeout_secs);
    let requested_profile = target.sensor.as_deref().and_then(profile_by_id);
    adapter.start_scan(ScanFilter::default()).await?;

    loop {
        let peripherals = adapter.peripherals().await?;
        for peripheral in peripherals {
            let props = peripheral.properties().await?;
            let Some(props) = props else { continue };

            let address = peripheral.address().to_string().to_ascii_uppercase();
            let name = props.local_name.clone();
            let detected = detect_profile(name.as_deref());
            let profile = detected.or(requested_profile);
            let guid = name
                .as_deref()
                .and_then(|n| detected.and_then(|profile| profile.infer_guid(n)));

            let sensor_ok = match (&target.sensor, profile) {
                (Some(sensor), Some(profile)) => sensor == profile.id(),
                (Some(_), None) => false,
                (None, Some(_)) => true,
                (None, None) => false,
            };
            let address_ok = target
                .address
                .as_ref()
                .map(|wanted| address == wanted.to_ascii_uppercase())
                .unwrap_or(true);
            let guid_ok = target
                .guid
                .as_ref()
                .map(|wanted| guid.as_deref() == Some(wanted.as_str()))
                .unwrap_or(true);
            let inferred_guid_ok =
                !target.require_inferred_guid || target.guid.is_some() || guid.is_some();

            if sensor_ok && address_ok && guid_ok && inferred_guid_ok {
                let _ = adapter.stop_scan().await;
                let profile =
                    profile.ok_or_else(|| anyhow!("sensor profile could not be resolved"))?;
                return Ok((
                    peripheral,
                    DiscoveredDevice {
                        address: address.clone(),
                        name,
                        guid,
                        sensor: Some(profile.id().to_string()),
                    },
                    profile,
                ));
            }
        }

        if Instant::now() >= deadline {
            let _ = adapter.stop_scan().await;
            bail!("failed to resolve target device by scan");
        }
        sleep(Duration::from_millis(300)).await;
    }
}

pub async fn connect_and_find_chars(
    peripheral: &Peripheral,
    profile: &'static dyn SensorProfile,
) -> Result<(Characteristic, Characteristic)> {
    peripheral.connect().await.context("failed to connect")?;
    peripheral
        .discover_services()
        .await
        .context("failed to discover services")?;
    disable_all_notifications(peripheral).await;
    let chars = peripheral.characteristics();
    let write_char = chars
        .iter()
        .find(|c| c.uuid == profile.write_uuid())
        .cloned()
        .ok_or_else(|| anyhow!("write characteristic not found"))?;
    let notify_char = chars
        .iter()
        .find(|c| c.uuid == profile.notify_uuid())
        .cloned()
        .ok_or_else(|| anyhow!("notify characteristic not found"))?;
    Ok((write_char, notify_char))
}

pub async fn disable_all_notifications(peripheral: &Peripheral) {
    for characteristic in peripheral.characteristics() {
        if characteristic.properties.contains(CharPropFlags::NOTIFY)
            || characteristic.properties.contains(CharPropFlags::INDICATE)
        {
            let _ = peripheral.unsubscribe(&characteristic).await;
        }
    }
}

pub async fn wait_for_command(
    notifications: &mut (impl futures::Stream<Item = ValueNotification> + Unpin),
    cmd: u8,
    expected_guid: &str,
    timeout_secs: u64,
    verbose: bool,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut seen_other_cmds = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout_error(cmd, &seen_other_cmds));
        }
        let next = timeout(remaining, notifications.next()).await;
        let next = match next {
            Ok(next) => next,
            Err(_) => return Err(timeout_error(cmd, &seen_other_cmds)),
        };
        let notification = next.ok_or_else(|| anyhow!("notification stream ended"))?;
        let frame = notification.value;
        if verbose {
            println!("notify <- {}", format_frame_hex(&frame));
        }
        if response_matches(&frame, cmd, expected_guid)? {
            return Ok(frame);
        }
        if verify_response(&frame) {
            if let Some(seen_cmd) = frame.get(3).copied() {
                if seen_cmd != cmd && !seen_other_cmds.contains(&seen_cmd) {
                    seen_other_cmds.push(seen_cmd);
                }
            }
        }
    }
}

fn timeout_error(cmd: u8, seen_other_cmds: &[u8]) -> anyhow::Error {
    if seen_other_cmds.is_empty() {
        return anyhow!("timeout waiting for response 0x{cmd:02x}");
    }
    anyhow!(
        "timeout waiting for response 0x{cmd:02x}; saw other commands: {}",
        seen_other_cmds
            .iter()
            .map(|seen| format!("0x{seen:02x}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub async fn drain_notifications(
    notifications: &mut (impl futures::Stream<Item = ValueNotification> + Unpin),
    quiet_ms: u64,
    verbose: bool,
) -> Result<usize> {
    let mut drained = 0usize;
    loop {
        match timeout(Duration::from_millis(quiet_ms), notifications.next()).await {
            Ok(Some(notification)) => {
                drained += 1;
                if verbose {
                    println!("drain  <- {}", format_frame_hex(&notification.value));
                }
            }
            Ok(None) => return Ok(drained),
            Err(_) => return Ok(drained),
        }
    }
}

pub async fn request_once(
    peripheral: &Peripheral,
    write_char: &Characteristic,
    notify_char: &Characteristic,
    frame: &[u8],
    expected_cmd: u8,
    expected_guid: &str,
    timeout_secs: u64,
    verbose: bool,
) -> Result<Vec<u8>> {
    let mut notifications = peripheral.notifications().await?;
    peripheral.subscribe(notify_char).await?;
    if verbose {
        println!("write  -> {}", format_frame_hex(frame));
    }
    peripheral
        .write(write_char, frame, WriteType::WithoutResponse)
        .await?;
    let response = wait_for_command(
        &mut notifications,
        expected_cmd,
        expected_guid,
        timeout_secs,
        verbose,
    )
    .await?;
    let _ = peripheral.unsubscribe(notify_char).await;
    Ok(response)
}

pub fn format_frame_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn response_matches(frame: &[u8], expected_cmd: u8, expected_guid: &str) -> Result<bool> {
    Ok(verify_response(frame)
        && frame.get(3).copied() == Some(expected_cmd)
        && response_matches_guid(frame, expected_guid)?)
}

#[cfg(test)]
mod tests {
    use super::{format_frame_hex, response_matches};

    #[test]
    fn formats_hex_frames() {
        assert_eq!(format_frame_hex(&[0x5a, 0xa5, 0x0c]), "5a a5 0c");
    }

    #[test]
    fn accepts_matching_frame() {
        let frame = [
            0xd5, 0xc8, 0x12, 0x87, 0x5a, 0x0f, 0x57, 0x61, 0x2e, 0x38, 0x49, 0x34, 0x44, 0x55,
            0x01, 0x00, 0x02, 0x00, 0x12, 0x0b, 0x16, 0x6c,
        ];
        assert!(response_matches(&frame, 0x87, "90158797465673526885").unwrap());
    }

    #[test]
    fn rejects_mismatched_guid_or_command() {
        let frame = [
            0xd5, 0xc8, 0x12, 0x87, 0x5a, 0x0f, 0x57, 0x61, 0x2e, 0x38, 0x49, 0x34, 0x44, 0x55,
            0x01, 0x00, 0x02, 0x00, 0x12, 0x0b, 0x16, 0x6c,
        ];
        assert!(!response_matches(&frame, 0x88, "90158797465673526885").unwrap());
        assert!(!response_matches(&frame, 0x87, "00112233445566778899").unwrap());
    }
}
