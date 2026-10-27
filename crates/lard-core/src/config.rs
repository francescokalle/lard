//! Configuration loading, validation and defaults.

use crate::theme::{
    AnimationConfig, BarConfig, DockConfig, FontConfig, GreeterConfig, LauncherConfig,
    NotificationConfig, OsdConfig, ScreensaverConfig, Theme,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level `config.toml`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: Theme,
    pub font: FontConfig,
    pub bar: BarConfig,
    pub dock: DockConfig,
    pub launcher: LauncherConfig,
    pub osd: OsdConfig,
    pub notifications: NotificationConfig,
    pub greeter: GreeterConfig,
    pub animation: AnimationConfig,
    pub screensaver: ScreensaverConfig,
}

/// Only the wallpaper of another user's configuration matters to the greeter.
#[derive(Default, Deserialize)]
#[serde(default)]
struct WallpaperPeek {
    theme: WallpaperPeekTheme,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WallpaperPeekTheme {
    wallpaper: String,
}

impl Config {
    /// Parse and validate a config file.
    pub fn load(path: &Path) -> Result<Self, crate::ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| crate::ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| crate::ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Persist the configuration atomically, keeping a timestamped backup of
    /// the previous file. Used by the shell settings center.
    pub fn save(&self) -> Result<(), crate::ConfigError> {
        let path = Self::effective_path();
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), crate::ConfigError> {
        let text =
            toml::to_string_pretty(self).map_err(|source| crate::ConfigError::Serialize {
                path: path.to_path_buf(),
                source,
            })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| crate::ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if path.exists() {
            let backup = path.with_extension("toml.bak");
            let _ = std::fs::copy(path, &backup);
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|source| crate::ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| crate::ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Load `$NIRI_SUITE_CONFIG`, the user config, or the system-wide config.
    /// A malformed file is always an error: silently ignoring it makes theming
    /// bugs impossible to diagnose.
    pub fn load_default() -> Result<Self, crate::ConfigError> {
        // One-shot migration from the pre-rename config location.
        if let Some(path) = migrate_legacy_config() {
            tracing::info!(path = %path.display(), "migrated the legacy config");
        }
        let path = Self::effective_path();
        if path.exists() {
            Self::load(&path)
        } else {
            Ok(Self::default())
        }
    }

    /// Wallpaper to show behind the login card.
    ///
    /// The greeter runs as its own user (`greeter` with greetd) and reads
    /// `/etc/lard/config.toml`, which normally has no wallpaper path: the
    /// desktop user chooses it in the shell. Instead of copying pictures around
    /// as root, fall back to the most recently used user configuration, so the
    /// login screen shows the same picture as the desktop.
    #[must_use]
    pub fn login_wallpaper(&self) -> Option<PathBuf> {
        let configured = PathBuf::from(self.theme.wallpaper.trim());
        if !self.theme.wallpaper.trim().is_empty() && configured.is_file() {
            return Some(configured);
        }
        let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
        for entry in std::fs::read_dir("/home").ok()?.flatten() {
            let config = entry.path().join(".config/lard/config.toml");
            let Ok(text) = std::fs::read_to_string(&config) else {
                continue;
            };
            let Ok(peek) = toml::from_str::<WallpaperPeek>(&text) else {
                continue;
            };
            let wallpaper = PathBuf::from(peek.theme.wallpaper.trim());
            if !wallpaper.is_file() {
                continue;
            }
            let modified = std::fs::metadata(&config)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if newest
                .as_ref()
                .is_none_or(|(best, _)| modified > *best)
            {
                newest = Some((modified, wallpaper));
            }
        }
        newest.map(|(_, wallpaper)| wallpaper)
    }

    /// The config file that would be loaded. Useful for the file watcher and
    /// diagnostics.
    #[must_use]
    pub fn effective_path() -> PathBuf {
        // `LARD_CONFIG` wins; the old name still works for existing setups.
        for key in ["LARD_CONFIG", "NIRI_SUITE_CONFIG"] {
            if let Some(path) = std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
            {
                return path;
            }
        }
        let user = default_path();
        if user.exists() {
            return user;
        }
        let system = PathBuf::from("/etc/lard/config.toml");
        if system.exists() {
            return system;
        }
        user
    }

    pub fn validate(&self) -> Result<(), crate::ConfigError> {
        let invalid = |msg: &str| Err(crate::ConfigError::Invalid(msg.to_string()));
        if self.bar.height < 16.0 || self.bar.height > 200.0 {
            return invalid("bar.height must be between 16 and 200");
        }
        if self.bar.margin < 0.0 || self.bar.padding < 0.0 || self.bar.spacing < 0.0 {
            return invalid("bar margins/padding/spacing must be >= 0");
        }
        if !(0.0..=1.0).contains(&self.bar.opacity) {
            return invalid("bar.opacity must be in 0..=1");
        }
        if self.dock.height < 32.0 || self.dock.height > 160.0 {
            return invalid("dock.height must be between 32 and 160");
        }
        if self.dock.margin < 0.0 || self.dock.padding < 0.0 || self.dock.spacing < 0.0 {
            return invalid("dock margins/padding/spacing must be >= 0");
        }
        if self.launcher.width < 280.0 || self.launcher.max_height < 120.0 {
            return invalid("launcher dimensions are too small");
        }
        if self.launcher.max_results == 0 {
            return invalid("launcher.max_results must be > 0");
        }
        if self.osd.width < 80.0 || self.osd.height < 24.0 {
            return invalid("osd dimensions are too small");
        }
        if self.notifications.width < 180.0 || self.notifications.max_visible == 0 {
            return invalid("notifications.width/max_visible invalid");
        }
        if self.font.size_body < 6.0 || self.font.size_large < self.font.size_body {
            return invalid("font sizes are invalid");
        }
        if !(1..=120).contains(&self.screensaver.idle_minutes) {
            return invalid("screensaver.idle_minutes must be between 1 and 120");
        }
        if !(300..=2500).contains(&self.screensaver.entrance_ms) {
            return invalid("screensaver.entrance_ms must be between 300 and 2500");
        }
        if !(0.1..=3.0).contains(&self.screensaver.speed) {
            return invalid("screensaver.speed must be between 0.1 and 3");
        }
        if !(0.0..=1.0).contains(&self.screensaver.intensity) {
            return invalid("screensaver.intensity must be in 0..=1");
        }
        if !(0.0..=0.95).contains(&self.screensaver.veil) {
            return invalid("screensaver.veil must be in 0..=0.95");
        }
        for (name, spring) in [
            ("animation.panel", self.animation.panel),
            ("animation.launcher", self.animation.launcher),
            ("animation.osd", self.animation.osd),
            ("animation.notification", self.animation.notification),
        ] {
            if spring.stiffness <= 0.0
                || spring.damping < 0.0
                || spring.mass <= 0.0
                || spring.rest_distance < 0.0
            {
                return invalid(&format!("{name} spring parameters are invalid"));
            }
        }
        if !(0.0..=0.95).contains(&self.greeter.wallpaper.dim) {
            return invalid("greeter.wallpaper.dim must be in 0..=0.95");
        }
        if !self.greeter.pam_service.is_empty() {
            let valid = self.greeter.pam_service.len() <= 64
                && self
                    .greeter
                    .pam_service
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !valid {
                return invalid("greeter.pam_service may only contain [A-Za-z0-9_-]");
            }
        }
        Ok(())
    }
}

/// Old config location, kept so an existing setup can be migrated in place.
#[must_use]
pub fn legacy_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("niri-suite/config.toml"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    Some(home.join(".config/niri-suite/config.toml"))
}

/// Copy the pre-rename config to the new location, once, so the rename does
/// not lose the user's theme and layout.
pub fn migrate_legacy_config() -> Option<PathBuf> {
    let target = default_path();
    if target.exists() {
        return None;
    }
    let legacy = legacy_path()?;
    if !legacy.is_file() {
        return None;
    }
    if let Some(parent) = target.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return None;
        }
    }
    std::fs::copy(&legacy, &target).ok()?;
    Some(target)
}

/// `$XDG_CONFIG_HOME/lard/config.toml`, or `~/.config/...`.
#[must_use]
pub fn default_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("lard/config.toml");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/lard/config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn rejects_bad_spring() {
        let mut c = Config::default();
        c.animation.panel.stiffness = 0.0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn serializes_and_reloads() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        parsed.validate().unwrap();
        assert!(text.contains("[dock]"));
        assert!(text.contains("icons = \"Material Icons\""));
    }

    #[test]
    fn screensaver_defaults_and_roundtrip() {
        let config = Config::default();
        assert!(config.screensaver.enabled);
        assert_eq!(config.screensaver.idle_secs(), 300);
        assert_eq!(config.screensaver.effect, crate::theme::ScreensaverEffect::Liquid);

        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("[screensaver]"));
        assert!(text.contains("effect = \"liquid\""));
        let parsed: Config = toml::from_str(&text).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed.screensaver.idle_minutes, 5);
    }

    #[test]
    fn screensaver_rejects_out_of_range_values() {
        let mut c = Config::default();
        c.screensaver.idle_minutes = 0;
        assert!(c.validate().is_err());
        c.screensaver.idle_minutes = 121;
        assert!(c.validate().is_err());

        let mut c = Config::default();
        c.screensaver.speed = 0.0;
        assert!(c.validate().is_err());
        c.screensaver.speed = 4.0;
        assert!(c.validate().is_err());

        let mut c = Config::default();
        c.screensaver.intensity = 1.5;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_path_traversal_in_pam_service() {
        let mut c = Config::default();
        c.greeter.pam_service = "../../etc/pam.d/su".into();
        assert!(c.validate().is_err());
    }
}
