//! Floating application dock, Dank-style: one translucent island with
//! symbolic app tiles and running indicators.

use crate::ShellAction;
use crate::app_icons::AppIcons;
use crate::icons;
use lard_core::desktop::DesktopEntry;
use lard_core::theme::{DockConfig, Theme};
use lard_core::{Color, Point, Rect, Size};
use lard_render::{Scene, TextAlign, TextStyle};
use lard_services::niri::NiriSnapshot;

struct DockItem {
    entry: DesktopEntry,
    icon: &'static str,
    color: Color,
    running: bool,
    rect: Rect,
}

pub struct Dock {
    items: Vec<DockItem>,
    size: Size,
    hits: Vec<(Rect, usize)>,
    hovered: Option<usize>,
    invalidated: bool,
    /// Panel drawn this frame, used for the compositor-side backdrop blur.
    panel: Rect,
}

impl Dock {
    pub fn new() -> Self {
        Self {
            items: Vec::with_capacity(10),
            size: Size::new(0.0, 0.0),
            hits: Vec::with_capacity(10),
            hovered: None,
            invalidated: true,
            panel: Rect::ZERO,
        }
    }

    /// Panel rectangle of the dock island, used for the backdrop blur.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    pub fn invalidate(&mut self) {
        self.invalidated = true;
    }

    pub fn size(&self) -> Option<Size> {
        (self.size.w > 0.0 && self.size.h > 0.0).then_some(self.size)
    }

    pub fn items_len(&self) -> usize {
        self.items.len()
    }

    pub fn sync(
        &mut self,
        apps: &[DesktopEntry],
        niri: &NiriSnapshot,
        config: &DockConfig,
        theme: &Theme,
    ) {
        if !self.invalidated {
            return;
        }
        self.invalidated = false;
        self.items.clear();
        self.hits.clear();
        self.hovered = None;
        if !config.enabled {
            self.size = Size::new(0.0, 0.0);
            return;
        }

        let mut chosen: Vec<DesktopEntry> = Vec::with_capacity(10);
        for pin in &config.pinned {
            if chosen.len() >= 10 {
                break;
            }
            let best = apps
                .iter()
                .filter(|entry| !chosen.iter().any(|picked| picked.file == entry.file))
                .filter_map(|entry| pin_score(entry, pin).map(|score| (score, entry)))
                .max_by(|(a, entry_a), (b, entry_b)| {
                    a.cmp(b).then_with(|| entry_b.name.cmp(&entry_a.name))
                });
            if let Some((_, entry)) = best {
                chosen.push(entry.clone());
            }
        }

        if config.show_running {
            for window in &niri.windows {
                if chosen.len() >= 10 {
                    break;
                }
                if window.app_id.is_empty() {
                    continue;
                }
                if chosen
                    .iter()
                    .any(|entry| app_id_matches_entry(&window.app_id, entry))
                {
                    continue;
                }
                if let Some(entry) = apps
                    .iter()
                    .find(|entry| app_id_matches_entry(&window.app_id, entry))
                {
                    chosen.push(entry.clone());
                }
            }
        }

        // A fresh install may not have any pinned application installed;
        // still show a useful dock instead of an empty strip.
        if chosen.is_empty() {
            for entry in apps.iter().take(7) {
                chosen.push(entry.clone());
            }
        }

        let height = config.height.ceil().max(32.0);
        let padding = config.padding.max(0.0);
        let spacing = config.spacing.max(0.0);
        let tile = (height - padding * 2.0).max(24.0);
        let width = padding * 2.0
            + tile * chosen.len() as f32
            + spacing * chosen.len().saturating_sub(1) as f32;
        self.size = Size::new(width.ceil(), height);

        for (index, entry) in chosen.into_iter().enumerate() {
            let x = padding + index as f32 * (tile + spacing);
            let rect = Rect::new(x, padding, tile, tile);
            let icon = icons::app_icon(&entry);
            let color = icons::app_color(&entry, theme);
            let running = niri
                .windows
                .iter()
                .any(|window| app_id_matches_entry(&window.app_id, &entry));
            self.items.push(DockItem {
                entry,
                icon,
                color,
                running,
                rect,
            });
        }
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        theme: &Theme,
        fonts: &lard_core::theme::FontConfig,
        opacity: f32,
        icons: &mut AppIcons,
    ) {
        self.panel = Rect::ZERO;
        if self.items.is_empty() {
            return;
        }
        let frost = if theme.blur && theme.island_blur {
            theme.frost
        } else {
            1.0
        };
        let bg = theme
            .background
            .with_alpha((theme.background.a * opacity * frost).clamp(0.0, 1.0));
        self.panel = Rect::new(0.0, 0.0, size.w, size.h);
        scene.panel(self.panel, bg, theme.radius.panel(), None);
        scene.bordered(
            Rect::new(0.5, 0.5, size.w - 1.0, size.h - 1.0),
            Color::TRANSPARENT,
            theme.radius.panel(),
            1.0,
            theme.border.scale_alpha(0.75),
        );

        self.hits.clear();
        for (index, item) in self.items.iter().enumerate() {
            let hovered = self.hovered == Some(index);
            if hovered {
                scene.rect(item.rect, item.color.scale_alpha(0.18), theme.radius.card());
            } else {
                scene.rect(item.rect, item.color.scale_alpha(0.09), theme.radius.card());
            }
            match icons.id_for(&item.entry, scene) {
                Some(id) => {
                    scene.image(item.rect.inset(4.0), id, theme.radius.card());
                }
                None => {
                    scene.text_center_y(
                        item.icon,
                        item.rect.x,
                        item.rect.center().y,
                        TextStyle {
                            size: fonts.size_large.min(item.rect.h * 0.55),
                            weight: fonts.weight_regular,
                            align: TextAlign::Center,
                            max_width: Some(item.rect.w),
                            ..Default::default()
                        }
                        .icon(),
                        item.color,
                    );
                }
            }
            if item.running {
                scene.rect(
                    Rect::new(item.rect.center().x - 2.0, size.h - 5.5, 4.0, 2.0),
                    item.color,
                    1.0,
                );
            }
            self.hits.push((item.rect, index));
        }
    }

    pub fn pointer_move(&mut self, pos: Point) -> bool {
        let hovered = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, index)| *index);
        if hovered != self.hovered {
            self.hovered = hovered;
            true
        } else {
            false
        }
    }

    pub fn pointer_leave(&mut self) -> bool {
        self.hovered.take().is_some()
    }

    pub fn press(&mut self, pos: Point) -> Option<ShellAction> {
        let index = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, index)| *index)?;
        self.items
            .get(index)
            .map(|item| ShellAction::Spawn(item.entry.argv()))
    }
}

