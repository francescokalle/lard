//! Tiny `/proc` sampler for the CPU/RAM indicators in the bar.
//!
//! One thread, two small file reads per two seconds. No sysinfo crate and no
//! per-frame work.

use crate::ServiceEvent;
use calloop::channel::Sender as CalloopSender;
use std::fs;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemStats {
    pub cpu_percent: u8,
    pub memory_percent: u8,
}

pub fn spawn(ui: CalloopSender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("system-stats".into())
        .spawn(move || run(ui))
        .expect("failed to spawn system stats thread");
}

fn run(ui: CalloopSender<ServiceEvent>) {
    let mut last = SystemStats::default();
    let mut previous = read_cpu();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let current = read_cpu();
        let cpu_percent = match (previous, current) {
            (Some((prev_total, prev_idle)), Some((total, idle))) => {
                previous = Some((total, idle));
                let total_delta = total.saturating_sub(prev_total);
                let idle_delta = idle.saturating_sub(prev_idle);
                if total_delta == 0 {
                    last.cpu_percent
                } else {
                    (((total_delta - idle_delta.min(total_delta)) * 100) / total_delta).min(100)
                        as u8
                }
            }
            (_, current) => {
                previous = current;
                last.cpu_percent
            }
        };
        let state = SystemStats {
            cpu_percent,
            memory_percent: read_memory(),
        };
        if state != last {
            last = state;
            if ui.send(ServiceEvent::SystemStats(state)).is_err() {
                return;
            }
        }
    }
}

fn read_cpu() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    let mut values = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse::<u64>().ok());
    let user = values.next()?;
    let nice = values.next()?;
    let system = values.next()?;
    let idle = values.next()?;
    let iowait = values.next().unwrap_or(0);
    let irq = values.next().unwrap_or(0);
    let softirq = values.next().unwrap_or(0);
    let steal = values.next().unwrap_or(0);
    let total = user + nice + system + idle + iowait + irq + softirq + steal;
    Some((total, idle + iowait))
}

fn read_memory() -> u8 {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    let mut total = 0u64;
    let mut available = 0u64;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total = parse_kib(value);
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available = parse_kib(value);
        }
        if total != 0 && available != 0 {
            break;
        }
    }
    if total == 0 {
        return 0;
    }
    (((total.saturating_sub(available)) * 100) / total).min(100) as u8
}

fn parse_kib(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
