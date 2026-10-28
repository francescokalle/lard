//! Contextual popovers anchored below a bar icon (Dank-island style).
//!
//! Each bar slot opens its own small card instead of the shared side panel.

use crate::bar::BarIsland;
use crate::control_center::{ControlCenter, ControlData, ControlPage};
use crate::icons;
use crate::toasts::NotificationEntry;
use crate::widgets::{MorphAnchor, morph_rect, truncate};
use lard_core::anim::{Spring, SpringConfig};
use lard_core::{Color, Config, Point, Rect, Size};
use lard_render::{Scene, Shadow, TextAlign, TextStyle};
use lard_services::battery::BatteryState;
use lard_services::bluetooth::BluetoothState;
use lard_services::network::NetworkState;
use lard_services::power::PowerAction;
use lard_services::system::SystemStats;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverKind {
    Clock,
    Network,
    Bluetooth,
    Battery,
    Notifications,
    System,
    Power,
    /// macOS-style grouped controls opened from the single bar button.
    ControlCenter,
}

#[derive(Clone, Debug)]
pub enum PopoverAction {
    ToggleWifi(bool),
    ToggleBluetooth(bool),
    ToggleDnd,
    ExpandWifi,
    ExpandBluetooth,
    /// Grow the card into the mini task manager.
    ExpandSystem,
    OpenPowerMenu,
    Power(PowerAction),
    SetVolume(u8),
    ToggleMute,
    SetBrightness(u8),
    OpenSettings,
    ShowPower,
    BackControl,
    /// A D-Bus action of a notification in the notification center.
    NotificationAction {
        id: u32,
        key: String,
    },
    /// Dismiss a notification from the notification center.
    DismissNotification(u32),
}

pub struct PopoverData<'a> {
    pub clock: &'a str,
    pub date: &'a str,
    pub network: &'a NetworkState,
    pub bluetooth: BluetoothState,
    pub wifi_enabled: bool,
    pub battery: BatteryState,
    pub system: &'a SystemStats,
    pub dnd: bool,
    pub active_toasts: usize,
    pub volume: u8,
    pub muted: bool,
    pub brightness: u8,
    pub brightness_present: bool,
    /// Notifications waiting in the notification center.
    pub notifications: Vec<NotificationEntry>,
}

pub struct Popover {
    kind: PopoverKind,
    open: bool,
    progress: Spring,
    /// 0 = compact summary, 1 = full Wi-Fi/Bluetooth page.
    expand: Spring,
    expanded: Option<ControlPage>,
    anchor_x: f32,
    /// Compact size. The animated expanded size is derived from it.
    size: Size,
    hits: Vec<(Rect, PopoverAction)>,
    panel: Rect,
    /// Layout rect of the card content (the panel starts on the island row).
    content: Rect,
    cursor: Option<Point>,
    reduce_motion: bool,
    /// True while the Control Center card is showing its power submenu.
    cc_power: bool,
    /// Island the card grows out of: screen rectangle and corner radius.
    origin: Option<(Rect, f32)>,
    /// Shape of the card during the morph (the target rect when it is in
    /// place).
    shape: Rect,
    /// Corner radius the card is drawn with in this frame.
    radius: f32,
    /// Which island the card comes from, so the bar can fade it away.
    origin_island: Option<BarIsland>,
    /// Click/drag target for the two Control Center sliders, in panel space.
    volume_track: Rect,
    brightness_track: Rect,
    volume_value: u8,
    brightness_value: u8,
    /// Slider grabbed by the current press: motion events keep updating it
    /// until the button is released.
    drag: Option<CcSlider>,
}

const EXPANDED_WIDTH: f32 = 400.0;
const EXPANDED_HEIGHT: f32 = 640.0;

