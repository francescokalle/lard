//! Tiny control socket: lets niri keybinds drive the running shell without
//! spawning a second Wayland client.
//!
//! `$XDG_RUNTIME_DIR/lard.sock`, 0600, one newline-terminated command per
//! connection. This is intentionally not a general-purpose RPC.

use anyhow::{Context, Result};
use calloop::channel::Sender;
use lard_shell::settings::SettingsTab;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcCommand {
    Launcher,
    ControlCenter,
    /// Grouped macOS-style Control Center popover, anchored to the bar.
    Controls,
    /// Notification center (the toast stack with its actions).
    Notifications,
    Wifi,
    Bluetooth,
    PowerMenu,
    /// Settings center; the payload selects the tab to open.
    Settings(Option<SettingsTab>),
    Wallpapers,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    BrightnessUp,
    BrightnessDown,
    Reload,
    Lock,
    /// Toggle the built-in screensaver (useful from keybinds and for testing).
    Screensaver,
    /// Open / close the screensaver explicitly (scripting, tests).
    ScreensaverOn,
    ScreensaverOff,
    Quit,
}

impl IpcCommand {
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim() {
            "launcher" | "toggle-launcher" => Some(Self::Launcher),
            "control-center" | "toggle-control-center" => Some(Self::ControlCenter),
            "controls" | "control-center-popover" => Some(Self::Controls),
            "notifications" | "notification-center" => Some(Self::Notifications),
            "wifi" | "wifi-menu" => Some(Self::Wifi),
            "bluetooth" | "bluetooth-menu" => Some(Self::Bluetooth),
            "power" | "power-menu" => Some(Self::PowerMenu),
            "settings" | "settings-center" => Some(Self::Settings(None)),
            "settings-appearance" => Some(Self::Settings(Some(SettingsTab::Appearance))),
            "settings-bar" => Some(Self::Settings(Some(SettingsTab::Bar))),
            "settings-dock" => Some(Self::Settings(Some(SettingsTab::Dock))),
            "settings-effects" => Some(Self::Settings(Some(SettingsTab::Effects))),
            "settings-screen" | "settings-screensaver" => {
                Some(Self::Settings(Some(SettingsTab::Screen)))
            }
            "settings-niri" => Some(Self::Settings(Some(SettingsTab::Niri))),
            "settings-info" => Some(Self::Settings(Some(SettingsTab::Info))),
            "wallpapers" | "wallpaper" => Some(Self::Wallpapers),
            "volume-up" => Some(Self::VolumeUp),
            "volume-down" => Some(Self::VolumeDown),
            "toggle-mute" => Some(Self::ToggleMute),
            "brightness-up" => Some(Self::BrightnessUp),
            "brightness-down" => Some(Self::BrightnessDown),
            "reload" => Some(Self::Reload),
            "lock" => Some(Self::Lock),
            "screensaver" | "saver" => Some(Self::Screensaver),
            "screensaver-on" => Some(Self::ScreensaverOn),
            "screensaver-off" => Some(Self::ScreensaverOff),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

pub fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    Ok(PathBuf::from(runtime).join("lard.sock"))
}

/// Send one command to a running shell.
pub fn send(command: &str) -> Result<()> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("no running lard at {}", path.display()))?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

/// Bind the socket and forward commands into the calloop loop.
pub fn spawn(sender: Sender<IpcCommand>) -> Result<()> {
    let path = socket_path()?;
    if path.exists() {
        // A live shell owns the socket; do not steal it.
        if UnixStream::connect(&path).is_ok() {
            anyhow::bail!("another lard shell is already running");
        }
        let _ = fs::remove_file(&path);
    }

    let listener =
        UnixListener::bind(&path).with_context(|| format!("cannot bind {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    std::thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok()
                    && let Some(command) = IpcCommand::parse(&line)
                    && sender.send(command).is_err()
                {
                    return;
                }
            }
        })
        .context("cannot start IPC thread")?;
    Ok(())
}
