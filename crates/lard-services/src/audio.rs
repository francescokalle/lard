//! Volume via PipeWire's PulseAudio compatibility layer.
//!
//! `pactl subscribe` is the event source (no polling); `wpctl`/`pactl` are used
//! for the tiny state read and for actions. Commands are fixed argv vectors:
//! nothing is ever passed through a shell.

use crate::ServiceEvent;
use calloop::channel::Sender;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VolumeState {
    percent: u8,
    muted: bool,
}

pub fn spawn(ui: Sender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || run(ui))
        .expect("failed to spawn audio thread");
}

fn run(ui: Sender<ServiceEvent>) {
    let mut last = None;
    let mut last_query = Instant::now() - Duration::from_secs(1);
    let mut suppress_until = Instant::now();
    loop {
        let child = Command::new("pactl")
            .arg("subscribe")
            .env("LC_ALL", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        let Ok(mut child) = child else {
            tracing::debug!("pactl unavailable; polling volume");
            poll(&ui, &mut last, Duration::from_secs(2));
            continue;
        };
        let Some(stdout) = child.stdout.take() else {
            continue;
        };
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(_) => {
                    let now = Instant::now();
                    // `wpctl` itself appears as a short-lived PipeWire client, so a
                    // naive "one query per pactl event" becomes a feedback loop.
                    // Rate-limit and ignore the echo of our own query.
                    if now < suppress_until
                        || now.duration_since(last_query) < Duration::from_millis(600)
                    {
                        continue;
                    }
                    // Coalesce the burst emitted by a single volume key.
                    std::thread::sleep(Duration::from_millis(40));
                    let now = Instant::now();
                    if now < suppress_until {
                        continue;
                    }
                    emit_if_changed(&ui, &mut last);
                    last_query = Instant::now();
                    suppress_until = last_query + Duration::from_millis(300);
                }
                Err(_) => break,
            }
        }
        let _ = child.wait();
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn poll(ui: &Sender<ServiceEvent>, last: &mut Option<VolumeState>, interval: Duration) {
    emit_if_changed(ui, last);
    std::thread::sleep(interval);
}

fn emit_if_changed(ui: &Sender<ServiceEvent>, last: &mut Option<VolumeState>) {
    let Some(state) = read_state() else {
        return;
    };
    if *last != Some(state) {
        *last = Some(state);
        let _ = ui.send(ServiceEvent::Volume {
            percent: state.percent,
            muted: state.muted,
        });
    }
}

fn read_state() -> Option<VolumeState> {
    if let Some(state) = read_wpctl() {
        return Some(state);
    }
    read_pactl()
}

fn read_wpctl() -> Option<VolumeState> {
    let out = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let volume = text.split("Volume:").nth(1)?.split_whitespace().next()?;
    let value: f32 = volume.parse().ok()?;
    Some(VolumeState {
        percent: (value * 100.0).round().clamp(0.0, 150.0) as u8,
        muted: text.contains("[MUTED]"),
    })
}

fn read_pactl() -> Option<VolumeState> {
    let volume_out = Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !volume_out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&volume_out.stdout);
    let percent = text
        .split_whitespace()
        .find_map(|token| token.strip_suffix('%'))
        .and_then(|v| v.parse::<u8>().ok())?;

    let mute_out = Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    let mute_text = String::from_utf8_lossy(&mute_out.stdout);
    Some(VolumeState {
        percent,
        muted: mute_text.to_ascii_lowercase().contains("yes"),
    })
}

pub fn step_volume(delta_percent: i32) {
    if delta_percent == 0 {
        return;
    }
    let arg = format!(
        "{}%{}",
        delta_percent.unsigned_abs(),
        if delta_percent > 0 { "+" } else { "-" }
    );
    let _ = Command::new("wpctl")
        .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])
        .status()
        .or_else(|_| {
            Command::new("pactl")
                .args([
                    "set-sink-volume",
                    "@DEFAULT_SINK@",
                    &format!(
                        "{}{}%",
                        if delta_percent > 0 { "+" } else { "-" },
                        delta_percent.unsigned_abs()
                    ),
                ])
                .status()
        });
}

pub fn set_volume(percent: u8) {
    let fraction = format!("{:.2}", f32::from(percent.min(100)) / 100.0);
    let _ = Command::new("wpctl")
        .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &fraction])
        .status();
}

pub fn toggle_mute() {
    let _ = Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
        .status();
}
