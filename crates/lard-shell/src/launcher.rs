//! Spotlight-style application launcher.
//!
//! Fuzzy ranking uses `nucleo-matcher` (the matcher behind helix/nucleo) and is
//! recomputed only on keystrokes, never per frame.

use crate::ShellAction;
use crate::app_icons::AppIcons;
use crate::icons;
use crate::widgets::{approx_width, command_exists, truncate};
use lard_core::anim::{Spring, SpringConfig};
use lard_core::desktop::DesktopEntry;
use lard_core::{Point, Rect, Size};
use lard_render::{Scene, TextAlign, TextStyle};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LauncherKey {
    Char(String),
    Backspace,
    Enter,
    Escape,
    Up,
    Down,
    Tab,
}

/// One line of the launcher list: an application, a calculation result or a
/// file found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    App(usize),
    Calc { expression: String, value: String },
    File(PathBuf),
}

pub struct Launcher {
    open: bool,
    /// Configured feel of the open/close spring (the dismissal is quicker).
    animation: SpringConfig,
    progress: Spring,
    /// Island the launcher grows out of: screen rectangle and corner radius.
    origin: Option<(Rect, f32)>,
    /// Shape of the card during the morph, and the corner radius it is drawn
    /// with in this frame (the compositor blur must use the same one).
    shape: Rect,
    radius: f32,
    query: String,
    rows: Vec<Row>,
    selected: usize,
    /// First visible row: the mouse wheel scrolls the list.
    scroll: usize,
    /// Rows that fit in the card, kept from the last frame for the wheel.
    visible_rows: usize,
    hits: Vec<(Rect, usize)>,
    card: Rect,
    cursor: Option<Point>,
    caret_time: f32,
    elapsed: f32,
    last_activity: std::time::Instant,
    matcher: Matcher,
    buf: Vec<char>,
    reduce_motion: bool,
    /// File search runs on its own thread: the walk never blocks the UI.
    file_rx: Option<mpsc::Receiver<(u64, Vec<PathBuf>)>>,
    file_generation: u64,
    file_pending: bool,
    file_limit: usize,
}

impl Launcher {
    /// Bounds of the launcher card in surface-local logical pixels, used
    /// by the compositor-side backdrop blur region.
    pub fn card_rect(&self) -> Rect {
        self.card
    }

    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        Self {
            open: false,
            animation,
            progress: Spring::new(0.0, animation),
            origin: None,
            shape: Rect::ZERO,
            radius: 0.0,
            query: String::new(),
            rows: Vec::with_capacity(16),
            selected: 0,
            scroll: 0,
            visible_rows: 8,
            hits: Vec::with_capacity(12),
            card: Rect::ZERO,
            cursor: None,
            caret_time: 0.0,
            elapsed: 0.0,
            last_activity: std::time::Instant::now(),
            matcher: Matcher::new(MatcherConfig::DEFAULT),
            buf: Vec::with_capacity(64),
            reduce_motion,
            file_rx: None,
            file_generation: 0,
            file_pending: false,
            file_limit: 8,
        }
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Progress of the open/close animation.
    #[must_use]
    pub fn progress(&self) -> f32 {
        self.progress.value().clamp(0.0, 1.0)
    }

    /// Island the launcher card grows out of.
    pub fn set_origin(&mut self, origin: Option<(Rect, f32)>) {
        self.origin = origin;
    }

    /// Shape of the card while it morphs out of its island.
    pub fn shape_rect(&self) -> Rect {
        self.shape
    }

    /// Corner radius the card is currently drawn with. It differs from the
    /// configured one while the card morphs out of its island, and it follows
    /// the radius chain when `theme.radius.linked` is on.
    pub fn shape_radius(&self) -> f32 {
        self.radius
    }

    #[must_use]
    pub fn wants_surface(&self) -> bool {
        self.open || self.progress.value() > 0.01
    }

