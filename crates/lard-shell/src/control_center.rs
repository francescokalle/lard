//! Quick settings panel, sliding from the right with a scrim.
//!
//! The home page exposes the quick toggles. Wi-Fi, Bluetooth and power open
//! drill-down pages inside the same panel, so every tile is actually usable
//! instead of being a state-only switch.

use crate::widgets::{approx_width, truncate};
use crate::{ShellAction, ShellKey, icons};
use lard_core::anim::{Spring, SpringConfig};
use lard_core::{Color, Point, Rect, Size};
use lard_render::{Scene, Shadow, TextAlign, TextStyle};
use lard_services::battery::BatteryState;
use lard_services::bluetooth::{BluetoothDevice, BluetoothState};
use lard_services::network::{NetworkState, WifiNetwork};
use lard_services::power::PowerAction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlPage {
    Home,
    Wifi,
    Bluetooth,
    Power,
}

/// Result of interacting with a control-center page that is embedded in an
/// expandable bar popover instead of the side panel.
#[derive(Clone, Debug, PartialEq)]
pub enum EmbeddedCommand {
    /// Click outside the embedded card: close the whole popover.
    Close,
    /// Back arrow without an active password prompt: collapse the card back
    /// to its compact summary.
    Back,
    /// The page handled the click; the wrapped action (if any) still needs to
    /// be dispatched by the shell.
    Action(Option<ShellAction>),
}

/// Which field of the Wi-Fi prompt has the keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordField {
    Identity,
    Secret,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Close,
    WifiTile,
    BluetoothTile,
    Dnd,
    Battery,
    Mute,
    Volume,
    Brightness,
    Lock,
    PowerEntry,
    Back,
    WifiToggle(bool),
    WifiScan,
    WifiDisconnect,
    WifiNetwork(usize),
    WifiCancel,
    WifiSubmit,
    /// Focus the 802.1X user-name field / the secret field.
    WifiField(PasswordField),
    BtToggle(bool),
    BtScan,
    BtDevice(usize),
    Power(PowerAction),
}

pub struct ControlData<'a> {
    pub volume: u8,
    pub muted: bool,
    pub brightness: u8,
    pub brightness_present: bool,
    pub user: &'a str,
    pub output: Option<&'a str>,
    pub keyboard_layout: Option<&'a str>,
    pub niri_connected: bool,
    pub battery: BatteryState,
    pub network: &'a NetworkState,
    pub bluetooth: BluetoothState,
    pub dnd: bool,
    pub wifi_enabled: bool,
    pub wifi_networks: &'a [WifiNetwork],
    pub wifi_scanning: bool,
    pub wifi_error: Option<&'a str>,
    pub wifi_connecting: Option<&'a str>,
    pub bluetooth_devices: &'a [BluetoothDevice],
    pub bt_scanning: bool,
    pub bt_error: Option<&'a str>,
    pub bt_connecting: Option<&'a str>,
    pub clock: &'a str,
    pub date: &'a str,
}

pub struct ControlCenter {
    open: bool,
    progress: Spring,
    page: ControlPage,
    page_anim: Spring,
    hits: Vec<(Rect, Hit)>,
    panel: Rect,
    track: Rect,
    brightness_track: Rect,
    cursor: Option<Point>,
    reduce_motion: bool,
    password_ssid: Option<String>,
    password_enterprise: bool,
    password_identity: String,
    password_focus: PasswordField,
    password_input: String,
    network_rows: Vec<WifiNetwork>,
    device_rows: Vec<BluetoothDevice>,
    wifi_scroll: usize,
    bt_scroll: usize,
    wifi_visible: usize,
    bt_visible: usize,
}

