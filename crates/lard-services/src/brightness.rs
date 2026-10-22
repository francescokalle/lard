//! Backlight control via sysfs, with `brightnessctl` as a permission fallback.
//!
//! Reading is cheap; polling every 5 s catches changes made by hardware keys or
//! another tool. Writes are attempted directly first (udev often gives the
//! `video` group write access), then through `brightnessctl`.

use crate::ServiceEvent;
use calloop::channel::Sender;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrightnessState {
    pub present: bool,
    pub percent: u8,
}

pub fn spawn(ui: Sender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("brightness".into())
        .spawn(move || {
            let mut last = None;
            loop {
                let state = read_state();
                if last != Some(state) {
                    last = Some(state);
                    if ui.send(ServiceEvent::Brightness(state)).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        })
        .expect("failed to spawn brightness thread");
}

#[must_use]
pub fn read_state() -> BrightnessState {
    let Some((device, _max)) = find_device() else {
        return BrightnessState::default();
    };
    match read_percent(&device) {
        Some(percent) => BrightnessState {
            present: true,
            percent,
        },
        None => BrightnessState::default(),
    }
}

/// Step brightness by `delta` percentage points. Runs off the UI thread.
pub fn step(delta: i32, ui: Sender<ServiceEvent>) {
    let Some((device, max)) = find_device() else {
        tracing::debug!("no backlight device");
        return;
    };
    let current = read_percent(&device).unwrap_or(50);
    let target = (i32::from(current) + delta).clamp(1, 100) as u8;
    set_percent(&device, max, target);
    if let Some(percent) = read_percent(&device) {
        let _ = ui.send(ServiceEvent::Brightness(BrightnessState {
            present: true,
            percent,
        }));
    }
}

pub fn set(percent: u8, ui: Sender<ServiceEvent>) {
    let Some((device, max)) = find_device() else {
        tracing::debug!("no backlight device");
        return;
    };
    set_percent(&device, max, percent.clamp(1, 100));
    if let Some(percent) = read_percent(&device) {
        let _ = ui.send(ServiceEvent::Brightness(BrightnessState {
            present: true,
            percent,
        }));
    }
}

fn find_device() -> Option<(PathBuf, u32)> {
    let entries = fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("brightness").exists() && path.join("max_brightness").exists() {
            let Ok(text) = fs::read_to_string(path.join("max_brightness")) else {
                continue;
            };
            let Ok(max) = text.trim().parse::<u32>() else {
                continue;
            };
            if max > 0 {
                return Some((path, max));
            }
        }
    }
    None
}

fn read_percent(device: &PathBuf) -> Option<u8> {
    let current = fs::read_to_string(device.join("brightness"))
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    let max = fs::read_to_string(device.join("max_brightness"))
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    if max == 0 {
        return None;
    }
    Some(((current.saturating_mul(100) + max / 2) / max).min(100) as u8)
}

fn set_percent(device: &PathBuf, max: u32, percent: u8) {
    let raw = (u64::from(max) * u64::from(percent) / 100).max(1) as u32;
    if fs::write(device.join("brightness"), raw.to_string()).is_ok() {
        return;
    }
    // Some systems only expose brightness through a setuid/udev helper.
    let name = device
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let percentage = format!("{percent}%");
    let status = if name.is_empty() {
        Command::new("brightnessctl")
            .args(["set", &percentage])
            .env("LC_ALL", "C")
            .status()
    } else {
        Command::new("brightnessctl")
            .args(["-d", name, "set", &percentage])
            .env("LC_ALL", "C")
            .status()
    };
    match status {
        Ok(status) if !status.success() => {
            tracing::debug!(?status, "brightnessctl could not set backlight");
        }
        Err(error) => {
            tracing::debug!(%error, "cannot write backlight and brightnessctl is unavailable");
        }
        Ok(_) => {}
    }
}

/// Path hint for diagnostics/packaging.
#[must_use]
pub fn backlight_dir() -> PathBuf {
    PathBuf::from("/sys/class/backlight")
}
