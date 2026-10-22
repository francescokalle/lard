//! NetworkManager state.
//!
//! Primary path: native D-Bus (zbus) property streams, so the shell sleeps until
//! NM emits a change. If the system bus or NM is unavailable we fall back to the
//! previous `nmcli` poll (15 s), keeping the suite usable in minimal systems.

use crate::ServiceEvent;
use calloop::channel::Sender;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_wireless_enabled(&self, value: bool) -> zbus::Result<()>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NetworkState {
    Connected {
        ssid: Option<String>,
    },
    Connecting,
    Disconnected,
    #[default]
    Unavailable,
}

impl NetworkState {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Connected { ssid: Some(ssid) } if !ssid.is_empty() => ssid.clone(),
            Self::Connected { .. } => "Online".into(),
            Self::Connecting => "Connecting".into(),
            Self::Disconnected => "Offline".into(),
            Self::Unavailable => "No NM".into(),
        }
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

pub fn spawn(ui: Sender<ServiceEvent>) {
    // Automatic scan: the access point list must not be frozen at the moment
    // the panel was opened. The scanner only publishes a new list when it
    // actually changed, so the bar/panel is not repainted for nothing.
    let scan_ui = ui.clone();
    std::thread::Builder::new()
        .name("wifi-scan".into())
        .spawn(move || auto_scan(scan_ui))
        .expect("failed to spawn wifi scan thread");
    std::thread::Builder::new()
        .name("network".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            if let Err(error) = runtime.block_on(native_loop(ui.clone())) {
                tracing::debug!(%error, "NetworkManager D-Bus unavailable; falling back to nmcli");
                poll_loop(ui);
            }
        })
        .expect("failed to spawn network thread");
}

async fn native_loop(ui: Sender<ServiceEvent>) -> anyhow::Result<()> {
    let connection = zbus::Connection::system().await?;
    let proxy = NetworkManagerProxy::new(&connection).await?;
    let mut states = proxy.receive_state_changed().await;
    let mut wireless = proxy.receive_wireless_enabled_changed().await;
    let mut last_state = None;
    let mut last_wireless = None;

    emit(&ui, &proxy, &mut last_state, &mut last_wireless).await?;

    loop {
        tokio::select! {
            item = states.next() => {
                if item.is_none() {
                    anyhow::bail!("NetworkManager State stream ended");
                }
                emit(&ui, &proxy, &mut last_state, &mut last_wireless).await?;
            }
            item = wireless.next() => {
                if item.is_none() {
                    anyhow::bail!("NetworkManager WirelessEnabled stream ended");
                }
                emit(&ui, &proxy, &mut last_state, &mut last_wireless).await?;
            }
        }
    }
}

async fn emit(
    ui: &Sender<ServiceEvent>,
    proxy: &NetworkManagerProxy<'_>,
    last_state: &mut Option<NetworkState>,
    last_wireless: &mut Option<bool>,
) -> anyhow::Result<()> {
    let state = proxy.state().await?;
    let wireless_enabled = proxy.wireless_enabled().await?;
    let connected = matches!(state, 50 | 60 | 70);
    let next = map_state(state, connected.then(active_wifi_ssid).flatten());

    if last_state.as_ref() != Some(&next) {
        *last_state = Some(next.clone());
        let _ = ui.send(ServiceEvent::Network(next));
    }
    if *last_wireless != Some(wireless_enabled) {
        *last_wireless = Some(wireless_enabled);
        let _ = ui.send(ServiceEvent::WifiEnabled(wireless_enabled));
    }
    Ok(())
}

fn map_state(state: u32, ssid: Option<String>) -> NetworkState {
    match state {
        50 | 60 | 70 => NetworkState::Connected { ssid },
        40 => NetworkState::Connecting,
        20 | 30 => NetworkState::Disconnected,
        _ => NetworkState::Unavailable,
    }
}

/// How often the background scanner refreshes the access point list.
const AUTO_SCAN_INTERVAL: Duration = Duration::from_secs(20);

