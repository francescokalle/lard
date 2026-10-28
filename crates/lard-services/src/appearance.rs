//! Desktop color scheme, read and published.
//!
//! LARD owns the appearance: when the user picks light/dark, every other app
//! should follow, and when the desktop preference changes from outside LARD
//! should follow too. Two mechanisms cover the ecosystem:
//!
//! * `org.gnome.desktop.interface color-scheme` — the setting the XDG portal
//!   (`xdg-desktop-portal-gtk`) reads for `org.freedesktop.appearance
//!   color-scheme`, which is what GTK apps, Firefox/Zen, Chromium and Electron
//!   apps query;
//! * `gtk-application-prefer-dark-theme` in `~/.config/gtk-3.0/settings.ini`
//!   and `gtk-4.0/settings.ini`, read directly by GTK when the portal is absent.
//!
//! Reading prefers the portal through `gsettings` (cheap, no D-Bus client
//! needed) and falls back to the GTK files, then to "dark".

use crate::ServiceEvent;
use calloop::channel::Sender;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// How often the desktop preference is re-read while LARD runs.
const POLL: Duration = Duration::from_secs(5);

/// What the desktop says about the color scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    NoPreference,
    Dark,
    Light,
}

impl Scheme {
    #[must_use]
    pub fn prefers_dark(self) -> bool {
        !matches!(self, Self::Light)
    }
}

/// Read the current desktop preference.
#[must_use]
pub fn read_scheme() -> Scheme {
    if let Some(value) = gsettings_get() {
        return match value.as_str() {
            "prefer-dark" => Scheme::Dark,
            "prefer-light" => Scheme::Light,
            _ => Scheme::NoPreference,
        };
    }
    if let Some(dark) = gtk_prefers_dark() {
        return if dark { Scheme::Dark } else { Scheme::Light };
    }
    Scheme::NoPreference
}

/// `true` when the desktop wants a dark interface (default: dark).
#[must_use]
pub fn prefers_dark() -> bool {
    read_scheme().prefers_dark()
}

/// Publish the appearance so the rest of the desktop follows.
///
/// Returns `true` when at least one of the two mechanisms was written.
pub fn publish(dark: bool) -> bool {
    let mut written = false;
    let scheme = if dark { "prefer-dark" } else { "prefer-light" };
    if Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "color-scheme", scheme])
        .status()
        .is_ok_and(|status| status.success())
    {
        written = true;
    }
    for dir in ["gtk-3.0", "gtk-4.0"] {
        if write_gtk_preference(dir, dark) {
            written = true;
        }
    }
    written
}

/// Watch the desktop preference and notify the shell on change.
pub fn spawn(ui: Sender<ServiceEvent>) {
    std::thread::Builder::new()
        .name("appearance".into())
        .spawn(move || {
            let mut last = read_scheme();
            let _ = ui.send(ServiceEvent::ColorScheme(last.prefers_dark()));
            loop {
                std::thread::sleep(POLL);
                let current = read_scheme();
                if current != last {
                    last = current;
                    if ui
                        .send(ServiceEvent::ColorScheme(current.prefers_dark()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
        .expect("failed to spawn appearance thread");
}

fn gsettings_get() -> Option<String> {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().trim_matches('\'').to_string())
}

fn gtk_settings_path(dir: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join(dir).join("settings.ini"))
}

fn gtk_prefers_dark() -> Option<bool> {
    for dir in ["gtk-4.0", "gtk-3.0"] {
        let path = gtk_settings_path(dir)?;
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("gtk-application-prefer-dark-theme") {
                let value = value.trim_start_matches([' ', '=']).trim();
                return Some(matches!(value, "1" | "true" | "True"));
            }
        }
    }
    None
}

/// Set `gtk-application-prefer-dark-theme` without touching the other keys.
fn write_gtk_preference(dir: &str, dark: bool) -> bool {
    let Some(path) = gtk_settings_path(dir) else {
        return false;
    };
    let value = if dark { "1" } else { "0" };
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    if text.trim().is_empty() {
        text.push_str("[Settings]\n");
    }
    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("gtk-application-prefer-dark-theme") {
            lines.push(format!("gtk-application-prefer-dark-theme={value}"));
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        if !lines.iter().any(|line| line.trim_start().starts_with('[')) {
            lines.insert(0, "[Settings]".into());
        }
        lines.push(format!("gtk-application-prefer-dark-theme={value}"));
    }
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    let body = format!("{}\n", lines.join("\n"));
    if fs::read_to_string(&path).is_ok_and(|current| current == body) {
        return true;
    }
    fs::write(&path, body).is_ok()
}
