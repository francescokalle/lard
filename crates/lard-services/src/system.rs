//! Tiny `/proc` sampler: CPU/RAM indicators plus the process list behind the
//! mini task manager.
//!
//! One thread, a handful of small file reads per two seconds. No `sysinfo`
//! crate and no per-frame work: the shell only reads the snapshot.

use crate::ServiceEvent;
use calloop::channel::Sender as CalloopSender;
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

/// One process, as shown in the task manager.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    /// Percentage of one core (can exceed 100 for multi-threaded work).
    pub cpu_percent: f32,
    pub memory_mb: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemStats {
    pub cpu_percent: u8,
    pub memory_percent: u8,
    pub memory_used_mb: u32,
    pub memory_total_mb: u32,
    pub load: [f32; 3],
    pub cpu_count: u32,
    pub uptime_secs: u64,
    /// Hottest processes, already sorted and trimmed.
    pub top_cpu: Vec<ProcInfo>,
    pub top_mem: Vec<ProcInfo>,
}

const TOP: usize = 5;
/// Linux reports process times in clock ticks; 100 Hz on every architecture
/// this shell runs on.
const CLK_TCK: f32 = 100.0;
/// `statm` reports pages; the page size is 4 KiB on every supported target.
const PAGE_KIB: f32 = 4.0;

pub fn spawn(ui: CalloopSender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("system-stats".into())
        .spawn(move || run(ui))
        .expect("failed to spawn system stats thread");
}

fn run(ui: CalloopSender<ServiceEvent>) {
    let mut last = SystemStats::default();
    let mut previous = read_cpu();
    let mut processes: HashMap<u32, u64> = HashMap::new();
    // Warm-up sample so the first snapshot sent to the shell already has
    // per-process deltas instead of a list of zeroes.
    read_processes(&mut processes, 0.5);
    std::thread::sleep(Duration::from_millis(400));
    let mut sampled_at = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = Instant::now();
        let elapsed = now.duration_since(sampled_at).as_secs_f32().max(0.1);
        sampled_at = now;

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
        let (memory_percent, memory_used_mb, memory_total_mb) = read_memory();
        let (top_cpu, top_mem) = read_processes(&mut processes, elapsed);
        let state = SystemStats {
            cpu_percent,
            memory_percent,
            memory_used_mb,
            memory_total_mb,
            load: read_load(),
            cpu_count: std::thread::available_parallelism()
                .map_or(1, |value| value.get() as u32),
            uptime_secs: read_uptime(),
            top_cpu,
            top_mem,
        };
        if state != last {
            if ui.send(ServiceEvent::SystemStats(state.clone())).is_err() {
                return;
            }
            last = state;
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

/// `(percent, used MiB, total MiB)`.
fn read_memory() -> (u8, u32, u32) {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return (0, 0, 0);
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
        return (0, 0, 0);
    }
    let used = total.saturating_sub(available);
    (
        ((used * 100) / total).min(100) as u8,
        (used / 1024) as u32,
        (total / 1024) as u32,
    )
}

fn read_load() -> [f32; 3] {
    let Ok(text) = fs::read_to_string("/proc/loadavg") else {
        return [0.0; 3];
    };
    let mut parts = text.split_whitespace();
    let mut load = [0.0f32; 3];
    for slot in &mut load {
        *slot = parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0);
    }
    load
}

fn read_uptime() -> u64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .next()
                .and_then(|value| value.parse::<f64>().ok())
        })
        .unwrap_or(0.0) as u64
}

/// Sample every process and return the two leaderboards.
fn read_processes(previous: &mut HashMap<u32, u64>, elapsed: f32) -> (Vec<ProcInfo>, Vec<ProcInfo>) {
    let mut current = HashMap::with_capacity(previous.len() + 16);
    let mut all = Vec::with_capacity(previous.len() + 16);
    let Ok(entries) = fs::read_dir("/proc") else {
        return (Vec::new(), Vec::new());
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // The command name is in parentheses and may contain spaces: everything
        // after the last ')' is a clean whitespace-separated list.
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let mut fields = stat[close + 1..].split_whitespace().skip(11);
        let utime: u64 = fields.next().and_then(|value| value.parse().ok()).unwrap_or(0);
        let stime: u64 = fields.next().and_then(|value| value.parse().ok()).unwrap_or(0);
        let ticks = utime + stime;
        current.insert(pid, ticks);

        let delta = ticks.saturating_sub(previous.get(&pid).copied().unwrap_or(ticks));
        let cpu_percent = (delta as f32 / (elapsed * CLK_TCK) * 100.0).max(0.0);
        let memory_mb = read_rss_mb(&entry.path()).unwrap_or(0.0);
        if cpu_percent < 0.01 && memory_mb < 0.1 {
            continue;
        }
        let name = fs::read_to_string(entry.path().join("comm"))
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|_| format!("pid {pid}"));
        all.push(ProcInfo {
            pid,
            name,
            cpu_percent,
            memory_mb,
        });
    }
    *previous = current;

    let mut by_cpu = all.clone();
    by_cpu.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    by_cpu.truncate(TOP);
    let mut by_mem = all;
    by_mem.sort_by(|a, b| {
        b.memory_mb
            .partial_cmp(&a.memory_mb)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    by_mem.truncate(TOP);
    (by_cpu, by_mem)
}

fn read_rss_mb(proc_dir: &std::path::Path) -> Option<f32> {
    let statm = fs::read_to_string(proc_dir.join("statm")).ok()?;
    let resident: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident as f32 * PAGE_KIB / 1024.0)
}

fn parse_kib(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
