//! Battery and AC state from sysfs.
//!
//! Polling once every 30 seconds is deliberate: sysfs has no portable event
//! API and `UPower` is a D-Bus round-trip for a handful of integers. The cost
//! is negligible and the code has no external dependency.

use crate::ServiceEvent;
use calloop::channel::Sender;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatteryStatus {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Full,
    /// Plugged in, but the charge is being held (threshold, heat).
    NotCharging,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BatteryState {
    pub present: bool,
    pub percent: u8,
    pub charging: bool,
    pub external_power: bool,
    pub status: BatteryStatus,
    /// Instantaneous flow in watts: positive into the battery, negative out.
    pub power_w: f32,
    /// Seconds until empty (0 = unknown or charging).
    pub seconds_left: u32,
    /// Seconds until full (0 = unknown or not charging).
    pub seconds_full: u32,
    pub energy_now_wh: f32,
    pub energy_full_wh: f32,
    pub energy_full_design_wh: f32,
    /// `energy_full` / `energy_full_design`, in percent.
    pub health_percent: u8,
    pub cycles: u32,
    pub voltage_v: f32,
}

impl BatteryState {
    /// Human readable status, for the battery panel.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self.status {
            BatteryStatus::Charging => "Charging",
            BatteryStatus::Discharging => "Discharging",
            BatteryStatus::Full => "Full",
            BatteryStatus::NotCharging => "Paused (external power)",
            BatteryStatus::Unknown => {
                if self.external_power {
                    "External power"
                } else {
                    "Unknown"
                }
            }
        }
    }
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
    let mut batteries = Vec::with_capacity(2);
    let mut external_power = false;
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return BatteryState::default();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ty = read_trim(&path.join("type")).unwrap_or_default();
        match ty.as_str() {
            "Battery" => batteries.push(read_battery(&path)),
            "Mains" | "USB" | "USB_C" => {
                external_power |= read_trim(&path.join("online")).as_deref() == Some("1");
            }
            _ => {}
        }
    }
    if batteries.is_empty() {
        return BatteryState {
            external_power,
            ..BatteryState::default()
        };
    }
    // Machines with two batteries (this one has BAT0 and BAT1) must be summed,
    // not sampled: the first battery found is not the whole machine.
    let mut state = BatteryState {
        present: true,
        external_power,
        ..BatteryState::default()
    };
    let mut now = 0.0;
    let mut full = 0.0;
    let mut design = 0.0;
    let mut power = 0.0;
    let mut volts = 0.0;
    let mut volt_count = 0.0f32;
    for battery in &batteries {
        now += battery.energy_now_wh;
        full += battery.energy_full_wh;
        design += battery.energy_full_design_wh;
        power += battery.power_w;
        if battery.voltage_v > 0.5 {
            volts += battery.voltage_v;
            volt_count += 1.0;
        }
        state.cycles = state.cycles.max(battery.cycles);
        state.status = worse(state.status, battery.status);
    }
    state.energy_now_wh = now;
    state.energy_full_wh = full;
    state.energy_full_design_wh = design;
    state.power_w = power;
    state.voltage_v = if volt_count > 0.0 { volts / volt_count } else { 0.0 };
    state.percent = if full > 0.1 {
        ((now / full) * 100.0).clamp(0.0, 100.0) as u8
    } else {
        batteries.iter().map(|b| b.percent).max().unwrap_or(0)
    };
    state.charging = matches!(state.status, BatteryStatus::Charging | BatteryStatus::Full);
    if full > 0.1 && design > 0.1 {
        state.health_percent = ((full / design) * 100.0).clamp(0.0, 150.0) as u8;
    }
    // Times only make sense while the pack is actually moving.
    if power > 0.05 {
        if !matches!(state.status, BatteryStatus::Discharging)
            && state.energy_full_wh > state.energy_now_wh
        {
            let missing = f64::from(state.energy_full_wh - state.energy_now_wh);
            state.seconds_full = (missing / f64::from(power) * 3600.0).clamp(0.0, 100.0 * 3600.0) as u32;
        }
        if matches!(state.status, BatteryStatus::Discharging) && state.energy_now_wh > 0.0 {
            let remaining = f64::from(state.energy_now_wh);
            state.seconds_left = (remaining / f64::from(power) * 3600.0).clamp(0.0, 100.0 * 3600.0) as u32;
        }
    }
    state
}

/// Status with the most urgency wins when several batteries disagree.
const fn worse(current: BatteryStatus, candidate: BatteryStatus) -> BatteryStatus {
    const fn rank(status: BatteryStatus) -> u8 {
        match status {
            BatteryStatus::Discharging => 4,
            BatteryStatus::Charging => 3,
            BatteryStatus::NotCharging => 2,
            BatteryStatus::Full => 1,
            BatteryStatus::Unknown => 0,
        }
    }
    if rank(candidate) > rank(current) {
        candidate
    } else {
        current
    }
}

fn read_battery(path: &Path) -> BatteryState {
    let read = |name: &str| -> Option<f64> {
        read_trim(&path.join(name)).and_then(|value| value.parse().ok())
    };
    let mut battery = BatteryState::default();
    battery.percent = read_trim(&path.join("capacity"))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    battery.status = match read_trim(&path.join("status")).as_deref() {
        Some("Charging") => BatteryStatus::Charging,
        Some("Discharging") => BatteryStatus::Discharging,
        Some("Full") => BatteryStatus::Full,
        Some("Not charging") => BatteryStatus::NotCharging,
        _ => BatteryStatus::Unknown,
    };
    battery.voltage_v = read("voltage_now").map_or(0.0, |micro| (micro / 1.0e6) as f32);
    battery.cycles = read("cycle_count").map_or(0, |value| value.max(0.0) as u32);

    // Two sysfs dialects: energy_* in µWh, or charge_* in µAh (needs voltage).
    if let Some(now) = read("energy_now") {
        battery.energy_now_wh = (now / 1.0e6) as f32;
        battery.energy_full_wh = read("energy_full").map_or(0.0, |value| (value / 1.0e6) as f32);
        battery.energy_full_design_wh = read("energy_full_design")
            .map_or(0.0, |value| (value / 1.0e6) as f32);
    } else {
        let volts = f64::from(battery.voltage_v);
        battery.energy_now_wh = read("charge_now")
            .map_or(0.0, |value| (value * volts / 1.0e6) as f32);
        battery.energy_full_wh = read("charge_full")
            .map_or(0.0, |value| (value * volts / 1.0e6) as f32);
        battery.energy_full_design_wh = read("charge_full_design")
            .map_or(0.0, |value| (value * volts / 1.0e6) as f32);
    }
    battery.power_w = if let Some(power) = read("power_now") {
        (power / 1.0e6) as f32
    } else {
        read("current_now").map_or(0.0, |current| {
            (current * f64::from(battery.voltage_v) / 1.0e6) as f32
        })
    };
    battery
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

/// Called by the greeter's power menu.
#[must_use]
pub fn battery_path_hint() -> PathBuf {
    PathBuf::from("/sys/class/power_supply")
}
