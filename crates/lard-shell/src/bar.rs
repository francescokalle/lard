//! Dank-style floating bar: three translucent islands (workspaces + title,
//! clock, status) with a spring-animated workspace indicator.

use crate::ShellAction;
use crate::bar_icons;
use crate::icons;
use crate::osd::OsdState;
use crate::popover::PopoverKind;
use crate::toasts::ToastHeadline;
use crate::widgets::{approx_width, truncate, truncate_to_width};
use lard_core::anim::{Spring, SpringConfig};
use lard_core::theme::{BarConfig, FontConfig};
use lard_core::{Color, Point, Rect, Size};
use lard_render::{Scene, TextAlign, TextStyle};
use lard_services::battery::BatteryState;
use lard_services::bluetooth::BluetoothState;
use lard_services::network::NetworkState;
use lard_services::niri::NiriSnapshot;
use lard_services::system::SystemStats;

/// The three floating islands of the bar. Panels opened from an island grow
/// out of it, so both sides need to agree on which island is which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarIsland {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarHit {
    Workspace(u64),
    Clock,
    /// Bell in the centre island: opens the notification center.
    Notifications,
    ControlCenter,
}

pub struct BarPress {
    pub action: Option<ShellAction>,
    pub popover: Option<(PopoverKind, f32)>,
}

#[derive(Clone, Debug)]
struct Pill {
    ws_id: u64,
    rect: Rect,
    label: String,
    focused: bool,
    active: bool,
    urgent: bool,
    has_windows: bool,
}

pub struct Bar {
    output: String,
    pills: Vec<Pill>,
    indicator_x: Spring,
    indicator_width: Spring,
    indicator_alpha: Spring,
    /// Width of the centre island. Animated so the clock/HUD swap and clock
    /// text changes never snap the island to a new size.
    center_width: Spring,
    center_initialized: bool,
    initialized: bool,
    invalidated: bool,
    hits: Vec<(Rect, BarHit)>,
    /// Rounded rectangles of the drawn islands, in surface-local coordinates.
    /// The compositor uses them to clip the backdrop blur to the islands and
    /// to know where a panel has to grow from.
    islands: Vec<(BarIsland, Rect)>,
    /// Where the centre island lives even when a notification takes it over:
    /// panels opened from it are anchored there.
    center_home: Rect,
    cursor: Option<Point>,
    hovered: Option<BarHit>,
}

pub struct BarData<'a> {
    pub niri: &'a NiriSnapshot,
    pub volume: u8,
    pub muted: bool,
    /// Active volume/brightness HUD, rendered in place of the clock.
    pub osd: Option<OsdState>,
    pub battery: BatteryState,
    pub network: &'a NetworkState,
    pub bluetooth: BluetoothState,
    pub wifi_enabled: bool,
    pub system: SystemStats,
    pub clock: &'a str,
    pub date: &'a str,
    pub dnd: bool,
    pub active_toasts: usize,
    /// Newest notification, shown in place of the clock in the centre island.
    pub toast: Option<ToastHeadline>,
    /// Notifications waiting in the notification center.
    pub notification_count: usize,
    /// Island that is currently growing into a panel, with the progress of the
    /// morph: the island fades out while the panel takes its place.
    pub morph: Option<(BarIsland, f32)>,
}

