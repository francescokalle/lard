//! Volume on-screen display with a spring slide/fade and a self-hiding timer.

use lard_core::anim::{Spring, SpringConfig};
use lard_core::theme::OsdConfig;
use lard_core::{Rect, Size};
use lard_render::{Scene, TextAlign, TextStyle};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OsdKind {
    Volume,
    Brightness,
}

/// Snapshot consumed by the bar: the OSD is rendered inside the centre island
/// (where the clock normally lives) instead of in its own bottom layer.
#[derive(Clone, Copy, Debug)]
pub struct OsdState {
    pub progress: f32,
    pub value: u8,
    pub muted: bool,
    pub brightness: bool,
}

pub struct Osd {
    visible: bool,
    progress: Spring,
    kind: OsdKind,
    value: u8,
    percent_text: String,
    muted: bool,
    hide_at: Option<Instant>,
    reduce_motion: bool,
}

impl Osd {
    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        Self {
            visible: false,
            progress: Spring::new(0.0, animation),
            kind: OsdKind::Volume,
            value: 0,
            percent_text: String::new(),
            muted: false,
            hide_at: None,
            reduce_motion,
        }
    }

    pub fn show_volume(&mut self, value: u8, muted: bool, config: &OsdConfig) {
        if !config.enabled {
            return;
        }
        self.visible = true;
        self.kind = OsdKind::Volume;
        self.value = value;
        self.percent_text = format!("{value}%");
        self.muted = muted;
        self.hide_at = Some(Instant::now() + Duration::from_millis(config.duration_ms.max(400)));
        if self.reduce_motion {
            self.progress.snap_to(1.0);
        } else {
            self.progress.set_target(1.0);
        }
    }

    pub fn show_brightness(&mut self, value: u8, config: &OsdConfig) {
        if !config.enabled {
            return;
        }
        self.visible = true;
        self.kind = OsdKind::Brightness;
        self.value = value;
        self.percent_text = format!("{value}%");
        self.muted = false;
        self.hide_at = Some(Instant::now() + Duration::from_millis(config.duration_ms.max(400)));
        if self.reduce_motion {
            self.progress.snap_to(1.0);
        } else {
            self.progress.set_target(1.0);
        }
    }

    pub fn update(&mut self, dt: f32) -> bool {
        if let Some(deadline) = self.hide_at
            && Instant::now() >= deadline
        {
            self.hide_at = None;
            self.visible = false;
            if self.reduce_motion {
                self.progress.snap_to(0.0);
            } else {
                self.progress.set_target(0.0);
            }
        }
        self.progress.update(dt)
    }

    #[must_use]
    pub fn wants_surface(&self) -> bool {
        self.visible || self.progress.value() > 0.01
    }

    /// State for the in-bar volume/brightness HUD, or `None` when idle.
    #[must_use]
    pub fn state(&self) -> Option<OsdState> {
        let progress = self.progress.value().clamp(0.0, 1.0);
        (progress > 0.001).then_some(OsdState {
            progress,
            value: self.value,
            muted: self.muted,
            brightness: matches!(self.kind, OsdKind::Brightness),
        })
    }

    #[must_use]
    pub fn preferred_size(config: &OsdConfig) -> Size {
        Size::new(config.width, config.height)
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.hide_at
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        config: &OsdConfig,
        theme: &lard_core::theme::Theme,
        fonts: &lard_core::theme::FontConfig,
    ) {
        let p = self.progress.value().clamp(0.0, 1.0);
        if p <= 0.001 {
            return;
        }
        // The compositor anchors this small surface `margin_bottom` above the
        // output edge; the scene is local to the surface.
        let y = (1.0 - p) * 16.0;
        let card = Rect::new(0.0, y, size.w, size.h);
        scene.bordered(
            card,
            theme.surface.with_alpha(0.95 * p),
            theme.radius.panel(),
            theme.border_width,
            theme.border.scale_alpha(p),
        );

        let is_volume = self.kind == OsdKind::Volume;
        let accent = if self.muted && is_volume {
            theme.danger
        } else {
            theme.accent
        };
        if config.show_label {
            scene.text_center_y(
                if self.muted && is_volume {
                    "MUTE"
                } else if is_volume {
                    "VOL"
                } else {
                    "LUM"
                },
                card.x + 18.0,
                card.y + 18.0,
                TextStyle {
                    size: fonts.size_small - 0.5,
                    weight: fonts.weight_semibold,
                    ..Default::default()
                },
                accent.scale_alpha(p),
            );
        }
        if config.show_percentage {
            scene.text_center_y(
                &self.percent_text,
                card.right() - 70.0,
                card.y + 18.0,
                TextStyle {
                    size: fonts.size_small,
                    weight: fonts.weight_medium,
                    align: TextAlign::Right,
                    max_width: Some(56.0),
                    ..Default::default()
                },
                theme.text.scale_alpha(0.9 * p),
            );
        }

        let track = Rect::new(card.x + 18.0, card.y + 34.0, card.w - 36.0, 6.0);
        scene.rect(track, theme.surface_hover.scale_alpha(0.8 * p), 3.0);
        let fill = if self.muted && is_volume {
            0.0
        } else {
            track.w * f32::from(self.value.min(100)) / 100.0
        };
        if fill > 0.0 {
            scene.rect(
                Rect::new(track.x, track.y, fill.max(6.0), track.h),
                accent.scale_alpha(p),
                3.0,
            );
        }
        if self.muted && is_volume {
            scene.text_center_y(
                "audio muted",
                card.x,
                card.y + 44.0,
                TextStyle {
                    size: 8.5,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.6 * p),
            );
        }
    }
}
