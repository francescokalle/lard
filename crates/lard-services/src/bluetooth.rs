//! Bluetooth adapter state.
//!
//! Primary path: BlueZ `org.bluez.Adapter1` property streams via zbus, so the
//! shell is event-driven. If BlueZ is not reachable we keep the old
//! `bluetoothctl show` poll (30 s) as a fallback.

use crate::ServiceEvent;
use calloop::channel::Sender;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[zbus::proxy(
    interface = "org.bluez.Adapter1",
    default_service = "org.bluez",
    default_path = "/org/bluez/hci0"
)]
trait Adapter {
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_powered(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn discovering(&self) -> zbus::Result<bool>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BluetoothState {
    pub present: bool,
    pub powered: bool,
    pub discovering: bool,
}

pub fn spawn(ui: Sender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("bluetooth".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            if let Err(error) = runtime.block_on(native_loop(ui.clone())) {
                tracing::debug!(%error, "BlueZ D-Bus unavailable; falling back to bluetoothctl");
                poll_loop(ui);
            }
        })
        .expect("failed to spawn bluetooth thread");
}

async fn native_loop(ui: Sender<ServiceEvent>) -> anyhow::Result<()> {
    let connection = zbus::Connection::system().await?;
    let paths = adapter_paths();
    if paths.is_empty() {
        anyhow::bail!("no /sys/class/bluetooth adapter");
    }

    for path in paths {
        let proxy = AdapterProxy::builder(&connection)
            .path(path)
            .build()
            .await?;
        // A path may exist in sysfs but not be exported yet; probe the property.
        if proxy.powered().await.is_err() {
            continue;
        }
        return adapter_loop(&ui, &proxy).await;
    }
    anyhow::bail!("BlueZ did not export an adapter")
}

async fn adapter_loop(ui: &Sender<ServiceEvent>, proxy: &AdapterProxy<'_>) -> anyhow::Result<()> {
    let mut powered = proxy.receive_powered_changed().await;
    let mut discovering = proxy.receive_discovering_changed().await;
    let mut last = None;

    emit(ui, proxy, &mut last).await?;
    loop {
        tokio::select! {
            item = powered.next() => {
                if item.is_none() {
                    anyhow::bail!("BlueZ Powered stream ended");
                }
                emit(ui, proxy, &mut last).await?;
            }
            item = discovering.next() => {
                if item.is_none() {
                    anyhow::bail!("BlueZ Discovering stream ended");
                }
                emit(ui, proxy, &mut last).await?;
            }
        }
    }
}

async fn emit(
    ui: &Sender<ServiceEvent>,
    proxy: &AdapterProxy<'_>,
    last: &mut Option<BluetoothState>,
) -> anyhow::Result<()> {
    let state = BluetoothState {
        present: true,
        powered: proxy.powered().await?,
        discovering: proxy.discovering().await?,
    };
    if *last != Some(state) {
        *last = Some(state);
        let _ = ui.send(ServiceEvent::Bluetooth(state));
    }
    Ok(())
}

fn adapter_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/bluetooth") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("hci") {
                paths.push(format!("/org/bluez/{name}"));
            }
        }
    }
    if paths.is_empty() {
        paths.push("/org/bluez/hci0".into());
    }
    paths
}

