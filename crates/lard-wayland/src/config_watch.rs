//! Watch the effective `config.toml` and trigger the same reload path as
//! `lard ipc reload`.
//!
//! Editors commonly replace files atomically (write temp + rename), so we watch
//! the parent directory and filter by file name. The watch is best-effort: if
//! inotify is unavailable the IPC reload still works.

use crate::ipc::IpcCommand;
use calloop::channel::Sender;
use inotify::{Inotify, WatchMask};
use lard_core::Config;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn spawn(sender: Sender<IpcCommand>) {
    std::thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || run(sender))
        .expect("failed to spawn config watcher");
}

fn run(sender: Sender<IpcCommand>) {
    let mut inotify = match Inotify::init() {
        Ok(inotify) => inotify,
        Err(error) => {
            tracing::debug!(%error, "inotify unavailable; config reload remains manual");
            return;
        }
    };

    let mut parents: Vec<PathBuf> = Vec::new();
    let mut candidates = vec![
        lard_core::config::default_path(),
        PathBuf::from("/etc/lard/config.toml"),
    ];
    if std::env::var_os("NIRI_SUITE_CONFIG").is_some() {
        candidates.push(Config::effective_path());
    }
    // The niri configuration: the shell follows some of its values, so a
    // change there has to trigger the same reload.
    let niri_dir = lard_shell::niri_config::niri_config_dir();
    for dir in [niri_dir.clone(), niri_dir.join("lard")] {
        if dir.exists() {
            candidates.push(dir.join("config.kdl"));
        }
    }
    for path in candidates {
        let Some(parent) = path.parent() else {
            continue;
        };
        if parents.iter().any(|existing| existing == parent) {
            continue;
        }
        if !parent.exists() {
            continue;
        }
        let mask =
            WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::CREATE | WatchMask::DELETE;
        if inotify.watches().add(parent, mask).is_ok() {
            parents.push(parent.to_path_buf());
        }
    }
    if parents.is_empty() {
        tracing::debug!("no config directory to watch");
        return;
    }

    let mut buffer = [0u8; 4096];
    let mut last_reload = Instant::now() - Duration::from_secs(2);
    loop {
        let events = match inotify.read_events_blocking(&mut buffer) {
            Ok(events) => events,
            Err(error) => {
                tracing::debug!(%error, "config watcher stopped");
                return;
            }
        };
        let mut changed = false;
        for event in events {
            if let Some(event_name) = event.name.as_deref() {
                // The shell configuration and the niri files it follows.
                let is_shell = event_name == "config.toml";
                let is_niri = event_name.to_string_lossy().ends_with(".kdl");
                if is_shell || is_niri {
                    changed = true;
                }
            }
        }
        if !changed {
            continue;
        }
        let now = Instant::now();
        if now.duration_since(last_reload) < Duration::from_millis(350) {
            continue;
        }
        last_reload = now;
        // Coalesce the common editor sequence (truncate, write, close, rename).
        std::thread::sleep(Duration::from_millis(180));
        if sender.send(IpcCommand::Reload).is_err() {
            return;
        }
        tracing::debug!("config.toml changed; reload requested");
    }
}