impl Bar {
    pub fn new(output: &str) -> Self {
        let spring = SpringConfig::SNAPPY;
        Self {
            output: output.to_string(),
            pills: Vec::with_capacity(10),
            indicator_x: Spring::new(0.0, spring),
            indicator_width: Spring::new(0.0, spring),
            indicator_alpha: Spring::new(0.0, spring),
            center_width: Spring::new(0.0, spring),
            center_initialized: false,
            initialized: false,
            invalidated: true,
            hits: Vec::with_capacity(16),
            islands: Vec::with_capacity(3),
            center_home: Rect::ZERO,
            cursor: None,
            hovered: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.invalidated = true;
    }

    /// Rectangles of the islands currently drawn, used for the backdrop blur.
    pub fn island_rects(&self) -> &[(BarIsland, Rect)] {
        &self.islands
    }

    /// Rectangle of the centre island: the drawn one, or its home when a
    /// notification has taken it over.
    pub fn center_island_rect(&self) -> Rect {
        self.island(BarIsland::Center).unwrap_or(self.center_home)
    }

    /// Surface-local rectangle of one island.
    pub fn island(&self, island: BarIsland) -> Option<Rect> {
        self.islands
            .iter()
            .find(|(kind, _)| *kind == island)
            .map(|(_, rect)| *rect)
    }

    /// Surface-local x of the grouped Control Center button, used to anchor
    /// the popover when it is opened from a keybind instead of a click.
    pub fn control_center_anchor(&self) -> Option<f32> {
        self.hits
            .iter()
            .find(|(_, hit)| *hit == BarHit::ControlCenter)
            .map(|(rect, _)| rect.center().x)
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let mut active = self.indicator_x.update(dt);
        active |= self.indicator_width.update(dt);
        active |= self.indicator_alpha.update(dt);
        active |= self.center_width.update(dt);
        active
    }

    pub fn pointer_move(&mut self, pos: Point) -> bool {
        self.cursor = Some(pos);
        let hovered = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, hit)| *hit);
        if hovered != self.hovered {
            self.hovered = hovered;
            true
        } else {
            false
        }
    }

    pub fn pointer_leave(&mut self) -> bool {
        self.cursor = None;
        self.hovered.take().is_some()
    }

    /// Synchronise workspace pills. Only called when niri state changed, so
    /// labels are not rebuilt every frame.
    pub(crate) fn sync(&mut self, niri: &NiriSnapshot, bar: &BarConfig, fonts: &FontConfig) {
        if !self.invalidated {
            return;
        }
        self.invalidated = false;
        self.pills.clear();

        let mut x = bar.padding;

        if bar.show_workspaces {
            let same_output = |ws: &&lard_services::niri::WorkspaceView| {
                ws.output.as_deref() == Some(self.output.as_str())
                    || ws.output.is_none()
                    || self.output.is_empty()
            };
            let mut candidates: Vec<&lard_services::niri::WorkspaceView> =
                niri.workspaces.iter().filter(same_output).collect();
            if candidates.is_empty() {
                candidates = niri.workspaces.iter().collect();
            }

            for ws in candidates {
                let label = if bar.workspace_labels.use_names {
                    ws.name
                        .as_deref()
                        .filter(|n| !n.is_empty())
                        .map(|n| truncate(n, 12))
                        .unwrap_or_else(|| u16::from(ws.idx).to_string())
                } else {
                    u16::from(ws.idx).to_string()
                };
                let width = (approx_width(&label, fonts.size_small) + 22.0).max(30.0);
                let rect = Rect::new(x, (bar.height - 26.0) * 0.5, width, 26.0);
                self.pills.push(Pill {
                    ws_id: ws.id,
                    rect,
                    label,
                    focused: ws.is_focused,
                    active: ws.is_active,
                    urgent: ws.is_urgent,
                    has_windows: ws.has_windows,
                });
                x += width + bar.spacing.max(2.0);
            }
        }

        if bar.workspace_labels.hide_empty {
            self.pills
                .retain(|pill| pill.has_windows || pill.focused || pill.active);
        }

        let target = self
            .pills
            .iter()
            .find(|pill| pill.focused)
            .or_else(|| self.pills.iter().find(|pill| pill.active));
        if let Some(pill) = target {
            let target_x = pill.rect.x - 2.0;
            let target_w = pill.rect.w + 4.0;
            if !self.initialized {
                self.indicator_x.snap_to(target_x);
                self.indicator_width.snap_to(target_w);
                self.indicator_alpha.snap_to(1.0);
                self.initialized = true;
            } else {
                self.indicator_x.set_target(target_x);
                self.indicator_width.set_target(target_w);
                self.indicator_alpha.set_target(1.0);
            }
        } else if !self.initialized {
            self.indicator_alpha.snap_to(0.0);
            self.initialized = true;
        } else {
            self.indicator_alpha.set_target(0.0);
        }
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        data: &BarData<'_>,
        config: &lard_core::Config,
    ) {
        let bar = &config.bar;
        let fonts = &config.font;
        let theme = &config.theme;
        self.sync(data.niri, bar, fonts);

        // With compositor-side blur the islands become frosted glass, so the
        // fill has to be translucent enough to actually show the blur.
        let frost = if theme.blur && theme.island_blur {
            theme.frost
        } else {
            1.0
        };
        let bg = theme
            .background
            .with_alpha((theme.background.a * bar.opacity * frost).clamp(0.0, 1.0));
        // The chain ties the islands to the master radius as well.
        let radius = if theme.radius.linked {
            theme.radius.card()
        } else {
            bar.corner_radius
        }
        .min(size.h * 0.5);
        let pad = bar.padding;
        let gap = 6.0;
        self.hits.clear();
        self.islands.clear();
        // A panel that grows out of an island fades the island away while it
        // morphs into the panel.
        // The island is gone by a third of the animation: the panel grows
        // out of it instead of appearing next to it.
        let island_alpha = |island: BarIsland| -> f32 {
            match data.morph {
                Some((kind, progress)) if kind == island => {
                    (1.0 - (progress / 0.3).min(1.0)).clamp(0.0, 1.0)
                }
                _ => 1.0,
            }
        };
        let left_alpha = island_alpha(BarIsland::Left);
        let center_alpha = island_alpha(BarIsland::Center);
        let right_alpha = island_alpha(BarIsland::Right);

        // ---- left island: workspaces + focused title ----
        let left_marker = scene.marker();
        for pill in &self.pills {
            self.hits
                .push((pill.rect.inset(-2.0), BarHit::Workspace(pill.ws_id)));
        }
        let title = if bar.show_window_title
            && data
                .niri
                .focused_output
                .as_deref()
                .is_none_or(|output| output == self.output)
        {
            data.niri.focused_window.as_ref().map(|window| {
                if window.title.is_empty() {
                    window.app_id.clone()
                } else {
                    window.title.clone()
                }
            })
        } else {
            None
        };
        let base_end = self.pills.last().map_or(pad, |pill| pill.rect.right());
        let max_left = size.w * 0.46;
        let title_room = (max_left - base_end - pad - 12.0).max(0.0);
        let title = title.filter(|_| title_room > 40.0).map(|title| {
            truncate_to_width(
                &truncate(&title, bar.max_title_chars),
                fonts.size_small,
                title_room,
            )
        });
        let title_width = title
            .as_ref()
            .map_or(0.0, |title| approx_width(title, fonts.size_small) + 14.0);
        let left_w = (base_end + title_width + pad).min(max_left).max(pad + 30.0);

        self.islands
            .push((BarIsland::Left, Rect::new(0.0, 0.0, left_w, size.h)));
        scene.panel(
            Rect::new(0.0, 0.0, left_w, size.h),
            bg.scale_alpha(left_alpha),
            radius,
            None,
        );
        scene.bordered(
            Rect::new(0.5, 0.5, left_w - 1.0, size.h - 1.0),
            Color::TRANSPARENT,
            radius,
            1.0,
            theme.border.scale_alpha(0.7 * left_alpha),
        );

        for pill in &self.pills {
            let hovered = matches!(self.hovered, Some(BarHit::Workspace(id)) if id == pill.ws_id);
            let color = if pill.focused {
                Color::TRANSPARENT
            } else if hovered {
                theme.surface_hover.scale_alpha(0.8)
            } else {
                theme
                    .surface
                    .scale_alpha(if pill.has_windows { 0.5 } else { 0.26 })
            };
            // The workspace numbers are tiles: they follow the card radius.
            scene.rect(pill.rect, color, theme.radius.card());
        }

        let alpha = self.indicator_alpha.value().clamp(0.0, 1.0);
        if alpha > 0.001 {
            scene.rect(
                Rect::new(
                    self.indicator_x.value(),
                    (size.h - 26.0) * 0.5,
                    self.indicator_width.value(),
                    26.0,
                ),
                theme.accent.scale_alpha(alpha),
                theme.radius.card(),
            );
        }

        for pill in &self.pills {
            let hovered = matches!(self.hovered, Some(BarHit::Workspace(id)) if id == pill.ws_id);
            let color = if pill.focused {
                theme.accent_text
            } else if pill.active || hovered {
                theme.text
            } else {
                theme.text_muted.scale_alpha(0.85)
            };
            scene.text_center_y(
                &pill.label,
                pill.rect.x,
                pill.rect.center().y,
                TextStyle {
                    size: fonts.size_small,
                    weight: if pill.focused {
                        fonts.weight_semibold
                    } else {
                        fonts.weight_medium
                    },
                    align: TextAlign::Center,
                    max_width: Some(pill.rect.w),
                    ..Default::default()
                },
                color,
            );
            if pill.urgent && bar.workspace_labels.show_urgent_dot {
                scene.rect(
                    Rect::new(pill.rect.right() - 6.0, pill.rect.y + 4.0, 4.0, 4.0),
                    theme.danger,
                    2.0,
                );
            }
        }

        if let Some(title) = &title {
            let x = base_end + 10.0;
            let width = (left_w - x - pad).max(0.0);
            if width > 24.0 {
                scene.text_center_y(
                    title,
                    x,
                    size.h * 0.5,
                    TextStyle {
                        size: fonts.size_small,
                        weight: fonts.weight_medium,
                        align: TextAlign::Left,
                        max_width: Some(width),
                        wrap: false,
                        ..Default::default()
                    },
                    theme.text.scale_alpha(0.8),
                );
            }
        }

        scene.fade_since(left_marker, left_alpha);

        // ---- right island: cpu/ram, then the grouped Control Center button ----
        let right_marker = scene.marker();
        // The load indicators stay on the bar, the rest of the status lives
        // inside the macOS-style panel.
        let cpu_text = format!("{}%", data.system.cpu_percent);
        let ram_text = format!("{}%", data.system.memory_percent);
        let meter_w = 16.0 + approx_width(&cpu_text, fonts.size_small - 0.5) + 8.0;
        let button_w = 34.0;
        let right_w = pad * 2.0 + meter_w * 2.0 + button_w + 6.0;
        let right_x = size.w - right_w;
        self.islands
            .push((BarIsland::Right, Rect::new(right_x, 0.0, right_w, size.h)));
        scene.panel(
            Rect::new(right_x, 0.0, right_w, size.h),
            bg.scale_alpha(right_alpha),
            radius,
            None,
        );
        scene.bordered(
            Rect::new(right_x + 0.5, 0.5, right_w - 1.0, size.h - 1.0),
            Color::TRANSPARENT,
            radius,
            1.0,
            theme.border.scale_alpha(0.7 * right_alpha),
        );

        // The cpu and ram load.
        let mut meter_x = right_x + pad;
        for (icon, text) in [(icons::CPU, &cpu_text), (icons::MEMORY, &ram_text)] {
            scene.text_center_y(
                icon,
                meter_x,
                size.h * 0.5,
                TextStyle {
                    size: fonts.size_small,
                    align: TextAlign::Center,
                    max_width: Some(16.0),
                    ..Default::default()
                }
                .icon(),
                theme.text_muted.scale_alpha(0.85 * right_alpha),
            );
            scene.text_center_y(
                text,
                meter_x + 19.0,
                size.h * 0.5,
                TextStyle {
                    size: fonts.size_small - 0.5,
                    weight: fonts.weight_medium,
                    align: TextAlign::Left,
                    max_width: Some(28.0),
                    ..Default::default()
                },
                theme.text.scale_alpha(0.9 * right_alpha),
            );
            meter_x += meter_w;
        }

        let control = Rect::new(meter_x + 6.0, (size.h - 26.0) * 0.5, button_w, 26.0);
        if self.hovered == Some(BarHit::ControlCenter) {
            scene.rect(
                control,
                theme.surface_hover.scale_alpha(0.8),
                theme.radius.control(),
            );
        }
        bar_icons::control_center(
            scene,
            Rect::new(control.x + 6.0, control.center().y - 8.0, 18.0, 16.0),
            if data.dnd {
                theme.accent
            } else {
                theme.text_muted
            },
        );
        if data.active_toasts > 0 {
            scene.rect(
                Rect::new(control.right() - 6.0, control.y + 3.0, 5.0, 5.0),
                theme.accent,
                2.5,
            );
        }
        // The whole island is the target; the glyph is the visible affordance.
        self.hits.push((
            Rect::new(right_x, 0.0, right_w, size.h),
            BarHit::ControlCenter,
        ));

        scene.fade_since(right_marker, right_alpha);

        // ---- centre island: clock + date + bell, or the HUD in the same slot ----
        let center_marker = scene.marker();
        // The island is a single slot: the volume/brightness HUD and an
        // incoming notification both replace the clock, sliding up from below.
        // An incoming notification takes over the centre island: it is drawn
        // on its own overlay surface (above the windows), so the island of
        // the bar steps aside instead of resizing the bar surface.
        let notification = data.toast.is_some();
        let osd = data.osd.filter(|osd| osd.progress > 0.001 && !notification);
        let osd_p = osd.map_or(0.0, |osd| osd.progress.clamp(0.0, 1.0));
        let hud_p = osd_p;

        let clock_w = approx_width(data.clock, fonts.size_small + 1.0) + 6.0;
        let date_w = approx_width(data.date, fonts.size_small - 1.0) + 4.0;
        let bell_w = 22.0;
        let osd_center_w = 208.0;
        let mut target_w = if bar.show_clock {
            pad * 2.0 + clock_w + 17.0 + date_w + bell_w
        } else {
            0.0
        };
        if notification {
            target_w = 0.0;
        } else if osd.is_some() {
            target_w = target_w.max(osd_center_w);
        }
        let available = (right_x - left_w - gap * 2.0).max(0.0);
        let show_date = bar.show_clock && target_w <= available;
        if bar.show_clock && !show_date && !notification {
            target_w =
                (pad * 2.0 + clock_w + bell_w).max(if osd.is_some() { osd_center_w } else { 0.0 });
        }
        // The centre island's home: tracked even while the island is hidden,
        // so a panel opened from it still knows where to grow from.
        let home_w = if bar.show_clock {
            pad * 2.0 + clock_w + 17.0 + date_w + bell_w
        } else {
            0.0
        };
        let home_x = ((size.w - home_w) * 0.5)
            .max(left_w + gap)
            .min(right_x - gap - home_w);
        self.center_home = Rect::new(home_x, 0.0, home_w, size.h);
        if !self.center_initialized {
            self.center_width.snap_to(target_w);
            self.center_initialized = true;
        } else {
            self.center_width.set_target(target_w);
        }
        // The island grows/shrinks around its centre while the clock and the
        // HUD cross-fade, instead of jumping between two widths.
        let center_w = self.center_width.value();
        if center_w > 0.0 && center_w <= available {
            let center_x = ((size.w - center_w) * 0.5)
                .max(left_w + gap)
                .min(right_x - gap - center_w);
            self.islands.push((
                BarIsland::Center,
                Rect::new(center_x, 0.0, center_w, size.h),
            ));
            scene.panel(
                Rect::new(center_x, 0.0, center_w, size.h),
                bg.scale_alpha(center_alpha),
                radius,
                None,
            );
            scene.bordered(
                Rect::new(center_x + 0.5, 0.5, center_w - 1.0, size.h - 1.0),
                Color::TRANSPARENT,
                radius,
                1.0,
                theme.border.scale_alpha(0.7 * center_alpha),
            );

            // The clock slides up and fades while the HUD slides up into the
            // slot from below (Apple-style vertical content swap).
            let clock_dy = -size.h * hud_p;
            let clock_alpha = 1.0 - hud_p;
            let mut bell_x = 0.0;
            if bar.show_clock && clock_alpha > 0.01 {
                let clock_x = center_x + pad;
                scene.text_center_y(
                    data.clock,
                    clock_x,
                    size.h * 0.5 + clock_dy,
                    TextStyle {
                        size: fonts.size_small + 1.0,
                        weight: fonts.weight_semibold,
                        align: TextAlign::Left,
                        max_width: Some(clock_w + 2.0),
                        ..Default::default()
                    },
                    theme.text.scale_alpha(clock_alpha),
                );
                let date_x = clock_x + clock_w + 17.0;
                if show_date {
                    scene.rect(
                        Rect::new(date_x - 10.0, size.h * 0.5 + clock_dy - 1.5, 3.0, 3.0),
                        theme.accent.scale_alpha(0.8 * clock_alpha),
                        1.5,
                    );
                    scene.text_center_y(
                        data.date,
                        date_x,
                        size.h * 0.5 + clock_dy,
                        TextStyle {
                            size: fonts.size_small - 1.0,
                            weight: fonts.weight_regular,
                            align: TextAlign::Left,
                            max_width: Some(date_w + 2.0),
                            ..Default::default()
                        },
                        theme.text_muted.scale_alpha(clock_alpha),
                    );
                    bell_x = date_x + date_w + 6.0;
                } else {
                    bell_x = clock_x + clock_w + 8.0;
                }

                // Notification bell, right of the date, with a badge while
                // notifications are waiting in the notification center.
                let bell_color = if data.notification_count > 0 {
                    theme.text.scale_alpha(clock_alpha)
                } else {
                    theme.text_muted.scale_alpha(0.75 * clock_alpha)
                };
                bar_icons::bell(
                    scene,
                    Rect::new(bell_x, size.h * 0.5 + clock_dy - 7.0, 13.0, 14.0),
                    bell_color,
                );
                if data.notification_count > 0 {
                    scene.rect(
                        Rect::new(bell_x + 9.0, size.h * 0.5 + clock_dy - 6.0, 5.0, 5.0),
                        theme.accent.scale_alpha(clock_alpha),
                        2.5,
                    );
                }
                self.hits.push((
                    Rect::new(bell_x - 4.0, 0.0, 22.0, size.h),
                    BarHit::Notifications,
                ));
            }

            if let Some(osd) = osd {
                let dy = size.h * (1.0 - osd_p);
                let mid_y = size.h * 0.5 - 2.0 + dy;
                let accent = if osd.muted && !osd.brightness {
                    theme.danger
                } else {
                    theme.accent
                };
                if config.osd.show_label {
                    let label = if osd.brightness {
                        "LUM"
                    } else if osd.muted {
                        "MUTE"
                    } else {
                        "VOL"
                    };
                    scene.text_center_y(
                        label,
                        center_x + pad,
                        mid_y,
                        TextStyle {
                            size: fonts.size_small - 0.5,
                            weight: fonts.weight_semibold,
                            ..Default::default()
                        },
                        accent.scale_alpha(osd_p),
                    );
                }
                if config.osd.show_percentage {
                    scene.text_center_y(
                        &format!("{}%", osd.value),
                        center_x + center_w - pad,
                        mid_y,
                        TextStyle {
                            size: fonts.size_small - 1.0,
                            weight: fonts.weight_medium,
                            align: TextAlign::Right,
                            max_width: Some(44.0),
                            ..Default::default()
                        },
                        theme.text.scale_alpha(0.95 * osd_p),
                    );
                }
                let track = Rect::new(center_x + pad, size.h - 7.0 + dy, center_w - pad * 2.0, 3.0);
                scene.rect(track, theme.surface_hover.scale_alpha(0.8 * osd_p), 1.5);
                let fill = if osd.muted && !osd.brightness {
                    0.0
                } else {
                    track.w * f32::from(osd.value.min(100)) / 100.0
                };
                if fill > 0.0 {
                    scene.rect(
                        Rect::new(track.x, track.y, fill.max(4.0), track.h),
                        accent.scale_alpha(osd_p),
                        1.5,
                    );
                }
            }

            // The clock is clickable only when the HUD is not occupying the
            // slot; the bell always is.
            if hud_p <= 0.05 {
                let clock_x = center_x + pad;
                let clock_x_end = if bell_x > 0.0 {
                    bell_x - 6.0
                } else {
                    center_x + center_w
                };
                self.hits.push((
                    Rect::new(clock_x, 0.0, (clock_x_end - clock_x).max(8.0), size.h),
                    BarHit::Clock,
                ));
            }
        }
        scene.fade_since(center_marker, center_alpha);
    }

    pub fn press(&mut self, pos: Point) -> Option<BarPress> {
        let (rect, hit) = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(rect, hit)| (*rect, *hit))?;
        let popover = |kind| Some((kind, rect.center().x));
        let (action, popover) = match hit {
            BarHit::Workspace(id) => (Some(ShellAction::FocusWorkspace(id)), None),
            BarHit::Clock => (None, popover(PopoverKind::Clock)),
            BarHit::Notifications => (None, popover(PopoverKind::Notifications)),
            BarHit::ControlCenter => (None, popover(PopoverKind::ControlCenter)),
        };
        Some(BarPress { action, popover })
    }

    pub fn axis(&mut self, _pos: Point, _delta: f64) -> Option<ShellAction> {
        // Volume and brightness live in the Control Center panel now; the
        // bar itself no longer has a scrollable status item.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lard_core::Config;
    use lard_services::battery::BatteryState;
    use lard_services::bluetooth::BluetoothState;
    use lard_services::network::NetworkState;
    use lard_services::niri::NiriSnapshot;
    use lard_services::system::SystemStats;

    #[test]
    fn notification_bell_lives_in_the_centre_island() {
        let config = Config::default();
        let niri = NiriSnapshot::default();
        let network = NetworkState::default();
        let data = BarData {
            niri: &niri,
            volume: 40,
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
            notification_count: 2,
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
        let bell = bar
            .hits
            .iter()
            .find(|(_, hit)| *hit == BarHit::Notifications)
            .map(|(rect, _)| *rect)
            .expect("the bell sits in the centre island");
        // Right of the date, so in the right half of the island.
        assert!(bell.x > 700.0);
        let press = bar.press(bell.center()).expect("bell is clickable");
        assert!(matches!(
            press.popover,
            Some((PopoverKind::Notifications, _))
        ));
    }

    #[test]
    fn incoming_notification_replaces_the_clock() {
        let config = Config::default();
        let niri = NiriSnapshot::default();
        let network = NetworkState::default();
        let data = BarData {
            niri: &niri,
            volume: 40,
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
            active_toasts: 1,
            toast: Some(ToastHeadline {
                app: "Firefox".into(),
                summary: "Nuova scheda".into(),
                body: String::new(),
                progress: 1.0,
            }),
            notification_count: 1,
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
        let labels: Vec<&str> = scene.texts.iter().map(|t| scene.text_of(t)).collect();
        // The centre island steps aside: the notification is drawn on its own
        // overlay surface, so the clock is not shown next to it.
        assert!(!labels.contains(&"12:00"), "{labels:?}");
        assert!(!labels.contains(&"mar 20 ott"), "{labels:?}");
    }

    #[test]
    fn status_icons_are_grouped_behind_one_control_center_button() {
        let config = Config::default();
        let niri = NiriSnapshot::default();
        let network = NetworkState::default();
        let data = BarData {
            niri: &niri,
            volume: 40,
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
        let button = bar
            .hits
            .iter()
            .find(|(_, hit)| *hit == BarHit::ControlCenter)
            .map(|(rect, _)| *rect)
            .expect("control center button should be in the status island");
        let press = bar.press(button.center()).expect("button is clickable");
        assert_eq!(press.action, None);
        assert!(matches!(
            press.popover,
            Some((PopoverKind::ControlCenter, _))
        ));
    }
}