    pub fn open(&mut self, apps: &[DesktopEntry], max_results: usize) {
        if self.open {
            return;
        }
        self.open = true;
        self.last_activity = std::time::Instant::now();
        self.query.clear();
        self.selected = 0;
        self.scroll = 0;
        self.caret_time = 0.0;
        self.search(apps, max_results);
        if self.reduce_motion {
            self.progress.snap_to(1.0);
        } else {
            self.progress.animate_to(1.0, self.animation);
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        // The origin is kept so the closing animation is the reverse of the
        // opening one: the card flies back into the island it came from.
        if self.reduce_motion {
            self.progress.snap_to(0.0);
        } else {
            // Dismissals read as faster than presentations: the card leaves in
            // 80% of the time it took to arrive, with the same shape.
            let faster = self
                .animation
                .with_response(self.animation.response() * 0.8);
            self.progress.animate_to(0.0, faster);
        }
    }

    pub fn update(&mut self, dt: f32) -> bool {
        self.elapsed += dt;
        self.caret_time += dt;
        let mut active = self.progress.update(dt);
        // Files arrive asynchronously: append them when their query is still
        // the current one.
        if let Some(rx) = &self.file_rx {
            let mut files = None;
            loop {
                match rx.try_recv() {
                    Ok((generation, found)) => {
                        if generation == self.file_generation {
                            files = Some(found);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.file_rx = None;
                        self.file_pending = false;
                        active = true;
                        break;
                    }
                }
            }
            if let Some(found) = files {
                for path in found {
                    self.rows.push(Row::File(path));
                }
                self.file_rx = None;
                self.file_pending = false;
                self.clamp_selection();
                active = true;
            }
        }
        active
    }

    pub fn pointer_move(&mut self, pos: Point) {
        self.last_activity = std::time::Instant::now();
        self.cursor = Some(pos);
        if let Some((_, row)) = self.hits.iter().rev().find(|(rect, _)| rect.contains(pos)) {
            self.selected = *row;
        }
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
    }

    pub fn pointer_press(&mut self, pos: Point, apps: &[DesktopEntry]) -> Option<ShellAction> {
        self.last_activity = std::time::Instant::now();
        if !self.card.contains(pos) {
            self.close();
            return None;
        }
        let row = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, row)| *row)?;
        self.selected = row.min(self.rows.len().saturating_sub(1));
        self.launch_selected(apps)
    }

    /// Mouse wheel over the list: scroll one row per notch.
    pub fn axis(&mut self, delta: f64) {
        self.last_activity = std::time::Instant::now();
        if self.rows.is_empty() || delta == 0.0 {
            return;
        }
        let visible = self.visible_rows.max(1);
        let max_scroll = self.rows.len().saturating_sub(visible);
        if delta > 0.0 {
            self.scroll = (self.scroll + 1).min(max_scroll);
        } else {
            self.scroll = self.scroll.saturating_sub(1);
        }
        // Keep the highlighted row inside the scrolled window.
        if self.selected < self.scroll {
            self.selected = self.scroll;
        }
        if self.selected >= self.scroll + visible {
            self.selected = self.scroll + visible - 1;
        }
    }

