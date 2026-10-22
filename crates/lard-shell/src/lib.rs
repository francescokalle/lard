//! Shell composition: bar, launcher, OSD, notifications and control center.
//! This crate is deliberately instantiated once per process; it contains no
//! Wayland or GPU state.

pub mod app_icons;
pub mod bar;
pub mod bar_icons;
pub mod control_center;
pub mod dock;
pub mod icons;
pub mod launcher;
pub mod niri_config;
pub mod osd;
pub mod popover;
pub mod settings;
pub mod toasts;
pub mod wallpaper;
pub mod widgets;

use app_icons::AppIcons;
use bar::{Bar, BarData, BarIsland};
use chrono::{DateTime, Local};
use control_center::{ControlCenter, ControlData, ControlPage, EmbeddedCommand};
use dock::Dock;
use lard_core::anim::Spring;
use lard_core::desktop::{DesktopEntry, scan_applications};
use lard_core::{Color, Config};
use lard_core::{Point, Rect, Size};
use lard_render::{Scene, TextAlign, TextStyle};
use lard_services::battery::BatteryState;
use lard_services::bluetooth::BluetoothState;
use lard_services::network::NetworkState;
use lard_services::niri::NiriSnapshot;
use lard_services::notifications::CloseReason;
use lard_services::power::PowerAction;
use lard_services::system::SystemStats;
use lard_services::{BluetoothDevice, ServiceEvent, WifiNetwork};
use launcher::{Launcher, LauncherKey};
use osd::Osd;
use popover::{Popover, PopoverAction, PopoverData, PopoverKind};
use settings::{SettingsAction, SettingsTab, SettingsView, Step, Toggle};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use toasts::ToastStack;
use wallpaper::{WallpaperAction, WallpaperView};
use widgets::{approx_width, truncate_to_width};

#[derive(Clone, Debug, PartialEq)]
pub enum ShellAction {
    OpenLauncher,
    ToggleControlCenter,
    OpenPowerMenu,
    OpenWifiPanel,
    OpenBluetoothPanel,
    ToggleBluetooth,
    ToggleDnd,
    FocusWorkspace(u64),
    Spawn(Vec<String>),
    VolumeStep(i32),
    SetVolume(u8),
    ToggleMute,
    BrightnessStep(i32),
    SetBrightness(u8),
    SetWifi(bool),
    SetBluetooth(bool),
    ScanWifi,
    ConnectWifi {
        ssid: String,
        password: Option<String>,
    },
    DisconnectWifi,
    RefreshBluetooth,
    ScanBluetooth,
    ConnectBluetooth {
        address: String,
    },
    DisconnectBluetooth {
        address: String,
    },
    Power(PowerAction),
    CloseNotification {
        id: u32,
        reason: CloseReason,
    },
    NotificationAction {
        id: u32,
        action: String,
    },
    Lock,
    OpenSettings,
    ReloadConfig,
    /// Reload a niri configuration file after it was edited.
    ReloadNiriConfig(std::path::PathBuf),
    /// Copy text to the Wayland clipboard (launcher calculator).
    Copy(String),
    Quit,
}

/// Overlay surfaces that request a compositor-side backdrop blur. The blur
/// region is always the panel rectangle itself (never the whole surface),
/// so the desktop behind the window stays sharp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Launcher,
    ControlCenter,
    Settings,
    Popover,
    Wallpapers,
    /// Expanded centre island with an incoming notification.
    Notification,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellKey {
    Char(String),
    Backspace,
    Delete,
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
}

pub struct Shell {
    pub config: Config,
    pub niri: NiriSnapshot,
    /// The connected outputs and the modes they support.
    pub niri_outputs: Vec<lard_services::niri::OutputView>,
    pub volume: u8,
    pub muted: bool,
    pub volume_seen: bool,
    pub brightness: u8,
    pub brightness_present: bool,
    pub brightness_seen: bool,
    pub user: String,
    pub battery: BatteryState,
    pub network: NetworkState,
    pub bluetooth: BluetoothState,
    pub system: SystemStats,
    pub dnd: bool,
    /// Width of the expanded notification island, animated like the centre
    /// island of the bar.
    notification_width: Spring,
    notification_initialized: bool,
    /// Last drawn notification island, used for the backdrop blur region.
    notification_rect: Rect,
    pub wifi_enabled: bool,
    pub wifi_networks: Vec<WifiNetwork>,
    pub wifi_scanning: bool,
    pub wifi_error: Option<String>,
    pub wifi_connecting: Option<String>,
    pub bluetooth_devices: Vec<BluetoothDevice>,
    pub bt_scanning: bool,
    pub bt_error: Option<String>,
    pub bt_connecting: Option<String>,
    /// Last radio state reported by NetworkManager/BlueZ (not the optimistic
    /// UI state), used to refresh lists exactly once on power-on.
    wifi_service_enabled: bool,
    bt_service_powered: bool,
    pub clock: String,
    pub date: String,
    apps: Vec<DesktopEntry>,
    /// Cache of the real application icons used by the launcher and the dock.
    icons: AppIcons,
    bars: HashMap<String, Bar>,
    dock: Dock,
    launcher: Launcher,
    control: ControlCenter,
    osd: Osd,
    settings: SettingsView,
    settings_backup: Option<Config>,
    popover: Popover,
    wallpaper: WallpaperView,
    toasts: ToastStack,
    actions: Vec<ShellAction>,
    dirty: bool,
    reduce_motion: bool,
}

impl Shell {
    /// Rectangle that should be blurred behind an open overlay. Returns
    /// `None` until the overlay has laid itself out.
    ///
    /// Cards that morph out of an island are blurred following the shape
    /// they currently have: the blurred region grows with the card instead of
    /// appearing as a grey rectangle in the final spot before the card is
    /// even there.
    pub fn overlay_rect(&self, kind: OverlayKind) -> Option<lard_core::Rect> {
        let rect = match kind {
            OverlayKind::Launcher => self.launcher.shape_rect(),
            OverlayKind::ControlCenter => self.control.panel_rect(),
            OverlayKind::Settings => self.settings.panel_rect(),
            OverlayKind::Popover => self.popover.shape_rect(),
            OverlayKind::Wallpapers => self.wallpaper.panel_rect(),
            OverlayKind::Notification => self.notification_rect,
        };
        (rect.w > 1.0 && rect.h > 1.0).then_some(rect)
    }

    /// Rect plus corner radius for the compositor-side backdrop blur. Using
    /// the real radius lets the compositor clip the blurred region with the
    /// same rounded corners as the panel.
    pub fn overlay_shape(&self, kind: OverlayKind) -> Option<(lard_core::Rect, f32)> {
        let radius = match kind {
            OverlayKind::Launcher => self.config.launcher.corner_radius,
            OverlayKind::Popover => self.config.theme.radius.card(),
            OverlayKind::ControlCenter | OverlayKind::Settings | OverlayKind::Wallpapers => {
                self.config.theme.radius.panel()
            }
            OverlayKind::Notification => self.config.bar.corner_radius,
        };
        self.overlay_rect(kind).map(|rect| (rect, radius))
    }

    pub fn new(config: Config) -> Self {
        let reduce_motion = config.animation.reduce_motion;
        let notification_spring = config.animation.panel;
        let mut apps = scan_applications();
        // Favourites first in the launcher (DankLauncher-style), while keeping
        // the full list alphabetical for search results.
        apps.sort_by_cached_key(|entry| {
            (
                dock::pin_rank(entry, &config.dock.pinned),
                entry.name.to_lowercase(),
            )
        });
        tracing::info!(count = apps.len(), "application index loaded");
        let icons = AppIcons::new();
        let launcher = Launcher::new(config.animation.launcher, reduce_motion);
        let control = ControlCenter::new(config.animation.panel, reduce_motion);
        let osd = Osd::new(config.animation.osd, reduce_motion);
        let settings = SettingsView::new(config.animation.panel, reduce_motion);
        let popover = Popover::new(config.animation.panel, reduce_motion);
        let wallpaper = WallpaperView::new(config.animation.panel, reduce_motion);
        let toasts = ToastStack::new(config.animation.notification, reduce_motion);
        let (clock, date) = clock_strings(&config, Local::now());
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "user".into());