/// Keep the visible networks fresh without the user pressing "Scansiona".
///
/// `nmcli device wifi rescan` is rate-limited by NetworkManager/drivers, so a
/// 20 s cadence is safe. The list is only published when it differs from the
/// previous one: a quiet network does not cause repaints.
fn auto_scan(ui: Sender<ServiceEvent>) {
    let mut last: Vec<WifiNetwork> = Vec::new();
    // Let the compositor finish starting before the first scan.
    std::thread::sleep(Duration::from_secs(4));
    loop {
        if wifi_radio_enabled() {
            let _ = Command::new("nmcli")
                .args(["device", "wifi", "rescan"])
                .env("LC_ALL", "C")
                .output();
            std::thread::sleep(Duration::from_millis(1200));
            match read_networks() {
                Ok(networks) => {
                    if networks != last {
                        last = networks.clone();
                        if ui.send(ServiceEvent::WifiNetworks(networks)).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => tracing::debug!(%error, "wifi auto scan failed"),
            }
        } else {
            last.clear();
        }
        std::thread::sleep(AUTO_SCAN_INTERVAL);
    }
}

fn wifi_radio_enabled() -> bool {
    nmcli(&["-t", "-f", "WIFI", "general"]).is_some_and(|state| state == "enabled")
}

fn poll_loop(ui: Sender<ServiceEvent>) {
    let mut last = None;
    loop {
        let state = read_state();
        if last.as_ref() != Some(&state) {
            last = Some(state.clone());
            if ui.send(ServiceEvent::Network(state)).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(15));
    }
}

fn nmcli(args: &[&str]) -> Option<String> {
    let out = Command::new("nmcli")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[must_use]
pub fn read_state() -> NetworkState {
    let Some(state) = nmcli(&["-t", "-f", "STATE", "general"]) else {
        return NetworkState::Unavailable;
    };
    match state.as_str() {
        "connected" | "connected (site only)" | "connected (local only)" => {
            NetworkState::Connected {
                ssid: active_wifi_ssid(),
            }
        }
        "connecting" | "connecting (prepare)" | "connecting (configuring)" => {
            NetworkState::Connecting
        }
        "disconnected" | "deactivating" | "sleeping" => NetworkState::Disconnected,
        _ => NetworkState::Disconnected,
    }
}

fn active_wifi_ssid() -> Option<String> {
    let list = nmcli(&["-t", "-f", "ACTIVE,SSID", "device", "wifi"])?;
    list.lines().find_map(|line| {
        let (active, ssid) = line.split_once(':')?;
        (active == "yes" && !ssid.is_empty()).then(|| ssid.to_string())
    })
}

pub fn set_wifi(enabled: bool) {
    // Prefer the native property (the event stream updates the UI); fall back
    // to nmcli if polkit/D-Bus is not available. This runs on the action thread.
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let native_ok = runtime.block_on(async move {
        let Ok(connection) = zbus::Connection::system().await else {
            return false;
        };
        let Ok(proxy) = NetworkManagerProxy::new(&connection).await else {
            return false;
        };
        proxy.set_wireless_enabled(enabled).await.is_ok()
    });
    if !native_ok {
        let _ = Command::new("nmcli")
            .args(["radio", "wifi", if enabled { "on" } else { "off" }])
            .status();
    }
}

/// One visible Wi-Fi access point, already de-duplicated by SSID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8,
    /// WPA/WPA2/WPA3/WEP/enterprise: a password (or more) is required.
    pub secured: bool,
    /// 802.1X (WPA-Enterprise): the login needs a user name and a password,
    /// not a pre-shared key.
    pub enterprise: bool,
    /// A NetworkManager profile with this SSID already exists.
    pub saved: bool,
    pub active: bool,
}

/// Ask NetworkManager for a fresh scan, then publish the list. This runs on
/// an action thread; the UI thread never shells out.
pub fn scan(ui: &Sender<ServiceEvent>) {
    // `rescan` is best effort: it can be rate-limited by the driver. Even if
    // it fails, NetworkManager's cached list is still useful.
    let _ = Command::new("nmcli")
        .args(["device", "wifi", "rescan"])
        .env("LC_ALL", "C")
        .output();
    std::thread::sleep(Duration::from_millis(1100));
    refresh(ui);
}

/// Publish the current list without forcing a radio scan.
pub fn refresh(ui: &Sender<ServiceEvent>) {
    match read_networks() {
        Ok(networks) => {
            let _ = ui.send(ServiceEvent::WifiNetworks(networks));
        }
        Err(error) => {
            let _ = ui.send(ServiceEvent::WifiScanError(error));
        }
    }
}

/// Connect through nmcli. Existing profiles need no password; new secured
/// networks receive the password entered in the control center. Enterprise
/// (802.1X) networks also need the user name (`identity`). The password is
/// never logged.
pub fn connect(
    ui: &Sender<ServiceEvent>,
    ssid: String,
    identity: Option<String>,
    password: Option<String>,
) {
    let identity = identity.filter(|value| !value.trim().is_empty());
    let password = password.filter(|value| !value.is_empty());
    let (ok, message) = match (identity.as_deref(), password.as_deref()) {
        (Some(identity), Some(secret)) => enterprise_connect(&ssid, identity.trim(), secret),
        _ => psk_connect(&ssid, password.as_deref()),
    };
    let _ = ui.send(ServiceEvent::WifiConnectResult {
        ssid: ssid.clone(),
        ok,
        message,
    });
    std::thread::sleep(Duration::from_millis(500));
    refresh(ui);
}

/// WPA-PSK: let NetworkManager create/adopt the profile.
fn psk_connect(ssid: &str, password: Option<&str>) -> (bool, Option<String>) {
    let mut command = Command::new("nmcli");
    command
        .args(["--wait", "25", "device", "wifi", "connect"])
        .arg(ssid)
        .env("LC_ALL", "C");
    if let Some(password) = password.filter(|value| !value.is_empty()) {
        command.args(["password", password]);
    }
    match command.output() {
        Ok(out) if out.status.success() => (true, None),
        Ok(out) => (false, Some(command_error(&out))),
        Err(error) => (false, Some(format!("nmcli: {error}"))),
    }
}

/// 802.1X (WPA-Enterprise): `nmcli device wifi connect` only knows the PSK
/// flow, so the profile is created (or updated) explicitly with `wpa-eap`.
///
/// PEAP + MSCHAPv2 covers eduroam and most university/corporate networks;
/// some deployments only accept TTLS/PAP, so a failed attempt is retried with
/// that inner method before the error reaches the UI.
fn enterprise_connect(ssid: &str, identity: &str, password: &str) -> (bool, Option<String>) {
    match enterprise_attempt(ssid, identity, password, "peap", "mschapv2") {
        Ok(()) => (true, None),
        Err(peap_error) => match enterprise_attempt(ssid, identity, password, "ttls", "pap") {
            Ok(()) => (true, None),
            Err(ttls_error) => (
                false,
                Some(if ttls_error.is_empty() {
                    peap_error
                } else {
                    ttls_error
                }),
            ),
        },
    }
}

fn enterprise_attempt(
    ssid: &str,
    identity: &str,
    password: &str,
    eap: &str,
    phase2: &str,
) -> Result<(), String> {
    let existing = nmcli_output(&["-t", "-f", "NAME", "connection", "show"])
        .map(|raw| raw.lines().any(|line| line.trim() == ssid))
        .unwrap_or(false);

    let mut command = Command::new("nmcli");
    if existing {
        command.args(["connection", "modify", "id", ssid]);
    } else {
        command.args([
            "connection",
            "add",
            "type",
            "wifi",
            "con-name",
            ssid,
            "ssid",
            ssid,
        ]);
    }
    command
        .args([
            "wifi-sec.key-mgmt",
            "wpa-eap",
            "802-1x.eap",
            eap,
            "802-1x.phase2-auth",
            phase2,
            "802-1x.identity",
            identity,
            "802-1x.password",
            password,
            "connection.autoconnect",
            "yes",
        ])
        .env("LC_ALL", "C");
    let out = command
        .output()
        .map_err(|error| format!("nmcli: {error}"))?;
    if !out.status.success() {
        return Err(command_error(&out));
    }

    let up = Command::new("nmcli")
        .args(["--wait", "20", "connection", "up", "id", ssid])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("nmcli: {error}"))?;
    if !up.status.success() {
        return Err(command_error(&up));
    }
    Ok(())
}

pub fn disconnect(ui: &Sender<ServiceEvent>) {
    if let Ok(raw) = nmcli_output(&["-t", "-f", "DEVICE,TYPE", "device"]) {
        for line in raw.lines() {
            let fields = split_terse(line);
            if fields.len() >= 2
                && matches!(fields[1].trim(), "wifi" | "802-11-wireless")
                && let Some(device) = fields.first()
            {
                let _ = Command::new("nmcli")
                    .args(["--wait", "10", "device", "disconnect", device.trim()])
                    .env("LC_ALL", "C")
                    .output();
                break;
            }
        }
    }
    refresh(ui);
}

fn read_networks() -> Result<Vec<WifiNetwork>, String> {
    let raw = nmcli_output(&[
        "-t",
        "-f",
        "IN-USE,SSID,SIGNAL,SECURITY",
        "device",
        "wifi",
        "list",
    ])?;
    let saved = saved_wifi_ssids();
    Ok(parse_wifi_list(&raw, &saved))
}

fn parse_wifi_list(raw: &str, saved: &HashSet<String>) -> Vec<WifiNetwork> {
    let mut networks: Vec<WifiNetwork> = Vec::new();

    for line in raw.lines() {
        let fields = split_terse(line);
        if fields.len() < 4 {
            continue;
        }
        let ssid = fields[1].trim();
        if ssid.is_empty() {
            continue;
        }
        let signal = fields[2].trim().parse::<u8>().unwrap_or(0).min(100);
        let security = fields[3].trim();
        let secured = !security.is_empty() && security != "--";
        // nmcli reports enterprise networks as "WPA2 802.1X" (or "WPA-EAP").
        let enterprise = security.to_ascii_uppercase().contains("802.1X")
            || security.to_ascii_uppercase().contains("EAP");
        let active = fields[0].trim() == "*";

        if let Some(network) = networks.iter_mut().find(|network| network.ssid == ssid) {
            network.signal = network.signal.max(signal);
            network.secured |= secured;
            network.enterprise |= enterprise;
            network.active |= active;
        } else {
            networks.push(WifiNetwork {
                ssid: ssid.to_string(),
                signal,
                secured,
                enterprise,
                saved: saved.contains(ssid),
                active,
            });
        }
    }

    networks.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| b.saved.cmp(&a.saved))
            .then_with(|| b.signal.cmp(&a.signal))
            .then_with(|| a.ssid.cmp(&b.ssid))
    });
    networks.truncate(16);
    networks
}