impl Popover {
    /// Bounds of the popover panel, used for the backdrop blur region.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    /// Layout rect of the card content: the panel starts on the island row
    /// but the controls live below the bar.
    pub fn content_rect(&self) -> Rect {
        self.content
    }

    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        Self {
            kind: PopoverKind::Clock,
            open: false,
            progress: Spring::new(0.0, animation),
            expand: Spring::new(0.0, animation),
            expanded: None,
            anchor_x: 0.0,
            size: Size::new(280.0, 120.0),
            hits: Vec::with_capacity(8),
            panel: Rect::ZERO,
            content: Rect::ZERO,
            cursor: None,
            reduce_motion,
            cc_power: false,
            origin: None,
            origin_island: None,
            shape: Rect::ZERO,
            radius: 0.0,
            volume_track: Rect::ZERO,
            brightness_track: Rect::ZERO,
            volume_value: 0,
            brightness_value: 0,
            drag: None,
        }
    }

    pub fn preferred_size(kind: PopoverKind) -> Size {
        match kind {
            PopoverKind::Clock => Size::new(288.0, 292.0),
            PopoverKind::Network => Size::new(268.0, 142.0),
            PopoverKind::Bluetooth => Size::new(240.0, 134.0),
            PopoverKind::Battery => Size::new(360.0, 318.0),
            PopoverKind::Notifications => Size::new(268.0, 118.0),
            PopoverKind::System => Size::new(384.0, 436.0),
            PopoverKind::Power => Size::new(248.0, 244.0),
            PopoverKind::ControlCenter => Size::new(360.0, 470.0),
        }
    }

    pub fn open(
        &mut self,
        kind: PopoverKind,
        anchor_x: f32,
        origin: Option<(Rect, f32, BarIsland)>,
    ) {
        let switching = self.open && self.kind != kind;
        self.origin = origin.map(|(rect, radius, _)| (rect, radius));
        self.origin_island = origin.map(|(_, _, island)| island);
        tracing::debug!(origin = ?self.origin, island = ?self.origin_island, "popover open");
        self.kind = kind;
        self.anchor_x = anchor_x;
        self.size = Self::preferred_size(kind);
        self.expanded = None;
        self.expand.snap_to(0.0);
        self.cc_power = false;
        self.volume_track = Rect::ZERO;
        self.brightness_track = Rect::ZERO;
        self.drag = None;
        self.open = true;
        if self.reduce_motion {
            self.progress.snap_to(1.0);
        } else if switching {
            // Another panel was open: instead of jumping to a different size,
            // the card goes back onto its island and grows again as the new
            // one. The island row stays on screen, so nothing blinks.
            self.progress.snap_to(0.0);
            self.progress.set_target(1.0);
        } else {
            self.progress.set_target(1.0);
        }
    }

    /// Restart the opening spring once the surface is mapped (see
    /// `Launcher::start_entrance`).
    pub fn start_entrance(&mut self) {
        if self.reduce_motion {
            return;
        }
        self.progress.snap_to(0.0);
        self.progress.set_target(1.0);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.drag = None;
        // The origin is kept: the closing animation is the exact reverse of
        // the opening one, so the panel shrinks back into its island.
        if self.reduce_motion {
            self.expanded = None;
            self.expand.snap_to(0.0);
            self.progress.snap_to(0.0);
        } else {
            // Keep the expanded geometry while the card fades out; snapping
            // back to the compact size here would make it visibly jump.
            self.progress.set_target(0.0);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn wants_surface(&self) -> bool {
        self.open || self.progress.value() > 0.01
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let mut active = self.progress.update(dt);
        active |= self.expand.update(dt);
        if !self.open && self.progress.value() <= 0.001 {
            // The panel is back inside its island: the morph origin can go.
            self.origin = None;
            self.origin_island = None;
        }
        if self.expanded.is_some()
            && ((!self.open && self.progress.value() <= 0.001)
                || (self.expand.target() <= 0.0 && self.expand.value() <= 0.001))
        {
            self.expanded = None;
            active = true;
        }
        active
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// Current spring-animated card size. Used by the compositor left margin,
    /// so the card stays glued to the same right edge while it expands.
    pub fn current_size(&self) -> Size {
        let Some(page) = self.expanded else {
            return self.size;
        };
        let target = self.expanded_size(page);
        let t = self.expand.value().clamp(0.0, 1.0);
        Size::new(
            self.size.w + (target.w - self.size.w) * t,
            self.size.h + (target.h - self.size.h) * t,
        )
    }

    fn expanded_size(&self, page: ControlPage) -> Size {
        match page {
            ControlPage::Wifi | ControlPage::Bluetooth => {
                Size::new(EXPANDED_WIDTH, EXPANDED_HEIGHT)
            }
            ControlPage::System => Size::new(EXPANDED_WIDTH, 512.0),
            ControlPage::Home | ControlPage::Power => self.size,
        }
    }

    /// Ask the existing card to grow into the full page. The panel is the
    /// same surface and the same rounded rectangle; only its geometry and
    /// content change, so there is no second side panel to animate in.
    pub fn expand_page(&mut self, page: ControlPage) {
        if !matches!(
            page,
            ControlPage::Wifi | ControlPage::Bluetooth | ControlPage::System
        ) {
            return;
        }
        self.expanded = Some(page);
        if self.reduce_motion {
            self.expand.snap_to(1.0);
        } else {
            self.expand.set_target(1.0);
        }
    }

    pub fn collapse_page(&mut self) {
        if self.reduce_motion {
            self.expanded = None;
            self.expand.snap_to(0.0);
        } else {
            self.expand.set_target(0.0);
        }
    }

    /// Page currently visible in the expanded card. During the first half of
    /// the expansion the compact summary is still the interactive content.
    pub fn expanded_page(&self) -> Option<ControlPage> {
        self.expanded
            .filter(|_| self.expand.value().clamp(0.0, 1.0) > 0.55)
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded.is_some()
    }

    pub fn anchor_x(&self) -> f32 {
        self.anchor_x
    }

    /// Kind of card currently shown, used by the shell and by tests.
    pub fn kind(&self) -> PopoverKind {
        self.kind
    }

    pub fn set_power_screen(&mut self, on: bool) {
        self.cc_power = on;
    }

    pub fn power_screen(&self) -> bool {
        self.cc_power
    }

    /// Island the card grows out of.
    pub fn origin_island(&self) -> Option<BarIsland> {
        self.origin_island
    }

    /// Override the card size: the notification center grows with the number
    /// of pending notifications.
    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    /// Size of the notification center card for the given notifications.
    pub fn notifications_size(entries: &[NotificationEntry]) -> Size {
        let mut height = 58.0;
        for entry in entries {
            height += notification_height(entry) + 8.0;
        }
        Size::new(360.0, height.clamp(96.0, 520.0))
    }

    /// Shape of the card while it morphs out of its island.
    pub fn shape_rect(&self) -> Rect {
        self.shape
    }

    /// Corner radius the panel is currently drawn with (it morphs from the
    /// island radius while the card inflates).
    pub fn shape_radius(&self) -> f32 {
        self.radius
    }

    /// Progress of the open/close animation.
    pub fn progress(&self) -> f32 {
        self.progress.value().clamp(0.0, 1.0)
    }

    /// Pointer motion. While a slider is grabbed this returns the new value,
    /// so dragging the knob (not just clicking the track) works.
    pub fn pointer_move(&mut self, pos: Point) -> Option<PopoverAction> {
        self.cursor = Some(pos);
        let drag = self.drag?;
        let track = match drag {
            CcSlider::Volume => self.volume_track,
            CcSlider::Brightness => self.brightness_track,
        };
        if track.w <= 0.0 {
            return None;
        }
        Some(match drag {
            CcSlider::Volume => PopoverAction::SetVolume(slider_value(track, pos.x)),
            CcSlider::Brightness => PopoverAction::SetBrightness(slider_value(track, pos.x)),
        })
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
        self.drag = None;
    }

    /// Button released: the drag ends and, if a slider was grabbed, its final
    /// value is returned so the shell can flush it past the throttle.
    pub fn pointer_release(&mut self) -> Option<PopoverAction> {
        let drag = self.drag.take()?;
        let track = match drag {
            CcSlider::Volume => self.volume_track,
            CcSlider::Brightness => self.brightness_track,
        };
        if track.w <= 0.0 {
            return None;
        }
        let pos = self.cursor?;
        Some(match drag {
            CcSlider::Volume => PopoverAction::SetVolume(slider_value(track, pos.x)),
            CcSlider::Brightness => PopoverAction::SetBrightness(slider_value(track, pos.x)),
        })
    }

    pub fn pointer_press(&mut self, pos: Point) -> Option<PopoverAction> {
        // The compact grid is not interactive while it morphs into a full
        // Wi-Fi/Bluetooth page, and the power submenu has no sliders.
        if self.kind == PopoverKind::ControlCenter && !self.cc_power && self.expanded.is_none() {
            if self.volume_track.contains(pos) {
                self.drag = Some(CcSlider::Volume);
                return Some(PopoverAction::SetVolume(slider_value(
                    self.volume_track,
                    pos.x,
                )));
            }
            if self.brightness_track.contains(pos) {
                self.drag = Some(CcSlider::Brightness);
                return Some(PopoverAction::SetBrightness(slider_value(
                    self.brightness_track,
                    pos.x,
                )));
            }
        }
        if let Some((_, action)) = self.hits.iter().rev().find(|(rect, _)| rect.contains(pos)) {
            return Some(action.clone());
        }
        if !self.panel.contains(pos) {
            self.close();
        }
        None
    }

    /// Mouse-wheel support over the Control Center sliders. Returns the same
    /// actions a click would, so the shell updates the service immediately.
    pub fn axis(&mut self, delta: f32) -> Option<PopoverAction> {
        let cursor = self.cursor?;
        if self.kind != PopoverKind::ControlCenter || self.cc_power || self.expanded.is_some() {
            return None;
        }
        let step = if delta < 0.0 { 5 } else { -5 };
        if self.volume_track.contains(cursor) {
            return Some(PopoverAction::SetVolume(
                (i16::from(self.volume_value) + step).clamp(0, 100) as u8,
            ));
        }
        if self.brightness_track.contains(cursor) {
            return Some(PopoverAction::SetBrightness(
                (i16::from(self.brightness_value) + step).clamp(0, 100) as u8,
            ));
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        data: &PopoverData<'_>,
        config: &Config,
        left: f32,
        embedded: Option<(&mut ControlCenter, &ControlData<'_>)>,
    ) {
        let p = self.progress.value().clamp(0.0, 1.0);
        if p <= 0.001 {
            return;
        }
        self.hits.clear();
        self.volume_track = Rect::ZERO;
        self.brightness_track = Rect::ZERO;
        let theme = &config.theme;
        let top = config.bar.height + config.bar.margin + 6.0;
        let e = self.expand.value().clamp(0.0, 1.0);
        let target = self
            .expanded
            .map(|page| self.expanded_size(page))
            .unwrap_or(self.size);
        let expanded_w = self.size.w + (target.w - self.size.w) * e;
        let expanded_h =
            (self.size.h + (target.h - self.size.h) * e).min((size.h - top - 12.0).max(240.0));
        // The card takes the place of the island it was opened from: the
        // surface starts on the island row (where the bar is) and grows
        // downwards, while the content stays laid out below the bar.
        let content = Rect::new(left, top, expanded_w, expanded_h);
        let shape = Rect::new(left, 0.0, expanded_w, expanded_h + top);
        self.panel = shape;
        self.content = content;
        // The shape lands on its final geometry early; the content then fades
        // in over a panel that is already in place, so text is never drawn
        // outside the growing rectangle.
        let eased = smoothstep(0.0, 0.62, p);
        // The card keeps the corner of the island it came from pinned while it
        // inflates, so it never slides out from under the island.
        let anchor = match self.origin_island {
            Some(BarIsland::Left) => MorphAnchor::Left,
            Some(BarIsland::Center) => MorphAnchor::Center,
            _ => MorphAnchor::Right,
        };
        let (panel, radius) = match self.origin {
            Some((origin, origin_radius)) => (
                morph_rect(origin, shape, eased, anchor),
                lerp_f32(origin_radius, theme.radius.card(), eased),
            ),
            None => (
                Rect::new(left, top + (1.0 - p) * -8.0, expanded_w, expanded_h)
                    .scale_centered(0.96 + 0.04 * p),
                theme.radius.card(),
            ),
        };
        self.shape = panel;
        self.radius = radius;
        // The shape is solid as soon as it starts growing and the content
        // only appears when the shape has reached its final size: the content
        // must never be drawn outside the panel that is still growing.
        let shape_p = if self.origin.is_some() {
            (p / 0.25).min(1.0)
        } else {
            p
        };
        let content_p = if self.origin.is_some() {
            // The card lands before its content, but not by so much that the
            // content appears when everything has already stopped.
            smoothstep(0.62, 0.95, p)
        } else {
            p
        };
        // Barely-visible scrim: focuses the card and captures outside clicks.
        scene.rect(
            Rect::new(0.0, 0.0, size.w, size.h),
            theme.background.with_alpha(0.05 * p),
            0.0,
        );
        scene.shadow(
            panel,
            radius,
            Shadow {
                color: theme.shadow.scale_alpha(0.7 * shape_p),
                blur: (theme.shadow_blur * 1.2).clamp(8.0, 48.0),
                offset_y: theme.shadow_offset_y.clamp(4.0, 20.0),
                spread: 1.0,
            },
        );
        scene.bordered(
            panel,
            theme
                .surface
                .with_alpha(crate::widgets::panel_alpha(config, 0.72) * shape_p),
            radius,
            theme.border_width,
            theme.border.scale_alpha(p),
        );

        if let Some(page) = self.expanded {
            // The compact summary fades out quickly while the rounded card
            // grows; the full page then fades/reveals in place as the same
            // card becomes tall enough for its list.
            let compact_p = content_p * (1.0 - smoothstep(0.04, 0.34, e));
            if compact_p > 0.005 {
                self.build_kind(scene, content, data, config, compact_p);
                // Compact controls are not interactive during the morph.
                self.hits.clear();
            }
            if e > 0.30
                && let Some((control, control_data)) = embedded
            {
                let embedded_p = p * smoothstep(0.30, 0.80, e);
                if embedded_p > 0.005 {
                    control.build_embedded(scene, page, content, control_data, config, embedded_p);
                }
            }
        } else {
            self.build_kind(scene, content, data, config, content_p);
        }
    }

    fn build_kind(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        match self.kind {
            PopoverKind::Clock => self.build_clock(scene, panel, data, config, p),
            PopoverKind::Network => self.build_network(scene, panel, data, config, p),
            PopoverKind::Bluetooth => self.build_bluetooth(scene, panel, data, config, p),
            PopoverKind::Battery => self.build_battery(scene, panel, data, config, p),
            PopoverKind::Notifications => self.build_notifications(scene, panel, data, config, p),
            PopoverKind::System => self.build_system(scene, panel, data, config, p),
            PopoverKind::Power => self.build_power(scene, panel, config, p),
            PopoverKind::ControlCenter => self.build_control_center(scene, panel, data, config, p),
        }
    }

    fn title(&self, scene: &mut Scene, panel: Rect, text: &str, config: &Config, p: f32) {
        scene.text(
            text,
            Point::new(panel.x + 18.0, panel.y + 15.0),
            TextStyle {
                size: config.font.size_body,
                weight: config.font.weight_semibold,
                ..Default::default()
            },
            config.theme.text.scale_alpha(p),
        );
    }

    fn build_clock(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text(
            data.clock,
            Point::new(panel.x + 18.0, panel.y + 14.0),
            TextStyle {
                size: fonts.size_large,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            data.date,
            Point::new(panel.x + 18.0, panel.y + 52.0),
            TextStyle {
                size: fonts.size_small + 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.85 * p),
        );

        use chrono::{Datelike, Local};
        let today = Local::now();
        let first = chrono::NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
            .unwrap_or(today.date_naive());
        let first_weekday = first.weekday().num_days_from_monday();
        let days = chrono::NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
            .map(|next| (next - first).num_days() as u32)
            .unwrap_or_else(|| {
                let next = if today.month() == 12 {
                    chrono::NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
                } else {
                    chrono::NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
                };
                next.map(|n| (n - first).num_days() as u32).unwrap_or(30)
            });

        let weekday_headers = ["L", "M", "M", "G", "V", "S", "D"];
        let grid_x = panel.x + 14.0;
        let grid_y = panel.y + 80.0;
        let cell = 34.0;
        for (i, wd) in weekday_headers.iter().enumerate() {
            scene.text_center_y(
                wd,
                grid_x + i as f32 * cell,
                grid_y + 8.0,
                TextStyle {
                    size: 10.0,
                    weight: fonts.weight_medium,
                    align: TextAlign::Center,
                    max_width: Some(cell),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.75 * p),
            );
        }
        for day in 1..=days {
            let idx = first_weekday as i64 + day as i64 - 1;
            let col = idx % 7;
            let row = idx / 7;
            let cx = grid_x + col as f32 * cell;
            let cy = grid_y + 26.0 + row as f32 * cell;
            let is_today = day == today.day();
            if is_today {
                scene.rect(
                    Rect::new(cx + 5.0, cy + 1.0, 26.0, 26.0),
                    theme.accent.scale_alpha(0.9 * p),
                    13.0,
                );
            }
            scene.text_center_y(
                &day.to_string(),
                cx,
                cy + 14.0,
                TextStyle {
                    size: 11.0,
                    weight: if is_today {
                        fonts.weight_semibold
                    } else {
                        fonts.weight_regular
                    },
                    align: TextAlign::Center,
                    max_width: Some(cell),
                    ..Default::default()
                },
                if is_today {
                    theme.accent_text.scale_alpha(p)
                } else {
                    theme.text.scale_alpha(0.9 * p)
                },
            );
        }
    }

    fn build_network(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        self.title(scene, panel, "Network", config, p);
        let label = if data.wifi_enabled {
            data.network.label()
        } else {
            "Wi-Fi off".into()
        };
        scene.text(
            crate::widgets::truncate(&label, 24),
            Point::new(panel.x + 18.0, panel.y + 42.0),
            TextStyle {
                size: fonts.size_body,
                max_width: Some(panel.w - 110.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.92 * p),
        );
        scene.text(
            if !data.wifi_enabled {
                "Wi-Fi disabled"
            } else if data.network.is_connected() {
                "Connected"
            } else {
                "Not connected"
            },
            Point::new(panel.x + 18.0, panel.y + 62.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );
        self.toggle_switch(
            scene,
            Rect::new(panel.right() - 72.0, panel.y + 42.0, 52.0, 26.0),
            data.wifi_enabled,
            PopoverAction::ToggleWifi(!data.wifi_enabled),
            config,
            p,
        );
        self.manage_button(
            scene,
            Rect::new(panel.x + 14.0, panel.y + 96.0, panel.w - 28.0, 34.0),
            "Manage networks",
            PopoverAction::ExpandWifi,
            config,
            p,
        );
    }

    fn build_bluetooth(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        self.title(scene, panel, "Bluetooth", config, p);
        scene.text(
            if data.bluetooth.present {
                if data.bluetooth.powered {
                    "Active"
                } else {
                    "Off"
                }
            } else {
                "Unavailable"
            },
            Point::new(panel.x + 18.0, panel.y + 44.0),
            TextStyle {
                size: fonts.size_body,
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        if data.bluetooth.present {
            self.toggle_switch(
                scene,
                Rect::new(panel.right() - 72.0, panel.y + 40.0, 52.0, 26.0),
                data.bluetooth.powered,
                PopoverAction::ToggleBluetooth(!data.bluetooth.powered),
                config,
                p,
            );
        }
        if data.bluetooth.present {
            self.manage_button(
                scene,
                Rect::new(panel.x + 14.0, panel.y + 82.0, panel.w - 28.0, 34.0),
                "Manage devices",
                PopoverAction::ExpandBluetooth,
                config,
                p,
            );
        }
    }

    fn build_power(&mut self, scene: &mut Scene, panel: Rect, config: &Config, p: f32) {
        let theme = &config.theme;
        let fonts = &config.font;
        self.title(scene, panel, "Power", config, p);
        let actions = [
            (PowerAction::Lock, icons::LOCK),
            (PowerAction::Logout, icons::LOGOUT),
            (PowerAction::Suspend, icons::NIGHT),
            (PowerAction::Reboot, icons::RESTART),
            (PowerAction::PowerOff, icons::POWER),
        ];
        for (index, (action, icon)) in actions.iter().enumerate() {
            let row = Rect::new(
                panel.x + 10.0,
                panel.y + 42.0 + index as f32 * 37.0,
                panel.w - 20.0,
                33.0,
            );
            let hovered = self.cursor.is_some_and(|cursor| row.contains(cursor));
            let danger = *action == PowerAction::PowerOff;
            if hovered {
                scene.rect(
                    row,
                    if danger {
                        theme.danger.with_alpha(0.14 * p)
                    } else {
                        theme.background.scale_alpha(0.82 * p)
                    },
                    8.0,
                );
            }
            scene.text_center_y(
                icon,
                row.x + 14.0,
                row.center().y,
                TextStyle {
                    size: 15.0,
                    align: TextAlign::Center,
                    max_width: Some(20.0),
                    ..Default::default()
                }
                .icon(),
                if danger {
                    theme.danger.scale_alpha(0.95 * p)
                } else {
                    theme.text_muted.scale_alpha(0.85 * p)
                },
            );
            scene.text_center_y(
                action.label(),
                row.x + 40.0,
                row.center().y,
                TextStyle {
                    size: fonts.size_small,
                    weight: fonts.weight_medium,
                    max_width: Some(row.w - 50.0),
                    ..Default::default()
                },
                if danger {
                    theme.danger.scale_alpha(0.95 * p)
                } else {
                    theme.text.scale_alpha(0.92 * p)
                },
            );
            self.hits.push((row, PopoverAction::Power(*action)));
        }
    }

    fn manage_button(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        label: &str,
        action: PopoverAction,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let hovered = self.cursor.is_some_and(|cursor| rect.contains(cursor));
        scene.rect(
            rect,
            theme
                .background
                .scale_alpha(if hovered { 0.85 } else { 0.5 } * p),
            10.0,
        );
        scene.text_center_y(
            label,
            rect.x + 12.0,
            rect.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 38.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.88 * p),
        );
        scene.text_center_y(
            icons::CHEVRON_RIGHT,
            rect.right() - 28.0,
            rect.center().y,
            TextStyle {
                size: 13.0,
                align: TextAlign::Center,
                max_width: Some(16.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.55 * p),
        );
        self.hits.push((rect, action));
    }

    fn build_battery(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let battery = data.battery;
        self.title(scene, panel, "Battery", config, p);
        if !battery.present {
            scene.text(
                "No battery",
                Point::new(panel.x + 18.0, panel.y + 46.0),
                TextStyle {
                    size: fonts.size_body,
                    ..Default::default()
                },
                theme.text.scale_alpha(0.9 * p),
            );
            return;
        }

        // The reading at a glance: percentage, state, and how full the cell is.
        scene.text(
            format!("{}%", battery.percent),
            Point::new(panel.x + 18.0, panel.y + 34.0),
            TextStyle {
                size: 32.0,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            battery.label(),
            Point::new(panel.x + 18.0, panel.y + 46.0),
            TextStyle {
                size: fonts.size_small,
                align: TextAlign::Right,
                max_width: Some(panel.w - 36.0),
                ..Default::default()
            },
            if battery.charging {
                theme.accent.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.85 * p)
            },
        );
        let track = Rect::new(panel.x + 18.0, panel.y + 82.0, panel.w - 36.0, 6.0);
        scene.rect(track, theme.border.scale_alpha(0.45 * p), 3.0);
        let fill = (track.w * f32::from(battery.percent) / 100.0).max(3.0);
        scene.rect(
            Rect::new(track.x, track.y, fill, track.h),
            if battery.percent <= 15 {
                theme.danger.scale_alpha(p)
            } else {
                theme.accent.scale_alpha(p)
            },
            3.0,
        );

        let width = panel.w - 36.0;
        let mut y = panel.y + 104.0;
        let mut row = |label: &str, value: String| {
            self.stat_row(scene, panel.x + 18.0, y, width, label, &value, config, p);
            y += 25.0;
        };
        if battery.seconds_full > 0 {
            row("Time to full", format_duration(battery.seconds_full));
        }
        if battery.seconds_left > 0 {
            row("Time left", format_duration(battery.seconds_left));
        }
        if battery.power_w > 0.05 {
            let watts = if battery.charging || battery.status == lard_services::battery::BatteryStatus::NotCharging {
                battery.power_w
            } else {
                -battery.power_w
            };
            row("Power draw", format!("{watts:+.1} W"));
        }
        if battery.energy_full_wh > 0.0 {
            row(
                "Charge",
                format!(
                    "{:.1} / {:.1} Wh",
                    battery.energy_now_wh, battery.energy_full_wh
                ),
            );
        }
        if battery.health_percent > 0 {
            row(
                "Health",
                if battery.energy_full_design_wh > 0.0 {
                    format!(
                        "{}% · {:.1} Wh",
                        battery.health_percent, battery.energy_full_design_wh
                    )
                } else {
                    format!("{}%", battery.health_percent)
                },
            );
        }
        if battery.cycles > 0 {
            row("Cycles", battery.cycles.to_string());
        }
        if battery.voltage_v > 0.5 {
            row("Voltage", format!("{:.1} V", battery.voltage_v));
        }
        if !battery.charging && battery.external_power && battery.seconds_left == 0 {
            row("Power", "External".into());
        }
    }

    fn build_notifications(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        self.title(scene, panel, "Notifications", config, p);
        let count = data.notifications.len();
        let status = match count {
            0 => "No notifications".to_string(),
            1 => "1 notification".to_string(),
            n => format!("{n} notifications"),
        };
        scene.text(
            &status,
            Point::new(panel.x + 18.0, panel.y + 42.0),
            TextStyle {
                size: fonts.size_small,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.85 * p),
        );

        // The list of the pending notifications, with their actions.
        let mut y = panel.y + 60.0;
        for entry in &data.notifications {
            let height = notification_height(entry);
            let rect = Rect::new(panel.x + 12.0, y, panel.w - 24.0, height);
            scene.rect(
                rect,
                theme.surface_hover.scale_alpha(0.28 * p),
                theme.radius.card(),
            );
            scene.text(
                &entry.app,
                Point::new(rect.x + 12.0, rect.y + 8.0),
                TextStyle {
                    size: fonts.size_small - 0.5,
                    weight: fonts.weight_semibold,
                    max_width: Some(rect.w - 60.0),
                    ..Default::default()
                },
                theme.accent.scale_alpha(0.85 * p),
            );
            // The close button dismisses the notification.
            let close = Rect::new(rect.right() - 26.0, rect.y + 4.0, 22.0, 22.0);
            if self.cursor.is_some_and(|c| close.contains(c)) {
                scene.rect(
                    close,
                    theme.surface_hover.scale_alpha(0.7 * p),
                    theme.radius.control(),
                );
            }
            scene.text_center_y(
                "\u{d7}",
                close.x,
                close.center().y,
                TextStyle {
                    size: fonts.size_body,
                    align: TextAlign::Center,
                    max_width: Some(close.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.75 * p),
            );
            self.hits
                .push((close, PopoverAction::DismissNotification(entry.id)));
            scene.text(
                crate::widgets::truncate_to_width(
                    entry.summary.as_str(),
                    fonts.size_body - 0.5,
                    rect.w - 24.0,
                ),
                Point::new(rect.x + 12.0, rect.y + 24.0),
                TextStyle {
                    size: fonts.size_body - 0.5,
                    weight: fonts.weight_semibold,
                    max_width: Some(rect.w - 24.0),
                    wrap: false,
                    ..Default::default()
                },
                theme.text.scale_alpha(p),
            );
            if !entry.body.is_empty() {
                scene.text(
                    crate::widgets::truncate_to_width(
                        entry.body.as_str(),
                        fonts.size_small - 0.5,
                        rect.w - 24.0,
                    ),
                    Point::new(rect.x + 12.0, rect.y + 42.0),
                    TextStyle {
                        size: fonts.size_small - 0.5,
                        max_width: Some(rect.w - 24.0),
                        wrap: false,
                        ..Default::default()
                    },
                    theme.text_muted.scale_alpha(0.78 * p),
                );
            }
            if !entry.actions.is_empty() {
                let mut x = rect.x + 12.0;
                for (key, label) in entry.actions.iter().take(2) {
                    let width = (label.chars().count() as f32 * 6.2 + 24.0).min(140.0);
                    let button = Rect::new(x, rect.bottom() - 30.0, width, 24.0);
                    let hover = self.cursor.is_some_and(|c| button.contains(c));
                    scene.rect(
                        button,
                        theme
                            .accent
                            .with_alpha((if hover { 0.26 } else { 0.15 }) * p),
                        8.0,
                    );
                    scene.text_center_y(
                        crate::widgets::truncate(label, 18),
                        button.x,
                        button.center().y,
                        TextStyle {
                            size: fonts.size_small - 0.5,
                            weight: fonts.weight_medium,
                            align: TextAlign::Center,
                            max_width: Some(button.w),
                            ..Default::default()
                        },
                        theme.text.scale_alpha(0.92 * p),
                    );
                    self.hits.push((
                        button,
                        PopoverAction::NotificationAction {
                            id: entry.id,
                            key: key.clone(),
                        },
                    ));
                    x += width + 6.0;
                }
            }
            y += height + 8.0;
        }
    }

    fn build_system(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        self.title(scene, panel, "System", config, p);
        crate::widgets::system_monitor(scene, panel, data.system, config, p);
    }

    /// Label on the left, value right-aligned: the battery panel rows.
    fn stat_row(
        &self,
        scene: &mut Scene,
        x: f32,
        y: f32,
        width: f32,
        label: &str,
        value: &str,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text(
            label,
            Point::new(x, y),
            TextStyle {
                size: fonts.size_small,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.75 * p),
        );
        scene.text(
            value,
            Point::new(x, y),
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Right,
                max_width: Some(width),
                ..Default::default()
            },
            theme.text.scale_alpha(0.92 * p),
        );
    }

    // ---- grouped Control Center -------------------------------------------

    fn build_control_center(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        if self.cc_power {
            self.build_cc_power(scene, panel, data, config, p);
            return;
        }
        let theme = &config.theme;
        let fonts = &config.font;
        let bar = &config.bar;
        let pad = 14.0;
        let inner_x = panel.x + pad;
        let inner_w = panel.w - pad * 2.0;
        let gap = 8.0;

        scene.text(
            "Control Center",
            Point::new(inner_x, panel.y + 13.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_semibold,
                max_width: Some(inner_w - 38.0),
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );

        let gear = Rect::new(panel.right() - pad - 26.0, panel.y + 9.0, 26.0, 26.0);
        if self.cursor.is_some_and(|cursor| gear.contains(cursor)) {
            scene.rect(
                gear,
                theme.surface_hover.scale_alpha(0.75 * p),
                theme.radius.control(),
            );
        }
        scene.text_center_y(
            icons::SETTINGS,
            gear.x,
            gear.center().y,
            TextStyle {
                size: 16.0,
                align: TextAlign::Center,
                max_width: Some(gear.w),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.85 * p),
        );
        self.hits.push((gear, PopoverAction::OpenSettings));

        let mut y = panel.y + 45.0;
        let show_wifi = bar.show_network;
        let show_bluetooth = bar.show_bluetooth && data.bluetooth.present;
        if show_wifi || show_bluetooth {
            let tile_h = 88.0;
            let columns = usize::from(show_wifi) + usize::from(show_bluetooth);
            let tile_w = if columns > 1 {
                (inner_w - gap) * 0.5
            } else {
                inner_w
            };
            let mut x = inner_x;
            if show_wifi {
                self.build_cc_wifi_tile(scene, Rect::new(x, y, tile_w, tile_h), data, config, p);
                x += tile_w + gap;
            }
            if show_bluetooth {
                self.build_cc_bluetooth_tile(
                    scene,
                    Rect::new(x, y, tile_w, tile_h),
                    data,
                    config,
                    p,
                );
            }
            y += tile_h + gap;
        }

        let tile_h = 88.0;
        let tile_w = (inner_w - gap) * 0.5;
        self.build_cc_dnd_tile(
            scene,
            Rect::new(inner_x, y, tile_w, tile_h),
            data,
            config,
            p,
        );
        self.build_cc_notifications_tile(
            scene,
            Rect::new(inner_x + tile_w + gap, y, tile_w, tile_h),
            data,
            config,
            p,
        );
        y += tile_h + gap;

        // CPU/RAM at a glance; the row is the door to the task manager.
        let system_row = Rect::new(inner_x, y, inner_w, 44.0);
        let row_hovered = self.cursor.is_some_and(|cursor| system_row.contains(cursor));
        scene.rect(
            system_row,
            theme
                .background
                .scale_alpha((if row_hovered { 0.85 } else { 0.55 }) * p),
            12.0,
        );
        let meter_w = (inner_w - 36.0) * 0.5;
        crate::widgets::meter(
            scene,
            Rect::new(inner_x + 12.0, y + 12.0, meter_w, 24.0),
            "CPU",
            data.system.cpu_percent,
            config,
            p,
        );
        crate::widgets::meter(
            scene,
            Rect::new(inner_x + 24.0 + meter_w, y + 12.0, meter_w, 24.0),
            "RAM",
            data.system.memory_percent,
            config,
            p,
        );
        self.hits.push((system_row, PopoverAction::ExpandSystem));
        y += 44.0 + gap;

        if data.brightness_present {
            self.build_cc_slider(
                scene,
                Rect::new(inner_x, y, inner_w, 56.0),
                CcSlider::Brightness,
                data,
                config,
                p,
            );
            y += 56.0 + gap;
        }
        if bar.show_volume {
            self.build_cc_slider(
                scene,
                Rect::new(inner_x, y, inner_w, 56.0),
                CcSlider::Volume,
                data,
                config,
                p,
            );
        }
        let footer_y = panel.bottom() - pad - 34.0;
        let footer_w = (inner_w - gap) * 0.5;
        self.build_cc_footer_button(
            scene,
            Rect::new(inner_x, footer_y, footer_w, 34.0),
            icons::LOCK,
            "Lock",
            PopoverAction::Power(PowerAction::Lock),
            false,
            config,
            p,
        );
        self.build_cc_footer_button(
            scene,
            Rect::new(inner_x + footer_w + gap, footer_y, footer_w, 34.0),
            icons::POWER,
            "Power",
            PopoverAction::ShowPower,
            false,
            config,
            p,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn cc_tile_base(
        &self,
        scene: &mut Scene,
        rect: Rect,
        icon: &'static str,
        on: bool,
        interactive: bool,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let hovered = interactive && self.cursor.is_some_and(|cursor| rect.contains(cursor));
        scene.rect(
            rect,
            theme
                .background
                .scale_alpha((if hovered { 0.82 } else { 0.55 }) * p),
            theme.radius.card(),
        );
        scene.bordered(
            rect,
            Color::TRANSPARENT,
            theme.radius.card(),
            theme.border_width,
            theme
                .border
                .scale_alpha((if hovered { 0.75 } else { 0.42 }) * p),
        );
        let circle = Rect::new(rect.x + 12.0, rect.y + 12.0, 28.0, 28.0);
        scene.rect(
            circle,
            if on {
                theme.accent.scale_alpha(0.17 * p)
            } else {
                theme.surface_hover.scale_alpha(0.55 * p)
            },
            14.0,
        );
        scene.text_center_y(
            icon,
            circle.x,
            circle.center().y,
            TextStyle {
                size: 15.0,
                align: TextAlign::Center,
                max_width: Some(circle.w),
                ..Default::default()
            }
            .icon(),
            if on {
                theme.accent.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.8 * p)
            },
        );
    }

    fn build_cc_wifi_tile(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let on = data.wifi_enabled;
        self.cc_tile_base(
            scene,
            rect,
            if on { icons::WIFI } else { icons::WIFI_OFF },
            on,
            true,
            config,
            p,
        );
        scene.text_center_y(
            "Wi-Fi",
            rect.x + 50.0,
            rect.y + 24.0,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 72.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.94 * p),
        );
        let status = if !on {
            "Off".to_string()
        } else if data.network.is_connected() {
            data.network.label()
        } else {
            "Not connected".to_string()
        };
        scene.text_center_y(
            truncate(&status, if rect.w > 220.0 { 28 } else { 17 }),
            rect.x + 50.0,
            rect.y + 42.0,
            TextStyle {
                size: fonts.size_small - 1.0,
                max_width: Some(rect.w - 64.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.78 * p),
        );
        scene.text_center_y(
            icons::CHEVRON_RIGHT,
            rect.right() - 18.0,
            rect.y + 24.0,
            TextStyle {
                size: 11.0,
                align: TextAlign::Center,
                max_width: Some(14.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.45 * p),
        );
        self.hits.push((rect, PopoverAction::ExpandWifi));
        self.toggle_switch(
            scene,
            Rect::new(rect.right() - 64.0, rect.bottom() - 36.0, 52.0, 26.0),
            on,
            PopoverAction::ToggleWifi(!on),
            config,
            p,
        );
    }

    fn build_cc_bluetooth_tile(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let on = data.bluetooth.powered;
        self.cc_tile_base(
            scene,
            rect,
            if on {
                icons::BLUETOOTH
            } else {
                icons::BLUETOOTH_OFF
            },
            on,
            true,
            config,
            p,
        );
        scene.text_center_y(
            "Bluetooth",
            rect.x + 50.0,
            rect.y + 24.0,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 72.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.94 * p),
        );
        scene.text_center_y(
            if on { "Active" } else { "Off" },
            rect.x + 50.0,
            rect.y + 42.0,
            TextStyle {
                size: fonts.size_small - 1.0,
                max_width: Some(rect.w - 64.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.78 * p),
        );
        scene.text_center_y(
            icons::CHEVRON_RIGHT,
            rect.right() - 18.0,
            rect.y + 24.0,
            TextStyle {
                size: 11.0,
                align: TextAlign::Center,
                max_width: Some(14.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.45 * p),
        );
        self.hits.push((rect, PopoverAction::ExpandBluetooth));
        self.toggle_switch(
            scene,
            Rect::new(rect.right() - 64.0, rect.bottom() - 36.0, 52.0, 26.0),
            on,
            PopoverAction::ToggleBluetooth(!on),
            config,
            p,
        );
    }

    fn build_cc_dnd_tile(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        self.cc_tile_base(scene, rect, icons::DND, data.dnd, true, config, p);
        scene.text_center_y(
            "Do not disturb",
            rect.x + 50.0,
            rect.y + 24.0,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 62.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.94 * p),
        );
        scene.text_center_y(
            if data.dnd { "Active" } else { "Off" },
            rect.x + 50.0,
            rect.y + 42.0,
            TextStyle {
                size: fonts.size_small - 1.0,
                max_width: Some(rect.w - 64.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.78 * p),
        );
        self.hits.push((rect, PopoverAction::ToggleDnd));
        self.toggle_switch(
            scene,
            Rect::new(rect.right() - 64.0, rect.bottom() - 36.0, 52.0, 26.0),
            data.dnd,
            PopoverAction::ToggleDnd,
            config,
            p,
        );
    }

    fn build_cc_notifications_tile(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        self.cc_tile_base(
            scene,
            rect,
            icons::NOTIFICATIONS,
            data.active_toasts > 0,
            false,
            config,
            p,
        );
        scene.text_center_y(
            "Notifications",
            rect.x + 50.0,
            rect.y + 24.0,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 62.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.94 * p),
        );
        let status = if data.active_toasts == 1 {
            "1 active".to_string()
        } else if data.active_toasts > 1 {
            format!("{} active", data.active_toasts)
        } else {
            "None".to_string()
        };
        scene.text_center_y(
            &status,
            rect.x + 50.0,
            rect.y + 42.0,
            TextStyle {
                size: fonts.size_small - 1.0,
                max_width: Some(rect.w - 64.0),
                ..Default::default()
            },
            if data.active_toasts > 0 {
                theme.accent.scale_alpha(0.9 * p)
            } else {
                theme.text_muted.scale_alpha(0.78 * p)
            },
        );
        if data.active_toasts > 0 {
            scene.rect(
                Rect::new(rect.right() - 18.0, rect.y + 18.0, 6.0, 6.0),
                theme.accent,
                3.0,
            );
        }
    }

    fn build_cc_slider(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        slider: CcSlider,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let (label, value, icon, is_volume) = match slider {
            CcSlider::Volume => (
                "Volume",
                data.volume,
                if data.muted {
                    icons::VOLUME_OFF
                } else {
                    icons::VOLUME_UP
                },
                true,
            ),
            CcSlider::Brightness => ("Brightness", data.brightness, icons::BRIGHTNESS, false),
        };
        let muted = is_volume && data.muted;
        scene.rect(
            rect,
            theme.background.scale_alpha(0.55 * p),
            theme.radius.card(),
        );
        scene.bordered(
            rect,
            Color::TRANSPARENT,
            theme.radius.card(),
            theme.border_width,
            theme.border.scale_alpha(0.42 * p),
        );
        let icon_rect = Rect::new(rect.x + 10.0, rect.y + 12.0, 28.0, 28.0);
        let icon_hovered =
            is_volume && self.cursor.is_some_and(|cursor| icon_rect.contains(cursor));
        if icon_hovered {
            scene.rect(
                icon_rect,
                theme.surface_hover.scale_alpha(0.8 * p),
                theme.radius.pill(),
            );
        } else {
            scene.rect(
                icon_rect,
                theme.background.scale_alpha(0.8 * p),
                theme.radius.pill(),
            );
        }
        scene.text_center_y(
            icon,
            icon_rect.x,
            icon_rect.center().y,
            TextStyle {
                size: 15.0,
                align: TextAlign::Center,
                max_width: Some(icon_rect.w),
                ..Default::default()
            }
            .icon(),
            if muted {
                theme.text_muted.scale_alpha(0.8 * p)
            } else {
                theme.accent.scale_alpha(p)
            },
        );
        if is_volume {
            self.hits.push((icon_rect, PopoverAction::ToggleMute));
        }
        scene.text_center_y(
            label,
            rect.x + 48.0,
            rect.y + 18.0,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 120.0),
                ..Default::default()
            },
            theme.text.scale_alpha(0.92 * p),
        );
        let value_text = if muted {
            "Muted".to_string()
        } else {
            format!("{value}%")
        };
        scene.text_center_y(
            &value_text,
            rect.right() - 14.0,
            rect.y + 18.0,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_semibold,
                align: TextAlign::Right,
                max_width: Some(64.0),
                ..Default::default()
            },
            if muted {
                theme.text_muted.scale_alpha(0.8 * p)
            } else {
                theme.text.scale_alpha(0.94 * p)
            },
        );
        let track = Rect::new(rect.x + 48.0, rect.y + 35.0, rect.w - 62.0, 6.0);
        let track_hovered = self.cursor.is_some_and(|cursor| {
            Rect::new(track.x, track.y - 7.0, track.w, 20.0).contains(cursor)
        });
        scene.rect(
            track,
            theme
                .surface_hover
                .scale_alpha((if track_hovered { 0.95 } else { 0.72 }) * p),
            3.0,
        );
        let fill = track.w * f32::from(value.min(100)) / 100.0;
        if fill > 0.0 {
            scene.rect(
                Rect::new(track.x, track.y, fill.max(4.0), track.h),
                theme.accent.scale_alpha(if muted { 0.35 * p } else { p }),
                3.0,
            );
            let knob_x = (track.x + fill - 6.0).clamp(track.x, track.right() - 12.0);
            scene.rect(
                Rect::new(knob_x, track.y - 3.0, 12.0, 12.0),
                theme
                    .accent_text
                    .scale_alpha(if muted { 0.7 * p } else { p }),
                6.0,
            );
        }
        let hit = Rect::new(track.x, track.y - 7.0, track.w, 20.0);
        match slider {
            CcSlider::Volume => {
                self.volume_track = hit;
                self.volume_value = value;
            }
            CcSlider::Brightness => {
                self.brightness_track = hit;
                self.brightness_value = value;
            }
        }
    }
    fn build_cc_footer_button(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        icon: &'static str,
        label: &str,
        action: PopoverAction,
        danger: bool,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let hovered = self.cursor.is_some_and(|cursor| rect.contains(cursor));
        scene.rect(
            rect,
            if hovered {
                if danger {
                    theme.danger.with_alpha(0.14 * p)
                } else {
                    theme.surface_hover.scale_alpha(0.8 * p)
                }
            } else {
                theme.background.scale_alpha(0.5 * p)
            },
            theme.radius.control(),
        );
        scene.text_center_y(
            icon,
            rect.x + 12.0,
            rect.center().y,
            TextStyle {
                size: 14.0,
                align: TextAlign::Center,
                max_width: Some(20.0),
                ..Default::default()
            }
            .icon(),
            if danger {
                theme.danger.scale_alpha(0.95 * p)
            } else {
                theme.text_muted.scale_alpha(0.85 * p)
            },
        );
        scene.text_center_y(
            label,
            rect.x + 36.0,
            rect.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                max_width: Some(rect.w - 44.0),
                ..Default::default()
            },
            if danger {
                theme.danger.scale_alpha(0.95 * p)
            } else {
                theme.text.scale_alpha(0.9 * p)
            },
        );
        self.hits.push((rect, action));
    }

    fn build_cc_power(
        &mut self,
        scene: &mut Scene,
        panel: Rect,
        data: &PopoverData<'_>,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let pad = 14.0;
        let inner_w = panel.w - pad * 2.0;
        let back = Rect::new(panel.x + pad, panel.y + 9.0, 28.0, 28.0);
        if self.cursor.is_some_and(|cursor| back.contains(cursor)) {
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
                size: 17.0,
                align: TextAlign::Center,
                max_width: Some(back.w),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.9 * p),
        );
        self.hits.push((back, PopoverAction::BackControl));
        scene.text_center_y(
            "Power",
            panel.x + pad + 40.0,
            panel.y + 23.0,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_semibold,
                max_width: Some(inner_w - 40.0),
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            "Choose an action for the system",
            Point::new(panel.x + pad, panel.y + 50.0),
            TextStyle {
                size: fonts.size_small,
                max_width: Some(inner_w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.72 * p),
        );
        let actions = [
            (
                PowerAction::Lock,
                icons::LOCK,
                "Lock",
                "Lock the session",
            ),
            (
                PowerAction::Logout,
                icons::LOGOUT,
                "Log out",
                "Close the session",
            ),
            (
                PowerAction::Suspend,
                icons::NIGHT,
                "Suspend",
                "Suspend the system",
            ),
            (
                PowerAction::Reboot,
                icons::RESTART,
                "Restart",
                "Restart the system",
            ),
            (
                PowerAction::PowerOff,
                icons::POWER,
                "Power off",
                "Power off the system",
            ),
        ];
        let mut y = panel.y + 76.0;
        for (action, icon, label, description) in actions {
            let row = Rect::new(panel.x + pad, y, inner_w, 64.0);
            let hovered = self.cursor.is_some_and(|cursor| row.contains(cursor));
            let danger = action == PowerAction::PowerOff;
            if hovered {
                scene.rect(
                    row,
                    if danger {
                        theme.danger.with_alpha(0.14 * p)
                    } else {
                        theme.surface_hover.scale_alpha(0.75 * p)
                    },
                    theme.radius.card(),
                );
            } else {
                scene.rect(
                    row,
                    theme.background.scale_alpha(0.45 * p),
                    theme.radius.card(),
                );
            }
            let circle = Rect::new(row.x + 12.0, row.y + 17.0, 30.0, 30.0);
            scene.rect(
                circle,
                if danger {
                    theme.danger.with_alpha(0.14 * p)
                } else {
                    theme.accent.scale_alpha(0.14 * p)
                },
                15.0,
            );
            scene.text_center_y(
                icon,
                circle.x,
                circle.center().y,
                TextStyle {
                    size: 15.0,
                    align: TextAlign::Center,
                    max_width: Some(circle.w),
                    ..Default::default()
                }
                .icon(),
                if danger {
                    theme.danger.scale_alpha(0.95 * p)
                } else {
                    theme.accent.scale_alpha(p)
                },
            );
            scene.text(
                label,
                Point::new(row.x + 54.0, row.y + 13.0),
                TextStyle {
                    size: fonts.size_small,
                    weight: fonts.weight_semibold,
                    max_width: Some(row.w - 82.0),
                    ..Default::default()
                },
                if danger {
                    theme.danger.scale_alpha(0.95 * p)
                } else {
                    theme.text.scale_alpha(0.94 * p)
                },
            );
            scene.text(
                description,
                Point::new(row.x + 54.0, row.y + 34.0),
                TextStyle {
                    size: 10.0,
                    max_width: Some(row.w - 82.0),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.7 * p),
            );
            scene.text_center_y(
                icons::CHEVRON_RIGHT,
                row.right() - 20.0,
                row.center().y,
                TextStyle {
                    size: 13.0,
                    align: TextAlign::Center,
                    max_width: Some(16.0),
                    ..Default::default()
                }
                .icon(),
                theme.text_muted.scale_alpha(0.4 * p),
            );
            self.hits.push((row, PopoverAction::Power(action)));
            y += 72.0;
        }
        if data.battery.present {
            scene.text(
                format!(
                    "Battery {}%{}",
                    data.battery.percent,
                    if data.battery.charging {
                        " · charging"
                    } else {
                        ""
                    }
                ),
                Point::new(panel.x + pad, panel.bottom() - 30.0),
                TextStyle {
                    size: fonts.size_small,
                    max_width: Some(inner_w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.62 * p),
            );
        }
    }

    fn toggle_switch(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        on: bool,
        action: PopoverAction,
        config: &Config,
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
            13.0,
        );
        scene.rect(
            Rect::new(
                rect.x + if on { 28.0 } else { 2.0 },
                rect.y + 2.0,
                22.0,
                22.0,
            ),
            if on {
                theme.accent_text.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.8 * p)
            },
            11.0,
        );
        self.hits.push((rect, action));
    }
}

#[derive(Clone, Copy)]
enum CcSlider {
    Volume,
    Brightness,
}

/// Height of one notification in the notification center.
fn notification_height(entry: &NotificationEntry) -> f32 {
    let mut height = 40.0 + if entry.body.is_empty() { 0.0 } else { 16.0 };
    if !entry.actions.is_empty() {
        height += 30.0;
    }
    height
}

/// The morph of the card: the centre travels from the island to the final
/// place while the rectangle inflates. Interpolating the edges instead would
/// move the top-left corner first and the card would look like it appears
/// somewhere else.
fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

fn slider_value(track: Rect, x: f32) -> u8 {
    let t = ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
    (t * 100.0).round() as u8
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0).max(f32::EPSILON)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}


/// "2 h 14 min" / "47 min" from a number of seconds.
fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lard_services::battery::BatteryState;
    use lard_services::bluetooth::BluetoothState;
    use lard_services::network::NetworkState;
    use lard_services::system::SystemStats;

    fn data(network: &NetworkState) -> PopoverData<'_> {
        // `PopoverData` borrows the system snapshot: give the tests one that
        // lives long enough instead of a temporary.
        static SYSTEM: std::sync::OnceLock<SystemStats> = std::sync::OnceLock::new();
        PopoverData {
            clock: "12:00",
            date: "Tue 27 Oct",
            network,
            bluetooth: BluetoothState {
                present: true,
                powered: true,
                discovering: false,
            },
            wifi_enabled: true,
            battery: BatteryState::default(),
            system: SYSTEM.get_or_init(SystemStats::default),
            dnd: false,
            active_toasts: 0,
            volume: 50,
            muted: false,
            brightness: 70,
            brightness_present: true,
            notifications: Vec::new(),
        }
    }

    fn manage_action(
        config: &Config,
        kind: PopoverKind,
        matcher: fn(&PopoverAction) -> bool,
    ) -> PopoverAction {
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, true);
        popover.open(kind, 100.0, None);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            config,
            100.0,
            None,
        );
        let rect = popover
            .hits
            .iter()
            .find(|(_, action)| matcher(action))
            .map(|(rect, _)| *rect)
            .expect("manage button");
        popover
            .pointer_press(rect.center())
            .expect("manage button is clickable")
    }

    fn wifi_manage(action: &PopoverAction) -> bool {
        matches!(action, PopoverAction::ExpandWifi)
    }

    fn bluetooth_manage(action: &PopoverAction) -> bool {
        matches!(action, PopoverAction::ExpandBluetooth)
    }

    #[test]
    fn wifi_popover_expands_its_manage_button() {
        let config = Config::default();
        let action = manage_action(&config, PopoverKind::Network, wifi_manage);
        assert!(wifi_manage(&action));
    }

    #[test]
    fn bluetooth_popover_expands_its_manage_button() {
        let config = Config::default();
        let action = manage_action(&config, PopoverKind::Bluetooth, bluetooth_manage);
        assert!(bluetooth_manage(&action));
    }

    #[test]
    fn manage_grows_the_existing_card() {
        let config = Config::default();
        let mut popover = Popover::new(config.animation.panel, true);
        popover.open(PopoverKind::Network, 1500.0, None);
        popover.expand_page(ControlPage::Wifi);
        let expanded = popover.current_size();
        assert!(expanded.w > Popover::preferred_size(PopoverKind::Network).w);
        assert_eq!(popover.expanded_page(), Some(ControlPage::Wifi));
        popover.collapse_page();
        popover.update(1.0 / 60.0);
        assert_eq!(
            popover.current_size(),
            Popover::preferred_size(PopoverKind::Network)
        );
    }

    #[test]
    fn grouped_control_center_has_tiles_and_sliders() {
        let mut config = Config::default();
        config.bar.show_volume = true;
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, true);
        popover.open(PopoverKind::ControlCenter, 1500.0, None);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1230.0,
            None,
        );
        let labels: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        for label in [
            "Control Center",
            "Wi-Fi",
            "Bluetooth",
            "Do not disturb",
            "Notifications",
            "Brightness",
            "Volume",
            "Lock",
            "Power",
        ] {
            assert!(
                labels.contains(&label),
                "expected grouped control {label}: {labels:?}"
            );
        }
        let volume = popover.volume_track;
        assert!(volume.w > 0.0);
        assert!(matches!(
            popover.pointer_press(volume.center()),
            Some(PopoverAction::SetVolume(50))
        ));
        popover.pointer_move(volume.center());
        assert!(matches!(
            popover.axis(-1.0),
            Some(PopoverAction::SetVolume(55))
        ));
        assert!(matches!(
            popover.axis(1.0),
            Some(PopoverAction::SetVolume(45))
        ));
    }

    #[test]
    fn sliders_can_be_dragged_while_the_button_is_held() {
        let mut config = Config::default();
        config.bar.show_volume = true;
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, true);
        popover.open(PopoverKind::ControlCenter, 1500.0, None);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1230.0,
            None,
        );

        // Volume: press on the track, then drag to the right edge. The knob
        // must follow the pointer (a click-only slider was the bug).
        let volume = popover.volume_track;
        assert!(matches!(
            popover.pointer_press(volume.center()),
            Some(PopoverAction::SetVolume(50))
        ));
        let right = Point::new(volume.right() - 0.5, volume.center().y);
        assert!(matches!(
            popover.pointer_move(right),
            Some(PopoverAction::SetVolume(100))
        ));
        let left = Point::new(volume.x + 1.0, volume.center().y);
        assert!(matches!(
            popover.pointer_move(left),
            Some(PopoverAction::SetVolume(0))
        ));
        assert!(matches!(
            popover.pointer_release(),
            Some(PopoverAction::SetVolume(0))
        ));
        // After the release the knob is no longer grabbed.
        assert!(popover.pointer_move(right).is_none());

        // Brightness: same drag, on the brightness track.
        let brightness = popover.brightness_track;
        assert!(brightness.w > 0.0);
        assert!(matches!(
            popover.pointer_press(brightness.center()),
            Some(PopoverAction::SetBrightness(50))
        ));
        assert!(matches!(
            popover.pointer_move(Point::new(brightness.right() - 0.5, brightness.center().y)),
            Some(PopoverAction::SetBrightness(100))
        ));
        assert!(matches!(
            popover.pointer_release(),
            Some(PopoverAction::SetBrightness(100))
        ));
    }

    #[test]
    fn grouped_control_center_exposes_expand_and_power_screens() {
        let config = Config::default();
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, true);
        popover.open(PopoverKind::ControlCenter, 1500.0, None);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1230.0,
            None,
        );
        let wifi = popover
            .hits
            .iter()
            .find(|(_, action)| matches!(action, PopoverAction::ExpandWifi))
            .map(|(rect, _)| *rect)
            .expect("wifi tile");
        assert!(matches!(
            popover.pointer_press(wifi.center()),
            Some(PopoverAction::ExpandWifi)
        ));
        let power = popover
            .hits
            .iter()
            .find(|(_, action)| matches!(action, PopoverAction::ShowPower))
            .map(|(rect, _)| *rect)
            .expect("power footer button");
        assert!(matches!(
            popover.pointer_press(power.center()),
            Some(PopoverAction::ShowPower)
        ));
        popover.set_power_screen(true);
        assert!(popover.power_screen());
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1230.0,
            None,
        );
        let labels: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        for label in ["Lock", "Log out", "Suspend", "Restart", "Power off"] {
            assert!(
                labels.contains(&label),
                "expected power action {label}: {labels:?}"
            );
        }
        let power_off = popover
            .hits
            .iter()
            .find(|(_, action)| matches!(action, PopoverAction::Power(PowerAction::PowerOff)))
            .map(|(rect, _)| *rect)
            .expect("power off row");
        assert!(matches!(
            popover.pointer_press(power_off.center()),
            Some(PopoverAction::Power(PowerAction::PowerOff))
        ));
    }

    #[test]
    fn control_center_system_row_expands_into_the_monitor() {
        let config = Config::default();
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, false);
        popover.open(PopoverKind::ControlCenter, 800.0, None);
        for _ in 0..60 {
            popover.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1230.0,
            None,
        );
        let row = popover
            .hits
            .iter()
            .find(|(_, action)| matches!(action, PopoverAction::ExpandSystem))
            .map(|(rect, _)| *rect)
            .expect("the CPU/RAM row must be clickable");
        assert!(matches!(
            popover.pointer_press(row.center()),
            Some(PopoverAction::ExpandSystem)
        ));
        let compact_height = popover.current_size().h;
        popover.expand_page(ControlPage::System);
        // The page becomes interactive once the card has grown.
        for _ in 0..60 {
            popover.update(1.0 / 60.0);
        }
        assert_eq!(popover.expanded_page(), Some(ControlPage::System));
        assert!(
            popover.current_size().h > compact_height,
            "the card must grow for the monitor"
        );
    }

    #[test]
    fn the_card_starts_on_the_island() {
        let config = Config::default();
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, false);
        let island = Rect::new(1400.0, 0.0, 54.0, 38.0);
        popover.open(
            PopoverKind::ControlCenter,
            1427.0,
            Some((island, 30.0, BarIsland::Right)),
        );
        // Right after the opening the card still sits on the island.
        popover.update(1.0 / 240.0);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1427.0,
            None,
        );
        let shape = popover.shape_rect();
        assert!(
            shape.x > island.x - 40.0 && shape.right() < island.right() + 40.0,
            "{shape:?} vs {island:?}"
        );
        // While it grows the card stays glued to the island edge.
        assert!(
            (shape.right() - island.right()).abs() < 2.0,
            "{shape:?} vs {island:?}"
        );
        // At the end the card has taken the place of the island: it starts on
        // the bar row and its content stays below the bar.
        for _ in 0..200 {
            popover.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            1427.0,
            None,
        );
        let shape = popover.shape_rect();
        assert!(shape.w > 300.0 && shape.h > 200.0, "{shape:?}");
        assert!(shape.y < 1.0, "{shape:?}");
        let content = popover.content_rect();
        assert!(content.y > 40.0, "{content:?}");
    }

    #[test]
    fn popover_morphs_out_of_its_island() {
        let config = Config::default();
        let mut popover = Popover::new(config.animation.panel, false);
        let island = Rect::new(600.0, 0.0, 400.0, 38.0);
        popover.open(
            PopoverKind::ControlCenter,
            800.0,
            Some((island, 30.0, BarIsland::Right)),
        );
        assert_eq!(popover.origin_island(), Some(BarIsland::Right));
        // Right after the opening the card still sits on the island.
        popover.update(1.0 / 240.0);
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            600.0,
            None,
        );
        let shape = popover.shape_rect();
        assert!(
            shape.x < island.right() && shape.right() > island.x,
            "{shape:?} vs {island:?}"
        );
        // The card grows anchored to the island edge, it never slides out
        // from under it.
        assert!(
            (shape.right() - island.right()).abs() < 2.0,
            "{shape:?} vs {island:?}"
        );
        // Once the animation is over the card has taken the place of the
        // island, on the bar row.
        for _ in 0..200 {
            popover.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            600.0,
            None,
        );
        let shape = popover.shape_rect();
        assert!(shape.w > 300.0 && shape.y < 1.0, "{shape:?}");
    }

    #[test]
    fn power_popover_has_every_session_action() {
        let config = Config::default();
        let network = NetworkState::Disconnected;
        let data = data(&network);
        let mut popover = Popover::new(config.animation.panel, true);
        popover.open(PopoverKind::Power, 100.0, None);
        let mut scene = Scene::new();
        popover.build(
            &mut scene,
            Size::new(1600.0, 900.0),
            &data,
            &config,
            100.0,
            None,
        );
        let labels: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        for label in ["Lock", "Log out", "Suspend", "Restart", "Power off"] {
            assert!(
                labels.contains(&label),
                "expected power action {label}: {labels:?}"
            );
        }
        let rect = popover
            .hits
            .iter()
            .find(|(_, action)| matches!(action, PopoverAction::Power(PowerAction::PowerOff)))
            .map(|(rect, _)| *rect)
            .expect("power off row");
        assert!(matches!(
            popover.pointer_press(rect.center()),
            Some(PopoverAction::Power(PowerAction::PowerOff))
        ));
    }
}