        let mut shell = Self {
            config,
            niri: NiriSnapshot::default(),
            niri_outputs: Vec::new(),
            volume: 50,
            muted: false,
            volume_seen: false,
            brightness: 50,
            brightness_present: false,
            brightness_seen: false,
            user,
            battery: BatteryState::default(),
            network: NetworkState::default(),
            bluetooth: BluetoothState::default(),
            system: SystemStats::default(),
            dnd: false,
            notification_width: Spring::new(0.0, notification_spring),
            notification_initialized: false,
            notification_rect: Rect::ZERO,
            wifi_enabled: true,
            wifi_networks: Vec::new(),
            wifi_scanning: false,
            wifi_error: None,
            wifi_connecting: None,
            bluetooth_devices: Vec::new(),
            bt_scanning: false,
            bt_error: None,
            bt_connecting: None,
            wifi_service_enabled: true,
            bt_service_powered: false,
            clock,
            date,
            apps,
            icons,
            bars: HashMap::new(),
            dock: Dock::new(),
            launcher,
            control,
            osd,
            settings,
            settings_backup: None,
            popover,
            wallpaper,
            toasts,
            actions: Vec::new(),
            dirty: true,
            reduce_motion,
        };
        shell.apply_niri_theme();
        shell.restore_wallpaper();
        shell
    }

    /// Apply a reloaded configuration without losing shell state (open
    /// launcher, notifications, niri snapshot).
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
        self.apply_niri_theme();
        for bar in self.bars.values_mut() {
            bar.invalidate();
        }
        self.dock.invalidate();
        self.dirty = true;
    }

    pub fn refresh_clock(&mut self, now: DateTime<Local>) {
        let (clock, date) = clock_strings(&self.config, now);
        if clock != self.clock || date != self.date {
            self.clock = clock;
            self.date = date;
            self.dirty = true;
        }
    }

    pub fn apply(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Outputs(outputs) => {
                self.niri_outputs = outputs;
                self.dirty = true;
            }
            ServiceEvent::Niri(snapshot) => {
                self.niri = snapshot;
                for bar in self.bars.values_mut() {
                    bar.invalidate();
                }
                self.dock.invalidate();
            }
            ServiceEvent::Volume { percent, muted } => {
                let changed = percent != self.volume || muted != self.muted;
                self.volume = percent;
                self.muted = muted;
                if changed {
                    if self.volume_seen {
                        self.osd.show_volume(percent, muted, &self.config.osd);
                    }
                    self.volume_seen = true;
                }
            }
            ServiceEvent::Battery(state) => self.battery = state,
            ServiceEvent::Brightness(state) => {
                self.brightness_present = state.present;
                if state.present {
                    let changed = self.brightness != state.percent;
                    self.brightness = state.percent;
                    if changed && self.brightness_seen {
                        self.osd.show_brightness(state.percent, &self.config.osd);
                    }
                    self.brightness_seen = true;
                } else {
                    self.brightness = 0;
                    self.brightness_seen = false;
                }
            }
            ServiceEvent::Network(state) => self.network = state,
            ServiceEvent::WifiEnabled(enabled) => {
                let was_enabled = self.wifi_service_enabled;
                self.wifi_service_enabled = enabled;
                self.wifi_enabled = enabled;
                self.wifi_error = None;
                if !enabled {
                    self.wifi_scanning = false;
                    self.wifi_networks.clear();
                    self.wifi_connecting = None;
                    self.control.cancel_password();
                } else if !was_enabled && self.wifi_page_visible() {
                    self.request_wifi_scan();
                }
            }
            ServiceEvent::WifiNetworks(networks) => {
                self.wifi_networks = networks;
                self.wifi_scanning = false;
            }
            ServiceEvent::WifiScanError(error) => {
                self.wifi_scanning = false;
                self.wifi_error = Some(error);
            }
            ServiceEvent::WifiConnectResult { ok, message, .. } => {
                self.wifi_connecting = None;
                if ok {
                    self.wifi_error = None;
                } else {
                    self.wifi_error =
                        Some(message.unwrap_or_else(|| "Connessione non riuscita".into()));
                }
            }
            ServiceEvent::Bluetooth(state) => {
                let was_powered = self.bt_service_powered;
                self.bt_service_powered = state.powered;
                self.bluetooth = state;
                if !state.powered {
                    self.bt_scanning = false;
                    self.bluetooth_devices.clear();
                    self.bt_connecting = None;
                    self.bt_error = None;
                } else if !was_powered && self.bluetooth_page_visible() {
                    self.request_bt_refresh();
                }
            }
            ServiceEvent::BluetoothDevices(devices) => {
                self.bluetooth_devices = devices;
                self.bt_scanning = false;
            }
            ServiceEvent::BluetoothScanError(error) => {
                self.bt_scanning = false;
                self.bt_error = Some(error);
            }
            ServiceEvent::BluetoothConnectResult { ok, message, .. } => {
                self.bt_connecting = None;
                if ok {
                    self.bt_error = None;
                } else {
                    self.bt_error =
                        Some(message.unwrap_or_else(|| "Connessione non riuscita".into()));
                }
            }
            ServiceEvent::SystemStats(stats) => self.system = stats,
            ServiceEvent::Notification(notification) => {
                if self.dnd || !self.config.notifications.enabled {
                    self.actions.push(ShellAction::CloseNotification {
                        id: notification.id,
                        reason: CloseReason::Dismissed,
                    });
                } else if let Some(evicted) =
                    self.toasts.push(notification, &self.config.notifications)
                {
                    self.actions.push(ShellAction::CloseNotification {
                        id: evicted,
                        reason: CloseReason::Dismissed,
                    });
                }
            }
            ServiceEvent::NotificationClosed { id, .. } => {
                self.toasts.remove(id);
            }
        }
        self.dirty = true;
    }

    pub fn update(&mut self, dt: f32) -> bool {
        if self.launcher.is_open()
            && self
                .launcher
                .idle_deadline(self.config.launcher.idle_timeout_secs)
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.launcher.close();
            self.dirty = true;
        }

        let mut active = false;
        self.dock.sync(
            &self.apps,
            &self.niri,
            &self.config.dock,
            &self.config.theme,
        );
        for bar in self.bars.values_mut() {
            bar.sync(&self.niri, &self.config.bar, &self.config.font);
            active |= bar.update(dt);
        }
        active |= self.launcher.update(dt);
        active |= self.control.update(dt);
        active |= self.osd.update(dt);
        active |= self.settings.update(dt);
        active |= self.popover.update(dt);
        active |= self.wallpaper.update(dt);
        for id in self.toasts.update(dt) {
            self.actions.push(ShellAction::CloseNotification {
                id,
                reason: CloseReason::Expired,
            });
        }
        active |= self.toasts.active();
        if active {
            self.dirty = true;
        }
        active
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        [
            self.osd.next_deadline(),
            self.toasts.next_deadline(),
            self.launcher
                .idle_deadline(self.config.launcher.idle_timeout_secs),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn take_actions(&mut self) -> Vec<ShellAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    #[must_use]
    pub fn wants_overlay_frame(&self) -> bool {
        self.launcher.wants_surface()
            || self.control.wants_surface()
            || self.settings.wants_surface()
            || self.popover.wants_surface()
            || self.wallpaper.wants_surface()
            || self.osd.wants_surface()
            || self.toast_size().is_some()
            || self.notification_surface_size().is_some()
    }

    // ---- bar ----

    /// Take the values niri owns (background, accent, danger) from the niri
    /// configuration, so they are not duplicated in the shell configuration.
    fn apply_niri_theme(&mut self) {
        if !self.config.theme.follow_niri {
            return;
        }
        let theme = niri_config::niri_theme(&niri_config::niri_config_dir());
        if let Some(color) = theme.background {
            // Keep the transparency of the shell, take the colour from niri.
            self.config.theme.background = color.with_alpha(self.config.theme.background.a);
        }
        if let Some(color) = theme.accent {
            self.config.theme.accent = color;
        }
        if let Some(color) = theme.danger {
            self.config.theme.danger = color;
        }
    }

    /// Apply the wallpaper chosen in the carousel again at start-up. The
    /// awww daemon may still be starting, so the wallpaper is set a few
    /// times before giving up.
    fn restore_wallpaper(&self) {
        let path = self.config.theme.wallpaper.clone();
        if path.is_empty() {
            return;
        }
        let path = std::path::PathBuf::from(path);
        if !path.exists() {
            return;
        }
        std::thread::Builder::new()
            .name("lard-wallpaper".into())
            .spawn(move || {
                for attempt in 0..6 {
                    if attempt > 0 {
                        std::thread::sleep(Duration::from_millis(1200));
                    }
                    if std::process::Command::new("awww")
                        .args(["img", &path.to_string_lossy()])
                        .status()
                        .is_ok_and(|status| status.success())
                    {
                        return;
                    }
                }
            })
            .ok();
    }

    pub fn build_bar(&mut self, scene: &mut Scene, output: &str, size: Size) {
        if !self.config.bar.enabled {
            return;
        }
        if !self.bars.contains_key(output) {
            self.bars.insert(output.to_string(), Bar::new(output));
        }
        let data = BarData {
            niri: &self.niri,
            volume: self.volume,
            muted: self.muted,
            osd: self.osd.state(),
            battery: self.battery,
            network: &self.network,
            bluetooth: self.bluetooth,
            wifi_enabled: self.wifi_enabled,
            system: self.system,
            clock: &self.clock,
            date: &self.date,
            dnd: self.dnd,
            active_toasts: self.toasts.len(),
            toast: self.toasts.preview(),
            notification_count: self.toasts.pending(),
            morph: self.morph_state(),
        };
        if let Some(bar) = self.bars.get_mut(output) {
            bar.build(scene, size, &data, &self.config);
        }
    }

    /// Rounded regions of the bar islands (surface-local coordinates) used to
    /// clip the compositor-side backdrop blur to the islands only.
    pub fn bar_blur_regions(&self, output: &str) -> Vec<(lard_core::Rect, f32)> {
        let Some(bar) = self.bars.get(output) else {
            return Vec::new();
        };
        let radius = self
            .config
            .bar
            .corner_radius
            .min(self.config.bar.height * 0.5)
            .max(0.0);
        bar.island_rects()
            .iter()
            .filter(|(_, rect)| rect.w > 1.0 && rect.h > 1.0)
            .map(|(_, rect)| (*rect, radius))
            .collect()
    }

    /// Rounded region of the dock island, used for the backdrop blur.
    pub fn dock_blur_regions(&self) -> Vec<(lard_core::Rect, f32)> {
        let panel = self.dock.panel_rect();
        if panel.w <= 1.0 || panel.h <= 1.0 {
            return Vec::new();
        }
        vec![(panel, self.config.theme.radius.panel())]
    }

    pub fn bar_press(&mut self, output: &str, pos: Point) -> Option<ShellAction> {
        let press = self.bars.get_mut(output).and_then(|bar| bar.press(pos));
        let Some(press) = press else {
            return None;
        };
        if let Some((kind, x)) = press.popover {
            let anchor = self.config.bar.margin + x;
            let island = match kind {
                PopoverKind::ControlCenter => Some(BarIsland::Right),
                PopoverKind::Clock | PopoverKind::Notifications => Some(BarIsland::Center),
                _ => None,
            };
            if kind == PopoverKind::Notifications {
                // The panel opens under the centre island, not under the bell.
                let center = self
                    .island_origin(BarIsland::Center)
                    .map(|(rect, _, _)| rect.x + rect.w * 0.5)
                    .unwrap_or(anchor);
                self.open_popover(kind, center, island);
            } else {
                self.open_popover(kind, anchor, island);
            }
        }
        match press.action {
            Some(ShellAction::FocusWorkspace(id)) => Some(ShellAction::FocusWorkspace(id)),
            Some(ShellAction::ToggleMute) => Some(ShellAction::ToggleMute),
            Some(ShellAction::VolumeStep(step)) => Some(ShellAction::VolumeStep(step)),
            Some(ShellAction::OpenLauncher) => {
                self.open_launcher();
                None
            }
            Some(ShellAction::ToggleBluetooth) => {
                let enabled = !self.bluetooth.powered;
                self.bluetooth.powered = enabled;
                self.force_wifi_bt_dirty();
                Some(ShellAction::SetBluetooth(enabled))
            }
            Some(ShellAction::ToggleControlCenter) => {
                self.toggle_control();
                None
            }
            Some(ShellAction::OpenSettings) => {
                self.open_settings();
                None
            }
            Some(ShellAction::OpenPowerMenu) => {
                self.open_power_menu();
                None
            }
            Some(ShellAction::OpenWifiPanel) => {
                self.open_wifi_panel();
                None
            }
            Some(ShellAction::OpenBluetoothPanel) => {
                self.open_bluetooth_panel();
                None
            }
            Some(other) => Some(other),
            None => None,
        }
    }

    pub fn bar_move(&mut self, output: &str, pos: Point) -> bool {
        self.bars
            .get_mut(output)
            .is_some_and(|bar| bar.pointer_move(pos))
    }

    pub fn bar_leave(&mut self, output: &str) -> bool {
        self.bars.get_mut(output).is_some_and(Bar::pointer_leave)
    }

    pub fn bar_axis(&mut self, output: &str, pos: Point, delta: f64) -> Option<ShellAction> {
        self.bars
            .get_mut(output)
            .and_then(|bar| bar.axis(pos, delta))
    }

    // ---- dock ----

    #[must_use]
    pub fn dock_size(&self) -> Option<Size> {
        self.dock.size()
    }

    pub fn build_dock(&mut self, scene: &mut Scene, size: Size) {
        self.icons.begin_frame();
        self.dock.build(
            scene,
            size,
            &self.config.theme,
            &self.config.font,
            self.config.dock.opacity,
            &mut self.icons,
        );
    }

    pub fn dock_press(&mut self, pos: Point) -> Option<ShellAction> {
        self.dock.press(pos)
    }

    pub fn dock_move(&mut self, pos: Point) -> bool {
        self.dock.pointer_move(pos)
    }

    pub fn dock_leave(&mut self) -> bool {
        self.dock.pointer_leave()
    }

    // ---- launcher ----

    #[must_use]
    pub fn launcher_open(&self) -> bool {
        self.launcher.is_open()
    }

    #[must_use]
    pub fn launcher_wants_surface(&self) -> bool {
        self.launcher.wants_surface()
    }

    pub fn open_launcher(&mut self) {
        self.close_popover();
        if self.control.is_open() {
            self.control.close();
        }
        let origin = self
            .island_origin(BarIsland::Left)
            .map(|(rect, radius, _)| (rect, radius));
        self.launcher.set_origin(origin);
        self.launcher
            .open(&self.apps, self.config.launcher.max_results);
        self.dirty = true;
    }

    pub fn close_launcher(&mut self) {
        self.launcher.close();
        self.dirty = true;
    }

    pub fn build_launcher(&mut self, scene: &mut Scene, size: Size) {
        self.icons.begin_frame();
        self.launcher
            .build(scene, size, &self.apps, &self.config, &mut self.icons);
    }

    pub fn launcher_press(&mut self, pos: Point) -> Option<ShellAction> {
        self.launcher.pointer_press(pos, &self.apps)
    }

    pub fn launcher_move(&mut self, pos: Point) {
        self.launcher.pointer_move(pos);
    }

    pub fn launcher_leave(&mut self) {
        self.launcher.pointer_leave();
    }

    pub fn launcher_axis(&mut self, delta: f64) {
        self.launcher.axis(delta);
    }

    pub fn launcher_key(&mut self, key: ShellKey) -> Option<ShellAction> {
        let key = match key {
            ShellKey::Char(text) => LauncherKey::Char(text),
            ShellKey::Backspace => LauncherKey::Backspace,
            ShellKey::Enter => LauncherKey::Enter,
            ShellKey::Escape => LauncherKey::Escape,
            ShellKey::Up => LauncherKey::Up,
            ShellKey::Down => LauncherKey::Down,
            ShellKey::Tab => LauncherKey::Tab,
            ShellKey::Left
            | ShellKey::Right
            | ShellKey::Home
            | ShellKey::End
            | ShellKey::PageUp
            | ShellKey::PageDown
            | ShellKey::Delete => return None,
        };
        self.launcher
            .key(key, &self.apps, self.config.launcher.max_results)
    }

    // ---- control center ----

    #[must_use]
    pub fn control_open(&self) -> bool {
        self.control.is_open()
    }

    #[must_use]
    pub fn control_wants_surface(&self) -> bool {
        self.control.wants_surface()
    }

    pub fn toggle_control(&mut self) {
        if self.control.is_open() {
            self.control.close();
            self.dirty = true;
        } else {
            self.open_control_home();
        }
    }

    pub fn open_control_home(&mut self) {
        self.prepare_control_open();
        self.control.show_page(ControlPage::Home);
        self.dirty = true;
    }

    pub fn open_control_page(&mut self, page: ControlPage) {
        self.prepare_control_open();
        self.control.show_page(page);
        match page {
            ControlPage::Wifi => {
                self.wifi_error = None;
                self.request_wifi_scan();
            }
            ControlPage::Bluetooth => {
                self.bt_error = None;
                self.request_bt_refresh();
            }
            ControlPage::Home | ControlPage::Power => {}
        }
        self.dirty = true;
    }

    pub fn open_power_menu(&mut self) {
        self.open_control_page(ControlPage::Power);
    }

    /// Open the grouped Control Center popover under the bar button. Used by
    /// the `controls` IPC command so a keybind can reach the same panel.
    pub fn open_controls_popover(&mut self) {
        let output = self
            .niri
            .focused_output
            .clone()
            .or_else(|| self.bars.keys().next().cloned());
        let Some(output) = output else {
            return;
        };
        let Some(anchor) = self
            .bars
            .get(&output)
            .and_then(|bar| bar.control_center_anchor())
        else {
            return;
        };
        let anchor = self.config.bar.margin + anchor;
        self.open_popover(PopoverKind::ControlCenter, anchor, Some(BarIsland::Right));
    }

    pub fn open_wifi_panel(&mut self) {
        self.open_control_page(ControlPage::Wifi);
    }

    pub fn open_bluetooth_panel(&mut self) {
        self.open_control_page(ControlPage::Bluetooth);
    }

    fn wifi_page_visible(&self) -> bool {
        (self.control.is_open() && self.control.page() == ControlPage::Wifi)
            || self.popover.expanded_page() == Some(ControlPage::Wifi)
    }

    fn bluetooth_page_visible(&self) -> bool {
        (self.control.is_open() && self.control.page() == ControlPage::Bluetooth)
            || self.popover.expanded_page() == Some(ControlPage::Bluetooth)
    }

    fn prepare_control_open(&mut self) {
        self.close_popover();
        if self.launcher.is_open() {
            self.launcher.close();
        }
        if self.wallpaper.is_open() {
            self.wallpaper.close();
        }
        self.control.open();
    }

    fn request_wifi_scan(&mut self) {
        if !self.wifi_enabled || self.wifi_scanning {
            return;
        }
        self.wifi_scanning = true;
        self.wifi_error = None;
        self.actions.push(ShellAction::ScanWifi);
    }

    fn request_bt_refresh(&mut self) {
        if !self.bluetooth.present || !self.bluetooth.powered || self.bt_scanning {
            return;
        }
        self.bt_error = None;
        self.actions.push(ShellAction::RefreshBluetooth);
    }

    pub fn close_control(&mut self) {
        self.control.close();
        self.dirty = true;
    }

    pub fn build_control(&mut self, scene: &mut Scene, size: Size) {
        let data = ControlData {
            volume: self.volume,
            muted: self.muted,
            brightness: self.brightness,
            brightness_present: self.brightness_present,
            user: &self.user,
            output: self.niri.focused_output.as_deref(),
            keyboard_layout: self.niri.keyboard_layout.as_deref(),
            niri_connected: self.niri.connected,
            battery: self.battery,
            network: &self.network,
            bluetooth: self.bluetooth,
            dnd: self.dnd,
            wifi_enabled: self.wifi_enabled,
            wifi_networks: &self.wifi_networks,
            wifi_scanning: self.wifi_scanning,
            wifi_error: self.wifi_error.as_deref(),
            wifi_connecting: self.wifi_connecting.as_deref(),
            bluetooth_devices: &self.bluetooth_devices,
            bt_scanning: self.bt_scanning,
            bt_error: self.bt_error.as_deref(),
            bt_connecting: self.bt_connecting.as_deref(),
            clock: &self.clock,
            date: &self.date,
        };
        self.control.build(scene, size, &data, &self.config);
    }

    fn apply_control_action(&mut self, action: Option<ShellAction>) -> Option<ShellAction> {
        match action {
            Some(ShellAction::ToggleDnd) => {
                self.dnd = !self.dnd;
                self.dirty = true;
                None
            }
            Some(ShellAction::SetWifi(enabled)) => {
                self.wifi_enabled = enabled;
                self.wifi_error = None;
                if !enabled {
                    self.wifi_scanning = false;
                    self.wifi_networks.clear();
                    self.wifi_connecting = None;
                }
                self.dirty = true;
                Some(ShellAction::SetWifi(enabled))
            }
            Some(ShellAction::SetBluetooth(enabled)) => {
                self.bluetooth.powered = enabled;
                self.bt_error = None;
                if !enabled {
                    self.bt_scanning = false;
                    self.bluetooth_devices.clear();
                    self.bt_connecting = None;
                }
                self.dirty = true;
                Some(ShellAction::SetBluetooth(enabled))
            }
            Some(ShellAction::ScanWifi) => {
                if !self.wifi_enabled || self.wifi_scanning || self.wifi_connecting.is_some() {
                    return None;
                }
                self.wifi_scanning = true;
                self.wifi_error = None;
                self.dirty = true;
                Some(ShellAction::ScanWifi)
            }
            Some(ShellAction::RefreshBluetooth) => {
                if !self.bluetooth.present || !self.bluetooth.powered || self.bt_scanning {
                    return None;
                }
                self.bt_error = None;
                self.dirty = true;
                Some(ShellAction::RefreshBluetooth)
            }
            Some(ShellAction::ScanBluetooth) => {
                if !self.bluetooth.powered || self.bt_scanning || self.bt_connecting.is_some() {
                    return None;
                }
                self.bt_scanning = true;
                self.bt_error = None;
                self.dirty = true;
                Some(ShellAction::ScanBluetooth)
            }
            Some(ShellAction::ConnectWifi { ssid, password }) => {
                self.wifi_error = None;
                self.wifi_connecting = Some(ssid.clone());
                self.dirty = true;
                Some(ShellAction::ConnectWifi { ssid, password })
            }
            Some(ShellAction::DisconnectWifi) => {
                self.wifi_error = None;
                self.dirty = true;
                Some(ShellAction::DisconnectWifi)
            }
            Some(ShellAction::ConnectBluetooth { address }) => {
                self.bt_error = None;
                self.bt_connecting = Some(address.clone());
                self.dirty = true;
                Some(ShellAction::ConnectBluetooth { address })
            }
            Some(ShellAction::DisconnectBluetooth { address }) => {
                self.bt_error = None;
                self.dirty = true;
                Some(ShellAction::DisconnectBluetooth { address })
            }
            Some(ShellAction::SetBrightness(percent)) => {
                self.brightness = percent;
                self.dirty = true;
                Some(ShellAction::SetBrightness(percent))
            }
            Some(ShellAction::SetVolume(percent)) => {
                self.volume = percent;
                self.dirty = true;
                Some(ShellAction::SetVolume(percent))
            }
            other => other,
        }
    }

    pub fn control_press(&mut self, pos: Point) -> Option<ShellAction> {
        let action = self.control.pointer_press(pos);
        let action = self.apply_control_action(action);
        if matches!(action, Some(ShellAction::Power(_))) {
            self.control.close();
            self.dirty = true;
        }
        action
    }

    pub fn control_move(&mut self, pos: Point) {
        self.control.pointer_move(pos);
    }

    pub fn control_leave(&mut self) {
        self.control.pointer_leave();
    }

    pub fn control_axis(&mut self, delta: f32) {
        if self.control.axis(delta) {
            self.dirty = true;
        }
    }

    pub fn control_key(&mut self, key: ShellKey) -> Option<ShellAction> {
        let action = self.control.key(key);
        if let Some(ShellAction::ConnectWifi { ssid, .. }) = &action {
            self.wifi_error = None;
            self.wifi_connecting = Some(ssid.clone());
        }
        self.dirty = true;
        action
    }

    // ---- wallpapers ----

    #[must_use]
    pub fn wallpaper_open(&self) -> bool {
        self.wallpaper.is_open()
    }

    #[must_use]
    pub fn wallpaper_wants_surface(&self) -> bool {
        self.wallpaper.wants_surface()
    }

    pub fn open_wallpaper(&mut self) {
        self.close_popover();
        if self.control.is_open() {
            self.control.close();
        }
        if self.launcher.is_open() {
            self.launcher.close();
        }
        if self.settings.is_open() {
            self.settings.close_now();
        }
        self.wallpaper.open();
        self.dirty = true;
    }

    pub fn close_wallpaper(&mut self) {
        self.wallpaper.close();
        self.dirty = true;
    }

    pub fn build_wallpaper(&mut self, scene: &mut Scene, size: Size) {
        self.wallpaper.build(scene, size, &self.config);
    }

    pub fn wallpaper_press(&mut self, pos: Point) {
        let Some(action) = self.wallpaper.pointer_press(pos) else {
            return;
        };
        match action {
            WallpaperAction::Apply { path, accent } => self.apply_wallpaper(path, accent),
            WallpaperAction::ApplySelected => {
                if let Some((path, accent)) = self.wallpaper.selected() {
                    self.apply_wallpaper(path, accent);
                }
            }
            WallpaperAction::Page(delta) => {
                self.wallpaper.page(delta);
                self.dirty = true;
            }
            WallpaperAction::Close => self.close_wallpaper(),
        }
    }

    pub fn wallpaper_move(&mut self, pos: Point) {
        self.wallpaper.pointer_move(pos);
    }

    pub fn wallpaper_leave(&mut self) {
        self.wallpaper.pointer_leave();
    }

    pub fn wallpaper_key(&mut self, escape: bool, left: bool, right: bool, enter: bool) {
        match self.wallpaper.key(escape, left, right, enter) {
            Some(WallpaperAction::ApplySelected) => {
                if let Some((path, accent)) = self.wallpaper.selected() {
                    self.apply_wallpaper(path, accent);
                }
            }
            Some(WallpaperAction::Close) => self.close_wallpaper(),
            Some(WallpaperAction::Apply { path, accent }) => {
                self.apply_wallpaper(path, accent);
            }
            Some(WallpaperAction::Page(delta)) => {
                self.wallpaper.page(delta);
                self.dirty = true;
            }
            None => {}
        }
    }

    fn apply_wallpaper(&mut self, path: std::path::PathBuf, accent: Color) {
        self.config.theme.accent = accent;
        self.config.theme.accent_text = accent.mix(Color::BLACK, 0.72);
        self.config.greeter.wallpaper.accent_glow = accent.scale_alpha(0.55);
        // Remember the choice: it is written to the configuration and applied
        // again at start-up, so the wallpaper survives a reboot.
        self.config.theme.wallpaper = path.to_string_lossy().into_owned();
        self.wallpaper.close();
        self.invalidate_views();
        // Set the wallpaper through the running awww daemon. This is what
        // actually changes the background: `omarchy-theme-apply` only derives
        // the palette and reloads kitty/niri colours.
        if let Some(p) = path.to_str() {
            let _ = std::process::Command::new("awww").args(["img", p]).spawn();
        }
        // waypaper keeps the selection in its own state (`waypaper --restore`
        // brings it back at the next login) and runs the theme hook.
        if crate::widgets::command_exists("waypaper") {
            // `--wallpaper` sets the image and stores it in the waypaper state,
            // so `waypaper --restore` brings it back at the next login.
            let _ = std::process::Command::new("waypaper")
                .args(["--wallpaper", &path.to_string_lossy()])
                .spawn();
        } else if crate::widgets::command_exists("omarchy-theme-apply") {
            let _ = std::process::Command::new("omarchy-theme-apply")
                .arg(&path)
                .spawn();
        }
        let cfg_path = std::env::var_os("NIRI_SUITE_CONFIG")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(lard_core::config::default_path);
        match self.config.save_to(&cfg_path) {
            Ok(()) => self.actions.push(ShellAction::ReloadConfig),
            Err(error) => tracing::error!(%error, "cannot save wallpaper accent"),
        }
    }

    // ---- popovers ----

    #[must_use]
    pub fn popover_open(&self) -> bool {
        self.popover.is_open()
    }

    #[must_use]
    pub fn popover_wants_surface(&self) -> bool {
        self.popover.wants_surface()
    }

    pub fn open_popover(&mut self, kind: PopoverKind, anchor_x: f32, island: Option<BarIsland>) {
        if self.popover.is_open()
            && self.popover.anchor_x() == anchor_x
            && self.popover.kind() == kind
        {
            self.popover.close();
            return;
        }
        let origin = island.and_then(|island| self.island_origin(island));
        self.popover.open(kind, anchor_x, origin);
        if kind == PopoverKind::Notifications {
            // The card grows with the number of pending notifications.
            let size = Popover::notifications_size(&self.toasts.entries());
            self.popover.set_size(size);
        }
        self.control.cancel_password();
        self.dirty = true;
    }

    pub fn close_popover(&mut self) {
        self.popover.close();
        self.control.cancel_password();
        self.dirty = true;
    }

    #[must_use]
    pub fn popover_card_size(&self) -> Option<Size> {
        self.popover
            .wants_surface()
            .then(|| self.popover.current_size())
    }

    pub fn popover_anchor_x(&self) -> f32 {
        self.popover.anchor_x()
    }

    /// Kind of popover card currently open, if any.
    pub fn popover_kind(&self) -> Option<PopoverKind> {
        self.popover.wants_surface().then(|| self.popover.kind())
    }

    pub fn build_popover(&mut self, scene: &mut Scene, size: Size, left: f32) {
        let data = PopoverData {
            clock: &self.clock,
            date: &self.date,
            network: &self.network,
            bluetooth: self.bluetooth,
            wifi_enabled: self.wifi_enabled,
            battery: self.battery,
            system: self.system,
            dnd: self.dnd,
            active_toasts: self.toasts.len(),
            volume: self.volume,
            muted: self.muted,
            brightness: self.brightness,
            brightness_present: self.brightness_present,
            notifications: self.toasts.entries(),
        };
        let control_data = ControlData {
            volume: self.volume,
            muted: self.muted,
            brightness: self.brightness,
            brightness_present: self.brightness_present,
            user: &self.user,
            output: self.niri.focused_output.as_deref(),
            keyboard_layout: self.niri.keyboard_layout.as_deref(),
            niri_connected: self.niri.connected,
            battery: self.battery,
            network: &self.network,
            bluetooth: self.bluetooth,
            dnd: self.dnd,
            wifi_enabled: self.wifi_enabled,
            wifi_networks: &self.wifi_networks,
            wifi_scanning: self.wifi_scanning,
            wifi_error: self.wifi_error.as_deref(),
            wifi_connecting: self.wifi_connecting.as_deref(),
            bluetooth_devices: &self.bluetooth_devices,
            bt_scanning: self.bt_scanning,
            bt_error: self.bt_error.as_deref(),
            bt_connecting: self.bt_connecting.as_deref(),
            clock: &self.clock,
            date: &self.date,
        };
        self.popover.build(
            scene,
            size,
            &data,
            &self.config,
            left,
            Some((&mut self.control, &control_data)),
        );
    }

    fn dispatch_embedded(&mut self, command: EmbeddedCommand) {
        match command {
            EmbeddedCommand::Close => self.close_popover(),
            EmbeddedCommand::Back => {
                self.control.cancel_password();
                self.popover.collapse_page();
                self.dirty = true;
            }
            EmbeddedCommand::Action(action) => {
                let action = self.apply_control_action(action);
                if let Some(action) = action {
                    if matches!(action, ShellAction::Power(_)) {
                        self.close_popover();
                    }
                    self.actions.push(action);
                }
                self.dirty = true;
            }
        }
    }

    pub fn popover_press(&mut self, pos: Point) {
        if self.popover.expanded_page().is_some() {
            let panel = self.popover.content_rect();
            if let Some(command) = self.control.embedded_press(pos, panel) {
                self.dispatch_embedded(command);
            }
            self.dirty = true;
            return;
        }

        let Some(action) = self.popover.pointer_press(pos) else {
            return;
        };
        match action {
            PopoverAction::ToggleWifi(enabled) => {
                if let Some(action) = self.apply_control_action(Some(ShellAction::SetWifi(enabled)))
                {
                    self.actions.push(action);
                }
            }
            PopoverAction::ToggleBluetooth(enabled) => {
                if let Some(action) =
                    self.apply_control_action(Some(ShellAction::SetBluetooth(enabled)))
                {
                    self.actions.push(action);
                }
            }
            PopoverAction::ToggleDnd => {
                self.dnd = !self.dnd;
                self.dirty = true;
            }
            PopoverAction::ExpandWifi => {
                self.popover.expand_page(ControlPage::Wifi);
                self.wifi_error = None;
                self.request_wifi_scan();
            }
            PopoverAction::ExpandBluetooth => {
                self.popover.expand_page(ControlPage::Bluetooth);
                self.bt_error = None;
                self.request_bt_refresh();
            }
            PopoverAction::OpenPowerMenu => {
                self.close_popover();
                self.open_power_menu();
            }
            PopoverAction::Power(action) => {
                self.actions.push(ShellAction::Power(action));
                self.close_popover();
            }
            PopoverAction::SetVolume(percent) => {
                if let Some(action) =
                    self.apply_control_action(Some(ShellAction::SetVolume(percent)))
                {
                    self.actions.push(action);
                }
            }
            PopoverAction::SetBrightness(percent) => {
                if let Some(action) =
                    self.apply_control_action(Some(ShellAction::SetBrightness(percent)))
                {
                    self.actions.push(action);
                }
            }
            PopoverAction::ToggleMute => {
                self.muted = !self.muted;
                self.actions.push(ShellAction::ToggleMute);
                self.dirty = true;
            }
            PopoverAction::OpenSettings => {
                self.open_settings();
            }
            PopoverAction::ShowPower => {
                self.popover.set_power_screen(true);
                self.dirty = true;
            }
            PopoverAction::BackControl => {
                self.popover.set_power_screen(false);
                self.dirty = true;
            }
            PopoverAction::NotificationAction { id, key } => {
                self.actions.push(ShellAction::CloseNotification {
                    id,
                    reason: CloseReason::Dismissed,
                });
                self.actions
                    .push(ShellAction::NotificationAction { id, action: key });
            }
            PopoverAction::DismissNotification(id) => {
                self.actions.push(ShellAction::CloseNotification {
                    id,
                    reason: CloseReason::Dismissed,
                });
            }
        }
        self.dirty = true;
    }

    pub fn popover_move(&mut self, pos: Point) {
        self.popover.pointer_move(pos);
        if self.popover.expanded_page().is_some() {
            self.control.pointer_move(pos);
        }
    }

    pub fn popover_leave(&mut self) {
        self.popover.pointer_leave();
        self.control.pointer_leave();
    }

    pub fn popover_axis(&mut self, delta: f32) -> Option<ShellAction> {
        if let Some(page) = self.popover.expanded_page() {
            if self.control.embedded_axis(page, delta) {
                self.dirty = true;
            }
            return None;
        }
        let action = self.popover.axis(delta)?;
        let action = match action {
            PopoverAction::SetVolume(percent) => Some(ShellAction::SetVolume(percent)),
            PopoverAction::SetBrightness(percent) => Some(ShellAction::SetBrightness(percent)),
            _ => None,
        }?;
        let action = self.apply_control_action(Some(action));
        if action.is_some() {
            self.dirty = true;
        }
        action
    }

    pub fn popover_key(&mut self, key: ShellKey) {
        if let Some(page) = self.popover.expanded_page() {
            if let Some(command) = self.control.embedded_key(page, key) {
                self.dispatch_embedded(command);
            }
        } else if key == ShellKey::Escape {
            self.close_popover();
        }
        self.dirty = true;
    }

    // ---- settings center ----

    #[must_use]
    pub fn settings_open(&self) -> bool {
        self.settings.is_open()
    }

    #[must_use]
    pub fn settings_wants_surface(&self) -> bool {
        self.settings.wants_surface()
    }

    pub fn open_settings(&mut self) {
        self.close_popover();
        if self.control.is_open() {
            self.control.close();
        }
        if self.launcher.is_open() {
            self.launcher.close();
        }
        self.settings_backup = Some(self.config.clone());
        self.settings.open();
        self.dirty = true;
    }

    pub fn close_settings(&mut self, apply: bool) {
        if apply {
            let path = std::env::var_os("NIRI_SUITE_CONFIG")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(lard_core::config::default_path);
            match self.config.save_to(&path) {
                Ok(()) => {
                    tracing::info!(path = %path.display(), "shell settings saved");
                    self.settings.close_now();
                    self.settings_backup = None;
                    self.actions.push(ShellAction::ReloadConfig);
                }
                Err(error) => {
                    tracing::error!(%error, "cannot save shell settings");
                    return;
                }
            }
        } else {
            if let Some(backup) = self.settings_backup.take() {
                self.config = backup;
            }
            self.settings.close_now();
            self.invalidate_views();
        }
        self.dirty = true;
    }

    /// Switch the settings center to a given tab (used by the IPC).
    pub fn select_settings_tab(&mut self, tab: SettingsTab) {
        self.settings.select_tab(tab);
        self.dirty = true;
    }

    pub fn build_settings(&mut self, scene: &mut Scene, size: Size) {
        self.settings.set_outputs(self.niri_outputs.clone());
        self.settings.set_output(self.niri.focused_output.clone());
        self.settings.build(scene, size, &self.config);
    }

    pub fn settings_press(&mut self, pos: Point) {
        let Some(action) = self.settings.pointer_press(pos) else {
            return;
        };
        match action {
            SettingsAction::Close | SettingsAction::Cancel => self.close_settings(false),
            SettingsAction::Save => self.close_settings(true),
            SettingsAction::Tab(tab) => self.settings.select_tab(tab),
            SettingsAction::Theme(index) => {
                settings::apply_palette(&mut self.config.theme, index);
                settings::apply_palette_to_wallpaper(&mut self.config.greeter.wallpaper, index);
                self.invalidate_views();
            }
            SettingsAction::Toggle(toggle) => {
                self.apply_setting_toggle(toggle);
                self.invalidate_views();
            }
            SettingsAction::Step(step, delta) => {
                self.apply_setting_step(step, delta);
                self.invalidate_views();
            }
            SettingsAction::NiriStep(setting, delta) => {
                self.settings
                    .niri_config(
                        &niri_config::niri_config_dir(),
                        self.niri.focused_output.as_deref(),
                    )
                    .step(setting, f64::from(delta));
                self.dirty = true;
            }
            SettingsAction::NiriToggle(setting) => {
                self.settings
                    .niri_config(
                        &niri_config::niri_config_dir(),
                        self.niri.focused_output.as_deref(),
                    )
                    .toggle(setting);
                self.dirty = true;
            }
            SettingsAction::NiriCycle(setting) => {
                self.settings
                    .niri_config(
                        &niri_config::niri_config_dir(),
                        self.niri.focused_output.as_deref(),
                    )
                    .cycle(setting);
                self.dirty = true;
            }
            SettingsAction::NiriOutputModeNext => {
                self.settings.niri_output_mode_next();
                self.dirty = true;
            }
            SettingsAction::NiriOutputScaleNext => {
                self.settings.niri_output_scale_next();
                self.dirty = true;
            }
            SettingsAction::SaveNiri => {
                self.settings_save_niri();
            }
            SettingsAction::NextNotificationPosition => {
                use lard_core::theme::ToastPosition;
                self.config.notifications.position = match self.config.notifications.position {
                    ToastPosition::TopRight => ToastPosition::TopLeft,
                    ToastPosition::TopLeft => ToastPosition::BottomLeft,
                    ToastPosition::BottomLeft => ToastPosition::BottomRight,
                    ToastPosition::BottomRight => ToastPosition::TopRight,
                };
                self.dirty = true;
            }
        }
        self.dirty = true;
    }

    pub fn settings_move(&mut self, pos: Point) {
        self.settings.pointer_move(pos);
    }

    pub fn settings_leave(&mut self) {
        self.settings.pointer_leave();
    }

    /// Keyboard input for the settings center. On the niri tab the keys edit
    /// the configuration file.
    pub fn settings_key(&mut self, key: ShellKey) {
        if let Some(SettingsAction::Cancel | SettingsAction::Close) = self.settings.key(&key) {
            self.close_settings(false);
        }
    }

    /// Save the edited niri configuration file and ask niri to reload the
    /// configuration it is running with.
    pub fn settings_save_niri(&mut self) {
        let written = self.settings.save_niri();
        if written.is_empty() {
            return;
        }
        let target = niri_config::active_niri_config();
        for path in &written {
            tracing::info!(path = %path.display(), "niri configuration saved");
        }
        self.actions.push(ShellAction::ReloadNiriConfig(target));
        self.dirty = true;
    }

    fn apply_setting_toggle(&mut self, toggle: Toggle) {
        match toggle {
            Toggle::Workspaces => {
                self.config.bar.show_workspaces = !self.config.bar.show_workspaces
            }
            Toggle::Title => self.config.bar.show_window_title = !self.config.bar.show_window_title,
            Toggle::Clock => self.config.bar.show_clock = !self.config.bar.show_clock,
            Toggle::System => self.config.bar.show_system = !self.config.bar.show_system,
            Toggle::Volume => self.config.bar.show_volume = !self.config.bar.show_volume,
            Toggle::Network => self.config.bar.show_network = !self.config.bar.show_network,
            Toggle::Bluetooth => self.config.bar.show_bluetooth = !self.config.bar.show_bluetooth,
            Toggle::Battery => self.config.bar.show_battery = !self.config.bar.show_battery,
            Toggle::Dock => self.config.dock.enabled = !self.config.dock.enabled,
            Toggle::DockRunning => self.config.dock.show_running = !self.config.dock.show_running,
            Toggle::Blur => self.config.theme.blur = !self.config.theme.blur,
            Toggle::IslandBlur => self.config.theme.island_blur = !self.config.theme.island_blur,
            Toggle::ReduceMotion => {
                self.config.animation.reduce_motion = !self.config.animation.reduce_motion
            }
            Toggle::RadiusLink => {
                self.config.theme.radius.linked = !self.config.theme.radius.linked;
            }
        }
    }

    fn apply_setting_step(&mut self, step: Step, delta: i32) {
        match step {
            Step::BarHeight => {
                self.config.bar.height = (self.config.bar.height + delta as f32).clamp(32.0, 52.0);
            }
            Step::BarRadius => {
                self.config.bar.corner_radius =
                    (self.config.bar.corner_radius + delta as f32).clamp(0.0, 32.0);
            }
            Step::BarOpacity => {
                self.config.bar.opacity =
                    (self.config.bar.opacity + delta as f32 / 100.0).clamp(0.3, 1.0);
            }
            Step::DockHeight => {
                self.config.dock.height =
                    (self.config.dock.height + delta as f32).clamp(40.0, 80.0);
            }
            Step::DockOpacity => {
                self.config.dock.opacity =
                    (self.config.dock.opacity + delta as f32 / 100.0).clamp(0.3, 1.0);
            }
            Step::OverlayOpacity => {
                self.config.theme.overlay_opacity =
                    (self.config.theme.overlay_opacity + delta as f32 / 100.0).clamp(0.3, 1.0);
            }
            Step::Frost => {
                self.config.theme.frost =
                    (self.config.theme.frost + delta as f32 / 100.0).clamp(0.2, 1.0);
            }
            Step::RadiusLg => {
                if self.config.theme.radius.linked {
                    return;
                }
                self.config.theme.radius.lg =
                    (self.config.theme.radius.lg + delta as f32).clamp(0.0, 48.0);
            }
            Step::RadiusMd => {
                self.config.theme.radius.md =
                    (self.config.theme.radius.md + delta as f32).clamp(0.0, 48.0);
            }
            Step::RadiusSm => {
                if self.config.theme.radius.linked {
                    return;
                }
                self.config.theme.radius.sm =
                    (self.config.theme.radius.sm + delta as f32).clamp(0.0, 48.0);
            }
        }
    }

    fn invalidate_views(&mut self) {
        for bar in self.bars.values_mut() {
            bar.invalidate();
        }
        self.dock.invalidate();
        self.dirty = true;
    }

    // ---- OSD ----

    #[must_use]
    pub fn osd_wants_surface(&self) -> bool {
        // The volume/brightness HUD is rendered inside the bar's centre
        // island, so no dedicated layer surface is needed anymore.
        false
    }

    #[must_use]
    pub fn osd_size(&self) -> Size {
        Osd::preferred_size(&self.config.osd)
    }

    pub fn build_osd(&mut self, scene: &mut Scene, size: Size) {
        self.osd.build(
            scene,
            size,
            &self.config.osd,
            &self.config.theme,
            &self.config.font,
        );
    }

    // ---- toasts ----

    /// Size of the overlay surface that hosts the expanded notification
    /// island. It is a separate layer surface so the island can grow over
    /// the other windows instead of resizing the bar.
    #[must_use]
    pub fn notification_surface_size(&self) -> Option<Size> {
        self.toasts.preview().map(|preview| {
            // A notification with a body needs two lines: the island grows a
            // little taller so neither line is squeezed against the edges.
            let extra = if preview.body.is_empty() { 0.0 } else { 12.0 };
            Size::new(0.0, self.config.bar.height + extra)
        })
    }

    /// Draw the expanded centre island with the incoming notification.
    pub fn build_notification_island(&mut self, scene: &mut Scene, size: Size) {
        let Some(preview) = self.toasts.preview() else {
            self.notification_width.set_target(0.0);
            self.notification_rect = Rect::ZERO;
            return;
        };
        let theme = &self.config.theme;
        let fonts = &self.config.font;
        let bar = &self.config.bar;
        let pad = bar.padding;
        let radius = bar.corner_radius.min(bar.height * 0.5);
        // Two lines (summary + body) get a slightly taller island than the bar
        // so the text is not squeezed at the top and bottom edges.
        let two_lines = !preview.body.is_empty();
        let height = bar.height + if two_lines { 12.0 } else { 0.0 };
        // The notification island is fully opaque: the text has to stay
        // readable over whatever is on the screen.
        let bg = theme.background.with_alpha(1.0);

        let icon_w = 22.0;
        // The advances come from the real font: the island is sized on the
        // real text, so nothing gets cut for a wrong guess.
        const SAFETY: f32 = 1.03;
        let app_w = approx_width(&preview.app, fonts.size_small - 0.5) * SAFETY;
        let summary_w = approx_width(&preview.summary, fonts.size_body) * SAFETY;
        let body_w = approx_width(&preview.body, fonts.size_small - 1.0) * SAFETY;
        let text_w = (app_w + 8.0 + summary_w).max(body_w);
        // The island always reads as a proper panel: it grows with the
        // notification but never shrinks below a comfortable width.
        let target_w = (pad * 2.0 + icon_w + 8.0 + text_w).max(320.0).min(560.0);
        if !self.notification_initialized {
            self.notification_width.snap_to(target_w);
            self.notification_initialized = true;
        } else {
            self.notification_width.set_target(target_w);
        }
        let width = self.notification_width.value();
        let alpha = preview.progress;
        if width < 6.0 || alpha <= 0.01 {
            self.notification_rect = Rect::ZERO;
            return;
        }
        let x = ((size.w - width) * 0.5).max(0.0);
        let rect = Rect::new(x, 0.0, width, height);
        self.notification_rect = rect;
        scene.panel(rect, bg.scale_alpha(alpha), radius, None);
        scene.bordered(
            Rect::new(rect.x + 0.5, 0.5, rect.w - 1.0, rect.h - 1.0),
            Color::TRANSPARENT,
            radius,
            1.0,
            theme.border.scale_alpha(0.7 * alpha),
        );

        // Bell, app name, summary and body, centred as a group.
        let group_w = icon_w + 8.0 + text_w;
        let icon_x = rect.x + (rect.w - group_w) * 0.5;
        let line_y = if two_lines {
            height * 0.5 - 9.0
        } else {
            height * 0.5
        };
        scene.text_center_y(
            icons::NOTIFICATIONS,
            icon_x,
            line_y,
            TextStyle {
                size: fonts.size_body + 1.0,
                align: TextAlign::Center,
                max_width: Some(icon_w),
                ..Default::default()
            }
            .icon(),
            theme.accent.scale_alpha(0.9 * alpha),
        );
        let text_x = icon_x + icon_w + 8.0;
        let summary_max = (rect.right() - pad - text_x).max(40.0);
        // The body sits on its own line: it can use the whole text area.
        let body_max = (rect.right() - pad - text_x).max(40.0);
        scene.text_center_y(
            &preview.app,
            text_x,
            line_y,
            TextStyle {
                size: fonts.size_small - 0.5,
                weight: fonts.weight_semibold,
                align: TextAlign::Left,
                max_width: Some(app_w + 6.0),
                ..Default::default()
            },
            theme.accent.scale_alpha(0.85 * alpha),
        );
        let summary_x = text_x + app_w + 8.0;
        scene.text_center_y(
            truncate_to_width(&preview.summary, fonts.size_body, summary_max),
            summary_x,
            line_y,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_semibold,
                align: TextAlign::Left,
                max_width: Some(summary_max),
                wrap: false,
                ..Default::default()
            },
            theme.text.scale_alpha(alpha),
        );
        if !preview.body.is_empty() {
            scene.text_center_y(
                truncate_to_width(&preview.body, fonts.size_small - 1.0, body_max),
                text_x,
                line_y + if two_lines { 17.0 } else { 15.0 },
                TextStyle {
                    size: fonts.size_small - 1.0,
                    align: TextAlign::Left,
                    max_width: Some(body_max),
                    wrap: false,
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.8 * alpha),
            );
        }
    }

    /// Origin of the morph for a panel opened from an island: the island
    /// rectangle in screen coordinates and its corner radius.
    fn island_origin(&self, island: BarIsland) -> Option<(Rect, f32, BarIsland)> {
        let output = self
            .niri
            .focused_output
            .clone()
            .or_else(|| self.bars.keys().next().cloned())?;
        let rect = self.bar_island_rect(&output, island)?;
        let radius = self
            .config
            .bar
            .corner_radius
            .min(self.config.bar.height * 0.5);
        Some((rect, radius, island))
    }

    /// Island that is currently growing into a panel, with the progress of
    /// the morph. The bar uses it to fade the island away.
    fn morph_state(&self) -> Option<(BarIsland, f32)> {
        if self.launcher.wants_surface() {
            return Some((BarIsland::Left, self.launcher.progress()));
        }
        if self.popover.wants_surface() {
            let island = self.popover.origin_island()?;
            return Some((island, self.popover.progress()));
        }
        if let Some(preview) = self.toasts.preview() {
            // The incoming notification takes over the centre island too.
            return Some((BarIsland::Center, preview.progress));
        }
        None
    }

    /// Screen rectangle of one bar island, used as the origin of the morph
    /// for the panels that open from it.
    pub fn bar_island_rect(&self, output: &str, island: BarIsland) -> Option<Rect> {
        let bar = self.bars.get(output)?;
        let rect = match island {
            // The centre island can be hidden by a notification: its home is
            // still a valid anchor for the panels opened from it.
            BarIsland::Center => bar.center_island_rect(),
            _ => bar.island(island)?,
        };
        // The bar surface is anchored with the margin on the sides.
        Some(Rect::new(
            rect.x + self.config.bar.margin,
            rect.y,
            rect.w,
            rect.h,
        ))
    }

    /// Open the notification center: a panel under the centre island with
    /// the pending notifications and their actions. The notifications
    /// themselves are shown in the island, not in a floating toast.
    pub fn open_notifications_panel(&mut self) {
        let entries = self.toasts.entries();
        let size = Popover::notifications_size(&entries);
        let anchor = self
            .island_origin(BarIsland::Center)
            .map(|(rect, _, _)| rect.x + rect.w * 0.5)
            .unwrap_or(self.config.bar.margin);
        if self.popover.is_open() && self.popover.kind() == PopoverKind::Notifications {
            self.popover.close();
            return;
        }
        let origin = self.island_origin(BarIsland::Center);
        self.popover
            .open(PopoverKind::Notifications, anchor, origin);
        // The card grows with the number of pending notifications.
        self.popover.set_size(size);
        self.control.cancel_password();
        self.dirty = true;
    }

    /// The toasts never float on their own: a notification is shown in the
    /// centre island and the panel is opened on demand from the bell.
    #[must_use]
    pub fn toast_size(&self) -> Option<Size> {
        None
    }

    pub fn build_toasts(&mut self, scene: &mut Scene, size: Size) {
        self.toasts.build(
            scene,
            size,
            &self.config.notifications,
            &self.config.theme,
            &self.config.font,
        );
    }

    pub fn toast_press(&mut self, pos: Point) -> Option<ShellAction> {
        use toasts::ToastPress;
        match self.toasts.pointer_press(pos)? {
            ToastPress::Dismiss(id) => Some(ShellAction::CloseNotification {
                id,
                reason: CloseReason::Dismissed,
            }),
            ToastPress::Action { id, action } => {
                self.actions.push(ShellAction::CloseNotification {
                    id,
                    reason: CloseReason::Dismissed,
                });
                Some(ShellAction::NotificationAction { id, action })
            }
        }
    }

    pub fn toast_move(&mut self, pos: Point) {
        self.toasts.pointer_move(pos);
    }

    pub fn toast_leave(&mut self) {
        self.toasts.pointer_leave();
    }

    #[must_use]
    pub fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    fn force_wifi_bt_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn set_volume(&mut self, percent: u8) {
        self.volume = percent;
        self.dirty = true;
    }
}