impl ControlCenter {
    /// Bounds of the control-center panel, used for the backdrop blur region.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        Self {
            open: false,
            progress: Spring::new(0.0, animation),
            page: ControlPage::Home,
            page_anim: Spring::new(1.0, animation),
            hits: Vec::with_capacity(16),
            panel: Rect::ZERO,
            track: Rect::ZERO,
            brightness_track: Rect::ZERO,
            cursor: None,
            reduce_motion,
            password_ssid: None,
            password_enterprise: false,
            password_identity: String::new(),
            password_focus: PasswordField::Secret,
            password_input: String::new(),
            network_rows: Vec::new(),
            device_rows: Vec::new(),
            wifi_scroll: 0,
            bt_scroll: 0,
            wifi_visible: 0,
            bt_visible: 0,
        }
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    #[must_use]
    pub fn page(&self) -> ControlPage {
        self.page
    }

    #[must_use]
    pub fn wants_surface(&self) -> bool {
        self.open || self.progress.value() > 0.01
    }

    pub fn open(&mut self) {
        self.open = true;
        if self.reduce_motion {
            self.progress.snap_to(1.0);
        } else {
            self.progress.set_target(1.0);
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.cancel_password();
        if self.reduce_motion {
            self.progress.snap_to(0.0);
        } else {
            self.progress.set_target(0.0);
        }
    }

    /// Switch the visible page. When the panel is already open the new page
    /// fades in; when it is just opening, the panel's own spring is enough.
    pub fn show_page(&mut self, page: ControlPage) {
        if self.page == page {
            return;
        }
        self.page = page;
        self.cancel_password();
        self.wifi_scroll = 0;
        self.bt_scroll = 0;
        if self.reduce_motion || !self.open || self.progress.value() < 0.2 {
            self.page_anim.snap_to(1.0);
        } else {
            self.page_anim.snap_to(0.0);
            self.page_anim.set_target(1.0);
        }
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let mut active = self.progress.update(dt);
        active |= self.page_anim.update(dt);
        active
    }

    pub fn pointer_move(&mut self, pos: Point) {
        self.cursor = Some(pos);
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
    }

    /// Scroll a long Wi-Fi or Bluetooth list. Returns whether the offset
    /// changed so the caller can request a redraw.
    fn scroll_page(&mut self, page: ControlPage, delta: f32) -> bool {
        if delta == 0.0 {
            return false;
        }
        let (length, visible, scroll) = match page {
            ControlPage::Wifi => (
                self.network_rows.len(),
                self.wifi_visible,
                &mut self.wifi_scroll,
            ),
            ControlPage::Bluetooth => {
                (self.device_rows.len(), self.bt_visible, &mut self.bt_scroll)
            }
            _ => return false,
        };
        let max = length.saturating_sub(visible);
        if max == 0 {
            return false;
        }
        let previous = *scroll;
        *scroll = if delta > 0.0 {
            (*scroll + 2).min(max)
        } else {
            (*scroll).saturating_sub(2)
        };
        *scroll != previous
    }

    pub fn axis(&mut self, delta: f32) -> bool {
        if !self.open {
            return false;
        }
        self.scroll_page(self.page, delta)
    }

    /// Scroll an embedded list. The popover is not the side-panel `open`
    /// state, so it cannot use `axis` directly.
    pub fn embedded_axis(&mut self, page: ControlPage, delta: f32) -> bool {
        self.scroll_page(page, delta)
    }

    fn hit_at(&self, pos: Point) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, hit)| *hit)
    }

    fn activate_hit(&mut self, hit: Hit, pos: Point) -> Option<ShellAction> {
        match hit {
            Hit::Close | Hit::Back => None,
            Hit::WifiTile => {
                self.show_page(ControlPage::Wifi);
                Some(ShellAction::ScanWifi)
            }
            Hit::BluetoothTile => {
                self.show_page(ControlPage::Bluetooth);
                Some(ShellAction::RefreshBluetooth)
            }
            Hit::Battery => None,
            Hit::Dnd => Some(ShellAction::ToggleDnd),
            Hit::Mute => Some(ShellAction::ToggleMute),
            Hit::Volume => {
                let t = ((pos.x - self.track.x) / self.track.w).clamp(0.0, 1.0);
                Some(ShellAction::SetVolume((t * 100.0).round() as u8))
            }
            Hit::Brightness => {
                let t =
                    ((pos.x - self.brightness_track.x) / self.brightness_track.w).clamp(0.0, 1.0);
                Some(ShellAction::SetBrightness((t * 100.0).round() as u8))
            }
            Hit::Lock => Some(ShellAction::Power(PowerAction::Lock)),
            Hit::PowerEntry => {
                self.show_page(ControlPage::Power);
                None
            }
            Hit::WifiToggle(enabled) => Some(ShellAction::SetWifi(enabled)),
            Hit::WifiScan => Some(ShellAction::ScanWifi),
            Hit::WifiDisconnect => Some(ShellAction::DisconnectWifi),
            Hit::WifiCancel => {
                self.cancel_password();
                None
            }
            Hit::WifiSubmit => self.submit_password(),
            Hit::WifiField(field) => {
                self.password_focus = field;
                None
            }
            Hit::WifiNetwork(index) => {
                let network = self.network_rows.get(index).cloned()?;
                if network.secured && !network.saved {
                    self.password_ssid = Some(network.ssid);
                    self.password_enterprise = network.enterprise;
                    self.password_identity.clear();
                    self.password_input.clear();
                    self.password_focus = if network.enterprise {
                        PasswordField::Identity
                    } else {
                        PasswordField::Secret
                    };
                    None
                } else {
                    Some(ShellAction::ConnectWifi {
                        ssid: network.ssid,
                        password: None,
                        identity: None,
                    })
                }
            }
            Hit::BtToggle(enabled) => Some(ShellAction::SetBluetooth(enabled)),
            Hit::BtScan => Some(ShellAction::ScanBluetooth),
            Hit::BtDevice(index) => {
                let device = self.device_rows.get(index).cloned()?;
                if device.connected {
                    Some(ShellAction::DisconnectBluetooth {
                        address: device.address,
                    })
                } else {
                    Some(ShellAction::ConnectBluetooth {
                        address: device.address,
                    })
                }
            }
            Hit::Power(action) => Some(ShellAction::Power(action)),
        }
    }

    pub fn pointer_press(&mut self, pos: Point) -> Option<ShellAction> {
        match self.hit_at(pos) {
            Some(Hit::Close) | None => {
                if !self.panel.contains(pos) {
                    self.close();
                }
                None
            }
            Some(Hit::Back) => {
                if self.password_ssid.is_some() {
                    self.cancel_password();
                } else {
                    self.show_page(ControlPage::Home);
                }
                None
            }
            Some(hit) => self.activate_hit(hit, pos),
        }
    }

    /// Render only a drill-down page, inside a caller-provided card. The
    /// expandable bar popover uses this instead of opening the side panel.
    pub fn build_embedded(
        &mut self,
        scene: &mut Scene,
        page: ControlPage,
        panel: Rect,
        data: &ControlData<'_>,
        config: &lard_core::Config,
        p: f32,
    ) {
        if !matches!(page, ControlPage::Wifi | ControlPage::Bluetooth) {
            return;
        }
        self.hits.clear();
        let pad = 18.0;
        let content_x = panel.x + pad;
        let content_w = panel.w - pad * 2.0;
        match page {
            ControlPage::Wifi => {
                self.page_header(scene, panel, content_x, p, "Wi-Fi", config);
                self.build_wifi(scene, panel, content_x, content_w, data, config, p);
            }
            ControlPage::Bluetooth => {
                self.page_header(scene, panel, content_x, p, "Bluetooth", config);
                self.build_bluetooth(scene, panel, content_x, content_w, data, config, p);
            }
            ControlPage::Home | ControlPage::Power => {}
        }
    }

    /// Pointer input for an embedded page. Outside clicks close the popover,
    /// the back arrow collapses it, and everything else is handled exactly as
    /// in the side panel.
    pub fn embedded_press(&mut self, pos: Point, panel: Rect) -> Option<EmbeddedCommand> {
        match self.hit_at(pos) {
            Some(Hit::Back) => {
                if self.password_ssid.is_some() {
                    self.cancel_password();
                    Some(EmbeddedCommand::Action(None))
                } else {
                    Some(EmbeddedCommand::Back)
                }
            }
            Some(Hit::Close) => Some(EmbeddedCommand::Close),
            Some(hit) => Some(EmbeddedCommand::Action(self.activate_hit(hit, pos))),
            None if !panel.contains(pos) => Some(EmbeddedCommand::Close),
            None => Some(EmbeddedCommand::Action(None)),
        }
    }

    /// Keyboard input for an embedded Wi-Fi/Bluetooth page. The popover is
    /// keyboard-exclusive, so text typed into the password field arrives here.
    pub fn embedded_key(&mut self, page: ControlPage, key: ShellKey) -> Option<EmbeddedCommand> {
        if self.password_ssid.is_some() {
            return Some(EmbeddedCommand::Action(self.password_key(key)));
        }
        match key {
            ShellKey::Escape | ShellKey::Left => Some(EmbeddedCommand::Back),
            ShellKey::Up => {
                self.scroll_page(page, -1.0);
                Some(EmbeddedCommand::Action(None))
            }
            ShellKey::Down => {
                self.scroll_page(page, 1.0);
                Some(EmbeddedCommand::Action(None))
            }
            _ => None,
        }
    }

    pub fn key(&mut self, key: ShellKey) -> Option<ShellAction> {
        if self.password_ssid.is_some() {
            return self.password_key(key);
        }
        match key {
            ShellKey::Escape => {
                if self.page == ControlPage::Home {
                    self.close();
                } else {
                    self.show_page(ControlPage::Home);
                }
                None
            }
            ShellKey::Up => {
                self.axis(-1.0);
                None
            }
            ShellKey::Down => {
                self.axis(1.0);
                None
            }
            ShellKey::Left => {
                if self.page != ControlPage::Home {
                    self.show_page(ControlPage::Home);
                }
                None
            }
            _ => None,
        }
    }

    /// Buffer of the field that currently has the keyboard focus.
    fn focused_buffer(&mut self) -> &mut String {
        if self.password_enterprise && self.password_focus == PasswordField::Identity {
            &mut self.password_identity
        } else {
            &mut self.password_input
        }
    }

    /// Keyboard input for the Wi-Fi prompt (PSK or 802.1X).
    fn password_key(&mut self, key: ShellKey) -> Option<ShellAction> {
        match key {
            ShellKey::Escape => {
                self.cancel_password();
                None
            }
            ShellKey::Backspace => {
                self.focused_buffer().pop();
                None
            }
            ShellKey::Char(text) => {
                let limit =
                    if self.password_enterprise && self.password_focus == PasswordField::Identity {
                        96
                    } else {
                        64
                    };
                let buffer = self.focused_buffer();
                if buffer.chars().count() < limit {
                    buffer.push_str(&text);
                }
                None
            }
            ShellKey::Tab => {
                if self.password_enterprise {
                    self.password_focus = match self.password_focus {
                        PasswordField::Identity => PasswordField::Secret,
                        PasswordField::Secret => PasswordField::Identity,
                    };
                }
                None
            }
            ShellKey::Up if self.password_enterprise => {
                self.password_focus = PasswordField::Identity;
                None
            }
            ShellKey::Down if self.password_enterprise => {
                self.password_focus = PasswordField::Secret;
                None
            }
            ShellKey::Enter => self.submit_password(),
            _ => None,
        }
    }

    fn submit_password(&mut self) -> Option<ShellAction> {
        let ssid = self.password_ssid.clone()?;
        let identity = if self.password_enterprise {
            let identity = self.password_identity.trim().to_string();
            if identity.is_empty() {
                self.password_focus = PasswordField::Identity;
                return None;
            }
            Some(identity)
        } else {
            None
        };
        if self.password_input.is_empty() {
            self.password_focus = PasswordField::Secret;
            return None;
        }
        let password = std::mem::take(&mut self.password_input);
        self.password_ssid = None;
        self.password_enterprise = false;
        self.password_identity.clear();
        Some(ShellAction::ConnectWifi {
            ssid,
            password: Some(password),
            identity,
        })
    }

    pub(crate) fn cancel_password(&mut self) {
        self.password_ssid = None;
        self.password_enterprise = false;
        self.password_identity.clear();
        self.password_input.clear();
        self.password_focus = PasswordField::Secret;
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        data: &ControlData<'_>,
        config: &lard_core::Config,
    ) {
        let theme = &config.theme;
        let p = self.progress.value().clamp(0.0, 1.0);
        if p <= 0.001 {
            return;
        }
        self.hits.clear();

        scene.rect(
            Rect::new(0.0, 0.0, size.w, size.h),
            theme.background.with_alpha(0.14 * p),
            0.0,
        );
        self.hits
            .push((Rect::new(0.0, 0.0, size.w, size.h), Hit::Close));

        let width = 374.0_f32.min(size.w - 24.0);
        let margin = 14.0;
        let x = size.w - width - margin + (1.0 - p) * (width + margin);
        let panel = Rect::new(x, margin, width, size.h - margin * 2.0);
        let panel = panel.scale_centered(0.96 + 0.04 * p);
        self.panel = panel;

        scene.shadow(
            panel,
            theme.radius.panel(),
            Shadow {
                color: theme.shadow.scale_alpha(0.75 * p),
                blur: (theme.shadow_blur * 1.4).clamp(8.0, 64.0),
                offset_y: theme.shadow_offset_y.clamp(4.0, 28.0),
                spread: 2.0,
            },
        );
        scene.bordered(
            panel,
            theme
                .surface
                .with_alpha(crate::widgets::panel_alpha(config, 0.62) * p),
            theme.radius.panel(),
            theme.border_width,
            theme.border.scale_alpha(p),
        );

        let pad = 18.0;
        let content_x = panel.x + pad;
        let content_w = panel.w - pad * 2.0;
        let content_p = p * self.page_anim.value().clamp(0.0, 1.0);

        match self.page {
            ControlPage::Home => {
                self.build_home(
                    scene, size, panel, content_x, content_w, data, config, content_p,
                );
            }
            ControlPage::Wifi => {
                self.page_header(scene, panel, content_x, content_p, "Wi-Fi", config);
                self.build_wifi(scene, panel, content_x, content_w, data, config, content_p);
            }
            ControlPage::Bluetooth => {
                self.page_header(scene, panel, content_x, content_p, "Bluetooth", config);
                self.build_bluetooth(scene, panel, content_x, content_w, data, config, content_p);
            }
            ControlPage::Power => {
                self.page_header(scene, panel, content_x, content_p, "Alimentazione", config);
                self.build_power(scene, panel, content_x, content_w, data, config, content_p);
            }
        }
    }

    fn build_home(
        &mut self,
        scene: &mut Scene,
        size: Size,
        panel: Rect,
        content_x: f32,
        content_w: f32,
        data: &ControlData<'_>,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;

        scene.text(
            "Controlli",
            Point::new(content_x, panel.y + 18.0),
            TextStyle {
                size: fonts.size_title,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            data.date,
            Point::new(content_x, panel.y + 40.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );
        scene.text(
            data.clock,
            Point::new(content_x, panel.y + 16.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                align: TextAlign::Right,
                max_width: Some(content_w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.85 * p),
        );

        // 2x2 tiles. Wi-Fi and Bluetooth drill down into their own pages.
        let gap = 10.0;
        let tile_w = (content_w - gap) * 0.5;
        let tile_h = 74.0;
        let battery_icon = if data.battery.charging {
            icons::BATTERY_CHARGING
        } else if data.battery.percent <= 12 {
            icons::BATTERY_ALERT
        } else {
            icons::BATTERY_FULL
        };
        let mut tiles = [
            (
                if data.wifi_enabled {
                    icons::WIFI
                } else {
                    icons::WIFI_OFF
                },
                "Wi-Fi".to_string(),
                if data.wifi_enabled {
                    truncate(&data.network.label(), 18)
                } else {
                    "Spento".into()
                },
                data.wifi_enabled,
                Hit::WifiTile,
            ),
            (
                if data.bluetooth.powered {
                    icons::BLUETOOTH
                } else {
                    icons::BLUETOOTH_OFF
                },
                "Bluetooth".to_string(),
                if data.bluetooth.present {
                    if data.bluetooth.powered {
                        "Attivo".into()
                    } else {
                        "Spento".into()
                    }
                } else {
                    "Assente".into()
                },
                data.bluetooth.powered,
                Hit::BluetoothTile,
            ),
            (
                icons::DND,
                "Non disturbare".to_string(),
                if data.dnd {
                    "Attivo".into()
                } else {
                    "Spento".into()
                },
                data.dnd,
                Hit::Dnd,
            ),
            (
                battery_icon,
                "Batteria".to_string(),
                if data.battery.present {
                    if data.battery.charging {
                        format!("{}% · in carica", data.battery.percent)
                    } else {
                        format!("{}%", data.battery.percent)
                    }
                } else {
                    "Assente".into()
                },
                data.battery.charging,
                Hit::Battery,
            ),
        ];

        for (index, (icon, title, subtitle, active, hit)) in tiles.iter_mut().enumerate() {
            let col = index % 2;
            let row = index / 2;
            let rect = Rect::new(
                content_x + col as f32 * (tile_w + gap),
                panel.y + 68.0 + row as f32 * (tile_h + gap),
                tile_w,
                tile_h,
            );
            let hovered = self.cursor.is_some_and(|c| rect.contains(c));
            let bg = if *active {
                theme
                    .accent
                    .with_alpha(if hovered { 0.24 } else { 0.16 } * p)
            } else {
                theme
                    .background
                    .with_alpha(if hovered { 0.82 } else { 0.55 } * p)
            };
            scene.rect(rect, bg, theme.radius.card());
            if *active {
                scene.bordered(
                    rect,
                    Color::TRANSPARENT,
                    theme.radius.card(),
                    1.0,
                    theme.accent.scale_alpha(0.45 * p),
                );
            }
            let title_color = theme.text;
            let subtitle_color = if *active {
                theme.text.scale_alpha(0.72)
            } else {
                theme.text_muted.scale_alpha(0.75)
            };
            scene.text_center_y(
                icon,
                rect.x + 12.0,
                rect.y + 24.0,
                TextStyle {
                    size: 20.0,
                    align: TextAlign::Center,
                    max_width: Some(24.0),
                    ..Default::default()
                }
                .icon(),
                if *active {
                    theme.text.scale_alpha(p)
                } else {
                    theme.text_muted.scale_alpha(0.8 * p)
                },
            );
            scene.text(
                title.as_str(),
                Point::new(rect.x + 42.0, rect.y + 13.0),
                TextStyle {
                    size: fonts.size_body,
                    weight: fonts.weight_medium,
                    ..Default::default()
                },
                title_color.scale_alpha(p),
            );
            scene.text(
                truncate(subtitle, 24),
                Point::new(rect.x + 42.0, rect.y + 35.0),
                TextStyle {
                    size: fonts.size_small - 0.5,
                    ..Default::default()
                },
                subtitle_color.scale_alpha(p),
            );

            if matches!(*hit, Hit::WifiTile | Hit::BluetoothTile) {
                scene.text_center_y(
                    icons::CHEVRON_RIGHT,
                    rect.right() - 30.0,
                    rect.center().y,
                    TextStyle {
                        size: 14.0,
                        align: TextAlign::Center,
                        max_width: Some(18.0),
                        ..Default::default()
                    }
                    .icon(),
                    theme.text_muted.scale_alpha(0.55 * p),
                );
            } else {
                scene.rect(
                    Rect::new(rect.right() - 16.0, rect.y + 14.0, 6.0, 6.0),
                    if *active {
                        theme.accent.scale_alpha(p)
                    } else {
                        theme.text_muted.scale_alpha(0.35 * p)
                    },
                    3.0,
                );
            }
            if !matches!(*hit, Hit::Battery) {
                self.hits.push((rect, *hit));
            }
        }

        // Volume.
        let volume_y = panel.y + 68.0 + 2.0 * (tile_h + gap) + 8.0;
        scene.text_center_y(
            icons::volume_icon(data.volume, data.muted),
            content_x,
            volume_y + 7.0,
            TextStyle {
                size: 17.0,
                align: TextAlign::Center,
                max_width: Some(22.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.9 * p),
        );
        scene.text(
            "Volume",
            Point::new(content_x + 26.0, volume_y),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            format!("{}%", data.volume),
            Point::new(content_x, volume_y),
            TextStyle {
                size: fonts.size_body,
                align: TextAlign::Right,
                max_width: Some(content_w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(p),
        );
        let track = Rect::new(content_x, volume_y + 28.0, content_w, 8.0);
        self.track = track;
        scene.rect(track, theme.background.scale_alpha(0.75 * p), 4.0);
        let fill = if data.muted {
            0.0
        } else {
            track.w * f32::from(data.volume.min(100)) / 100.0
        };
        if fill > 0.0 {
            scene.rect(
                Rect::new(track.x, track.y, fill, track.h),
                theme.accent.scale_alpha(p),
                4.0,
            );
        }
        let knob_x = track.x + fill.clamp(6.0, track.w - 6.0);
        scene.rect(
            Rect::new(knob_x - 6.0, track.center().y - 6.0, 12.0, 12.0),
            theme.text.scale_alpha(p),
            6.0,
        );
        self.hits.push((track.inset_xy(0.0, -10.0), Hit::Volume));

        let mute_rect = Rect::new(content_x, track.bottom() + 12.0, 120.0, 30.0);
        let hovered = self.cursor.is_some_and(|c| mute_rect.contains(c));
        scene.rect(
            mute_rect,
            if data.muted {
                theme.danger.scale_alpha(0.18 * p)
            } else {
                theme
                    .background
                    .scale_alpha(if hovered { 0.9 } else { 0.55 } * p)
            },
            10.0,
        );
        scene.text_center_y(
            icons::volume_icon(data.volume, data.muted),
            mute_rect.x + 10.0,
            mute_rect.center().y,
            TextStyle {
                size: 15.0,
                align: TextAlign::Center,
                max_width: Some(20.0),
                ..Default::default()
            }
            .icon(),
            if data.muted {
                theme.danger.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.85 * p)
            },
        );
        scene.text_center_y(
            if data.muted {
                "Riattiva audio"
            } else {
                "Disattiva audio"
            },
            mute_rect.x + 34.0,
            mute_rect.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        self.hits.push((mute_rect, Hit::Mute));

        // Brightness (only when a backlight is actually present).
        let mut session_y = mute_rect.bottom() + 28.0;
        if data.brightness_present {
            let brightness_y = mute_rect.bottom() + 26.0;
            scene.text_center_y(
                icons::BRIGHTNESS,
                content_x + 2.0,
                brightness_y + 7.0,
                TextStyle {
                    size: 17.0,
                    align: TextAlign::Center,
                    max_width: Some(22.0),
                    ..Default::default()
                }
                .icon(),
                theme.text_muted.scale_alpha(0.9 * p),
            );
            scene.text(
                "Luminosità",
                Point::new(content_x + 26.0, brightness_y),
                TextStyle {
                    size: fonts.size_body,
                    weight: fonts.weight_medium,
                    ..Default::default()
                },
                theme.text.scale_alpha(p),
            );
            scene.text(
                format!("{}%", data.brightness),
                Point::new(content_x, brightness_y),
                TextStyle {
                    size: fonts.size_body,
                    align: TextAlign::Right,
                    max_width: Some(content_w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(p),
            );
            let brightness_track = Rect::new(content_x, brightness_y + 28.0, content_w, 8.0);
            self.brightness_track = brightness_track;
            scene.rect(
                brightness_track,
                theme.background.scale_alpha(0.75 * p),
                4.0,
            );
            let brightness_fill = brightness_track.w * f32::from(data.brightness.min(100)) / 100.0;
            if brightness_fill > 0.0 {
                scene.rect(
                    Rect::new(
                        brightness_track.x,
                        brightness_track.y,
                        brightness_fill,
                        brightness_track.h,
                    ),
                    theme.accent.scale_alpha(p),
                    4.0,
                );
            }
            let brightness_knob =
                brightness_track.x + brightness_fill.clamp(6.0, brightness_track.w - 6.0);
            scene.rect(
                Rect::new(
                    brightness_knob - 6.0,
                    brightness_track.center().y - 6.0,
                    12.0,
                    12.0,
                ),
                theme.text.scale_alpha(p),
                6.0,
            );
            self.hits
                .push((brightness_track.inset_xy(0.0, -10.0), Hit::Brightness));

            session_y = brightness_track.bottom() + 28.0;
        }

        // Session card.
        let session = Rect::new(content_x, session_y, content_w, 76.0);
        scene.rect(
            session,
            theme.background.scale_alpha(0.55 * p),
            theme.radius.card(),
        );
        let initial = data
            .user
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        let avatar = Rect::new(session.x + 14.0, session.y + 21.0, 34.0, 34.0);
        scene.rect(avatar, theme.accent.with_alpha(0.20 * p), avatar.w * 0.5);
        scene.text_center_y(
            initial,
            avatar.x,
            avatar.center().y,
            TextStyle {
                size: 14.0,
                weight: fonts.weight_semibold,
                align: TextAlign::Center,
                max_width: Some(avatar.w),
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            "Sessione",
            Point::new(avatar.right() + 12.0, session.y + 17.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );
        scene.text(
            format!("@{}", data.user),
            Point::new(avatar.right() + 12.0, session.y + 34.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        let lock_button = Rect::new(session.right() - 102.0, session.y + 23.0, 88.0, 30.0);
        let lock_hover = self.cursor.is_some_and(|c| lock_button.contains(c));
        scene.rect(
            lock_button,
            theme
                .accent
                .with_alpha(if lock_hover { 0.25 } else { 0.15 } * p),
            10.0,
        );
        scene.text_center_y(
            "Blocca",
            lock_button.x,
            lock_button.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(lock_button.w),
                ..Default::default()
            },
            theme.accent.scale_alpha(p),
        );
        self.hits.push((lock_button, Hit::Lock));

        // System summary (hide on very short outputs to avoid overlapping
        // the bottom power entry).
        if size.h >= 700.0 {
            let system_y = session.bottom() + 16.0;
            let system = Rect::new(content_x, system_y, content_w, 88.0);
            scene.rect(
                system,
                theme.background.scale_alpha(0.45 * p),
                theme.radius.card(),
            );
            scene.text(
                "Sistema",
                Point::new(system.x + 14.0, system.y + 12.0),
                TextStyle {
                    size: fonts.size_small - 0.5,
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.65 * p),
            );
            let rows = [
                ("Output", data.output.unwrap_or("—").to_string()),
                (
                    "Layout",
                    data.keyboard_layout
                        .map(|layout| layout.to_uppercase())
                        .unwrap_or_else(|| "—".into()),
                ),
                (
                    "Compositor",
                    if data.niri_connected {
                        "niri · connesso".into()
                    } else {
                        "niri · assente".into()
                    },
                ),
            ];
            for (index, (label, value)) in rows.iter().enumerate() {
                let y = system.y + 34.0 + index as f32 * 17.0;
                scene.text(
                    *label,
                    Point::new(system.x + 14.0, y),
                    TextStyle {
                        size: 9.5,
                        ..Default::default()
                    },
                    theme.text_muted.scale_alpha(0.6 * p),
                );
                scene.text(
                    value,
                    Point::new(system.x + 14.0, y),
                    TextStyle {
                        size: 10.5,
                        weight: fonts.weight_medium,
                        align: TextAlign::Right,
                        max_width: Some(content_w - 28.0),
                        ..Default::default()
                    },
                    theme.text.scale_alpha(0.85 * p),
                );
            }
        }

        // A single, obvious entry point to the power menu.
        let power = Rect::new(content_x, panel.bottom() - 58.0, content_w, 44.0);
        let hovered = self.cursor.is_some_and(|c| power.contains(c));
        scene.rect(
            power,
            theme
                .background
                .scale_alpha(if hovered { 0.82 } else { 0.52 } * p),
            theme.radius.card(),
        );
        scene.text_center_y(
            icons::POWER,
            power.x + 16.0,
            power.center().y,
            TextStyle {
                size: 18.0,
                align: TextAlign::Center,
                max_width: Some(22.0),
                ..Default::default()
            }
            .icon(),
            theme.accent.scale_alpha(p),
        );
        scene.text(
            "Alimentazione",
            Point::new(power.x + 42.0, power.y + 8.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            truncate("Blocca, esci, sospendi, riavvia, spegni", 42),
            Point::new(power.x + 42.0, power.y + 25.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );
        scene.text_center_y(
            icons::CHEVRON_RIGHT,
            power.right() - 30.0,
            power.center().y,
            TextStyle {
                size: 14.0,
                align: TextAlign::Center,
                max_width: Some(18.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.55 * p),
        );
        self.hits.push((power, Hit::PowerEntry));
    }

    fn page_header(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        content_x: f32,
        p: f32,
        title: &str,
        config: &lard_core::Config,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let back = Rect::new(content_x, panel.y + 12.0, 28.0, 28.0);
        let back_hover = self.cursor.is_some_and(|cursor| back.contains(cursor));
        if back_hover {
            scene.rect(
                back,
                theme.surface_hover.scale_alpha(0.75 * p),
                theme.radius.control(),
            );
        }
        scene.text_center_y(
            icons::CHEVRON_LEFT,
            back.x,
            back.center().y,
            TextStyle {
                size: 18.0,
                align: TextAlign::Center,
                max_width: Some(back.w),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.9 * p),
        );
        self.hits.push((back, Hit::Back));

        scene.text(
            title,
            Point::new(content_x + 34.0, panel.y + 17.0),
            TextStyle {
                size: fonts.size_title,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
    }

    fn build_wifi(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        content_x: f32,
        content_w: f32,
        data: &ControlData<'_>,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let mut y = panel.y + 68.0;

        if self.password_ssid.is_some() {
            self.build_wifi_password(scene, panel, content_x, content_w, data, config, p);
            return;
        }

        let (status, status_color) = if let Some(error) = data.wifi_error {
            (truncate(error, 38), theme.danger)
        } else if let Some(ssid) = data.wifi_connecting {
            (
                format!("Connessione a {}…", truncate(ssid, 24)),
                theme.accent,
            )
        } else if !data.wifi_enabled {
            ("Wi-Fi spento".into(), theme.text_muted)
        } else if data.wifi_scanning {
            ("Scansione…".into(), theme.text_muted)
        } else if data.network.is_connected() {
            (
                format!("Connesso a {}", truncate(&data.network.label(), 24)),
                theme.text_muted,
            )
        } else {
            ("Nessuna rete connessa".into(), theme.text_muted)
        };
        scene.text(
            &status,
            Point::new(content_x + 34.0, panel.y + 40.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                max_width: Some(content_w - 124.0),
                ..Default::default()
            },
            status_color.scale_alpha(0.85 * p),
        );

        self.toggle_switch(
            scene,
            Rect::new(panel.right() - 18.0 - 52.0, panel.y + 10.0, 52.0, 26.0),
            data.wifi_enabled,
            Hit::WifiToggle(!data.wifi_enabled),
            config,
            p,
        );

        let scan = Rect::new(content_x + content_w - 106.0, panel.y + 36.0, 106.0, 28.0);
        self.scan_button(
            scene,
            scan,
            data.wifi_scanning,
            data.wifi_enabled && data.wifi_connecting.is_none(),
            Hit::WifiScan,
            config,
            p,
        );

        scene.rect(
            Rect::new(content_x, panel.y + 70.0, content_w, 1.0),
            theme.border.scale_alpha(0.35 * p),
            0.5,
        );

        if !data.wifi_enabled {
            let card = Rect::new(content_x, y + 14.0, content_w, 202.0);
            scene.rect(
                card,
                theme.background.scale_alpha(0.55 * p),
                theme.radius.card(),
            );
            scene.text_center_y(
                icons::WIFI_OFF,
                card.x,
                card.y + 46.0,
                TextStyle {
                    size: 28.0,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                }
                .icon(),
                theme.text_muted.scale_alpha(0.8 * p),
            );
            scene.text_center_y(
                "Wi-Fi spento",
                card.x,
                card.y + 84.0,
                TextStyle {
                    size: fonts.size_body,
                    weight: fonts.weight_medium,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                },
                theme.text.scale_alpha(p),
            );
            scene.text_center_y(
                "Attiva il Wi-Fi per vedere le reti disponibili.",
                card.x,
                card.y + 106.0,
                TextStyle {
                    size: fonts.size_small - 0.5,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.7 * p),
            );
            let button = Rect::new(card.center().x - 76.0, card.y + 140.0, 152.0, 34.0);
            let hovered = self.cursor.is_some_and(|cursor| button.contains(cursor));
            scene.rect(
                button,
                theme.accent.with_alpha(if hovered { 0.9 } else { 0.8 } * p),
                10.0,
            );
            scene.text_center_y(
                "Attiva Wi-Fi",
                button.x,
                button.center().y,
                TextStyle {
                    size: fonts.size_small,
                    weight: fonts.weight_medium,
                    align: TextAlign::Center,
                    max_width: Some(button.w),
                    ..Default::default()
                },
                theme.accent_text.scale_alpha(p),
            );
            self.hits.push((button, Hit::WifiToggle(true)));
            return;
        }

        y += 10.0;
        self.network_rows.clear();
        self.network_rows.extend_from_slice(data.wifi_networks);
        let available = (panel.bottom() - 14.0 - y).max(0.0);
        let max_rows = ((available / 52.0).floor() as usize).clamp(1, 15);
        self.wifi_visible = max_rows;
        self.wifi_scroll = self
            .wifi_scroll
            .min(data.wifi_networks.len().saturating_sub(max_rows));
        let rows: Vec<&WifiNetwork> = data
            .wifi_networks
            .iter()
            .skip(self.wifi_scroll)
            .take(max_rows)
            .collect();

        if rows.is_empty() {
            let text = if data.wifi_scanning {
                "Scansione delle reti…"
            } else {
                "Nessuna rete trovata"
            };
            scene.text_center_y(
                text,
                content_x,
                y + 34.0,
                TextStyle {
                    size: fonts.size_small,
                    align: TextAlign::Center,
                    max_width: Some(content_w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.7 * p),
            );
            return;
        }

        let busy = data.wifi_connecting.is_some();
        for (index, network) in rows.iter().enumerate() {
            let row = Rect::new(content_x, y + index as f32 * 52.0, content_w, 46.0);
            let hovered = !busy && self.cursor.is_some_and(|cursor| row.contains(cursor));
            let background = if network.active {
                theme.accent.with_alpha(0.11 * p)
            } else {
                theme
                    .background
                    .with_alpha(if hovered { 0.82 } else { 0.48 } * p)
            };
            scene.rect(row, background, theme.radius.card());
            if network.active {
                scene.bordered(
                    row,
                    Color::TRANSPARENT,
                    theme.radius.card(),
                    1.0,
                    theme.accent.scale_alpha(0.32 * p),
                );
            }

            let row_alpha = if busy { 0.55 } else { 1.0 };
            self.draw_signal(
                scene,
                row.x + 14.0,
                row.center().y,
                network.signal,
                if network.active {
                    theme.accent
                } else {
                    theme.text_muted
                },
                p * row_alpha,
            );
            scene.text_center_y(
                &truncate(&network.ssid, 26),
                row.x + 38.0,
                row.center().y,
                TextStyle {
                    size: fonts.size_body,
                    weight: if network.active {
                        fonts.weight_medium
                    } else {
                        fonts.weight_regular
                    },
                    max_width: Some(content_w - 134.0),
                    ..Default::default()
                },
                if network.active {
                    theme.text.scale_alpha(p * row_alpha)
                } else {
                    theme.text.scale_alpha(0.9 * p * row_alpha)
                },
            );

            if network.active {
                let disconnect = Rect::new(row.right() - 96.0, row.y + 10.0, 88.0, 26.0);
                let disconnect_hover = !busy
                    && self
                        .cursor
                        .is_some_and(|cursor| disconnect.contains(cursor));
                scene.rect(
                    disconnect,
                    theme
                        .accent
                        .with_alpha(if disconnect_hover { 0.24 } else { 0.16 } * p),
                    8.0,
                );
                scene.text_center_y(
                    "Disconnetti",
                    disconnect.x,
                    disconnect.center().y,
                    TextStyle {
                        size: 9.5,
                        weight: fonts.weight_medium,
                        align: TextAlign::Center,
                        max_width: Some(disconnect.w),
                        ..Default::default()
                    },
                    theme.accent.scale_alpha(p * row_alpha),
                );
                if !busy {
                    self.hits.push((disconnect, Hit::WifiDisconnect));
                }
            } else {
                scene.text_center_y(
                    if network.secured {
                        icons::LOCK
                    } else {
                        icons::CHEVRON_RIGHT
                    },
                    row.right() - 30.0,
                    row.center().y,
                    TextStyle {
                        size: 13.0,
                        align: TextAlign::Center,
                        max_width: Some(18.0),
                        ..Default::default()
                    }
                    .icon(),
                    theme.text_muted.scale_alpha(0.55 * p * row_alpha),
                );
                if !busy {
                    self.hits
                        .push((row, Hit::WifiNetwork(self.wifi_scroll + index)));
                }
            }
        }

        if data.wifi_networks.len() > max_rows {
            self.draw_scrollbar(
                scene,
                Rect::new(
                    content_x + content_w + 4.0,
                    y + 6.0,
                    3.0,
                    max_rows as f32 * 52.0 - 12.0,
                ),
                data.wifi_networks.len(),
                max_rows,
                self.wifi_scroll,
                config,
                p,
            );
        }
    }

    fn draw_scrollbar(
        &self,
        scene: &mut Scene,
        track: Rect,
        total: usize,
        visible: usize,
        offset: usize,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        scene.rect(track, theme.border.scale_alpha(0.18 * p), 1.5);
        let thumb_height = (track.h * visible as f32 / total as f32).max(24.0);
        let travel = (track.h - thumb_height).max(0.0);
        let max_scroll = total.saturating_sub(visible).max(1) as f32;
        let progress = (offset as f32 / max_scroll).clamp(0.0, 1.0);
        scene.rect(
            Rect::new(track.x, track.y + travel * progress, track.w, thumb_height),
            theme.text_muted.scale_alpha(0.45 * p),
            1.5,
        );
    }

    /// One text field of the Wi-Fi prompt: the 802.1X user name or the secret.
    #[allow(clippy::too_many_arguments)]
    fn wifi_field(
        &mut self,
        scene: &mut Scene,
        field: Rect,
        icon: &'static str,
        value: &str,
        placeholder: &str,
        masked: bool,
        focused: bool,
        hit: Hit,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.bordered(
            field,
            theme.background.scale_alpha(0.92 * p),
            theme.radius.control(),
            if focused { 1.5 } else { 1.0 },
            if focused {
                theme.accent.scale_alpha(0.85 * p)
            } else {
                theme.border.scale_alpha(0.55 * p)
            },
        );
        scene.text_center_y(
            icon,
            field.x + 10.0,
            field.center().y,
            TextStyle {
                size: 14.0,
                align: TextAlign::Center,
                max_width: Some(20.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.75 * p),
        );
        let shown = if masked {
            "•".repeat(value.chars().count().min(28))
        } else {
            truncate(value, 40)
        };
        let label = if value.is_empty() {
            placeholder.to_string()
        } else {
            shown.clone()
        };
        scene.text_center_y(
            &label,
            field.x + 32.0,
            field.center().y,
            TextStyle {
                size: fonts.size_body,
                max_width: Some(field.w - 44.0),
                wrap: false,
                ..Default::default()
            },
            if value.is_empty() {
                theme.text_muted.scale_alpha(0.55 * p)
            } else {
                theme.text.scale_alpha(p)
            },
        );
        if focused {
            let caret_x =
                (field.x + 33.0 + approx_width(&shown, fonts.size_body)).min(field.right() - 12.0);
            scene.rect(
                Rect::new(caret_x, field.center().y - 9.0, 1.5, 18.0),
                theme.accent.scale_alpha(p),
                0.75,
            );
        }
        self.hits.push((field, hit));
    }

    fn build_wifi_password(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        content_x: f32,
        content_w: f32,
        data: &ControlData<'_>,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let ssid = self.password_ssid.clone().unwrap_or_default();
        let enterprise = self.password_enterprise;
        let card_height = if enterprise { 356.0 } else { 250.0 };
        let card = Rect::new(content_x, panel.y + 84.0, content_w, card_height);
        scene.rect(
            card,
            theme.background.scale_alpha(0.60 * p),
            theme.radius.card(),
        );
        scene.text(
            if enterprise {
                "Accesso Wi-Fi"
            } else {
                "Password Wi-Fi"
            },
            Point::new(card.x + 16.0, card.y + 14.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            &truncate(&ssid, 32),
            Point::new(card.x + 16.0, card.y + 36.0),
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(card.w - 32.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.9 * p),
        );

        let field_w = card.w - 32.0;
        let mut y = card.y + 60.0;
        if enterprise {
            self.wifi_field(
                scene,
                Rect::new(card.x + 16.0, y, field_w, 40.0),
                icons::PERSON,
                &self.password_identity.clone(),
                "Nome utente",
                false,
                self.password_focus == PasswordField::Identity,
                Hit::WifiField(PasswordField::Identity),
                config,
                p,
            );
            y += 46.0;
        }
        self.wifi_field(
            scene,
            Rect::new(card.x + 16.0, y, field_w, 40.0),
            icons::LOCK,
            &self.password_input.clone(),
            "Password",
            true,
            self.password_focus == PasswordField::Secret,
            Hit::WifiField(PasswordField::Secret),
            config,
            p,
        );
        y += 50.0;

        if let Some(error) = data.wifi_error {
            scene.text(
                &truncate(error, 56),
                Point::new(card.x + 16.0, y),
                TextStyle {
                    size: fonts.size_small - 0.5,
                    max_width: Some(field_w),
                    ..Default::default()
                },
                theme.danger.scale_alpha(0.9 * p),
            );
            y += 20.0;
        }
        scene.text(
            if enterprise {
                "Rete 802.1X (PEAP/MSCHAPv2): le credenziali vengono salvate in NetworkManager. Tab cambia campo."
            } else {
                "La rete verrà salvata nelle connessioni di sistema."
            },
            Point::new(card.x + 16.0, y + 6.0),
            TextStyle {
                size: 9.5,
                max_width: Some(field_w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.6 * p),
        );

        let cancel = Rect::new(card.x + 16.0, card.bottom() - 46.0, 92.0, 32.0);
        let cancel_hover = self.cursor.is_some_and(|cursor| cancel.contains(cursor));
        scene.rect(
            cancel,
            theme
                .background
                .scale_alpha(if cancel_hover { 0.9 } else { 0.6 } * p),
            9.0,
        );
        scene.text_center_y(
            "Annulla",
            cancel.x,
            cancel.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(cancel.w),
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        self.hits.push((cancel, Hit::WifiCancel));

        let connect = Rect::new(
            card.right() - 16.0 - 108.0,
            card.bottom() - 46.0,
            108.0,
            32.0,
        );
        let connect_hover = self.cursor.is_some_and(|cursor| connect.contains(cursor));
        let enabled = !self.password_input.is_empty()
            && (!enterprise || !self.password_identity.trim().is_empty());
        scene.rect(
            connect,
            if enabled {
                theme
                    .accent
                    .with_alpha(if connect_hover { 0.92 } else { 0.82 } * p)
            } else {
                theme.accent.with_alpha(0.25 * p)
            },
            9.0,
        );
        scene.text_center_y(
            "Connetti",
            connect.x,
            connect.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(connect.w),
                ..Default::default()
            },
            if enabled {
                theme.accent_text.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.6 * p)
            },
        );
        if enabled {
            self.hits.push((connect, Hit::WifiSubmit));
        }
    }

    fn build_bluetooth(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        content_x: f32,
        content_w: f32,
        data: &ControlData<'_>,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let mut y = panel.y + 68.0;

        let connecting_name = data.bt_connecting.and_then(|address| {
            data.bluetooth_devices
                .iter()
                .find(|device| device.address == address)
                .map(|device| device.name.as_str())
                .or(Some(address))
        });
        let (status, status_color) = if let Some(error) = data.bt_error {
            (truncate(error, 38), theme.danger)
        } else if let Some(name) = connecting_name {
            (
                format!("Connessione a {}…", truncate(name, 24)),
                theme.accent,
            )
        } else if !data.bluetooth.present {
            ("Non disponibile".into(), theme.text_muted)
        } else if !data.bluetooth.powered {
            ("Bluetooth spento".into(), theme.text_muted)
        } else if data.bt_scanning {
            ("Scansione…".into(), theme.text_muted)
        } else if data.bluetooth_devices.is_empty() {
            ("Nessun dispositivo".into(), theme.text_muted)
        } else {
            (
                format!("{} dispositivi", data.bluetooth_devices.len()),
                theme.text_muted,
            )
        };
        scene.text(
            &status,
            Point::new(content_x + 34.0, panel.y + 40.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                max_width: Some(content_w - 124.0),
                ..Default::default()
            },
            status_color.scale_alpha(0.85 * p),
        );

        if data.bluetooth.present {
            self.toggle_switch(
                scene,
                Rect::new(panel.right() - 18.0 - 52.0, panel.y + 10.0, 52.0, 26.0),
                data.bluetooth.powered,
                Hit::BtToggle(!data.bluetooth.powered),
                config,
                p,
            );
        }

        let scan = Rect::new(content_x + content_w - 106.0, panel.y + 36.0, 106.0, 28.0);
        self.scan_button(
            scene,
            scan,
            data.bt_scanning,
            data.bluetooth.present && data.bluetooth.powered && data.bt_connecting.is_none(),
            Hit::BtScan,
            config,
            p,
        );

        scene.rect(
            Rect::new(content_x, panel.y + 70.0, content_w, 1.0),
            theme.border.scale_alpha(0.35 * p),
            0.5,
        );

        if !data.bluetooth.present {
            self.bluetooth_state_card(
                scene,
                content_x,
                content_w,
                &mut y,
                icons::BLUETOOTH_OFF,
                "Bluetooth non disponibile",
                "Nessun adattatore Bluetooth rilevato.",
                None,
                config,
                p,
            );
            return;
        }
        if !data.bluetooth.powered {
            self.bluetooth_state_card(
                scene,
                content_x,
                content_w,
                &mut y,
                icons::BLUETOOTH_OFF,
                "Bluetooth spento",
                "Attiva il Bluetooth per cercare i dispositivi.",
                Some((Hit::BtToggle(true), "Attiva Bluetooth")),
                config,
                p,
            );
            return;
        }

        y += 10.0;
        self.device_rows.clear();
        self.device_rows.extend_from_slice(data.bluetooth_devices);
        let available = (panel.bottom() - 14.0 - y).max(0.0);
        let max_rows = ((available / 64.0).floor() as usize).clamp(1, 12);
        self.bt_visible = max_rows;
        self.bt_scroll = self
            .bt_scroll
            .min(data.bluetooth_devices.len().saturating_sub(max_rows));
        let rows: Vec<&BluetoothDevice> = data
            .bluetooth_devices
            .iter()
            .skip(self.bt_scroll)
            .take(max_rows)
            .collect();

        if rows.is_empty() {
            let text = if data.bt_scanning {
                "Scansione in corso…"
            } else {
                "Nessun dispositivo. Avvia la scansione."
            };
            scene.text_center_y(
                text,
                content_x,
                y + 34.0,
                TextStyle {
                    size: fonts.size_small,
                    align: TextAlign::Center,
                    max_width: Some(content_w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.7 * p),
            );
            return;
        }

        let busy = data.bt_connecting.is_some();
        for (index, device) in rows.iter().enumerate() {
            let row = Rect::new(content_x, y + index as f32 * 64.0, content_w, 56.0);
            let hovered = !busy && self.cursor.is_some_and(|cursor| row.contains(cursor));
            let background = if device.connected {
                theme.accent.with_alpha(0.10 * p)
            } else {
                theme
                    .background
                    .with_alpha(if hovered { 0.82 } else { 0.48 } * p)
            };
            scene.rect(row, background, theme.radius.card());
            if device.connected {
                scene.bordered(
                    row,
                    Color::TRANSPARENT,
                    theme.radius.card(),
                    1.0,
                    theme.accent.scale_alpha(0.28 * p),
                );
            }
            scene.text_center_y(
                icons::BLUETOOTH,
                row.x + 16.0,
                row.center().y,
                TextStyle {
                    size: 18.0,
                    align: TextAlign::Center,
                    max_width: Some(22.0),
                    ..Default::default()
                }
                .icon(),
                if device.connected {
                    theme.accent.scale_alpha(p)
                } else {
                    theme.text_muted.scale_alpha(0.8 * p)
                },
            );
            scene.text(
                &truncate(&device.name, 22),
                Point::new(row.x + 44.0, row.y + 10.0),
                TextStyle {
                    size: fonts.size_body,
                    weight: fonts.weight_medium,
                    max_width: Some(content_w - 100.0),
                    ..Default::default()
                },
                theme.text.scale_alpha(if busy { 0.6 } else { p }),
            );
            scene.text(
                &device.address,
                Point::new(row.x + 44.0, row.y + 31.0),
                TextStyle {
                    size: 9.5,
                    max_width: Some(content_w - 100.0),
                    ..Default::default()
                },
                theme
                    .text_muted
                    .scale_alpha(if busy { 0.4 } else { 0.6 * p }),
            );

            let (label, color) = if data.bt_connecting == Some(device.address.as_str()) {
                ("Connessione…", theme.accent)
            } else if device.connected {
                if hovered {
                    ("Disconnetti", theme.text_muted)
                } else {
                    ("Connesso", theme.accent)
                }
            } else if device.paired {
                ("Connetti", theme.text_muted)
            } else {
                ("Associa", theme.accent)
            };
            scene.text_center_y(
                label,
                row.right() - 76.0,
                row.center().y,
                TextStyle {
                    size: 9.5,
                    weight: fonts.weight_medium,
                    align: TextAlign::Right,
                    max_width: Some(64.0),
                    ..Default::default()
                },
                color.scale_alpha(if busy { 0.6 } else { 0.9 } * p),
            );
            if device.connected {
                scene.text_center_y(
                    icons::CHECK,
                    row.right() - 22.0,
                    row.center().y,
                    TextStyle {
                        size: 13.0,
                        align: TextAlign::Center,
                        max_width: Some(16.0),
                        ..Default::default()
                    }
                    .icon(),
                    theme.accent.scale_alpha(0.9 * p),
                );
            }
            if !busy {
                self.hits.push((row, Hit::BtDevice(self.bt_scroll + index)));
            }
        }

        if data.bluetooth_devices.len() > max_rows {
            self.draw_scrollbar(
                scene,
                Rect::new(
                    content_x + content_w + 4.0,
                    y + 6.0,
                    3.0,
                    max_rows as f32 * 64.0 - 16.0,
                ),
                data.bluetooth_devices.len(),
                max_rows,
                self.bt_scroll,
                config,
                p,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn bluetooth_state_card(
        &mut self,
        scene: &mut Scene,
        content_x: f32,
        content_w: f32,
        y: &mut f32,
        icon: &'static str,
        title: &str,
        subtitle: &str,
        button: Option<(Hit, &str)>,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let card = Rect::new(content_x, *y + 14.0, content_w, 202.0);
        scene.rect(
            card,
            theme.background.scale_alpha(0.55 * p),
            theme.radius.card(),
        );
        scene.text_center_y(
            icon,
            card.x,
            card.y + 46.0,
            TextStyle {
                size: 28.0,
                align: TextAlign::Center,
                max_width: Some(card.w),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.8 * p),
        );
        scene.text_center_y(
            title,
            card.x,
            card.y + 84.0,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(card.w),
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text_center_y(
            subtitle,
            card.x,
            card.y + 106.0,
            TextStyle {
                size: fonts.size_small - 0.5,
                align: TextAlign::Center,
                max_width: Some(card.w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );
        if let Some((hit, label)) = button {
            let action = Rect::new(card.center().x - 84.0, card.y + 140.0, 168.0, 34.0);
            let hovered = self.cursor.is_some_and(|cursor| action.contains(cursor));
            scene.rect(
                action,
                theme.accent.with_alpha(if hovered { 0.9 } else { 0.8 } * p),
                10.0,
            );
            scene.text_center_y(
                label,
                action.x,
                action.center().y,
                TextStyle {
                    size: fonts.size_small,
                    weight: fonts.weight_medium,
                    align: TextAlign::Center,
                    max_width: Some(action.w),
                    ..Default::default()
                },
                theme.accent_text.scale_alpha(p),
            );
            self.hits.push((action, hit));
        }
    }

    fn build_power(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        content_x: f32,
        content_w: f32,
        data: &ControlData<'_>,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let mut y = panel.y + 68.0;

        let session = Rect::new(content_x, y, content_w, 64.0);
        scene.rect(
            session,
            theme.background.scale_alpha(0.55 * p),
            theme.radius.card(),
        );
        let initial = data
            .user
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        let avatar = Rect::new(session.x + 14.0, session.y + 15.0, 34.0, 34.0);
        scene.rect(avatar, theme.accent.with_alpha(0.20 * p), avatar.w * 0.5);
        scene.text_center_y(
            initial,
            avatar.x,
            avatar.center().y,
            TextStyle {
                size: 14.0,
                weight: fonts.weight_semibold,
                align: TextAlign::Center,
                max_width: Some(avatar.w),
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            "Sessione",
            Point::new(avatar.right() + 12.0, session.y + 12.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );
        scene.text(
            format!("@{}", data.user),
            Point::new(avatar.right() + 12.0, session.y + 29.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text_center_y(
            if data.niri_connected {
                "niri · connesso"
            } else {
                "niri · assente"
            },
            session.right() - 120.0,
            session.center().y,
            TextStyle {
                size: 9.5,
                align: TextAlign::Right,
                max_width: Some(104.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.65 * p),
        );

        y = session.bottom() + 20.0;
        self.section_label(scene, y, content_x, "Sessione", config, p);
        y += 22.0;
        self.power_row(
            scene,
            Rect::new(content_x, y, content_w, 54.0),
            icons::LOCK,
            PowerAction::Lock,
            "Blocca",
            "Blocca la sessione",
            false,
            config,
            p,
        );
        y += 60.0;
        self.power_row(
            scene,
            Rect::new(content_x, y, content_w, 54.0),
            icons::LOGOUT,
            PowerAction::Logout,
            "Esci",
            "Termina la sessione",
            false,
            config,
            p,
        );

        y += 74.0;
        self.section_label(scene, y, content_x, "Sistema", config, p);
        y += 22.0;
        self.power_row(
            scene,
            Rect::new(content_x, y, content_w, 54.0),
            icons::NIGHT,
            PowerAction::Suspend,
            "Sospendi",
            "Metti il sistema in sospensione",
            false,
            config,
            p,
        );
        y += 60.0;
        self.power_row(
            scene,
            Rect::new(content_x, y, content_w, 54.0),
            icons::RESTART,
            PowerAction::Reboot,
            "Riavvia",
            "Riavvia il sistema",
            false,
            config,
            p,
        );
        y += 60.0;
        self.power_row(
            scene,
            Rect::new(content_x, y, content_w, 54.0),
            icons::POWER,
            PowerAction::PowerOff,
            "Spegni",
            "Spegni il sistema",
            true,
            config,
            p,
        );

        scene.text_center_y(
            "Le azioni vengono eseguite immediatamente.",
            content_x,
            panel.bottom() - 28.0,
            TextStyle {
                size: 9.5,
                align: TextAlign::Center,
                max_width: Some(content_w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.55 * p),
        );
    }

    fn section_label(
        &self,
        scene: &mut Scene,
        y: f32,
        x: f32,
        label: &str,
        config: &lard_core::Config,
        p: f32,
    ) {
        scene.text(
            label,
            Point::new(x, y),
            TextStyle {
                size: config.font.size_small - 1.0,
                weight: config.font.weight_medium,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.6 * p),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn power_row(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        icon: &'static str,
        action: PowerAction,
        title: &str,
        subtitle: &str,
        danger: bool,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let hovered = self.cursor.is_some_and(|cursor| rect.contains(cursor));
        let background = if danger {
            theme
                .danger
                .with_alpha(if hovered { 0.22 } else { 0.12 } * p)
        } else {
            theme
                .background
                .with_alpha(if hovered { 0.82 } else { 0.48 } * p)
        };
        scene.rect(rect, background, theme.radius.card());
        if danger {
            scene.bordered(
                rect,
                Color::TRANSPARENT,
                theme.radius.card(),
                1.0,
                theme.danger.scale_alpha(0.32 * p),
            );
        }
        let color = if danger { theme.danger } else { theme.accent };
        scene.text_center_y(
            icon,
            rect.x + 14.0,
            rect.center().y,
            TextStyle {
                size: 18.0,
                align: TextAlign::Center,
                max_width: Some(22.0),
                ..Default::default()
            }
            .icon(),
            color.scale_alpha(p),
        );
        scene.text(
            title,
            Point::new(rect.x + 50.0, rect.y + 9.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            if danger {
                theme.danger.scale_alpha(p)
            } else {
                theme.text.scale_alpha(p)
            },
        );
        scene.text(
            subtitle,
            Point::new(rect.x + 50.0, rect.y + 28.0),
            TextStyle {
                size: fonts.size_small - 0.5,
                max_width: Some(rect.w - 90.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.65 * p),
        );
        scene.text_center_y(
            icons::CHEVRON_RIGHT,
            rect.right() - 30.0,
            rect.center().y,
            TextStyle {
                size: 13.0,
                align: TextAlign::Center,
                max_width: Some(16.0),
                ..Default::default()
            }
            .icon(),
            color.scale_alpha(0.55 * p),
        );
        self.hits.push((rect, Hit::Power(action)));
    }

    fn toggle_switch(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        on: bool,
        action: Hit,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        scene.rect(
            rect,
            if on {
                theme.accent.scale_alpha(0.85 * p)
            } else {
                theme.background.scale_alpha(0.9 * p)
            },
            rect.h * 0.5,
        );
        scene.rect(
            Rect::new(
                rect.x + if on { rect.w - 24.0 } else { 2.0 },
                rect.y + 2.0,
                22.0,
                rect.h - 4.0,
            ),
            if on {
                theme.accent_text.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.8 * p)
            },
            (rect.h - 4.0) * 0.5,
        );
        self.hits.push((rect, action));
    }

    fn scan_button(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        scanning: bool,
        enabled: bool,
        action: Hit,
        config: &lard_core::Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let hovered = enabled && self.cursor.is_some_and(|cursor| rect.contains(cursor));
        scene.rect(
            rect,
            theme
                .background
                .scale_alpha(if hovered { 0.9 } else { 0.55 } * p),
            9.0,
        );
        let color = if enabled {
            theme.text.scale_alpha(0.9 * p)
        } else {
            theme.text_muted.scale_alpha(0.55 * p)
        };
        scene.text_center_y(
            icons::REFRESH,
            rect.x + 10.0,
            rect.center().y,
            TextStyle {
                size: 13.0,
                align: TextAlign::Center,
                max_width: Some(16.0),
                ..Default::default()
            }
            .icon(),
            color,
        );
        scene.text_center_y(
            if scanning {
                "Scansione…"
            } else {
                "Scansiona"
            },
            rect.x + 28.0,
            rect.center().y,
            TextStyle {
                size: fonts.size_small - 0.5,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 32.0),
                ..Default::default()
            },
            color,
        );
        if enabled && !scanning {
            self.hits.push((rect, action));
        }
    }

    fn draw_signal(
        &self,
        scene: &mut Scene,
        x: f32,
        center_y: f32,
        signal: u8,
        color: Color,
        p: f32,
    ) {
        let heights = [4.0, 7.0, 10.0, 13.0];
        let thresholds = [1, 30, 55, 80];
        for (index, height) in heights.iter().enumerate() {
            let active = signal >= thresholds[index];
            scene.rect(
                Rect::new(
                    x + index as f32 * 4.0,
                    center_y + 6.0 - *height,
                    2.5,
                    *height,
                ),
                color.scale_alpha(if active { p } else { 0.22 * p }),
                1.2,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lard_services::battery::BatteryState;
    use lard_services::bluetooth::BluetoothState;
    use lard_services::network::NetworkState;

    fn data<'a>(
        network: &'a NetworkState,
        wifi_networks: &'a [WifiNetwork],
        bluetooth_devices: &'a [BluetoothDevice],
    ) -> ControlData<'a> {
        ControlData {
            volume: 50,
            muted: false,
            brightness: 70,
            brightness_present: true,
            user: "testuser",
            output: Some("eDP-1"),
            keyboard_layout: Some("it"),
            niri_connected: true,
            battery: BatteryState::default(),
            network,
            bluetooth: BluetoothState {
                present: true,
                powered: true,
                discovering: false,
            },
            dnd: false,
            wifi_enabled: true,
            wifi_networks,
            wifi_scanning: false,
            wifi_error: None,
            wifi_connecting: None,
            bluetooth_devices,
            bt_scanning: false,
            bt_error: None,
            bt_connecting: None,
            clock: "12:00",
            date: "mar 20 ott",
        }
    }

    fn texts(scene: &Scene) -> Vec<&str> {
        scene.texts.iter().map(|t| scene.text_of(t)).collect()
    }

    #[test]
    fn wifi_page_lists_networks_and_prompts_for_password() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let networks = [WifiNetwork {
            ssid: "Casa".into(),
            signal: 72,
            secured: true,
            enterprise: false,
            saved: false,
            active: false,
        }];
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Wifi);
        let data = data(&network, &networks, &[]);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        let labels = texts(&scene);
        assert!(labels.contains(&"Casa"), "expected SSID: {labels:?}");
        assert!(
            labels.contains(&"Scansiona"),
            "expected scan action: {labels:?}"
        );

        // First list row: click it to open the password prompt.
        assert!(view.pointer_press(Point::new(1300.0, 110.0)).is_none());
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        let labels = texts(&scene);
        assert!(
            labels.contains(&"Password Wi-Fi"),
            "expected password prompt: {labels:?}"
        );

        view.key(ShellKey::Char("segreta".into()));
        let action = view.key(ShellKey::Enter);
        assert!(matches!(
            action,
            Some(ShellAction::ConnectWifi {
                ssid,
                password: Some(password),
                identity: None,
            }) if ssid == "Casa" && password == "segreta"
        ));
    }

    #[test]
    fn enterprise_wifi_prompts_for_identity_and_secret() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let networks = [WifiNetwork {
            ssid: "eduroam".into(),
            signal: 70,
            secured: true,
            enterprise: true,
            saved: false,
            active: false,
        }];
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Wifi);
        let control = data(&network, &networks, &[]);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &control, &config);
        assert!(view.pointer_press(Point::new(1300.0, 110.0)).is_none());

        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &control, &config);
        let labels = texts(&scene);
        assert!(labels.contains(&"Accesso Wi-Fi"), "{labels:?}");
        assert!(labels.contains(&"Nome utente"), "{labels:?}");
        assert!(labels.contains(&"Password"), "{labels:?}");

        // The user name is focused first, Tab moves to the secret.
        view.key(ShellKey::Char("utente".into()));
        view.key(ShellKey::Tab);
        view.key(ShellKey::Char("segreta".into()));
        let action = view.key(ShellKey::Enter);
        assert!(matches!(
            action,
            Some(ShellAction::ConnectWifi {
                ssid,
                password: Some(password),
                identity: Some(identity),
            }) if ssid == "eduroam" && password == "segreta" && identity == "utente"
        ));

        // A PSK network keeps the single-field prompt and no identity.
        let psk = [WifiNetwork {
            ssid: "Casa".into(),
            signal: 70,
            secured: true,
            enterprise: false,
            saved: false,
            active: false,
        }];
        let control = data(&network, &psk, &[]);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &control, &config);
        assert!(view.pointer_press(Point::new(1300.0, 110.0)).is_none());
        view.key(ShellKey::Char("segreta".into()));
        let action = view.key(ShellKey::Enter);
        assert!(matches!(
            action,
            Some(ShellAction::ConnectWifi {
                password: Some(_),
                identity: None,
                ..
            })
        ));
    }

    #[test]
    fn wifi_list_scrolls_and_keeps_hit_indices_aligned() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let networks: Vec<WifiNetwork> = (0..20)
            .map(|index| WifiNetwork {
                ssid: format!("Rete {index}"),
                signal: 60,
                secured: true,
                enterprise: false,
                saved: false,
                active: false,
            })
            .collect();
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Wifi);
        let data = data(&network, &networks, &[]);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        assert!(texts(&scene).contains(&"Rete 0"));

        assert!(view.axis(1.0));
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        let labels = texts(&scene);
        assert!(!labels.contains(&"Rete 0"), "list should have scrolled");
        assert!(
            labels.contains(&"Rete 2"),
            "expected second page: {labels:?}"
        );

        // Clicking the first visible row must prompt for Rete 2, not Rete 0.
        assert!(view.pointer_press(Point::new(1300.0, 110.0)).is_none());
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        assert!(texts(&scene).contains(&"Password Wi-Fi"));
        assert!(view.password_ssid.as_deref() == Some("Rete 2"));
    }

    #[test]
    fn wifi_rows_are_disabled_while_connecting() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let networks = [WifiNetwork {
            ssid: "Casa".into(),
            signal: 72,
            secured: true,
            enterprise: false,
            saved: false,
            active: false,
        }];
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Wifi);
        let mut data = data(&network, &networks, &[]);
        data.wifi_connecting = Some("Altra rete");
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        assert!(view.pointer_press(Point::new(1300.0, 110.0)).is_none());
    }

    #[test]
    fn power_page_has_every_session_action() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Power);
        let data = data(&network, &[], &[]);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        let labels = texts(&scene);
        for label in ["Blocca", "Esci", "Sospendi", "Riavvia", "Spegni"] {
            assert!(
                labels.contains(&label),
                "expected power action {label}: {labels:?}"
            );
        }
    }

    #[test]
    fn power_rows_emit_their_action() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let data = data(&network, &[], &[]);
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Power);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        let actions: Vec<(Rect, PowerAction)> = view
            .hits
            .iter()
            .filter_map(|(rect, hit)| match hit {
                Hit::Power(action) => Some((*rect, *action)),
                _ => None,
            })
            .collect();
        assert_eq!(actions.len(), 5);
        for (rect, expected) in actions {
            view.open();
            view.show_page(ControlPage::Power);
            let mut scene = Scene::new();
            view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
            assert_eq!(
                view.pointer_press(rect.center()),
                Some(ShellAction::Power(expected))
            );
        }
    }

    #[test]
    fn bluetooth_page_lists_devices() {
        let config = lard_core::Config::default();
        let network = NetworkState::Disconnected;
        let devices = [BluetoothDevice {
            address: "2C:BE:EE:67:29:51".into(),
            name: "CMF Buds Pro 2".into(),
            paired: true,
            connected: false,
        }];
        let mut view = ControlCenter::new(config.animation.panel, true);
        view.open();
        view.show_page(ControlPage::Bluetooth);
        let data = data(&network, &[], &devices);
        let mut scene = Scene::new();
        view.build(&mut scene, Size::new(1600.0, 900.0), &data, &config);
        let labels = texts(&scene);
        assert!(
            labels.iter().any(|label| label.contains("CMF Buds")),
            "expected Bluetooth device: {labels:?}"
        );
        assert!(
            labels.contains(&"Connetti"),
            "expected connect action: {labels:?}"
        );
    }
}