/// Rank used to put favourite applications first in the launcher.
#[must_use]
pub fn pin_rank(entry: &DesktopEntry, pins: &[String]) -> usize {
    pins.iter()
        .position(|pin| pin_score(entry, pin).is_some())
        .unwrap_or(usize::MAX)
}

fn pin_score(entry: &DesktopEntry, query: &str) -> Option<u8> {
    let query = query.to_ascii_lowercase();
    if query.is_empty() {
        return None;
    }
    let stem = entry
        .file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if stem.contains("url-handler") || stem.contains("mimeinfo") {
        return None;
    }
    let name = entry.name.to_ascii_lowercase();
    let icon = entry.icon.to_ascii_lowercase();
    if stem == query {
        Some(120)
    } else if name == query {
        Some(110)
    } else if icon == query {
        Some(100)
    } else if stem.ends_with(&format!(".{query}")) || stem.ends_with(&format!("-{query}")) {
        Some(90)
    } else if stem.starts_with(&format!("{query}.")) || stem.starts_with(&format!("{query}-")) {
        Some(80)
    } else if stem.contains(&query) {
        Some(60)
    } else if name.contains(&query) {
        Some(45)
    } else if icon.contains(&query) || entry.exec.to_ascii_lowercase().contains(&query) {
        Some(30)
    } else {
        None
    }
}

fn normalize_app_id(app_id: &str) -> String {
    app_id
        .to_ascii_lowercase()
        .rsplit('.')
        .next()
        .unwrap_or(app_id)
        .replace(['_', '-', ' '], "")
}

fn app_id_matches_entry(app_id: &str, entry: &DesktopEntry) -> bool {
    let id = normalize_app_id(app_id);
    if id.is_empty() {
        return false;
    }
    let stem = entry
        .file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let normalized_stem = stem
        .rsplit('.')
        .next()
        .unwrap_or(&stem)
        .replace(['_', '-', ' '], "");
    let normalized_name = entry.name.to_ascii_lowercase().replace(['_', '-', ' '], "");
    let icon = entry.icon.to_ascii_lowercase().replace(['_', '-', ' '], "");
    id == normalized_stem
        || normalized_stem.contains(&id)
        || id.contains(&normalized_stem)
        || normalized_name == id
        || icon == id
        || (id.starts_with("zen") && stem.contains("zen"))
        || (id.starts_with("chrome") && stem.contains("chrome"))
        || (id.starts_with("code") && stem.contains("code"))
}