    pub fn key(
        &mut self,
        key: LauncherKey,
        apps: &[DesktopEntry],
        max_results: usize,
    ) -> Option<ShellAction> {
        self.last_activity = std::time::Instant::now();
        match key {
            LauncherKey::Char(text) => {
                if self.query.chars().count() < 64 {
                    self.query.push_str(&text);
                    self.selected = 0;
                    self.caret_time = 0.0;
                    self.search(apps, max_results);
                }
                None
            }
            LauncherKey::Backspace => {
                self.query.pop();
                self.selected = 0;
                self.caret_time = 0.0;
                self.search(apps, max_results);
                None
            }
            LauncherKey::Enter => self.launch_selected(apps),
            LauncherKey::Escape => {
                self.close();
                None
            }
            LauncherKey::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_visible();
                None
            }
            LauncherKey::Down | LauncherKey::Tab => {
                if !self.rows.is_empty() {
                    self.selected = (self.selected + 1).min(self.rows.len() - 1);
                    self.ensure_visible();
                }
                None
            }
        }
    }

    /// Scroll the list so the selected row is inside the window.
    fn ensure_visible(&mut self) {
        let visible = self.visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(visible));
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.ensure_visible();
    }

    #[must_use]
    pub fn idle_deadline(&self, timeout_secs: u64) -> Option<std::time::Instant> {
        (self.open && timeout_secs > 0)
            .then(|| self.last_activity + std::time::Duration::from_secs(timeout_secs))
    }

    fn launch_selected(&mut self, apps: &[DesktopEntry]) -> Option<ShellAction> {
        match self.rows.get(self.selected)?.clone() {
            Row::App(index) => {
                let app = apps.get(index)?;
                let mut argv = app.argv();
                if argv.is_empty() {
                    return None;
                }
                if app.terminal && command_exists("xdg-terminal-exec") {
                    argv.insert(0, "xdg-terminal-exec".into());
                }
                self.close();
                Some(ShellAction::Spawn(argv))
            }
            Row::Calc { value, .. } => {
                // The result goes to the clipboard, like a calculator should.
                self.close();
                Some(ShellAction::Copy(value))
            }
            Row::File(path) => {
                self.close();
                Some(ShellAction::Spawn(vec![
                    "xdg-open".into(),
                    path.to_string_lossy().into_owned(),
                ]))
            }
        }
    }

    fn search(&mut self, apps: &[DesktopEntry], max_results: usize) {
        self.rows.clear();
        self.scroll = 0;
        self.file_generation = self.file_generation.wrapping_add(1);
        self.file_rx = None;
        self.file_pending = false;
        self.file_limit = max_results;
        let query = self.query.trim();
        if query.is_empty() {
            self.rows
                .extend((0..apps.len().min(max_results)).map(Row::App));
            self.clamp_selection();
            return;
        }

        // 1. Calculator: a bare arithmetic expression becomes a copyable
        //    result on the first line.
        if let Some((expression, value)) = evaluate_expression(query) {
            self.rows.push(Row::Calc { expression, value });
        }

        let mut app_hits = 0;
        if !looks_like_path(query) {
            let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
            let mut matches: Vec<(u32, usize)> = Vec::with_capacity(64);
            for (index, app) in apps.iter().enumerate() {
                self.buf.clear();
                let haystack = Utf32Str::new(&app.name, &mut self.buf);
                let Some(score) = pattern.score(haystack, &mut self.matcher) else {
                    continue;
                };
                matches.push((score, index));
            }
            matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let room = max_results.saturating_sub(self.rows.len());
            for (_, index) in matches.into_iter().take(room) {
                self.rows.push(Row::App(index));
                app_hits += 1;
            }
        }

        if looks_like_path(query) {
            // 2. File finder for explicit paths: one directory, no walk.
            for path in list_path(query, max_results) {
                self.rows.push(Row::File(path));
            }
        } else if app_hits == 0 && query.chars().count() >= 3 {
            // 3. File finder by name, in the background.
            let limit = max_results.saturating_sub(self.rows.len());
            if limit > 0 {
                let generation = self.file_generation;
                let needle = query.to_string();
                let (tx, rx) = mpsc::channel();
                if std::thread::Builder::new()
                    .name("lard-launcher-files".into())
                    .spawn(move || {
                        let found = search_files_by_name(&needle, limit);
                        let _ = tx.send((generation, found));
                    })
                    .is_ok()
                {
                    self.file_rx = Some(rx);
                    self.file_pending = true;
                }
            }
        }
        self.clamp_selection();
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        apps: &[DesktopEntry],
        config: &lard_core::Config,
        icons: &mut AppIcons,
    ) {
        let launcher = &config.launcher;
        let fonts = &config.font;
        let theme = &config.theme;
        // The chain ties the launcher card to the master radius.
        let card_radius = if theme.radius.linked {
            theme.radius.panel()
        } else {
            launcher.corner_radius
        };
        let p = self.progress.value().clamp(0.0, 1.0);
        if p <= 0.001 {
            return;
        }
        self.hits.clear();

        scene.rect(
            Rect::new(0.0, 0.0, size.w, size.h),
            theme.background.with_alpha(0.22 * p),
            0.0,
        );

        let width = launcher.width.min(size.w - 40.0);
        let row_h = 46.0;
        // How many rows fit before the card hits its maximum height.
        let max_rows = (((launcher.max_height - 98.0) / row_h).floor() as usize).max(1);
        let visible = self.rows.len().clamp(1, max_rows);
        self.visible_rows = visible;
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(visible));
        let height = (64.0 + visible as f32 * row_h + 34.0).min(launcher.max_height);
        let x = (size.w - width) * 0.5;
        let y = (size.h - height) * 0.30;
        // The launcher morphs out of the left island: the card travels and
        // resizes, the content fades in once it is in place.
        let target = Rect::new(x, y, width, height);
        self.card = target;
        // The card lands on its final geometry early: the content then fades
        // in over a card that is already in place, never outside it.
        let eased = smoothstep(0.0, 0.62, p);
        let (shape, radius) = match self.origin {
            Some((origin, origin_radius)) => (
                lerp_rect(origin, target, eased),
                origin_radius + (card_radius - origin_radius) * eased,
            ),
            None => (target.scale_centered(0.96 + 0.04 * p), card_radius),
        };
        let content_p = if self.origin.is_some() {
            smoothstep(0.62, 0.92, p)
        } else {
            p
        };

        scene.shadow(
            shape,
            radius,
            lard_render::Shadow {
                color: theme.shadow.scale_alpha(0.8 * p),
                blur: (theme.shadow_blur * 1.35).clamp(8.0, 64.0),
                offset_y: theme.shadow_offset_y.clamp(4.0, 28.0),
                spread: 1.0,
            },
        );
        scene.bordered(
            shape,
            theme
                .surface
                .with_alpha(crate::widgets::panel_alpha(config, 0.66) * p),
            radius,
            theme.border_width,
            theme.border.scale_alpha(p),
        );
        self.shape = shape;
        self.radius = radius;
        // The content is laid out in the final card and fades in.
        let card = target;
        let p = content_p;

        // Search field.
        scene.text_center_y(
            icons::SEARCH,
            card.x + 20.0,
            card.y + 30.0,
            TextStyle {
                size: 19.0,
                align: TextAlign::Center,
                max_width: Some(22.0),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.75 * p),
        );
        let shown_query = truncate(&self.query, 46);
        scene.text(
            if self.query.is_empty() {
                launcher.placeholder.as_str()
            } else {
                shown_query.as_str()
            },
            Point::new(card.x + 48.0, card.y + 20.0),
            TextStyle {
                size: fonts.size_title + 2.0,
                weight: fonts.weight_regular,
                ..Default::default()
            },
            if self.query.is_empty() {
                theme.text_muted.scale_alpha(0.65 * p)
            } else {
                theme.text.scale_alpha(p)
            },
        );
        if (self.elapsed % 1.06) < 0.53 {
            // With an empty query the caret goes in the gap after the search
            // icon: at the text origin it would touch the first glyph.
            let caret_x = if self.query.is_empty() {
                card.x + 43.0
            } else {
                card.x
                    + 22.0
                    + approx_width(&shown_query, fonts.size_title + 2.0)
                    + 26.0
                    + 1.5
            };
            scene.rect(
                Rect::new(caret_x, card.y + 21.0, 1.5, 22.0),
                theme.accent.scale_alpha(p),
                0.75,
            );
        }
        scene.rect(
            Rect::new(card.x + 16.0, card.y + 60.0, card.w - 32.0, 1.0),
            theme.border.scale_alpha(0.7 * p),
            0.5,
        );

        if self.rows.is_empty() {
            scene.text(
                if self.file_pending {
                    "Cerco file…"
                } else {
                    "Nessun risultato"
                },
                Point::new(card.x, card.y + 76.0),
                TextStyle {
                    size: fonts.size_body,
                    align: TextAlign::Center,
                    max_width: Some(card.w),
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.7 * p),
            );
        }

        let first = self.scroll;
        for (row, item) in self.rows.iter().enumerate().skip(first).take(visible) {
            let row_rect = Rect::new(
                card.x + 10.0,
                card.y + 66.0 + (row - first) as f32 * row_h,
                card.w - 20.0,
                row_h - 4.0,
            );
            let selected = row == self.selected;
            if selected {
                scene.rect(
                    row_rect,
                    theme.accent.with_alpha(0.16 * p),
                    theme.radius.pill(),
                );
            } else if self.cursor.is_some_and(|c| row_rect.contains(c)) {
                scene.rect(
                    row_rect,
                    theme.accent.with_alpha(0.075 * p),
                    theme.radius.pill(),
                );
            }
            if selected {
                scene.rect(
                    Rect::new(row_rect.x, row_rect.y + 10.0, 2.5, row_rect.h - 20.0),
                    theme.accent.scale_alpha(p),
                    1.25,
                );
            }

            let avatar = Rect::new(row_rect.x + 12.0, row_rect.y + 6.0, 34.0, 34.0);
            match item {
                Row::App(index) => {
                    let Some(app) = apps.get(*index) else {
                        continue;
                    };
                    // Real application icon when the theme provides one,
                    // symbolic Material glyph as fallback.
                    let tint = icons::app_color(app, theme);
                    match icons.id_for(app, scene) {
                        Some(id) => {
                            scene.image_alpha(avatar.inset(3.0), id, 8.0, p);
                        }
                        None => {
                            scene.rect(
                                avatar,
                                tint.with_alpha(if selected { 0.24 } else { 0.13 } * p),
                                10.0,
                            );
                            scene.text_center_y(
                                icons::app_icon(app),
                                avatar.x,
                                avatar.center().y,
                                TextStyle {
                                    size: 19.0,
                                    align: TextAlign::Center,
                                    max_width: Some(avatar.w),
                                    ..Default::default()
                                }
                                .icon(),
                                tint.scale_alpha(p),
                            );
                        }
                    }

                    scene.text_center_y(
                        truncate(&app.name, 48),
                        row_rect.x + 56.0,
                        row_rect.center().y - 6.0,
                        TextStyle {
                            size: fonts.size_body,
                            weight: fonts.weight_medium,
                            ..Default::default()
                        },
                        theme.text.scale_alpha(p),
                    );
                    let mut subtitle = if !app.generic_name.is_empty() {
                        app.generic_name.clone()
                    } else {
                        app.comment.clone()
                    };
                    if launcher.show_categories
                        && let Some(category) = app.categories.first()
                    {
                        if subtitle.is_empty() {
                            subtitle.clone_from(category);
                        } else {
                            subtitle.push_str(" · ");
                            subtitle.push_str(category);
                        }
                    }
                    if !subtitle.is_empty() {
                        scene.text_center_y(
                            truncate(&subtitle, 70),
                            row_rect.x + 56.0,
                            row_rect.center().y + 10.0,
                            TextStyle {
                                size: fonts.size_small - 0.5,
                                ..Default::default()
                            },
                            theme.text_muted.scale_alpha(0.7 * p),
                        );
                    }
                }
                // Calculator: the value is the row title, the expression the
                // subtitle. Enter copies the value.
                Row::Calc { expression, value } => {
                    scene.rect(
                        avatar,
                        theme
                            .accent
                            .with_alpha(if selected { 0.26 } else { 0.14 } * p),
                        10.0,
                    );
                    scene.text_center_y(
                        "=",
                        avatar.x,
                        avatar.center().y,
                        TextStyle {
                            size: 21.0,
                            weight: fonts.weight_semibold,
                            align: TextAlign::Center,
                            max_width: Some(avatar.w),
                            ..Default::default()
                        },
                        theme.accent.scale_alpha(p),
                    );
                    scene.text_center_y(
                        format!("= {value}"),
                        row_rect.x + 56.0,
                        row_rect.center().y - 6.0,
                        TextStyle {
                            size: fonts.size_body + 1.0,
                            weight: fonts.weight_semibold,
                            ..Default::default()
                        },
                        theme.text.scale_alpha(p),
                    );
                    scene.text_center_y(
                        format!("{expression}    ↵ copia il risultato"),
                        row_rect.x + 56.0,
                        row_rect.center().y + 10.0,
                        TextStyle {
                            size: fonts.size_small - 0.5,
                            max_width: Some(row_rect.w - 70.0),
                            wrap: false,
                            ..Default::default()
                        },
                        theme.text_muted.scale_alpha(0.75 * p),
                    );
                }
                // File finder: name on the first line, parent directory with
                // the home replaced by `~` on the second.
                Row::File(path) => {
                    let is_dir = path.is_dir();
                    let tint = if is_dir {
                        theme.accent
                    } else {
                        theme.text_muted
                    };
                    scene.rect(
                        avatar,
                        tint.with_alpha(if selected { 0.24 } else { 0.12 } * p),
                        10.0,
                    );
                    scene.text_center_y(
                        if is_dir { icons::FOLDER } else { icons::FILE },
                        avatar.x,
                        avatar.center().y,
                        TextStyle {
                            size: 19.0,
                            align: TextAlign::Center,
                            max_width: Some(avatar.w),
                            ..Default::default()
                        }
                        .icon(),
                        tint.scale_alpha(0.95 * p),
                    );
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned());
                    scene.text_center_y(
                        truncate(&name, 48),
                        row_rect.x + 56.0,
                        row_rect.center().y - 6.0,
                        TextStyle {
                            size: fonts.size_body,
                            weight: fonts.weight_medium,
                            ..Default::default()
                        },
                        theme.text.scale_alpha(p),
                    );
                    scene.text_center_y(
                        truncate(&display_path(path), 76),
                        row_rect.x + 56.0,
                        row_rect.center().y + 10.0,
                        TextStyle {
                            size: fonts.size_small - 0.5,
                            ..Default::default()
                        },
                        theme.text_muted.scale_alpha(0.7 * p),
                    );
                }
            }
            if selected {
                scene.text_center_y(
                    "↵",
                    row_rect.right() - 28.0,
                    row_rect.center().y,
                    TextStyle {
                        size: fonts.size_body,
                        align: TextAlign::Right,
                        max_width: Some(18.0),
                        ..Default::default()
                    },
                    theme.accent.scale_alpha(0.9 * p),
                );
            }
            self.hits.push((row_rect, row));
        }

        // Scrollbar: only when the list does not fit in the card.
        if self.rows.len() > visible {
            let track = Rect::new(
                card.right() - 7.0,
                card.y + 68.0,
                3.0,
                visible as f32 * row_h - 8.0,
            );
            scene.rect(track, theme.border.scale_alpha(0.3 * p), 1.5);
            let thumb_h = (track.h * visible as f32 / self.rows.len() as f32).clamp(18.0, track.h);
            let max_scroll = self.rows.len() - visible;
            let t = if max_scroll == 0 {
                0.0
            } else {
                self.scroll as f32 / max_scroll as f32
            };
            scene.rect(
                Rect::new(track.x, track.y + (track.h - thumb_h) * t, track.w, thumb_h),
                theme.text_muted.scale_alpha(0.4 * p),
                1.5,
            );
        }

        scene.text(
            "↑↓ naviga   rotella scorre   ↵ apri   esc chiudi",
            Point::new(card.x, card.bottom() - 24.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                align: TextAlign::Center,
                max_width: Some(card.w),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.55 * p),
        );
    }
}

