//! Material Icons codepoints and small semantic mappings.
//!
//! The font is embedded in the renderer, so these always render even on a
//! minimal system without an icon theme or a Nerd Font.

use lard_core::Color;
use lard_core::desktop::DesktopEntry;
use lard_core::theme::Theme;

pub const APPS: &str = "\u{e5c3}";
pub const SEARCH: &str = "\u{e8b6}";
pub const WIFI: &str = "\u{e63e}";
pub const WIFI_OFF: &str = "\u{e648}";
pub const BLUETOOTH: &str = "\u{e1a7}";
pub const BLUETOOTH_OFF: &str = "\u{e1a9}";
pub const VOLUME_UP: &str = "\u{e050}";
pub const VOLUME_DOWN: &str = "\u{e04d}";
pub const VOLUME_MUTE: &str = "\u{e04e}";
pub const VOLUME_OFF: &str = "\u{e04f}";
pub const BATTERY_FULL: &str = "\u{e1a4}";
pub const BATTERY_ALERT: &str = "\u{e19c}";
pub const BATTERY_CHARGING: &str = "\u{e1a3}";
pub const NOTIFICATIONS: &str = "\u{e7f4}";
pub const NOTIFICATIONS_OFF: &str = "\u{e7f6}";
pub const SETTINGS: &str = "\u{e8b8}";
pub const LOCK: &str = "\u{e897}";
pub const POWER: &str = "\u{e8ac}";
pub const BRIGHTNESS: &str = "\u{e3ab}";
pub const DND: &str = "\u{e644}";
pub const MORE: &str = "\u{e5d3}";
pub const CHEVRON_LEFT: &str = "\u{e5cb}";
pub const CHEVRON_RIGHT: &str = "\u{e5cc}";
pub const REFRESH: &str = "\u{e5d5}";
pub const CALENDAR: &str = "\u{e935}";
pub const TERMINAL: &str = "\u{eb8e}";
pub const CODE: &str = "\u{e86f}";
pub const WEB: &str = "\u{e80b}";
pub const FOLDER: &str = "\u{e2c7}";
pub const MUSIC: &str = "\u{e405}";
pub const MOVIE: &str = "\u{e02c}";
pub const CHAT: &str = "\u{e0b7}";
pub const MAIL: &str = "\u{e158}";
pub const GAME: &str = "\u{ea28}";
pub const PERSON: &str = "\u{e7fd}";
pub const LOGOUT: &str = "\u{e9ba}";
pub const RESTART: &str = "\u{f053}";
pub const NIGHT: &str = "\u{f03d}";
pub const AIRPLANE: &str = "\u{e195}";
pub const GRID: &str = "\u{e9b0}";
pub const MIC: &str = "\u{e029}";
pub const KEYBOARD: &str = "\u{e312}";
pub const SCHEDULE: &str = "\u{e8b5}";
pub const TUNE: &str = "\u{e429}";
pub const LINK: &str = "\u{e157}";
pub const CPU: &str = "\u{e9e4}";
pub const MEMORY: &str = "\u{e322}";
pub const PALETTE: &str = "\u{e40a}";
pub const DASHBOARD: &str = "\u{e871}";
pub const CLOSE: &str = "\u{e5cd}";
pub const BLUR: &str = "\u{e3a5}";
pub const INFO: &str = "\u{e88e}";
pub const DOCK: &str = "\u{e30e}";
pub const SAVE: &str = "\u{e161}";
pub const FILE: &str = "\u{e24d}";
pub const CHECK: &str = "\u{e5ca}";
pub const WALLPAPER: &str = "\u{e1bc}";
pub const SCREEN: &str = "\u{e30c}";

/// Pick a symbolic icon from a desktop entry's icon/name/categories.
#[must_use]
pub fn app_icon(entry: &DesktopEntry) -> &'static str {
    let icon = entry.icon.to_ascii_lowercase();
    let name = entry.name.to_ascii_lowercase();
    let categories = entry.categories.join(" ").to_ascii_lowercase();
    let hay = format!(
        "{icon} {name} {categories} {}",
        entry.exec.to_ascii_lowercase()
    );

    if hay.contains("terminal")
        || hay.contains("kitty")
        || hay.contains("alacritty")
        || hay.contains("foot")
    {
        TERMINAL
    } else if hay.contains("code")
        || hay.contains("editor")
        || hay.contains("obsidian")
        || hay.contains("postman")
    {
        CODE
    } else if hay.contains("browser")
        || hay.contains("chrome")
        || hay.contains("firefox")
        || hay.contains("zen")
        || hay.contains("webkit")
        || hay.contains("web")
    {
        WEB
    } else if hay.contains("folder")
        || hay.contains("file")
        || hay.contains("nautilus")
        || hay.contains("dolphin")
    {
        FOLDER
    } else if hay.contains("music") || hay.contains("spotify") || hay.contains("audio") {
        MUSIC
    } else if hay.contains("video")
        || hay.contains("movie")
        || hay.contains("mpv")
        || hay.contains("cine")
    {
        MOVIE
    } else if hay.contains("discord")
        || hay.contains("chat")
        || hay.contains("telegram")
        || hay.contains("signal")
    {
        CHAT
    } else if hay.contains("mail") || hay.contains("thunderbird") {
        MAIL
    } else if hay.contains("steam")
        || hay.contains("game")
        || hay.contains("lutris")
        || hay.contains("melon")
    {
        GAME
    } else if hay.contains("setting") || hay.contains("control") || hay.contains("tune") {
        SETTINGS
    } else {
        APPS
    }
}

/// Deterministic pastel accent per application. Keeps the dock lively without
/// decoding PNG/SVG icon themes.
#[must_use]
pub fn app_color(entry: &DesktopEntry, theme: &Theme) -> Color {
    let mut hash: u32 = 2166136261;
    for byte in entry.name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    match hash % 6 {
        0 => theme.accent,
        1 => theme.success,
        2 => theme.warning,
        3 => theme.danger,
        4 => Color::from_hex("#C6B4FF").unwrap_or(theme.accent),
        _ => Color::from_hex("#7EDCE2").unwrap_or(theme.accent),
    }
}

#[must_use]
pub fn volume_icon(percent: u8, muted: bool) -> &'static str {
    if muted || percent == 0 {
        VOLUME_OFF
    } else if percent < 35 {
        VOLUME_DOWN
    } else {
        VOLUME_UP
    }
}
