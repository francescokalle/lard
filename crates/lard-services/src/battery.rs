//! Battery and AC state from sysfs.
//!
//! Polling once every 30 seconds is deliberate: sysfs has no portable event
//! API and `UPower` is a D-Bus round-trip for two integers. The cost is
//! negligible and the code has no external dependency.

use crate::ServiceEvent;
use calloop::channel::Sender;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatteryState {
    pub present: bool,
    pub percent: u8,
    pub charging: bool,
    pub external_power: bool,
}

pub fn spawn(ui: Sender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("battery".into())
        .spawn(move || {
            let mut last = None;
            loop {
                let state = read_state();
                if last != Some(state) {
                    last = Some(state);
                    if ui.send(ServiceEvent::Battery(state)).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(30));
            }
        })
        .expect("failed to spawn battery thread");
}

#[must_use]
pub fn read_state() -> BatteryState {
    let mut state = BatteryState::default();
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return state;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let ty = read_trim(&path.join("type")).unwrap_or_default();
        if ty == "Battery" && !state.present {
            state.present = true;
            state.percent = read_trim(&path.join("capacity"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let status = read_trim(&path.join("status")).unwrap_or_default();
            state.charging = matches!(status.as_str(), "Charging" | "Full");
        }
        if matches!(ty.as_str(), "Mains" | "USB" | "USB_C") {
            let online = read_trim(&path.join("online")).as_deref() == Some("1");
            state.external_power |= online;
        }
    }
    state
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Called by the greeter's power menu.
#[must_use]
pub fn battery_path_hint() -> PathBuf {
    PathBuf::from("/sys/class/power_supply")
}