/// The launcher doubles as a calculator and a file finder. Queries that look
/// like a path are listed from the file system right away; plain words fall
/// back to a bounded background search for files with that name.
fn looks_like_path(query: &str) -> bool {
    query == "~"
        || query.starts_with("~/")
        || query.starts_with('/')
        || query.starts_with("./")
        || query.starts_with("../")
}

/// Replace the home directory prefix with `~` for display.
fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = text.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    text.into_owned()
}

fn expand_home(query: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home {
        Some(home) if query == "~" => home,
        Some(home) => match query.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(query),
        },
        None => PathBuf::from(query),
    }
}

/// List a directory for a path-like query: the parent directory is read and
/// its entries are filtered by the last fragment of the query.
fn list_path(query: &str, limit: usize) -> Vec<PathBuf> {
    let expanded = expand_home(query);
    let (dir, needle) = if query.ends_with('/') || expanded.is_dir() {
        (expanded, String::new())
    } else {
        let needle = expanded
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = expanded
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        (dir, needle)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let needle = needle.to_lowercase();
    let show_hidden = needle.starts_with('.');
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    dirs.sort();
    files.sort();
    dirs.into_iter().chain(files).take(limit).collect()
}

/// Bounded walk of the home directory looking for files whose name contains
/// the query. Runs on a worker thread; the budget keeps it short.
fn search_files_by_name(needle: &str, limit: usize) -> Vec<PathBuf> {
    const BUDGET: usize = 6000;
    const MAX_DEPTH: usize = 4;
    const SKIP: [&str; 8] = [
        "node_modules",
        ".git",
        ".cache",
        "target",
        "venv",
        ".venv",
        "__pycache__",
        ".cargo",
    ];
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let needle = needle.to_lowercase();
    let mut found: Vec<PathBuf> = Vec::new();
    let mut stack = vec![(home, 0usize)];
    let mut budget = BUDGET;
    while let Some((dir, depth)) = stack.pop() {
        if found.len() >= limit || budget == 0 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 || found.len() >= limit {
                break;
            }
            budget -= 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if name.to_lowercase().contains(&needle) {
                found.push(path.clone());
            }
            if is_dir && depth < MAX_DEPTH && !SKIP.contains(&name.as_str()) {
                stack.push((path, depth + 1));
            }
        }
    }
    found
}