fn clock_strings(config: &Config, now: DateTime<Local>) -> (String, String) {
    (
        now.format(&config.bar.clock_format).to_string(),
        italian_date_short(now),
    )
}

/// Small Italian date formatter, matching the locale used by the rest of the
/// UI without pulling in a full localization database.
#[must_use]
pub fn italian_date_short(now: DateTime<Local>) -> String {
    use chrono::Datelike;
    const WEEKDAYS: [&str; 7] = ["lun", "mar", "mer", "gio", "ven", "sab", "dom"];
    const MONTHS: [&str; 12] = [
        "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
    ];
    format!(
        "{} {} {}",
        WEEKDAYS[now.weekday().num_days_from_monday() as usize],
        now.day(),
        MONTHS[now.month0() as usize]
    )
}

#[must_use]
pub fn italian_date_long(now: DateTime<Local>) -> String {
    use chrono::Datelike;
    const WEEKDAYS: [&str; 7] = [
        "lunedì",
        "martedì",
        "mercoledì",
        "giovedì",
        "venerdì",
        "sabato",
        "domenica",
    ];
    const MONTHS: [&str; 12] = [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ];
    format!(
        "{} {} {}",
        WEEKDAYS[now.weekday().num_days_from_monday() as usize],
        now.day(),
        MONTHS[now.month0() as usize]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lard_core::desktop::DesktopEntry;
    use lard_core::theme::DockConfig;
    use lard_render::Scene;
    use std::path::PathBuf;

    fn demo_niri() -> NiriSnapshot {
        NiriSnapshot {
            connected: true,
            workspaces: vec![lard_services::niri::WorkspaceView {
                id: 1,
                idx: 0,
                name: None,
                output: Some("eDP-1".into()),
                is_active: false,
                is_focused: true,
                is_urgent: false,
                has_windows: true,
            }],
            focused_window: Some(lard_services::niri::WindowView {
                id: 1,
                title: "Test".into(),
                app_id: "zen".into(),
            }),
            windows: vec![],
            keyboard_layout: None,
            focused_output: Some("eDP-1".into()),
        }
    }

    #[test]
    fn bar_launcher_button_is_clickable() {
        let config = Config::default();
        let niri = demo_niri();
        let network = NetworkState::default();
        let data = BarData {
            niri: &niri,
            volume: 50,
            muted: false,
            osd: None,
            battery: BatteryState::default(),
            network: &network,
            bluetooth: BluetoothState::default(),
            wifi_enabled: true,
            system: SystemStats::default(),
            clock: "12:00",
            date: "mar 20 ott",
            dnd: false,
            active_toasts: 0,
            toast: None,
            notification_count: 0,
            morph: None,
        };
        let mut bar = Bar::new("eDP-1");
        let mut scene = Scene::new();
        bar.build(
            &mut scene,
            Size::new(1600.0, config.bar.height),
            &data,
            &config,
        );
        // The launcher is not in the bar any more: the first pill of the left
        // island is the workspace.
        let press = bar.press(Point::new(25.0, config.bar.height * 0.5));
        assert_eq!(
            press.and_then(|p| p.action),
            Some(ShellAction::FocusWorkspace(1))
        );
    }

    #[test]
    fn bar_islands_expose_blur_regions() {
        let config = Config::default();
        let mut shell = Shell::new(config.clone());
        shell.niri = demo_niri();
        let mut scene = Scene::new();
        shell.build_bar(&mut scene, "eDP-1", Size::new(1600.0, config.bar.height));
        let regions = shell.bar_blur_regions("eDP-1");
        // Left, centre and right island.
        assert_eq!(regions.len(), 3);
        for (rect, radius) in &regions {
            assert!(rect.w > 1.0 && rect.h > 1.0);
            assert_eq!(*radius, config.bar.corner_radius);
        }
        assert!(shell.bar_blur_regions("missing").is_empty());
    }

    #[test]
    fn controls_popover_anchors_to_the_bar_button() {
        let config = Config::default();
        let mut shell = Shell::new(config.clone());
        shell.niri = demo_niri();
        let mut scene = Scene::new();
        shell.build_bar(&mut scene, "eDP-1", Size::new(1600.0, config.bar.height));
        shell.open_controls_popover();
        assert_eq!(shell.popover_kind(), Some(PopoverKind::ControlCenter));
    }

    #[test]
    fn notification_island_draws_the_preview() {
        let config = Config::default();
        let mut shell = Shell::new(config.clone());
        shell.toasts.push(
            lard_services::notifications::Notification {
                id: 1,
                app_name: "Firefox".into(),
                app_icon: String::new(),
                summary: "Nuova scheda".into(),
                body: "Ciao".into(),
                actions: Vec::new(),
                timeout_ms: 5000,
            },
            &config.notifications,
        );
        // Let the arrival spring settle.
        for _ in 0..30 {
            shell.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        shell.build_notification_island(&mut scene, Size::new(1600.0, config.bar.height));
        let labels: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        assert!(labels.contains(&"Firefox"), "{labels:?}");
        assert!(labels.contains(&"Nuova scheda"), "{labels:?}");
        // The island is centred and wide enough for the notification.
        let rect = shell.notification_rect;
        assert!(rect.w >= 320.0, "{rect:?}");
        assert!(rect.x > 600.0 && rect.right() < 1000.0, "{rect:?}");
    }

    #[test]
    fn notifications_panel_opens_under_the_centre_island() {
        let config = Config::default();
        let mut shell = Shell::new(config.clone());
        shell.niri = demo_niri();
        let mut scene = Scene::new();
        shell.build_bar(&mut scene, "eDP-1", Size::new(1600.0, config.bar.height));
        shell.open_notifications_panel();
        assert_eq!(shell.popover_kind(), Some(PopoverKind::Notifications));
        let center = shell
            .bar_island_rect("eDP-1", BarIsland::Center)
            .expect("centre island");
        let left = shell
            .bar_island_rect("eDP-1", BarIsland::Left)
            .expect("left island");
        assert!(center.x > left.x + 100.0, "center {center:?} left {left:?}");
        assert_eq!(
            shell.popover_anchor_x(),
            center.x + center.w * 0.5,
            "the panel is centred under the island"
        );

        // Even when a notification takes the island over, the panel is
        // anchored under it.
        shell.toasts.push(
            lard_services::notifications::Notification {
                id: 7,
                app_name: "Firefox".into(),
                app_icon: String::new(),
                summary: "Ciao".into(),
                body: String::new(),
                actions: Vec::new(),
                timeout_ms: 5000,
            },
            &config.notifications,
        );
        for _ in 0..30 {
            shell.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        shell.build_bar(&mut scene, "eDP-1", Size::new(1600.0, config.bar.height));
        let center = shell
            .bar_island_rect("eDP-1", BarIsland::Center)
            .expect("centre island");
        assert!(center.x > left.x + 100.0, "center {center:?} left {left:?}");
        // Let the open animation settle.
        for _ in 0..30 {
            shell.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        shell.build_popover(&mut scene, Size::new(1600.0, 900.0), 600.0);
        let labels: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        assert!(labels.contains(&"Notifiche"), "{labels:?}");
    }

    #[test]
    fn island_radius_setting_is_clamped() {
        let mut shell = Shell::new(Config::default());
        shell.apply_setting_step(Step::BarRadius, -100);
        assert_eq!(shell.config.bar.corner_radius, 0.0);
        shell.apply_setting_step(Step::BarRadius, 100);
        assert_eq!(shell.config.bar.corner_radius, 32.0);
        shell.apply_setting_step(Step::BarRadius, -2);
        assert_eq!(shell.config.bar.corner_radius, 30.0);
    }

    #[test]
    fn blur_and_opacity_settings_are_clamped() {
        let mut shell = Shell::new(Config::default());
        shell.apply_setting_step(Step::OverlayOpacity, -100);
        assert_eq!(shell.config.theme.overlay_opacity, 0.3);
        shell.apply_setting_step(Step::OverlayOpacity, 100);
        assert_eq!(shell.config.theme.overlay_opacity, 1.0);

        shell.apply_setting_step(Step::Frost, -100);
        assert_eq!(shell.config.theme.frost, 0.2);
        shell.apply_setting_step(Step::Frost, 100);
        assert_eq!(shell.config.theme.frost, 1.0);

        shell.apply_setting_step(Step::DockOpacity, -100);
        assert_eq!(shell.config.dock.opacity, 0.3);
        shell.apply_setting_step(Step::DockOpacity, 100);
        assert_eq!(shell.config.dock.opacity, 1.0);

        shell.apply_setting_step(Step::BarOpacity, -100);
        assert_eq!(shell.config.bar.opacity, 0.3);
        shell.apply_setting_step(Step::BarOpacity, 100);
        assert_eq!(shell.config.bar.opacity, 1.0);

        let before = shell.config.theme.island_blur;
        shell.apply_setting_toggle(Toggle::IslandBlur);
        assert_ne!(shell.config.theme.island_blur, before);
    }

    #[test]
    fn bar_osd_takes_over_the_centre_island() {
        let config = Config::default();
        let niri = demo_niri();
        let network = NetworkState::default();
        let data = BarData {
            niri: &niri,
            volume: 42,
            muted: false,
            osd: Some(crate::osd::OsdState {
                progress: 1.0,
                value: 42,
                muted: false,
                brightness: false,
            }),
            battery: BatteryState::default(),
            network: &network,
            bluetooth: BluetoothState::default(),
            wifi_enabled: true,
            system: SystemStats::default(),
            clock: "12:00",
            date: "mar 20 ott",
            dnd: false,
            active_toasts: 0,
            toast: None,
            notification_count: 0,
            morph: None,
        };
        let mut bar = Bar::new("eDP-1");
        let mut scene = Scene::new();
        bar.build(
            &mut scene,
            Size::new(1600.0, config.bar.height),
            &data,
            &config,
        );
        let texts: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        assert!(texts.contains(&"VOL"), "expected volume label: {texts:?}");
        assert!(texts.contains(&"42%"), "expected volume value: {texts:?}");
        assert!(
            !texts.contains(&"12:00"),
            "clock must be hidden while the OSD is up: {texts:?}"
        );
        // The centre island must not open the clock popover while busy.
        let press = bar.press(Point::new(800.0, config.bar.height * 0.5));
        assert!(press.is_none(), "OSD slot should not be clickable");
    }

    #[test]
    fn dock_tile_is_clickable() {
        let mut config = Config::default();
        config.dock = DockConfig {
            enabled: true,
            ..DockConfig::default()
        };
        let apps = vec![DesktopEntry {
            file: PathBuf::from("/usr/share/applications/zen.desktop"),
            name: "Zen Browser".into(),
            exec: "zen-browser".into(),
            ..DesktopEntry::default()
        }];
        let niri = demo_niri();
        let mut dock = Dock::new();
        dock.sync(&apps, &niri, &config.dock, &config.theme);
        let size = dock.size().expect("dock should have one tile");
        let mut scene = Scene::new();
        let mut icons = AppIcons::new();
        dock.build(
            &mut scene,
            size,
            &config.theme,
            &config.font,
            config.dock.opacity,
            &mut icons,
        );
        let action = dock.press(Point::new(size.w * 0.5, size.h * 0.5));
        assert!(matches!(action, Some(ShellAction::Spawn(_))));
    }
}
