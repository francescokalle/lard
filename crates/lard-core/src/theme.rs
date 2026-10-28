//! Visual tokens. All values are logical pixels / sRGB.

use crate::color::Color;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BarPosition {
    #[default]
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Radius {
    /// Small controls: buttons, pills, switches, chips.
    pub sm: f32,
    /// Cards and tiles. This is the master value of the chain.
    pub md: f32,
    /// Panels: launcher, control center, popover, settings.
    pub lg: f32,
    /// Fully rounded controls.
    pub pill: f32,
    /// Chain: when on, every radius follows `md`.
    pub linked: bool,
}

impl Radius {
    /// Radius of the small controls (buttons, pills, switches, chips).
    #[must_use]
    pub fn control(&self) -> f32 {
        if self.linked { self.md } else { self.sm }
    }

    /// Radius of the cards and tiles. Always the master value.
    #[must_use]
    pub fn card(&self) -> f32 {
        self.md
    }

    /// Radius of the panels.
    #[must_use]
    pub fn panel(&self) -> f32 {
        if self.linked { self.md } else { self.lg }
    }

    /// Radius of the fully rounded controls.
    #[must_use]
    pub fn pill(&self) -> f32 {
        if self.linked { self.md } else { self.pill }
    }
}

impl Default for Radius {
    fn default() -> Self {
        Self {
            sm: 8.0,
            md: 12.0,
            lg: 18.0,
            pill: 999.0,
            linked: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Bar / panel base color. Keep alpha < 1 to let the compositor blur show
    /// through (when `blur` is supported by niri).
    pub background: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub surface_active: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub danger: Color,
    pub warning: Color,
    pub success: Color,
    pub border: Color,
    pub shadow: Color,
    pub radius: Radius,
    pub border_width: f32,
    /// Request compositor-side background blur via `ext-background-effect-v1`.
    pub blur: bool,
    /// Request the same backdrop blur for the floating islands (bar, dock).
    pub island_blur: bool,
    /// Alpha of the frosted overlay panels (launcher, control center, settings,
    /// popover, wallpaper) when the compositor blur is on. Lower = more glass.
    pub overlay_opacity: f32,
    /// How much the backdrop blur shows through the frosted islands and dock.
    /// Lower = more translucent, higher = more solid.
    pub frost: f32,
    /// The wallpaper chosen in the carousel. It is applied again at start-up
    /// so the choice survives a reboot.
    pub wallpaper: String,
    /// Follow the values niri owns (background, accent, danger) instead of
    /// keeping a second copy of them here.
    pub follow_niri: bool,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            // Warm "Dank" palette: deep espresso surfaces with the sand-gold
            // accent used by DankMaterialShell's Matugen themes.
            background: Color::from_hex("#17130BF0").expect("valid default"),
            surface: Color::from_hex("#241F17F2").expect("valid default"),
            surface_hover: Color::from_hex("#342B1FFF").expect("valid default"),
            surface_active: Color::from_hex("#463A26FF").expect("valid default"),
            text: Color::from_hex("#EBE1D4").expect("valid default"),
            text_muted: Color::from_hex("#B7AA96").expect("valid default"),
            accent: Color::from_hex("#EBC16C").expect("valid default"),
            accent_text: Color::from_hex("#241F17").expect("valid default"),
            danger: Color::from_hex("#FF8A80").expect("valid default"),
            warning: Color::from_hex("#FFD889").expect("valid default"),
            success: Color::from_hex("#ADFF9E").expect("valid default"),
            border: Color::from_hex("#FFFFFF1A").expect("valid default"),
            shadow: Color::from_hex("#00000073").expect("valid default"),
            radius: Radius::default(),
            border_width: 1.0,
            blur: true,
            island_blur: true,
            overlay_opacity: 0.64,
            frost: 0.68,
            wallpaper: String::new(),
            follow_niri: true,
            shadow_blur: 28.0,
            shadow_offset_y: 10.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    pub sans: String,
    pub mono: String,
    /// Symbol font used for bar/dock/menu glyphs.
    pub icons: String,
    pub size_small: f32,
    pub size_body: f32,
    pub size_title: f32,
    pub size_large: f32,
    pub weight_regular: u16,
    pub weight_medium: u16,
    pub weight_semibold: u16,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            sans: "Inter".into(),
            mono: "JetBrains Mono".into(),
            icons: "Material Icons".into(),
            size_small: 11.0,
            size_body: 13.0,
            size_title: 15.0,
            size_large: 30.0,
            weight_regular: 400,
            weight_medium: 500,
            weight_semibold: 600,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    pub enabled: bool,
    pub position: BarPosition,
    pub height: f32,
    pub margin: f32,
    pub padding: f32,
    pub spacing: f32,
    pub corner_radius: f32,
    pub opacity: f32,
    pub show_workspaces: bool,
    pub show_window_title: bool,
    pub show_clock: bool,
    pub show_battery: bool,
    pub show_volume: bool,
    pub show_network: bool,
    pub show_bluetooth: bool,
    pub show_system: bool,
    pub clock_format: String,
    pub max_title_chars: usize,
    pub workspace_labels: WorkspaceLabels,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DockConfig {
    pub enabled: bool,
    pub height: f32,
    pub margin: f32,
    pub padding: f32,
    pub spacing: f32,
    pub icon_size: f32,
    /// Alpha multiplier of the dock island panel.
    pub opacity: f32,
    pub show_running: bool,
    /// Desktop-entry stems/name/icon fragments, in display order.
    pub pinned: Vec<String>,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            height: 58.0,
            margin: 14.0,
            padding: 8.0,
            spacing: 6.0,
            icon_size: 23.0,
            opacity: 0.94,
            show_running: true,
            pinned: vec![
                "zen".into(),
                "chrome".into(),
                "code".into(),
                "kitty".into(),
                "nautilus".into(),
                "spotify".into(),
                "obsidian".into(),
            ],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceLabels {
    /// Show workspace name when set, otherwise its index.
    pub use_names: bool,
    /// Hide empty workspaces (niri only reports existing workspaces anyway).
    pub hide_empty: bool,
    pub show_urgent_dot: bool,
}

impl Default for WorkspaceLabels {
    fn default() -> Self {
        Self {
            use_names: true,
            hide_empty: false,
            show_urgent_dot: true,
        }
    }
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position: BarPosition::Top,
            height: 38.0,
            margin: 6.0,
            padding: 10.0,
            spacing: 6.0,
            corner_radius: 12.0,
            opacity: 0.92,
            show_workspaces: true,
            show_window_title: true,
            show_clock: true,
            show_battery: true,
            show_volume: true,
            show_network: true,
            show_bluetooth: true,
            show_system: true,
            clock_format: "%H:%M".into(),
            max_title_chars: 72,
            workspace_labels: WorkspaceLabels::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherConfig {
    pub width: f32,
    pub max_height: f32,
    pub max_results: usize,
    pub corner_radius: f32,
    pub placeholder: String,
    /// Seconds after which an unused launcher closes itself.
    pub idle_timeout_secs: u64,
    pub show_categories: bool,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            width: 660.0,
            max_height: 480.0,
            // All the applications fit in the list: the card caps its height
            // and the list scrolls, instead of hiding the tail.
            max_results: 64,
            corner_radius: 20.0,
            placeholder: "Search applications…".into(),
            idle_timeout_secs: 25,
            show_categories: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OsdConfig {
    pub enabled: bool,
    pub width: f32,
    pub height: f32,
    pub margin_bottom: f32,
    pub duration_ms: u64,
    pub show_label: bool,
    pub show_percentage: bool,
}

impl Default for OsdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 248.0,
            height: 56.0,
            margin_bottom: 72.0,
            duration_ms: 1600,
            show_label: true,
            show_percentage: true,
        }
    }
}

/// Background of the lock screen and the greeter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LockBackground {
    /// The palette gradient (`top` / `bottom` / `accent_glow`).
    Gradient,
    /// The desktop wallpaper, heavily scaled and darkened.
    #[default]
    Wallpaper,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastPosition {
    #[default]
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub width: f32,
    pub margin: f32,
    pub gap: f32,
    pub max_visible: usize,
    pub default_timeout_ms: i32,
    pub position: ToastPosition,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 360.0,
            margin: 14.0,
            gap: 10.0,
            max_visible: 4,
            default_timeout_ms: 5000,
            position: ToastPosition::TopRight,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GreeterConfig {
    /// PAM service file, installed as `/etc/pam.d/<pam_service>`.
    pub pam_service: String,
    pub allow_empty_password: bool,
    /// Failed attempts before a cooldown is applied.
    pub attempts_before_backoff: u32,
    pub max_backoff_secs: u64,
    pub show_power_buttons: bool,
    /// Add "Suspend" between reboot and power off.
    pub show_suspend: bool,
    pub default_session: String,
    pub clock_format: String,
    pub wallpaper: GreeterWallpaper,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GreeterWallpaper {
    pub top: Color,
    pub bottom: Color,
    pub accent_glow: Color,
    /// What the lock screen shows behind the card.
    pub background: LockBackground,
    /// 0..0.95: how much the wallpaper is darkened behind the card.
    pub dim: f32,
}

impl Default for GreeterWallpaper {
    fn default() -> Self {
        Self {
            top: Color::from_hex("#141008").expect("valid default"),
            bottom: Color::from_hex("#2A2113").expect("valid default"),
            accent_glow: Color::from_hex("#8C6A2A").expect("valid default"),
            background: LockBackground::Wallpaper,
            dim: 0.45,
        }
    }
}

impl Default for GreeterConfig {
    fn default() -> Self {
        Self {
            pam_service: "lard-greeter".into(),
            allow_empty_password: false,
            attempts_before_backoff: 3,
            max_backoff_secs: 8,
            show_power_buttons: true,
            show_suspend: true,
            default_session: "niri".into(),
            clock_format: "%H:%M".into(),
            wallpaper: GreeterWallpaper::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimationConfig {
    pub panel: crate::anim::SpringConfig,
    pub launcher: crate::anim::SpringConfig,
    pub osd: crate::anim::SpringConfig,
    pub notification: crate::anim::SpringConfig,
    /// Respect users who prefer less motion: all springs snap and fades become
    /// near-instant.
    pub reduce_motion: bool,
}

impl Default for AnimationConfig {
    /// The shipped feel: one family of springs, each tuned to what it animates.
    /// Panels and popovers land decisively, the launcher glides, the OSD is
    /// immediate and notifications pop slightly.
    fn default() -> Self {
        Self {
            panel: crate::anim::SpringConfig::SMOOTH,
            launcher: crate::anim::SpringConfig::SMOOTH,
            osd: crate::anim::SpringConfig::STRICT,
            notification: crate::anim::SpringConfig::BOUNCY,
            reduce_motion: false,
        }
    }
}

/// Visual effect behind the built-in screensaver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreensaverEffect {
    /// Warm liquid blobs with glossy highlights.
    #[default]
    Liquid,
    /// Chrome-like liquid metal.
    Mercury,
    /// Iridescent fluid ribbons.
    Aurora,
    /// Legacy: stacked filaments (removed; configuration still loads).
    #[serde(alias = "ripples")]
    Waves,
    /// Legacy: the grid scene was removed (its configuration still loads).
    #[serde(alias = "prisma")]
    Grid,
    /// Legacy: the dust scene (removed; configuration still loads).
    #[serde(alias = "simmetria")]
    Dust,
    /// Volumetric clouds of gas, lit from above.
    Gas,
    // Legacy names: kept only so existing configuration files still load. They
    // are not offered in the settings and fall back to the closest family.
    Vortex,
    Storm,
    Tunnel,
}

impl ScreensaverEffect {
    pub const ALL: [Self; 4] = [Self::Liquid, Self::Mercury, Self::Aurora, Self::Gas];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Liquid => "Liquid",
            Self::Mercury => "Mercury",
            Self::Aurora => "Aurora",
            Self::Waves => "Waves",
            Self::Grid => "Grid",
            Self::Dust => "Dust",
            Self::Gas => "Gas",
            Self::Vortex => "Vortex",
            Self::Storm => "Storm",
            Self::Tunnel => "Tunnel",
        }
    }
}

/// Built-in screensaver: a full-screen fluid that appears when the session has
/// been idle for a while. It is not a lock screen, so any input dismisses it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScreensaverConfig {
    pub enabled: bool,
    /// Minutes of inactivity before the screensaver appears.
    pub idle_minutes: u32,
    pub effect: ScreensaverEffect,
    /// Pick a random scenario and composition on every activation.
    pub random: bool,
    /// Include the scenes written in `~/.config/lard/scenes/*.rhai`.
    pub script_scenes: bool,
    /// Length of the liquid flood that brings the overlay on screen.
    pub entrance_ms: u32,
    /// Flow speed multiplier.
    pub speed: f32,
    /// Glow / contrast of the effect, 0..1.
    pub intensity: f32,
    /// How strongly the frosted backdrop is darkened, 0..1.
    pub veil: f32,
    /// Bright dust drifting over every scene, 0..1.
    pub dust: f32,
    /// Show the clock and the date in the corner.
    pub show_clock: bool,
}

impl Default for ScreensaverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_minutes: 5,
            effect: ScreensaverEffect::Liquid,
            random: true,
            script_scenes: true,
            entrance_ms: 1300,
            speed: 1.0,
            intensity: 0.65,
            veil: 0.35,
            dust: 0.35,
            show_clock: true,
        }
    }
}

impl ScreensaverConfig {
    /// Idle timeout in whole seconds, as the idle-notify protocol wants it.
    #[must_use]
    pub fn idle_secs(&self) -> u32 {
        self.idle_minutes.saturating_mul(60).max(1)
    }

    /// Entrance (and exit) duration in seconds.
    #[must_use]
    pub fn entrance_secs(&self) -> f32 {
        self.entrance_ms as f32 / 1000.0
    }
}