/// Interpolate two rectangles (position and size).
fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    let t = t.clamp(0.0, 1.0);
    Rect::new(
        from.x + (to.x - from.x) * t,
        from.y + (to.y - from.y) * t,
        from.w + (to.w - from.w) * t,
        from.h + (to.h - from.h) * t,
    )
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Evaluate a launcher query as an arithmetic expression.
///
/// Only expressions made of numbers, `+ - * / % ^` and parentheses are
/// accepted: a query that is just a number, or that contains letters, is not a
/// calculation.
fn evaluate_expression(input: &str) -> Option<(String, String)> {
    let input = input.trim();
    if input.is_empty() || input.chars().count() > 64 {
        return None;
    }
    let mut has_digit = false;
    let mut has_operator = false;
    for ch in input.chars() {
        match ch {
            '0'..='9' => has_digit = true,
            '+' | '-' | '*' | '/' | '%' | '^' => has_operator = true,
            '.' | '(' | ')' | ' ' | ',' => {}
            _ => return None,
        }
    }
    if !has_digit || !has_operator {
        return None;
    }
    let mut parser = CalcParser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let value = parser.expression()?;
    parser.skip_spaces();
    if parser.pos != parser.bytes.len() || !value.is_finite() {
        return None;
    }
    Some((input.to_string(), format_number(value)))
}