fn poll_loop(ui: Sender<ServiceEvent>) {
    let mut last = None;
    loop {
        let state = read_state();
        if last != Some(state) {
            last = Some(state);
            if ui.send(ServiceEvent::Bluetooth(state)).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

#[must_use]
pub fn read_state() -> BluetoothState {
    let Ok(out) = Command::new("bluetoothctl")
        .arg("show")
        .env("LC_ALL", "C")
        .output()
    else {
        return BluetoothState::default();
    };
    if !out.status.success() {
        return BluetoothState::default();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    BluetoothState {
        present: text.contains("Controller"),
        powered: text.lines().any(|line| line.trim() == "Powered: yes"),
        discovering: text.lines().any(|line| line.trim() == "Discovering: yes"),
    }
}

pub fn set_powered(enabled: bool) {
    if set_native(enabled) {
        return;
    }
    let _ = Command::new("bluetoothctl")
        .args(["power", if enabled { "on" } else { "off" }])
        .status();
}

fn set_native(enabled: bool) -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return false;
    };
    runtime.block_on(async move {
        let Ok(connection) = zbus::Connection::system().await else {
            return false;
        };
        for path in adapter_paths() {
            let Ok(proxy) = AdapterProxy::builder(&connection).path(path).build().await else {
                continue;
            };
            if proxy.set_powered(enabled).await.is_ok() {
                return true;
            }
        }
        false
    })
}

/// Path hint for diagnostics/packaging.
#[must_use]
pub fn adapter_dir() -> PathBuf {
    PathBuf::from("/sys/class/bluetooth")
}

/// A known BlueZ device, paired or merely discovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BluetoothDevice {
    pub address: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
}

/// Publish known/paired/connected devices without starting a discovery.
pub fn refresh(ui: &Sender<ServiceEvent>) {
    match read_devices() {
        Ok(devices) => {
            let _ = ui.send(ServiceEvent::BluetoothDevices(devices));
        }
        Err(error) => {
            let _ = ui.send(ServiceEvent::BluetoothScanError(error));
        }
    }
}

/// Run an 8 second discovery, then publish the refreshed device list.
pub fn scan(ui: &Sender<ServiceEvent>) {
    let _ = btctl(&["--timeout", "8", "scan", "on"]);
    refresh(ui);
}

/// Connect to a device, pairing it first when necessary. The permissive
/// agent accepts Just Works pairing; devices that require a PIN display still
/// need to be paired from another client.
pub fn connect(ui: &Sender<ServiceEvent>, address: String) {
    let mut last_error = btctl(&["--timeout", "12", "connect", &address]).err();
    let mut ok = is_connected(&address);
    if !ok {
        if let Err(error) = btctl(&[
            "--timeout",
            "12",
            "--agent",
            "NoInputNoOutput",
            "pair",
            &address,
        ]) {
            last_error = Some(error);
        }
        let _ = btctl(&["trust", &address]);
        if let Err(error) = btctl(&["--timeout", "12", "connect", &address]) {
            last_error = Some(error);
        }
        std::thread::sleep(Duration::from_millis(350));
        ok = is_connected(&address);
    }
    let message = if ok {
        None
    } else {
        Some(last_error.unwrap_or_else(|| "associazione o connessione non riuscita".into()))
    };
    let _ = ui.send(ServiceEvent::BluetoothConnectResult {
        address,
        ok,
        message,
    });
    std::thread::sleep(Duration::from_millis(250));
    refresh(ui);
}

pub fn disconnect(ui: &Sender<ServiceEvent>, address: String) {
    let error = btctl(&["--timeout", "8", "disconnect", &address]).err();
    std::thread::sleep(Duration::from_millis(250));
    let ok = !is_connected(&address);
    let message = if ok {
        None
    } else {
        error.or_else(|| Some("disconnessione non riuscita".into()))
    };
    let _ = ui.send(ServiceEvent::BluetoothConnectResult {
        address,
        ok,
        message,
    });
    std::thread::sleep(Duration::from_millis(200));
    refresh(ui);
}

fn is_connected(address: &str) -> bool {
    btctl(&["devices", "Connected"])
        .map(|text| {
            parse_device_list(&text)
                .iter()
                .any(|(candidate, _)| candidate.eq_ignore_ascii_case(address))
        })
        .unwrap_or(false)
}

fn read_devices() -> Result<Vec<BluetoothDevice>, String> {
    let all = btctl(&["devices"])?;
    let connected: HashSet<String> = btctl(&["devices", "Connected"])
        .map(|text| {
            parse_device_list(&text)
                .into_iter()
                .map(|(address, _)| address.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let paired: HashSet<String> = btctl(&["devices", "Paired"])
        .map(|text| {
            parse_device_list(&text)
                .into_iter()
                .map(|(address, _)| address.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let mut seen = HashSet::new();
    let mut devices = Vec::new();
    for (address, name) in parse_device_list(&all) {
        let key = address.to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }
        devices.push(BluetoothDevice {
            name: if name.is_empty() {
                address.clone()
            } else {
                name
            },
            paired: paired.contains(&key),
            connected: connected.contains(&key),
            address,
        });
    }

    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| b.paired.cmp(&a.paired))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    devices.truncate(20);
    Ok(devices)
}

fn parse_device_list(text: &str) -> Vec<(String, String)> {
    let mut devices = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("Device ") else {
            continue;
        };
        let (address, name) = rest.split_once(' ').unwrap_or((rest, ""));
        let address = address.trim();
        if address.is_empty() {
            continue;
        }
        devices.push((address.to_string(), name.trim().to_string()));
    }
    devices
}

fn btctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("bluetoothctl")
        .args(args)
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|error| format!("bluetoothctl: {error}"))?;
    if !out.status.success() {
        return Err(bt_error(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn bt_error(out: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stderr
        .lines()
        .chain(stdout.lines())
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("operazione non riuscita");
    let line = line.strip_prefix("Failed to ").map_or(line, |rest| rest);
    let mut text: String = line.chars().take(140).collect();
    if line.chars().count() > 140 {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_list_parses_addresses_and_names() {
        let text = "\
Device AA:BB:CC:DD:EE:01 Test Phone
Device AA:BB:CC:DD:EE:02 Test Buds
";
        let devices = parse_device_list(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].0, "AA:BB:CC:DD:EE:01");
        assert_eq!(devices[0].1, "Test Phone");
        assert_eq!(devices[1].0, "AA:BB:CC:DD:EE:02");
    }

    #[test]
    fn device_list_ignores_non_device_lines() {
        assert!(parse_device_list("Controller 00:11:22:33:44:55\n").is_empty());
    }
}