fn saved_wifi_ssids() -> HashSet<String> {
    let mut ssids = HashSet::new();
    let Ok(raw) = nmcli_output(&["-t", "-f", "NAME,TYPE", "connection", "show"]) else {
        return ssids;
    };
    for line in raw.lines() {
        let fields = split_terse(line);
        if fields.len() < 2 {
            continue;
        }
        let kind = fields[1].trim();
        if !matches!(kind, "wifi" | "802-11-wireless") {
            continue;
        }
        let name = fields[0].trim();
        if name.is_empty() {
            continue;
        }
        // The profile name is normally the SSID, but a renamed profile is
        // still matched through its actual wireless SSID property.
        ssids.insert(name.to_string());
        if let Ok(detail) = nmcli_output(&[
            "-t",
            "-f",
            "802-11-wireless.ssid",
            "connection",
            "show",
            "id",
            name,
        ]) {
            for line in detail.lines() {
                let fields = split_terse(line);
                if fields.len() >= 2 && !fields[1].trim().is_empty() {
                    ssids.insert(fields[1].trim().to_string());
                }
            }
        }
    }
    ssids
}

fn nmcli_output(args: &[&str]) -> Result<String, String> {
    let out = Command::new("nmcli")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("nmcli: {error}"))?;
    if !out.status.success() {
        return Err(command_error(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn command_error(out: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("operazione non riuscita");
    let line = line.strip_prefix("Error: ").unwrap_or(line);
    let mut text: String = line.chars().take(140).collect();
    if line.chars().count() > 140 {
        text.push('…');
    }
    text
}

/// Split `nmcli -t` output, honouring its backslash escapes for `:` and `\`.
fn split_terse(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ':' => fields.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    fields.push(current);
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse_parser_handles_escaped_separators() {
        assert_eq!(
            split_terse(r"*:My\:Network:81:WPA2"),
            vec!["*", "My:Network", "81", "WPA2"]
        );
        assert_eq!(
            split_terse(r":Office\\Lab:50:--"),
            vec!["", r"Office\Lab", "50", "--"]
        );
    }

    #[test]
    fn wifi_list_deduplicates_and_sorts_active_first() {
        let saved = HashSet::from(["Casa".to_string()]);
        let raw = "\
*:Casa:54:WPA2
 :Casa:81:WPA2
 :Caffè:42:WPA2
 :Open:33:--
 :Casa:70:WPA2
";
        let networks = parse_wifi_list(raw, &saved);
        assert_eq!(networks.len(), 3);
        assert_eq!(networks[0].ssid, "Casa");
        assert_eq!(networks[0].signal, 81);
        assert!(networks[0].active);
        assert!(networks[0].saved);
        assert!(networks[0].secured);
        assert!(!networks[0].enterprise);
        assert_eq!(networks[1].ssid, "Caffè");
        assert!(!networks[2].secured);
    }

    #[test]
    fn enterprise_networks_are_flagged() {
        let networks = parse_wifi_list(":eduroam:72:WPA2 802.1X\n:Casa:50:WPA2\n", &HashSet::new());
        assert!(
            networks
                .iter()
                .find(|n| n.ssid == "eduroam")
                .unwrap()
                .enterprise
        );
        assert!(
            !networks
                .iter()
                .find(|n| n.ssid == "Casa")
                .unwrap()
                .enterprise
        );
    }

    #[test]
    fn open_network_is_not_secured() {
        let networks = parse_wifi_list(":Free WiFi:60:--\n", &HashSet::new());
        assert_eq!(networks.len(), 1);
        assert!(!networks[0].secured);
    }
}