struct CalcParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl CalcParser<'_> {
    fn skip_spaces(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos] == b' ' {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_spaces();
        self.bytes.get(self.pos).copied()
    }

    /// `+` and `-`, lowest precedence.
    fn expression(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    value += self.term()?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    value -= self.term()?;
                }
                _ => return Some(value),
            }
        }
    }

    /// `*`, `/` and `%`.
    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    value *= self.power()?;
                }
                Some(b'/') => {
                    self.pos += 1;
                    value /= self.power()?;
                }
                Some(b'%') => {
                    self.pos += 1;
                    value %= self.power()?;
                }
                _ => return Some(value),
            }
        }
    }

    /// `^`, right associative.
    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some(b'^') {
            self.pos += 1;
            let exponent = self.power()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    /// Unary minus.
    fn unary(&mut self) -> Option<f64> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
            return Some(-self.unary()?);
        }
        if self.peek() == Some(b'+') {
            self.pos += 1;
            return self.unary();
        }
        self.atom()
    }

    /// Number or parenthesised expression.
    fn atom(&mut self) -> Option<f64> {
        match self.peek()? {
            b'(' => {
                self.pos += 1;
                let value = self.expression()?;
                if self.peek()? != b')' {
                    return None;
                }
                self.pos += 1;
                Some(value)
            }
            b'0'..=b'9' | b'.' => {
                let start = self.pos;
                while self
                    .bytes
                    .get(self.pos)
                    .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
                {
                    self.pos += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.pos])
                    .ok()?
                    .parse()
                    .ok()
            }
            _ => None,
        }
    }
}

fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let rounded = (value * 1e10).round() / 1e10;
    if rounded.fract() == 0.0 && rounded.abs() < 1e15 {
        return format!("{}", rounded as i64);
    }
    let text = format!("{rounded:.10}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::{evaluate_expression, format_number, looks_like_path};

    #[test]
    fn calculator_handles_precedence_and_parentheses() {
        assert_eq!(
            evaluate_expression("2+3*4").map(|(_, value)| value),
            Some("14".into())
        );
        assert_eq!(
            evaluate_expression("(2+3)*4").map(|(_, value)| value),
            Some("20".into())
        );
        assert_eq!(
            evaluate_expression("10/4").map(|(_, value)| value),
            Some("2.5".into())
        );
        assert_eq!(
            evaluate_expression("-3 + 1").map(|(_, value)| value),
            Some("-2".into())
        );
        assert_eq!(
            evaluate_expression("2^10").map(|(_, value)| value),
            Some("1024".into())
        );
        assert_eq!(
            evaluate_expression("2*3").map(|(_, value)| value),
            Some("6".into())
        );
        assert!(evaluate_expression("42").is_none());
        assert!(evaluate_expression("firefox").is_none());
        assert!(evaluate_expression("1/0").is_none());
    }

    #[test]
    fn numbers_are_trimmed() {
        assert_eq!(format_number(2.5), "2.5");
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(1.0 / 3.0), "0.3333333333");
    }

    #[test]
    fn path_queries_are_detected() {
        assert!(looks_like_path("~"));
        assert!(looks_like_path("~/Documenti"));
        assert!(looks_like_path("/etc"));
        assert!(looks_like_path("./src"));
        assert!(!looks_like_path("report"));
    }

    #[test]
    fn typing_an_expression_adds_a_calculator_row() {
        use super::{Launcher, LauncherKey, Row};
        use lard_core::anim::SpringConfig;
        let mut launcher = Launcher::new(SpringConfig::default(), true);
        launcher.open(&[], 8);
        for ch in ["2", "*", "3"] {
            launcher.key(LauncherKey::Char(ch.into()), &[], 8);
        }
        match launcher.rows.first() {
            Some(Row::Calc { value, .. }) => assert_eq!(value, "6"),
            other => panic!("expected a calculator row, got {other:?}"),
        }
        // File search may still be running: give it no chance to hide the row.
        assert!(!launcher.rows.is_empty());
    }
}
