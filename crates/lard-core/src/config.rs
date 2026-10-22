//! Configuration loading, validation and defaults.

use crate::theme::{
    AnimationConfig, BarConfig, DockConfig, FontConfig, GreeterConfig, LauncherConfig,
    NotificationConfig, OsdConfig, Theme,
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
    fn rejects_path_traversal_in_pam_service() {
        let mut c = Config::default();
        c.greeter.pam_service = "../../etc/pam.d/su".into();
        assert!(c.validate().is_err());
    }
}
