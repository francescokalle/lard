//! Wallpaper carousel: scans the omarchy wallpaper packages lazily, shows real
//! thumbnails and derives a tonal accent per wallpaper.

use lard_core::anim::{Spring, SpringConfig};
use lard_core::{Color, Config, Point, Rect, Size};
use lard_render::{Scene, Shadow, TextAlign, TextStyle};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

const MAX_ENTRIES: usize = 120;
const VISIBLE: usize = 6;
const THUMB_W: u32 = 128;
const THUMB_H: u32 = 72;

#[derive(Clone, Debug)]
pub struct WallpaperEntry {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Clone)]
struct Thumb {
    rgba: Vec<u8>,
    accent: Color,
}

#[derive(Clone, Debug)]
pub enum WallpaperAction {
    Apply { path: PathBuf, accent: Color },
    ApplySelected,
    Page(i32),
    Close,
}

pub struct WallpaperView {
    open: bool,
    progress: Spring,
    entries: Vec<WallpaperEntry>,
    thumbs: HashMap<PathBuf, Thumb>,
    requested: HashSet<PathBuf>,
    thumb_tx: Sender<PathBuf>,
    thumb_rx: Receiver<(PathBuf, Thumb)>,
    selected: usize,
    offset: usize,
    hits: Vec<(Rect, WallpaperAction)>,
    panel: Rect,
    cursor: Option<Point>,
    reduce_motion: bool,
}

pub const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

pub fn default_dir() -> PathBuf {
    std::env::var_os("NIRI_SUITE_WALLPAPER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".config/niri/omarchy-themes/wallpapers")
        })
}

pub fn scan() -> Vec<WallpaperEntry> {
    let mut paths = Vec::new();
    let mut stack = vec![default_dir()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                continue;
            }
            paths.push(path);
        }
    }
    paths.sort();
    paths.truncate(MAX_ENTRIES);
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .replace(['_', '-'], " ");
            WallpaperEntry { path, name }
        })
        .collect()
}

fn decode(path: &std::path::Path) -> Thumb {
    let fallback = Thumb {
        rgba: vec![0; (THUMB_W * THUMB_H * 4) as usize],
        accent: Color::from_rgb8(0xeb, 0xc1, 0x6c),
    };
    let Ok(img) = image::open(path) else {
        return fallback;
    };
    let small = img.resize_exact(THUMB_W, THUMB_H, image::imageops::FilterType::Triangle);
    let rgba = small.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let mut top = [0.0f32; 3];
    let mut bot = [0.0f32; 3];
    let mut top_n = 0u32;
    let mut bot_n = 0u32;
    for y in 0..h {
        for x in 0..w {
            let px = rgba.get_pixel(x as u32, y as u32).0;
            let c = [
                px[0] as f32 / 255.0,
                px[1] as f32 / 255.0,
                px[2] as f32 / 255.0,
            ];
            if y < h / 2 {
                for (i, v) in c.iter().enumerate() {
                    top[i] += v;
                }
                top_n += 1;
            } else {
                for (i, v) in c.iter().enumerate() {
                    bot[i] += v;
                }
                bot_n += 1;
            }
        }
    }
    for v in &mut top {
        *v /= top_n.max(1) as f32;
    }
    for v in &mut bot {
        *v /= bot_n.max(1) as f32;
    }
    Thumb {
        rgba: rgba.into_raw(),
        accent: saturate([
            (top[0] + bot[0]) * 0.5,
            (top[1] + bot[1]) * 0.5,
            (top[2] + bot[2]) * 0.5,
        ]),
    }
}

fn saturate(rgb: [f32; 3]) -> Color {
    let max = rgb.iter().copied().fold(0.0f32, f32::max);
    let min = rgb.iter().copied().fold(1.0f32, f32::min);
    let l = (max + min) * 0.5;
    if max - min < 0.001 {
        return Color::rgb(0.92, 0.75, 0.42);
    }
    let s = if l < 0.5 {
        (max - min) / (max + min)
    } else {
        (max - min) / (2.0 - max - min)
    };
    let s = (s * 1.5 + 0.25).min(1.0);
    // Keep the tonal accent readable on the dark bar/panels, even for very
    // dark wallpapers (Apple-style vibrancy needs a minimum luminance).
    let l = (l * 0.9 + 0.12).clamp(0.40, 0.80);
    let c = max - min;
    let hue = if c < 0.0001 {
        0.0
    } else if max == rgb[0] {
        60.0 * (((rgb[1] - rgb[2]) / c) % 6.0)
    } else if max == rgb[1] {
        60.0 * (((rgb[2] - rgb[0]) / c) + 2.0)
    } else {
        60.0 * (((rgb[0] - rgb[1]) / c) + 4.0)
    };
    hsl_to_rgb(hue, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = ((h % 360.0) + 360.0) % 360.0 / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    Color::rgb(r + m, g + m, b + m)
}

impl WallpaperView {
    /// Bounds of the wallpaper panel, used for the backdrop blur region.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        // Decode thumbnails on a worker thread: opening the carousel must never
        // block the compositor, even on 4K JPEGs.
        let (thumb_tx, request_rx) = mpsc::channel::<PathBuf>();
        let (result_tx, thumb_rx) = mpsc::channel::<(PathBuf, Thumb)>();
        let _ = std::thread::Builder::new()
            .name("wallpaper-thumbs".into())
            .spawn(move || {
                while let Ok(path) = request_rx.recv() {
                    let thumb = decode(&path);
                    if result_tx.send((path, thumb)).is_err() {
                        break;
                    }
                }
            });
        Self {
            open: false,
            progress: Spring::new(0.0, animation),
            entries: Vec::new(),
            thumbs: HashMap::new(),
            requested: HashSet::new(),
            thumb_tx,
            thumb_rx,
            selected: 0,
            offset: 0,
            hits: Vec::with_capacity(16),
            panel: Rect::ZERO,
            cursor: None,
            reduce_motion,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn wants_surface(&self) -> bool {
        self.open || self.progress.value() > 0.01
    }

    pub fn open(&mut self) {
        self.open = true;
        if self.entries.is_empty() {
            self.entries = scan();
        }
        self.selected = 0;
        self.offset = 0;
        if self.reduce_motion {
            self.progress.snap_to(1.0);
        } else {
            self.progress.set_target(1.0);
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        if self.reduce_motion {
            self.progress.snap_to(0.0);
        } else {
            self.progress.set_target(0.0);
        }
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let mut received = false;
        while let Ok((path, thumb)) = self.thumb_rx.try_recv() {
            self.requested.remove(&path);
            self.thumbs.insert(path, thumb);
            received = true;
        }
        let animating = self.progress.update(dt);
        // Keep scheduling frames while a decode is in flight, so a thumbnail
        // that lands after the reveal animation still gets painted.
        animating || received || !self.requested.is_empty()
    }

    pub fn page(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let max_off = self.entries.len().saturating_sub(VISIBLE);
        self.offset = (self.offset as isize + delta as isize).clamp(0, max_off as isize) as usize;
    }

    pub fn pointer_move(&mut self, pos: Point) {
        self.cursor = Some(pos);
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
    }

    pub fn pointer_press(&mut self, pos: Point) -> Option<WallpaperAction> {
        if let Some((_, action)) = self.hits.iter().rev().find(|(rect, _)| rect.contains(pos)) {
            return Some(action.clone());
        }
        if !self.panel.contains(pos) {
            self.close();
        }
        None
    }

    pub fn key(
        &mut self,
        escape: bool,
        left: bool,
        right: bool,
        enter: bool,
    ) -> Option<WallpaperAction> {
        if escape {
            self.close();
            return None;
        }
        if enter && !self.entries.is_empty() {
            return Some(WallpaperAction::ApplySelected);
        }
        if left {
            self.selected =
                (self.selected + self.entries.len().saturating_sub(1)) % self.entries.len().max(1);
            self.clamp_offset();
        } else if right {
            self.selected = (self.selected + 1) % self.entries.len().max(1);
            self.clamp_offset();
        }
        None
    }

    fn clamp_offset(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + VISIBLE {
            self.offset = self.selected - VISIBLE + 1;
        }
    }

    /// Resolve the focused wallpaper plus its tonal accent (decoding on demand
    /// for keyboard activation, where waiting one frame would feel laggy).
    pub fn selected(&mut self) -> Option<(PathBuf, Color)> {
        let entry = self.entries.get(self.selected)?.clone();
        let thumb = match self.thumb_for(&entry.path) {
            Some(thumb) => thumb,
            None => {
                let thumb = decode(&entry.path);
                self.thumbs.insert(entry.path.clone(), thumb.clone());
                thumb
            }
        };
        Some((entry.path, thumb.accent))
    }

    /// Return the decoded thumbnail when ready, otherwise queue a background
    /// decode and return `None` (the card shows a neutral placeholder).
    fn thumb_for(&mut self, path: &PathBuf) -> Option<Thumb> {
        if let Some(t) = self.thumbs.get(path) {
            return Some(t.clone());
        }
        if self.requested.insert(path.clone()) {
            let _ = self.thumb_tx.send(path.clone());
        }
        None
    }

    pub fn build(&mut self, scene: &mut Scene, size: Size, config: &Config) {
        let p = self.progress.value().clamp(0.0, 1.0);
        if p <= 0.001 {
            return;
        }
        self.hits.clear();
        let theme = &config.theme;
        let fonts = &config.font;
        scene.rect(
            Rect::new(0.0, 0.0, size.w, size.h),
            theme.background.with_alpha(0.16 * p),
            0.0,
        );
        let width = 1100.0_f32.min(size.w - 60.0);
        let height = 236.0_f32.min(size.h - 60.0);
        let x = (size.w - width) * 0.5;
        let y = (size.h - height) * 0.5 + (1.0 - p) * 20.0;
        let panel = Rect::new(x, y, width, height);
        let panel = panel.scale_centered(0.96 + 0.04 * p);
        self.panel = panel;

        scene.shadow(
            panel,
            theme.radius.panel(),
            Shadow {
                color: theme.shadow.scale_alpha(0.8 * p),
                blur: (theme.shadow_blur * 1.5).clamp(10.0, 64.0),
                offset_y: theme.shadow_offset_y.clamp(4.0, 28.0),
                spread: 2.0,
            },
        );
        scene.bordered(
            panel,
            theme
                .surface
                .with_alpha(crate::widgets::panel_alpha(config, 0.64) * p),
            theme.radius.panel(),
            theme.border_width,
            theme.border.scale_alpha(p),
        );

        scene.text(
            "Sfondi",
            Point::new(panel.x + 24.0, panel.y + 20.0),
            TextStyle {
                size: fonts.size_title,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );

        let selected_accent = if let Some(e) = self.entries.get(self.selected).cloned() {
            self.thumb_for(&e.path)
                .map_or(theme.accent, |thumb| thumb.accent)
        } else {
            theme.accent
        };
        scene.rect(
            Rect::new(panel.right() - 108.0, panel.y + 22.0, 12.0, 12.0),
            selected_accent.scale_alpha(p),
            6.0,
        );
        scene.text_center_y(
            "punto tonale",
            panel.right() - 88.0,
            panel.y + 28.0,
            TextStyle {
                size: fonts.size_small - 0.5,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.8 * p),
        );

        if self.entries.is_empty() {
            scene.text(
                "Nessuno sfondo trovato in",
                Point::new(panel.x, panel.y + 140.0),
                TextStyle {
                    size: fonts.size_body,
                    align: TextAlign::Center,
                    max_width: Some(panel.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.8 * p),
            );
            scene.text(
                &default_dir().display().to_string(),
                Point::new(panel.x, panel.y + 164.0),
                TextStyle {
                    size: fonts.size_small,
                    align: TextAlign::Center,
                    max_width: Some(panel.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.6 * p),
            );
            return;
        }

        let card_w = 150.0;
        let card_h = 84.0;
        let gap = 16.0;
        let start_x = panel.x + (panel.w - (VISIBLE as f32 * (card_w + gap) - gap)) * 0.5;
        let top_y = panel.y + 66.0;

        for i in 0..VISIBLE {
            let Some(entry) = self.entries.get(self.offset + i).cloned() else {
                break;
            };
            let id = (self.offset + i) as u32;
            let thumb = self.thumb_for(&entry.path);
            let accent = thumb.as_ref().map_or(theme.accent, |thumb| thumb.accent);
            let cx = start_x + i as f32 * (card_w + gap);
            let card = Rect::new(cx, top_y, card_w, card_h);
            let selected = self.offset + i == self.selected;
            let hovered = self.cursor.is_some_and(|c| card.contains(c));
            // The border is drawn in the rect pass (before images), so inset
            // the thumbnail to leave a visible selection/hover ring.
            if selected {
                scene.bordered(
                    card,
                    Color::TRANSPARENT,
                    theme.radius.card(),
                    2.0,
                    accent.scale_alpha(p),
                );
            } else if hovered {
                scene.bordered(
                    card,
                    Color::TRANSPARENT,
                    theme.radius.card(),
                    1.0,
                    theme.text.scale_alpha(0.25 * p),
                );
            }
            let inset = if selected {
                2.0
            } else if hovered {
                1.0
            } else {
                0.0
            };
            if let Some(thumb) = &thumb {
                scene.upload_image(id, THUMB_W, THUMB_H, thumb.rgba.clone());
                scene.image(
                    card.inset(inset),
                    id,
                    (theme.radius.card() - inset).max(0.0),
                );
            } else {
                scene.rect(
                    card.inset(inset),
                    theme.surface_hover.scale_alpha(0.75 * p),
                    (theme.radius.card() - inset).max(0.0),
                );
            }
            scene.text_center_y(
                &truncate(&entry.name, 18),
                cx,
                card.bottom() + 12.0,
                TextStyle {
                    size: fonts.size_small - 0.5,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                },
                if selected {
                    accent.scale_alpha(p)
                } else {
                    theme.text.scale_alpha(0.85 * p)
                },
            );
            self.hits.push((
                Rect::new(card.x, card.y - 24.0, card.w, card.h + 48.0),
                WallpaperAction::Apply {
                    path: entry.path.clone(),
                    accent,
                },
            ));
        }

        if self.offset > 0 {
            let left = Rect::new(panel.x + 14.0, panel.y + 100.0, 34.0, 34.0);
            scene.rect(
                left,
                theme.background.scale_alpha(0.6 * p),
                theme.radius.card(),
            );
            scene.text_center_y(
                "‹",
                left.x,
                left.center().y,
                TextStyle {
                    size: 20.0,
                    weight: fonts.weight_semibold,
                    align: TextAlign::Center,
                    max_width: Some(left.w),
                    ..Default::default()
                },
                theme.text.scale_alpha(0.9 * p),
            );
            self.hits
                .push((left, WallpaperAction::Page(-(VISIBLE as i32))));
        }
        if self.offset + VISIBLE < self.entries.len() {
            let right = Rect::new(panel.right() - 48.0, panel.y + 100.0, 34.0, 34.0);
            scene.rect(
                right,
                theme.background.scale_alpha(0.6 * p),
                theme.radius.card(),
            );
            scene.text_center_y(
                "›",
                right.x,
                right.center().y,
                TextStyle {
                    size: 20.0,
                    weight: fonts.weight_semibold,
                    align: TextAlign::Center,
                    max_width: Some(right.w),
                    ..Default::default()
                },
                theme.text.scale_alpha(0.9 * p),
            );
            self.hits
                .push((right, WallpaperAction::Page(VISIBLE as i32)));
        }
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars && max_chars > 1 {
        out.pop();
        out.push('…');
    }
    out
}
