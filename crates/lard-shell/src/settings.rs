//! Shell settings center: appearance, bar, dock, effects and diagnostics.
//!
//! The view only emits semantic actions; `Shell` owns the config mutation and
//! persistence so a cancelled edit can be rolled back cleanly.

use crate::ShellKey;
use crate::truncate_to_width;
use crate::icons;
use crate::niri_config::{NiriConfig, NiriSetting, NiriValue};
use lard_core::anim::{Spring, SpringConfig};
use lard_core::theme::{BarPosition, GreeterWallpaper, Theme};
use lard_core::{Color, Config, Point, Rect, Size};
use lard_render::{Scene, Shadow, TextAlign, TextStyle};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Appearance,
    Bar,
    Dock,
    Effects,
    /// Screensaver effect and idle timeout.
    Screen,
    /// Lock screen background.
    Lock,
    /// Edit the niri configuration files.
    Niri,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Workspaces,
    Title,
    Clock,
    System,
    Volume,
    Network,
    Bluetooth,
    Battery,
    Dock,
    DockRunning,
    Blur,
    IslandBlur,
    ReduceMotion,
    /// Chain: every radius follows the card/tile value.
    RadiusLink,
    Screensaver,
    ScreensaverClock,
    /// Include the user-authored Rhai scenes.
    ScreensaverScripts,
    /// Random composition (and effect) on every activation.
    ScreensaverRandom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    BarHeight,
    BarRadius,
    BarOpacity,
    DockHeight,
    DockOpacity,
    RadiusLg,
    RadiusMd,
    RadiusSm,
    OverlayOpacity,
    Frost,
    ScreensaverIdle,
    ScreensaverSpeed,
    ScreensaverIntensity,
    ScreensaverVeil,
    ScreensaverEntrance,
    ScreensaverDust,
    LockDim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Close,
    Save,
    Cancel,
    Tab(SettingsTab),
    Theme(usize),
    Toggle(Toggle),
    Step(Step, i32),
    NextNotificationPosition,
    /// Change a niri setting by a step.
    NiriStep(NiriSetting, i32),
    /// Flip a boolean niri setting.
    NiriToggle(NiriSetting),
    /// Move to the next value of an enumerated niri setting.
    NiriCycle(NiriSetting),
    /// Pick the next output resolution.
    NiriOutputModeNext,
    /// Pick the next output scale.
    NiriOutputScaleNext,
    /// Choose the screensaver effect by index into `ScreensaverEffect::ALL`.
    ScreensaverEffect(usize),
    /// Cycle the lock screen background (gradient / wallpaper).
    NextLockBackground,
    /// Save the edited niri configuration file and reload it.
    SaveNiri,
}

pub struct SettingsView {
    open: bool,
    progress: Spring,
    tab: SettingsTab,
    hits: Vec<(Rect, SettingsAction)>,
    panel: Rect,
    cursor: Option<Point>,
    reduce_motion: bool,
    /// The niri configuration, edited with a form.
    niri: Option<NiriConfig>,
    /// The outputs of niri, for the resolution and scale pickers.
    outputs: Vec<lard_services::niri::OutputView>,
    /// The focused output, the one the form edits.
    output: Option<String>,
    /// Names of the user scenes and their folder.
    script_names: Vec<String>,
    script_dir: std::path::PathBuf,
}

#[derive(Clone, Copy)]
pub struct PalettePreview {
    pub name: &'static str,
    pub accent: Color,
    pub background: Color,
    pub surface: Color,
    pub text: Color,
}

pub const PALETTES: [PalettePreview; 5] = [
    PalettePreview {
        name: "Oro Dank",
        accent: Color::rgba(0.922, 0.757, 0.424, 1.0),
        background: Color::rgba(0.09, 0.075, 0.043, 0.94),
        surface: Color::rgba(0.141, 0.122, 0.09, 0.95),
        text: Color::rgba(0.922, 0.882, 0.831, 1.0),
    },
    PalettePreview {
        name: "Azzurro",
        accent: Color::rgba(0.608, 0.757, 1.0, 1.0),
        background: Color::rgba(0.043, 0.051, 0.063, 0.91),
        surface: Color::rgba(0.078, 0.09, 0.11, 0.94),
        text: Color::rgba(0.949, 0.961, 0.976, 1.0),
    },
    PalettePreview {
        name: "Verde",
        accent: Color::rgba(0.659, 0.835, 0.635, 1.0),
        background: Color::rgba(0.051, 0.071, 0.051, 0.94),
        surface: Color::rgba(0.09, 0.125, 0.09, 0.95),
        text: Color::rgba(0.929, 0.961, 0.918, 1.0),
    },
    PalettePreview {
        name: "Rosa",
        accent: Color::rgba(0.949, 0.655, 0.765, 1.0),
        background: Color::rgba(0.078, 0.051, 0.071, 0.94),
        surface: Color::rgba(0.129, 0.086, 0.114, 0.95),
        text: Color::rgba(0.973, 0.929, 0.949, 1.0),
    },
    PalettePreview {
        name: "Neutro",
        accent: Color::rgba(0.839, 0.839, 0.839, 1.0),
        background: Color::rgba(0.071, 0.071, 0.071, 0.94),
        surface: Color::rgba(0.118, 0.118, 0.118, 0.95),
        text: Color::rgba(0.941, 0.941, 0.941, 1.0),
    },
];

impl SettingsView {
    /// Bounds of the settings panel, used for the backdrop blur region.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    pub fn new(animation: SpringConfig, reduce_motion: bool) -> Self {
        Self {
            open: false,
            progress: Spring::new(0.0, animation),
            tab: SettingsTab::Appearance,
            hits: Vec::with_capacity(48),
            panel: Rect::ZERO,
            cursor: None,
            reduce_motion,
            niri: None,
            outputs: Vec::new(),
            output: None,
            script_names: Vec::new(),
            script_dir: std::path::PathBuf::new(),
        }
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    #[must_use]
    pub fn wants_surface(&self) -> bool {
        self.open || self.progress.value() > 0.01
    }

    pub fn open(&mut self) {
        self.open = true;
        self.tab = SettingsTab::Appearance;
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

    /// Close without an exit animation; used before rebuilding every surface.
    pub fn close_now(&mut self) {
        self.open = false;
        self.progress.snap_to(0.0);
    }

    pub fn select_tab(&mut self, tab: SettingsTab) {
        self.tab = tab;
    }

    pub fn update(&mut self, dt: f32) -> bool {
        self.progress.update(dt)
    }

    pub fn pointer_move(&mut self, pos: Point) {
        self.cursor = Some(pos);
    }

    pub fn pointer_leave(&mut self) {
        self.cursor = None;
    }

    pub fn pointer_press(&mut self, pos: Point) -> Option<SettingsAction> {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, action)| *action);
        match hit {
            Some(action) => Some(action),
            None if !self.panel.contains(pos) => Some(SettingsAction::Cancel),
            None => None,
        }
    }

    /// Keyboard input.
    pub fn key(&mut self, key: &ShellKey) -> Option<SettingsAction> {
        (*key == ShellKey::Escape).then_some(SettingsAction::Cancel)
    }

    /// The niri configuration model, loaded on demand.
    pub fn niri_config(&mut self, dir: &Path, output: Option<&str>) -> &mut NiriConfig {
        if self.niri.is_none() {
            self.niri = Some(NiriConfig::load(dir, output));
        }
        self.niri.as_mut().expect("niri config loaded")
    }

    /// Forget the loaded niri configuration, so it is read again.
    pub fn reset_niri(&mut self) {
        self.niri = None;
    }

    /// The outputs of niri, for the resolution picker.
    /// Names and folder of the user scenes, for the Screen tab.
    pub fn set_script_scenes(&mut self, names: Vec<String>, dir: std::path::PathBuf) {
        self.script_names = names;
        self.script_dir = dir;
    }

    pub fn set_outputs(&mut self, outputs: Vec<lard_services::niri::OutputView>) {
        self.outputs = outputs;
    }

    /// Apply the pending changes to the niri configuration files.
    pub fn save_niri(&mut self) -> Vec<std::path::PathBuf> {
        let Some(model) = self.niri.as_mut() else {
            return Vec::new();
        };
        model.apply();
        model.save()
    }

    /// Pick the next output resolution from the modes niri reports.
    pub fn niri_output_mode_next(&mut self) {
        let Some(output) = self.output.clone() else {
            return;
        };
        let modes: Vec<String> = self
            .outputs
            .iter()
            .find(|candidate| candidate.name == output)
            .map(|view| view.modes.clone())
            .unwrap_or_default();
        if modes.is_empty() {
            return;
        }
        let dir = crate::niri_config::niri_config_dir();
        let current = self
            .niri_config(&dir, Some(&output))
            .value(NiriSetting::OutputMode)
            .and_then(NiriValue::as_text)
            .unwrap_or_default()
            .to_string();
        let index = modes.iter().position(|mode| *mode == current).unwrap_or(0);
        let mode = modes[(index + 1) % modes.len()].clone();
        self.niri_config(&dir, Some(&output))
            .set(NiriSetting::OutputMode, NiriValue::Text(mode));
    }

    /// Pick the next output scale.
    pub fn niri_output_scale_next(&mut self) {
        const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
        let Some(output) = self.output.clone() else {
            return;
        };
        let dir = crate::niri_config::niri_config_dir();
        let current = self
            .niri_config(&dir, Some(&output))
            .value(NiriSetting::OutputScale)
            .and_then(NiriValue::as_float)
            .unwrap_or(1.0);
        let index = SCALES
            .iter()
            .position(|scale| (*scale - current).abs() < 0.01)
            .unwrap_or(0);
        let scale = SCALES[(index + 1) % SCALES.len()];
        self.niri_config(&dir, Some(&output))
            .set(NiriSetting::OutputScale, NiriValue::Float(scale));
    }

    /// The focused output, the one the form edits.
    pub fn set_output(&mut self, output: Option<String>) {
        if self.output != output {
            self.output = output;
            self.reset_niri();
        }
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

        let width = 1040.0_f32.min(size.w - 32.0);
        let height = 680.0_f32.min(size.h - 32.0);
        let x = (size.w - width) * 0.5;
        let y = (size.h - height) * 0.5 + (1.0 - p) * 16.0;
        let panel = Rect::new(x, y, width, height);
        let panel = panel.scale_centered(0.96 + 0.04 * p);
        self.panel = panel;

        scene.shadow(
            panel,
            theme.radius.panel(),
            Shadow {
                color: theme.shadow.scale_alpha(0.8 * p),
                blur: (theme.shadow_blur * 1.6).clamp(10.0, 72.0),
                offset_y: theme.shadow_offset_y.clamp(4.0, 30.0),
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

        let close = Rect::new(panel.right() - 44.0, panel.y + 14.0, 30.0, 30.0);
        scene.text_center_y(
            icons::CLOSE,
            close.x,
            close.center().y,
            TextStyle {
                size: 18.0,
                align: TextAlign::Center,
                max_width: Some(close.w),
                ..Default::default()
            }
            .icon(),
            theme.text_muted.scale_alpha(0.9 * p),
        );
        if self.cursor.is_some_and(|c| close.contains(c)) {
            scene.rect(
                close,
                theme.surface_hover.scale_alpha(0.8 * p),
                theme.radius.control(),
            );
        }
        self.hits.push((close, SettingsAction::Close));

        scene.text(
            "Impostazioni shell",
            Point::new(panel.x + 24.0, panel.y + 18.0),
            TextStyle {
                size: fonts.size_title,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            "LARD · light as rust dust",
            Point::new(panel.x + 24.0, panel.y + 40.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.65 * p),
        );

        let sidebar = Rect::new(panel.x + 16.0, panel.y + 72.0, 190.0, panel.h - 134.0);
        scene.rect(
            sidebar,
            theme.background.scale_alpha(0.42 * p),
            theme.radius.card(),
        );
        let tabs = [
            (SettingsTab::Appearance, icons::PALETTE, "Aspetto"),
            (SettingsTab::Bar, icons::DASHBOARD, "Barra"),
            (SettingsTab::Dock, icons::DOCK, "Dock"),
            (SettingsTab::Effects, icons::BLUR, "Effetti"),
            (SettingsTab::Screen, icons::SCREEN, "Schermo"),
            (SettingsTab::Lock, icons::LOCK, "Blocco"),
            (SettingsTab::Niri, icons::CODE, "Niri"),
            (SettingsTab::Info, icons::INFO, "Info"),
        ];
        for (index, (tab, icon, label)) in tabs.iter().enumerate() {
            let rect = Rect::new(
                sidebar.x + 8.0,
                sidebar.y + 8.0 + index as f32 * 42.0,
                sidebar.w - 16.0,
                36.0,
            );
            let selected = self.tab == *tab;
            if selected || self.cursor.is_some_and(|c| rect.contains(c)) {
                scene.rect(
                    rect,
                    theme
                        .accent
                        .with_alpha(if selected { 0.17 } else { 0.07 } * p),
                    10.0,
                );
            }
            scene.text_center_y(
                icon,
                rect.x + 12.0,
                rect.center().y,
                TextStyle {
                    size: 17.0,
                    align: TextAlign::Center,
                    max_width: Some(22.0),
                    ..Default::default()
                }
                .icon(),
                if selected {
                    theme.accent.scale_alpha(p)
                } else {
                    theme.text_muted.scale_alpha(0.8 * p)
                },
            );
            scene.text_center_y(
                label,
                rect.x + 42.0,
                rect.center().y,
                TextStyle {
                    size: fonts.size_body,
                    weight: if selected {
                        fonts.weight_medium
                    } else {
                        fonts.weight_regular
                    },
                    ..Default::default()
                },
                if selected {
                    theme.text.scale_alpha(p)
                } else {
                    theme.text_muted.scale_alpha(0.8 * p)
                },
            );
            self.hits.push((rect, SettingsAction::Tab(*tab)));
        }
        scene.text(
            "0.1 · Rust",
            Point::new(sidebar.x + 12.0, sidebar.bottom() - 24.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.45 * p),
        );

        let content = Rect::new(
            panel.x + 226.0,
            panel.y + 82.0,
            panel.w - 250.0,
            panel.h - 154.0,
        );
        self.build_tab(scene, content, config, p);

        let cancel = Rect::new(panel.right() - 214.0, panel.bottom() - 50.0, 92.0, 32.0);
        let save = Rect::new(panel.right() - 112.0, panel.bottom() - 50.0, 88.0, 32.0);
        self.button(scene, cancel, "Annulla", false, config, p);
        self.button(scene, save, "Salva", true, config, p);
        self.hits.push((cancel, SettingsAction::Cancel));
        self.hits.push((save, SettingsAction::Save));
        scene.text(
            "Layout e blur si applicano al salvataggio",
            Point::new(content.x, panel.bottom() - 44.0),
            TextStyle {
                size: fonts.size_small - 1.5,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.45 * p),
        );
    }

    fn button(
        &self,
        scene: &mut Scene,
        rect: Rect,
        label: &str,
        primary: bool,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let hovered = self.cursor.is_some_and(|c| rect.contains(c));
        let (bg, color) = if primary {
            (
                theme
                    .accent
                    .scale_alpha((if hovered { 1.0 } else { 0.88 }) * p),
                theme.accent_text.scale_alpha(p),
            )
        } else {
            (
                theme
                    .surface_hover
                    .scale_alpha((if hovered { 0.9 } else { 0.45 }) * p),
                theme.text.scale_alpha(0.9 * p),
            )
        };
        scene.rect(rect, bg, theme.radius.control());
        scene.text_center_y(
            label,
            rect.x,
            rect.center().y,
            TextStyle {
                size: config.font.size_small,
                weight: config.font.weight_medium,
                align: TextAlign::Center,
                max_width: Some(rect.w),
                ..Default::default()
            },
            color,
        );
    }

    fn build_tab(&mut self, scene: &mut Scene, content: Rect, config: &Config, p: f32) {
        let theme = &config.theme;
        let fonts = &config.font;
        let title = match self.tab {
            SettingsTab::Appearance => "Aspetto",
            SettingsTab::Bar => "Barra",
            SettingsTab::Dock => "Dock",
            SettingsTab::Effects => "Effetti",
            SettingsTab::Screen => "Schermo",
            SettingsTab::Lock => "Blocco",
            SettingsTab::Niri => "Config di niri",
            SettingsTab::Info => "Informazioni",
        };
        scene.text(
            title,
            Point::new(content.x, content.y),
            TextStyle {
                size: fonts.size_title + 2.0,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        let y = content.y + 42.0;
        match self.tab {
            SettingsTab::Appearance => self.build_appearance(scene, content, y, config, p),
            SettingsTab::Bar => self.build_bar(scene, content, y, config, p),
            SettingsTab::Dock => self.build_dock(scene, content, y, config, p),
            SettingsTab::Effects => self.build_effects(scene, content, y, config, p),
            SettingsTab::Screen => self.build_screen(scene, content, y, config, p),
            SettingsTab::Lock => self.build_lock(scene, content, y, config, p),
            SettingsTab::Niri => self.build_niri(scene, content, y, config, p),
            SettingsTab::Info => self.build_info(scene, content, y, config, p),
        }
    }

    /// The niri configuration, as a form.
    fn build_niri(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        let theme = &config.theme;
        let fonts = &config.font;
        let dir = crate::niri_config::niri_config_dir();
        let output = self
            .output
            .clone()
            .or_else(|| self.outputs.first().map(|output| output.name.clone()));
        if self.niri.is_none() {
            self.niri = Some(NiriConfig::load(&dir, output.as_deref()));
        }
        // Take the model out so the drawing code can borrow the view.
        let model = self.niri.take().expect("niri configuration loaded");
        scene.text(
            "Le impostazioni di niri si applicano ai file di configurazione, con un backup automatico.",
            Point::new(content.x, y),
            TextStyle {
                size: fonts.size_small,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.7 * p),
        );

        let mut cursor_y = y + 26.0;
        let row_h = 34.0;

        // --- the screen ---
        // The runtime values are the fallback when the configuration does not
        // pin the mode or the scale.
        let view = self
            .outputs
            .iter()
            .find(|candidate| Some(&candidate.name) == output.as_ref())
            .cloned();
        let mode = model
            .value(NiriSetting::OutputMode)
            .and_then(NiriValue::as_text)
            .map(str::to_string)
            .or_else(|| view.as_ref().and_then(|view| view.current_mode.clone()))
            .unwrap_or_default();
        scene.text(
            "Schermo",
            Point::new(content.x, cursor_y),
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.8 * p),
        );
        cursor_y += 20.0;
        let mode_button = Rect::new(content.x, cursor_y, 260.0, row_h);
        self.picker_row(
            scene,
            mode_button,
            "Risoluzione",
            &mode,
            SettingsAction::NiriOutputModeNext,
            config,
            p,
        );
        let scale = model
            .value(NiriSetting::OutputScale)
            .and_then(NiriValue::as_float)
            .or_else(|| view.as_ref().map(|view| view.scale))
            .unwrap_or(1.0);
        let scale_button = Rect::new(content.x + 280.0, cursor_y, 200.0, row_h);
        self.picker_row(
            scene,
            scale_button,
            "Scala",
            &format!("{scale:.2}"),
            SettingsAction::NiriOutputScaleNext,
            config,
            p,
        );
        cursor_y += row_h + 18.0;

        // --- the layout ---
        self.niri_section(scene, content.x, cursor_y, "Layout", config, p);
        cursor_y += 22.0;
        let gaps = model
            .value(NiriSetting::Gaps)
            .and_then(NiriValue::as_int)
            .unwrap_or(0);
        self.niri_number(
            scene,
            content,
            cursor_y,
            NiriSetting::Gaps,
            "Spazio fra le finestre",
            &format!("{gaps} px"),
            1,
            config,
            p,
        );
        cursor_y += row_h;
        let center = model
            .value(NiriSetting::CenterFocusedColumn)
            .and_then(NiriValue::as_text)
            .unwrap_or("never")
            .to_string();
        self.niri_choice(
            scene,
            content,
            cursor_y,
            NiriSetting::CenterFocusedColumn,
            "Colonna al centro",
            &center,
            config,
            p,
        );
        cursor_y += row_h;
        let column = model
            .value(NiriSetting::ColumnWidth)
            .and_then(NiriValue::as_float)
            .unwrap_or(0.5);
        self.niri_number(
            scene,
            content,
            cursor_y,
            NiriSetting::ColumnWidth,
            "Larghezza predefinita",
            &format!("{column:.2}"),
            5,
            config,
            p,
        );
        cursor_y += row_h;
        let height = model
            .value(NiriSetting::WindowHeight)
            .and_then(NiriValue::as_float)
            .unwrap_or(0.7);
        self.niri_number(
            scene,
            content,
            cursor_y,
            NiriSetting::WindowHeight,
            "Altezza predefinita",
            &format!("{height:.2}"),
            5,
            config,
            p,
        );
        cursor_y += row_h + 18.0;

        // The last two sections share the width.
        let column_w = (content.w - 20.0) * 0.5;
        let left = Rect::new(content.x, cursor_y, column_w, content.h);
        let right = Rect::new(content.x + column_w + 20.0, cursor_y, column_w, content.h);

        // --- the windows ---
        self.niri_section(scene, left.x, left.y, "Finestre", config, p);
        let mut y = left.y + 22.0;
        for (setting, label) in [
            (NiriSetting::BorderWidth, "Bordo delle finestre"),
            (NiriSetting::BorderRadius, "Angolo del bordo"),
            (NiriSetting::FocusRingWidth, "Bordo del focus"),
            (NiriSetting::FocusRingRadius, "Angolo del focus"),
        ] {
            let value = model
                .value(setting)
                .and_then(NiriValue::as_int)
                .unwrap_or(0);
            self.niri_number(
                scene,
                left,
                y,
                setting,
                label,
                &format!("{value} px"),
                1,
                config,
                p,
            );
            y += row_h;
        }

        // --- the tab bar ---
        self.niri_section(scene, right.x, right.y, "Barra delle schede", config, p);
        let mut y = right.y + 22.0;
        let tab_enabled = model
            .value(NiriSetting::TabBarEnabled)
            .and_then(NiriValue::as_bool)
            .unwrap_or(false);
        self.niri_switch(
            scene,
            right,
            y,
            NiriSetting::TabBarEnabled,
            "Barra delle schede",
            tab_enabled,
            config,
            p,
        );
        y += row_h;
        let position = model
            .value(NiriSetting::TabBarPosition)
            .and_then(NiriValue::as_text)
            .unwrap_or("top")
            .to_string();
        self.niri_choice(
            scene,
            right,
            y,
            NiriSetting::TabBarPosition,
            "Posizione",
            &position,
            config,
            p,
        );
        y += row_h;
        let title = model
            .value(NiriSetting::TabBarTitle)
            .and_then(NiriValue::as_bool)
            .unwrap_or(false);
        self.niri_switch(
            scene,
            right,
            y,
            NiriSetting::TabBarTitle,
            "Titolo delle schede",
            title,
            config,
            p,
        );

        // The apply button: the changes are written to the files and niri is
        // asked to reload them.
        let button = Rect::new(
            content.right() - 168.0,
            content.bottom() - 44.0,
            168.0,
            34.0,
        );
        let hovered = self.cursor.is_some_and(|c| button.contains(c));
        scene.rect(
            button,
            theme
                .accent
                .with_alpha((if hovered { 0.34 } else { 0.24 }) * p),
            theme.radius.control(),
        );
        scene.text_center_y(
            "Salva e ricarica niri",
            button.x,
            button.center().y,
            TextStyle {
                size: fonts.size_body - 1.0,
                weight: fonts.weight_semibold,
                align: TextAlign::Center,
                max_width: Some(button.w),
                ..Default::default()
            },
            theme.accent_text.scale_alpha(p),
        );
        self.hits.push((button, SettingsAction::SaveNiri));
        scene.text(
            "Le modifiche vengono scritte nei file di niri (con backup .bak) e niri viene ricaricato.",
            Point::new(content.x, content.bottom() - 30.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.5 * p),
        );
        self.niri = Some(model);
    }

    /// A section title of the niri form.
    fn niri_section(
        &mut self,
        scene: &mut Scene,
        x: f32,
        y: f32,
        title: &str,
        config: &Config,
        p: f32,
    ) {
        scene.text(
            title,
            Point::new(x, y),
            TextStyle {
                size: config.font.size_small,
                weight: config.font.weight_medium,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.8 * p),
        );
    }

    /// A number with a stepper.
    #[allow(clippy::too_many_arguments)]
    fn niri_number(
        &mut self,
        scene: &mut Scene,
        content: Rect,
        y: f32,
        setting: NiriSetting,
        label: &str,
        value: &str,
        step: i32,
        config: &Config,
        p: f32,
    ) {
        self.stepper_row(
            scene,
            Rect::new(content.x, y, content.w, 34.0),
            label,
            value,
            SettingsAction::NiriStep(setting, -step),
            SettingsAction::NiriStep(setting, step),
            config,
            p,
        );
    }

    /// An enumerated setting: clicking cycles through the values.
    fn niri_choice(
        &mut self,
        scene: &mut Scene,
        content: Rect,
        y: f32,
        setting: NiriSetting,
        label: &str,
        value: &str,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text_center_y(
            label,
            content.x + 8.0,
            y + 17.0,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        let button = Rect::new(content.right() - 150.0, y + 3.0, 142.0, 28.0);
        let hovered = self.cursor.is_some_and(|c| button.contains(c));
        scene.rect(
            button,
            theme
                .surface_hover
                .scale_alpha((if hovered { 0.9 } else { 0.5 }) * p),
            theme.radius.control(),
        );
        scene.text_center_y(
            value,
            button.x,
            button.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(button.w),
                ..Default::default()
            },
            theme.text.scale_alpha(0.92 * p),
        );
        self.hits.push((button, SettingsAction::NiriCycle(setting)));
    }

    /// A boolean setting.
    fn niri_switch(
        &mut self,
        scene: &mut Scene,
        content: Rect,
        y: f32,
        setting: NiriSetting,
        label: &str,
        on: bool,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text_center_y(
            label,
            content.x + 8.0,
            y + 17.0,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        let switch = Rect::new(content.right() - 58.0, y + 4.0, 52.0, 26.0);
        scene.rect(
            switch,
            if on {
                theme.accent.scale_alpha(0.85 * p)
            } else {
                theme.background.scale_alpha(0.9 * p)
            },
            13.0,
        );
        scene.rect(
            Rect::new(
                switch.x + if on { 28.0 } else { 2.0 },
                switch.y + 2.0,
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
        self.hits
            .push((switch, SettingsAction::NiriToggle(setting)));
    }

    /// A picker: a button that cycles through the values.
    #[allow(clippy::too_many_arguments)]
    fn picker_row(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        label: &str,
        value: &str,
        action: SettingsAction,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text_center_y(
            label,
            rect.x + 8.0,
            rect.center().y,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        let button = Rect::new(rect.x + 90.0, rect.y + 3.0, rect.w - 96.0, 28.0);
        let hovered = self.cursor.is_some_and(|c| button.contains(c));
        scene.rect(
            button,
            theme
                .surface_hover
                .scale_alpha((if hovered { 0.9 } else { 0.5 }) * p),
            theme.radius.control(),
        );
        scene.text_center_y(
            value,
            button.x,
            button.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(button.w),
                ..Default::default()
            },
            theme.text.scale_alpha(0.92 * p),
        );
        self.hits.push((button, action));
    }

    fn build_appearance(
        &mut self,
        scene: &mut Scene,
        content: Rect,
        y: f32,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text(
            "Palette",
            Point::new(content.x, y),
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.8 * p),
        );
        let columns = 3;
        let card_w = (content.w - 16.0 * (columns - 1) as f32) / columns as f32;
        let card_h = 66.0;
        for (index, palette) in PALETTES.iter().enumerate() {
            let col = index % columns;
            let row = index / columns;
            let rect = Rect::new(
                content.x + col as f32 * (card_w + 16.0),
                y + 24.0 + row as f32 * (card_h + 10.0),
                card_w,
                card_h,
            );
            let selected = theme
                .accent
                .to_hex()
                .eq_ignore_ascii_case(&palette.accent.to_hex());
            let hovered = self.cursor.is_some_and(|c| rect.contains(c));
            scene.rect(
                rect,
                if selected || hovered {
                    palette
                        .accent
                        .with_alpha(if selected { 0.16 } else { 0.08 } * p)
                } else {
                    theme.background.scale_alpha(0.55 * p)
                },
                theme.radius.card(),
            );
            if selected {
                scene.bordered(
                    rect,
                    Color::TRANSPARENT,
                    theme.radius.card(),
                    1.0,
                    palette.accent.scale_alpha(0.65 * p),
                );
            }
            scene.rect(
                Rect::new(rect.x + 12.0, rect.y + 17.0, 28.0, 28.0),
                palette.accent.scale_alpha(p),
                9.0,
            );
            scene.text(
                palette.name,
                Point::new(rect.x + 50.0, rect.y + 15.0),
                TextStyle {
                    size: fonts.size_body,
                    weight: fonts.weight_medium,
                    ..Default::default()
                },
                palette.text.scale_alpha(p),
            );
            scene.text(
                palette.accent.to_hex(),
                Point::new(rect.x + 50.0, rect.y + 34.0),
                TextStyle {
                    size: fonts.size_small - 1.0,
                    ..Default::default()
                },
                theme.text_muted.scale_alpha(0.65 * p),
            );
            self.hits.push((rect, SettingsAction::Theme(index)));
        }
        let r1_y = y + 24.0 + 2.0 * (card_h + 10.0) + 26.0;
        let linked = config.theme.radius.linked;
        let master = config.theme.radius.card().round() as u32;
        // The chain ties every radius to the card/tile value.
        let chain = Rect::new(content.right() - 36.0, r1_y + 6.0, 30.0, 30.0);
        let chain_hover = self.cursor.is_some_and(|c| chain.contains(c));
        scene.rect(
            chain,
            if linked {
                theme.accent.with_alpha(0.35 * p)
            } else if chain_hover {
                theme.surface_hover.scale_alpha(0.9 * p)
            } else {
                theme.surface_hover.scale_alpha(0.45 * p)
            },
            theme.radius.control(),
        );
        scene.text_center_y(
            icons::LINK,
            chain.x,
            chain.center().y,
            TextStyle {
                size: 15.0,
                align: TextAlign::Center,
                max_width: Some(chain.w),
                ..Default::default()
            }
            .icon(),
            if linked {
                theme.accent.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.8 * p)
            },
        );
        self.hits
            .push((chain, SettingsAction::Toggle(Toggle::RadiusLink)));
        self.stepper_row_state(
            scene,
            Rect::new(content.x, r1_y, content.w - 44.0, 44.0),
            "Angolo pannelli",
            &format!("{} px", config.theme.radius.panel().round() as u32),
            SettingsAction::Step(Step::RadiusLg, -2),
            SettingsAction::Step(Step::RadiusLg, 2),
            config,
            p,
            linked,
        );
        self.stepper_row_state(
            scene,
            Rect::new(content.x, r1_y + 48.0, content.w, 44.0),
            "Angolo card/tile",
            &format!("{master} px"),
            SettingsAction::Step(Step::RadiusMd, -2),
            SettingsAction::Step(Step::RadiusMd, 2),
            config,
            p,
            false,
        );
        self.stepper_row_state(
            scene,
            Rect::new(content.x, r1_y + 96.0, content.w, 44.0),
            "Angolo controlli",
            &format!("{} px", config.theme.radius.control().round() as u32),
            SettingsAction::Step(Step::RadiusSm, -2),
            SettingsAction::Step(Step::RadiusSm, 2),
            config,
            p,
            linked,
        );
        scene.text(
            if linked {
                "Catena attiva: ogni angolo segue il valore card/tile."
            } else if config.theme.follow_niri {
                "Sfondo, accento e pericolo seguono niri (layout.background-color, focus-ring.color, border.urgent-color)."
            } else {
                "Suggerimento: la palette si applica a barra, launcher, pannelli e lockscreen."
            },
            Point::new(content.x, r1_y + 150.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.5 * p),
        );
    }

    fn build_bar(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        self.stepper_row(
            scene,
            Rect::new(content.x, y, content.w, 44.0),
            "Altezza barra",
            &format!("{} px", config.bar.height.round() as u32),
            SettingsAction::Step(Step::BarHeight, -2),
            SettingsAction::Step(Step::BarHeight, 2),
            config,
            p,
        );
        self.stepper_row(
            scene,
            Rect::new(content.x, y + 52.0, content.w, 44.0),
            "Angolo isole",
            &format!("{} px", config.bar.corner_radius.round() as u32),
            SettingsAction::Step(Step::BarRadius, -2),
            SettingsAction::Step(Step::BarRadius, 2),
            config,
            p,
        );
        self.stepper_row(
            scene,
            Rect::new(content.x, y + 104.0, content.w, 44.0),
            "Opacità isole",
            &format!("{}%", (config.bar.opacity * 100.0).round() as u32),
            SettingsAction::Step(Step::BarOpacity, -5),
            SettingsAction::Step(Step::BarOpacity, 5),
            config,
            p,
        );
        let rows = [
            (Toggle::Workspaces, "Workspace", "Mostra i workspace niri"),
            (
                Toggle::Title,
                "Titolo finestra",
                "Titolo dell'app focalizzata",
            ),
            (Toggle::Clock, "Orologio", "Ora e data al centro"),
            (
                Toggle::System,
                "CPU / RAM",
                "Carico nel centro di controllo",
            ),
            (
                Toggle::Volume,
                "Volume",
                "Slider volume nel centro di controllo",
            ),
            (Toggle::Network, "Rete", "Wi-Fi nel centro di controllo"),
            (
                Toggle::Bluetooth,
                "Bluetooth",
                "Bluetooth nel centro di controllo",
            ),
            (
                Toggle::Battery,
                "Batteria",
                "Stato batteria nel centro di controllo",
            ),
        ];
        let start = y + 158.0;
        for (index, (toggle, label, hint)) in rows.iter().enumerate() {
            let rect = Rect::new(content.x, start + index as f32 * 42.0, content.w, 38.0);
            let value = match toggle {
                Toggle::Workspaces => config.bar.show_workspaces,
                Toggle::Title => config.bar.show_window_title,
                Toggle::Clock => config.bar.show_clock,
                Toggle::System => config.bar.show_system,
                Toggle::Volume => config.bar.show_volume,
                Toggle::Network => config.bar.show_network,
                Toggle::Bluetooth => config.bar.show_bluetooth,
                Toggle::Battery => config.bar.show_battery,
                _ => false,
            };
            self.toggle_row(
                scene,
                rect,
                label,
                hint,
                value,
                SettingsAction::Toggle(*toggle),
                config,
                p,
            );
        }
    }

    fn build_dock(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        let rows = [
            (Toggle::Dock, "Dock attivo", "Isola applicazioni in basso"),
            (
                Toggle::DockRunning,
                "App in esecuzione",
                "Aggiungi le app aperte alle preferite",
            ),
        ];
        for (index, (toggle, label, hint)) in rows.iter().enumerate() {
            let value = match toggle {
                Toggle::Dock => config.dock.enabled,
                Toggle::DockRunning => config.dock.show_running,
                _ => false,
            };
            self.toggle_row(
                scene,
                Rect::new(content.x, y + index as f32 * 48.0, content.w, 42.0),
                label,
                hint,
                value,
                SettingsAction::Toggle(*toggle),
                config,
                p,
            );
        }
        self.stepper_row(
            scene,
            Rect::new(content.x, y + 108.0, content.w, 46.0),
            "Altezza dock",
            &format!("{} px", config.dock.height.round() as u32),
            SettingsAction::Step(Step::DockHeight, -2),
            SettingsAction::Step(Step::DockHeight, 2),
            config,
            p,
        );
        self.stepper_row(
            scene,
            Rect::new(content.x, y + 162.0, content.w, 46.0),
            "Opacità dock",
            &format!("{}%", (config.dock.opacity * 100.0).round() as u32),
            SettingsAction::Step(Step::DockOpacity, -5),
            SettingsAction::Step(Step::DockOpacity, 5),
            config,
            p,
        );
        scene.text(
            "App fissate",
            Point::new(content.x, y + 230.0),
            TextStyle {
                size: config.font.size_small,
                weight: config.font.weight_medium,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.8 * p),
        );
        let pins = config.dock.pinned.join("  ·  ");
        scene.text(
            crate::widgets::truncate(&pins, 74),
            Point::new(content.x, y + 252.0),
            TextStyle {
                size: config.font.size_body,
                ..Default::default()
            },
            config.theme.text.scale_alpha(0.86 * p),
        );
        scene.text(
            "Modifica l'elenco in config.toml → [dock].pinned",
            Point::new(content.x, y + 276.0),
            TextStyle {
                size: config.font.size_small - 1.0,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.5 * p),
        );
    }

    fn build_effects(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        let rows = [
            (
                Toggle::Blur,
                "Blur pannelli",
                "ext-background-effect-v1 su launcher, control center, impostazioni, popover, wallpaper",
            ),
            (
                Toggle::IslandBlur,
                "Blur isole",
                "ext-background-effect-v1 su barre e dock",
            ),
            (
                Toggle::ReduceMotion,
                "Riduci movimento",
                "Disattiva le molle all'avvio",
            ),
        ];
        for (index, (toggle, label, hint)) in rows.iter().enumerate() {
            let value = match toggle {
                Toggle::Blur => config.theme.blur,
                Toggle::IslandBlur => config.theme.island_blur,
                Toggle::ReduceMotion => config.animation.reduce_motion,
                _ => false,
            };
            self.toggle_row(
                scene,
                Rect::new(content.x, y + index as f32 * 52.0, content.w, 46.0),
                label,
                hint,
                value,
                SettingsAction::Toggle(*toggle),
                config,
                p,
            );
        }
        self.stepper_row(
            scene,
            Rect::new(content.x, y + 160.0, content.w, 46.0),
            "Opacità pannelli",
            &format!("{}%", (config.theme.overlay_opacity * 100.0).round() as u32),
            SettingsAction::Step(Step::OverlayOpacity, -5),
            SettingsAction::Step(Step::OverlayOpacity, 5),
            config,
            p,
        );
        self.stepper_row(
            scene,
            Rect::new(content.x, y + 212.0, content.w, 46.0),
            "Livello vetro",
            &format!("{}%", (config.theme.frost * 100.0).round() as u32),
            SettingsAction::Step(Step::Frost, -5),
            SettingsAction::Step(Step::Frost, 5),
            config,
            p,
        );
        let pos = match config.notifications.position {
            lard_core::theme::ToastPosition::TopRight => "Alto destra",
            lard_core::theme::ToastPosition::TopLeft => "Alto sinistra",
            lard_core::theme::ToastPosition::BottomRight => "Basso destra",
            lard_core::theme::ToastPosition::BottomLeft => "Basso sinistra",
        };
        let rect = Rect::new(content.x, y + 272.0, content.w, 46.0);
        scene.text(
            "Posizione notifiche",
            Point::new(rect.x, rect.y + 5.0),
            TextStyle {
                size: config.font.size_body,
                weight: config.font.weight_medium,
                ..Default::default()
            },
            config.theme.text.scale_alpha(p),
        );
        scene.text(
            "Dove compaiono i toast",
            Point::new(rect.x, rect.y + 24.0),
            TextStyle {
                size: config.font.size_small - 1.0,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.6 * p),
        );
        let button = Rect::new(rect.right() - 132.0, rect.y + 7.0, 132.0, 30.0);
        let hovered = self.cursor.is_some_and(|c| button.contains(c));
        scene.rect(
            button,
            config
                .theme
                .surface_hover
                .scale_alpha((if hovered { 0.9 } else { 0.5 }) * p),
            9.0,
        );
        scene.text_center_y(
            pos,
            button.x,
            button.center().y,
            TextStyle {
                size: config.font.size_small,
                align: TextAlign::Center,
                max_width: Some(button.w),
                ..Default::default()
            },
            config.theme.text.scale_alpha(0.9 * p),
        );
        self.hits
            .push((button, SettingsAction::NextNotificationPosition));
        scene.text(
            "Il blur viene ricreato al salvataggio. Su niri richiede ext-background-effect-v1.",
            Point::new(content.x, y + 348.0),
            TextStyle {
                size: config.font.size_small - 1.0,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.5 * p),
        );
    }

    /// Built-in screensaver: scenarios, idle time and overlay options.
    fn build_screen(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        let theme = &config.theme;
        let fonts = &config.font;
        let saver = &config.screensaver;
        let dimmed = !saver.enabled;
        let row_h = 36.0;
        let pitch = 40.0;

        self.toggle_row(
            scene,
            Rect::new(content.x, y, content.w, row_h),
            "Salvaschermo",
            "Effetto fluido a schermo intero quando la sessione è inattiva",
            saver.enabled,
            SettingsAction::Toggle(Toggle::Screensaver),
            config,
            p,
        );
        self.stepper_row_state(
            scene,
            Rect::new(content.x, y + pitch, content.w, row_h),
            "Dopo",
            &format!("{} min", saver.idle_minutes),
            SettingsAction::Step(Step::ScreensaverIdle, -1),
            SettingsAction::Step(Step::ScreensaverIdle, 1),
            config,
            p,
            dimmed,
        );

        // Scenario picker: three rows of three chips.
        let rect = Rect::new(content.x, y + pitch * 2.0, content.w, 70.0);
        scene.text(
            "Scenario",
            Point::new(rect.x + 8.0, rect.y + 2.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            "Frecce ← → per cambiarli al volo",
            Point::new(rect.x + 8.0, rect.y + 21.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.58 * p),
        );
        let chip_h = 22.0;
        let gap = 4.0;
        let chip_w = ((rect.w * 0.44 - gap * 2.0) / 3.0).clamp(66.0, 118.0);
        let total = chip_w * 3.0 + gap * 2.0;
        for (index, effect) in lard_core::theme::ScreensaverEffect::ALL.iter().enumerate() {
            let column = (index % 3) as f32;
            let row = (index / 3) as f32;
            let chip = Rect::new(
                rect.right() - total + column * (chip_w + gap),
                rect.y + row * (chip_h + gap),
                chip_w,
                chip_h,
            );
            let selected = !saver.random && saver.effect == *effect;
            let hovered = self.cursor.is_some_and(|c| chip.contains(c));
            let alpha = if selected {
                0.9
            } else if hovered {
                0.30
            } else {
                0.14
            };
            scene.rect(
                chip,
                if selected {
                    theme.accent.scale_alpha(alpha * p)
                } else {
                    theme.surface_hover.scale_alpha(alpha * p)
                },
                chip_h * 0.5,
            );
            scene.text_center_y(
                effect.label(),
                chip.x,
                chip.center().y,
                TextStyle {
                    size: fonts.size_small,
                    weight: if selected {
                        fonts.weight_medium
                    } else {
                        fonts.weight_regular
                    },
                    align: TextAlign::Center,
                    max_width: Some(chip.w),
                    ..Default::default()
                },
                if selected {
                    theme.accent_text.scale_alpha(0.92 * p)
                } else if dimmed || saver.random {
                    theme.text_muted.scale_alpha(0.4 * p)
                } else {
                    theme.text.scale_alpha(0.85 * p)
                },
            );
            self.hits
                .push((chip, SettingsAction::ScreensaverEffect(index)));
        }

        self.toggle_row(
            scene,
            Rect::new(content.x, y + pitch * 2.0 + 74.0, content.w, row_h),
            "Casuale",
            "Scenario e composizione diversi a ogni ingresso",
            saver.random,
            SettingsAction::Toggle(Toggle::ScreensaverRandom),
            config,
            p,
        );
        self.toggle_row(
            scene,
            Rect::new(content.x, y + pitch * 3.0 + 74.0, content.w, row_h),
            "Scene da script",
            "Aggiunge le scene scritte in ~/.config/lard/scenes alle frecce",
            saver.script_scenes,
            SettingsAction::Toggle(Toggle::ScreensaverScripts),
            config,
            p,
        );

        // Where the scripts live, and what was found.
        let info_y = y + pitch * 4.0 + 74.0;
        let count = self.script_names.len();
        let info = if count == 0 {
            format!("Nessuna scena in {}", self.script_dir.display())
        } else {
            format!(
                "{}: {} ({})",
                self.script_dir.display(),
                self.script_names.join(", "),
                count
            )
        };
        scene.text(
            &truncate_to_width(&info, fonts.size_small - 1.0, content.w - 16.0),
            Point::new(content.x + 8.0, info_y + 2.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.55 * p),
        );

        let rows = [
            (
                Step::ScreensaverVeil,
                "Vetro",
                format!("{}%", (saver.veil * 100.0).round() as u32),
                -5,
                5,
            ),
            (
                Step::ScreensaverEntrance,
                "Ingresso",
                format!("{:.1} s", saver.entrance_secs()),
                -100,
                100,
            ),
            (
                Step::ScreensaverSpeed,
                "Velocità",
                format!("{:.1}×", saver.speed),
                -1,
                1,
            ),
            (
                Step::ScreensaverIntensity,
                "Intensità",
                format!("{}%", (saver.intensity * 100.0).round() as u32),
                -5,
                5,
            ),
            (
                Step::ScreensaverDust,
                "Polvere",
                format!("{}%", (saver.dust * 100.0).round() as u32),
                -5,
                5,
            ),
        ];
        for (index, (step, label, value, minus, plus)) in rows.iter().enumerate() {
            self.stepper_row_state(
                scene,
                Rect::new(
                    content.x,
                    info_y + 18.0 + index as f32 * pitch,
                    content.w,
                    row_h,
                ),
                label,
                value,
                SettingsAction::Step(*step, *minus),
                SettingsAction::Step(*step, *plus),
                config,
                p,
                dimmed,
            );
        }
        self.toggle_row(
            scene,
            Rect::new(
                content.x,
                info_y + 18.0 + rows.len() as f32 * pitch,
                content.w,
                row_h,
            ),
            "Orologio",
            "Ora e data nell'angolo del salvaschermo",
            saver.show_clock,
            SettingsAction::Toggle(Toggle::ScreensaverClock),
            config,
            p,
        );
    }

    /// Lock screen / greeter background.
    fn build_lock(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        let theme = &config.theme;
        let fonts = &config.font;
        let wallpaper = &config.greeter.wallpaper;
        let row_h = 40.0;
        let pitch = 44.0;

        let current = match wallpaper.background {
            lard_core::theme::LockBackground::Gradient => "Gradiente",
            lard_core::theme::LockBackground::Wallpaper => "Sfondo del desktop",
        };
        let rect = Rect::new(content.x, y, content.w, row_h + 8.0);
        scene.text(
            "Sfondo della schermata di blocco",
            Point::new(rect.x + 8.0, rect.y + 2.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            "Il wallpaper scelto, scurito, oppure il gradiente della palette",
            Point::new(rect.x + 8.0, rect.y + 21.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.58 * p),
        );
        let button = Rect::new(rect.right() - 186.0, rect.y + 5.0, 186.0, 30.0);
        let hovered = self.cursor.is_some_and(|c| button.contains(c));
        scene.rect(
            button,
            theme
                .surface_hover
                .scale_alpha((if hovered { 0.9 } else { 0.5 }) * p),
            9.0,
        );
        scene.text_center_y(
            current,
            button.x,
            button.center().y,
            TextStyle {
                size: fonts.size_small,
                align: TextAlign::Center,
                max_width: Some(button.w),
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        self.hits
            .push((button, SettingsAction::NextLockBackground));

        self.stepper_row_state(
            scene,
            Rect::new(content.x, y + pitch + 12.0, content.w, row_h),
            "Velo",
            &format!("{}%", (wallpaper.dim * 100.0).round() as u32),
            SettingsAction::Step(Step::LockDim, -5),
            SettingsAction::Step(Step::LockDim, 5),
            config,
            p,
            false,
        );
        scene.text(
            "Quanto il wallpaper viene scurito dietro la card di accesso.",
            Point::new(content.x + 8.0, y + pitch * 2.0 + 18.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.5 * p),
        );
    }

    fn build_info(&mut self, scene: &mut Scene, content: Rect, y: f32, config: &Config, p: f32) {
        let path = Config::effective_path();
        let lines = [
            ("Config", path.display().to_string()),
            ("Versione", format!("LARD {}", env!("CARGO_PKG_VERSION"))),
            ("Compositor", "niri · Wayland".into()),
            (
                "Barra",
                format!(
                    "{} px · {}% opacità",
                    config.bar.height.round() as u32,
                    (config.bar.opacity * 100.0).round() as u32
                ),
            ),
            (
                "Dock",
                if config.dock.enabled {
                    "attivo".into()
                } else {
                    "disattivo".into()
                },
            ),
            (
                "Blur",
                if config.theme.blur {
                    "compositor".into()
                } else {
                    "disattivo".into()
                },
            ),
        ];
        for (index, (label, value)) in lines.iter().enumerate() {
            let rect = Rect::new(content.x, y + index as f32 * 44.0, content.w, 36.0);
            if index % 2 == 0 {
                scene.rect(
                    rect,
                    config.theme.background.scale_alpha(0.32 * p),
                    config.theme.radius.control(),
                );
            }
            scene.text_center_y(
                *label,
                rect.x + 12.0,
                rect.center().y,
                TextStyle {
                    size: config.font.size_small,
                    ..Default::default()
                },
                config.theme.text_muted.scale_alpha(0.7 * p),
            );
            scene.text_center_y(
                value,
                rect.x + 120.0,
                rect.center().y,
                TextStyle {
                    size: config.font.size_small,
                    weight: config.font.weight_medium,
                    align: TextAlign::Right,
                    max_width: Some(rect.w - 132.0),
                    ..Default::default()
                },
                config.theme.text.scale_alpha(0.88 * p),
            );
        }
        scene.text(
            "La configurazione è TOML e viene salvata con backup .toml.bak.",
            Point::new(content.x, y + 312.0),
            TextStyle {
                size: config.font.size_small - 1.0,
                ..Default::default()
            },
            config.theme.text_muted.scale_alpha(0.5 * p),
        );
    }

    fn toggle_row(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        label: &str,
        hint: &str,
        value: bool,
        action: SettingsAction,
        config: &Config,
        p: f32,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let hovered = self.cursor.is_some_and(|c| rect.contains(c));
        if hovered {
            // The rows of the list are tiles.
            scene.rect(
                rect,
                theme.surface_hover.scale_alpha(0.32 * p),
                theme.radius.card(),
            );
        }
        scene.text(
            label,
            Point::new(rect.x + 8.0, rect.y + 3.0),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(p),
        );
        scene.text(
            hint,
            Point::new(rect.x + 8.0, rect.y + 22.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.58 * p),
        );
        let switch = Rect::new(rect.right() - 52.0, rect.y + 8.0, 44.0, 24.0);
        scene.rect(
            switch,
            if value {
                theme.accent.scale_alpha(0.85 * p)
            } else {
                theme.background.scale_alpha(0.9 * p)
            },
            12.0,
        );
        scene.rect(
            Rect::new(
                switch.x + if value { 22.0 } else { 2.0 },
                switch.y + 2.0,
                20.0,
                20.0,
            ),
            if value {
                theme.accent_text.scale_alpha(p)
            } else {
                theme.text_muted.scale_alpha(0.8 * p)
            },
            10.0,
        );
        self.hits.push((rect, action));
    }

    fn stepper_row(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        label: &str,
        value: &str,
        minus: SettingsAction,
        plus: SettingsAction,
        config: &Config,
        p: f32,
    ) {
        self.stepper_row_state(scene, rect, label, value, minus, plus, config, p, false)
    }

    /// A stepper that can be locked: the chain ties every radius to the
    /// master value, so the other ones become read only.
    #[allow(clippy::too_many_arguments)]
    fn stepper_row_state(
        &mut self,
        scene: &mut Scene,
        rect: Rect,
        label: &str,
        value: &str,
        minus: SettingsAction,
        plus: SettingsAction,
        config: &Config,
        p: f32,
        locked: bool,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        scene.text_center_y(
            label,
            rect.x + 8.0,
            rect.center().y,
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_medium,
                ..Default::default()
            },
            theme.text.scale_alpha(if locked { 0.4 * p } else { p }),
        );
        let minus_rect = Rect::new(rect.right() - 128.0, rect.y + 8.0, 30.0, 30.0);
        let plus_rect = Rect::new(rect.right() - 34.0, rect.y + 8.0, 30.0, 30.0);
        scene.rect(
            minus_rect,
            theme.surface_hover.scale_alpha(
                if self.cursor.is_some_and(|c| minus_rect.contains(c)) {
                    0.9
                } else {
                    0.45
                } * p,
            ),
            8.0,
        );
        scene.rect(
            plus_rect,
            theme.surface_hover.scale_alpha(
                if self.cursor.is_some_and(|c| plus_rect.contains(c)) {
                    0.9
                } else {
                    0.45
                } * p,
            ),
            8.0,
        );
        scene.text_center_y(
            "−",
            minus_rect.x,
            minus_rect.center().y,
            TextStyle {
                size: 16.0,
                align: TextAlign::Center,
                max_width: Some(minus_rect.w),
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        scene.text_center_y(
            "+",
            plus_rect.x,
            plus_rect.center().y,
            TextStyle {
                size: 16.0,
                align: TextAlign::Center,
                max_width: Some(plus_rect.w),
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        scene.text_center_y(
            value,
            rect.right() - 94.0,
            rect.center().y,
            TextStyle {
                size: fonts.size_small,
                weight: fonts.weight_medium,
                align: TextAlign::Center,
                max_width: Some(58.0),
                ..Default::default()
            },
            theme.accent.scale_alpha(0.95 * p),
        );
        if !locked {
            self.hits.push((minus_rect, minus));
            self.hits.push((plus_rect, plus));
        }
    }
}

/// Apply a named palette to the live theme.
pub fn apply_palette(theme: &mut Theme, index: usize) {
    let palette = PALETTES[index.min(PALETTES.len() - 1)];
    theme.background = palette.background;
    theme.surface = palette.surface;
    theme.surface_hover = palette.surface.mix(palette.text, 0.09).with_alpha(1.0);
    theme.surface_active = palette.surface.mix(palette.accent, 0.18).with_alpha(1.0);
    theme.text = palette.text;
    theme.text_muted = palette.text.mix(palette.background, 0.38).with_alpha(0.82);
    theme.accent = palette.accent;
    theme.accent_text = palette.background.mix(Color::BLACK, 0.35).with_alpha(1.0);
}

/// Recolour the lockscreen/greeter gradient to match the selected palette.
pub fn apply_palette_to_wallpaper(wallpaper: &mut GreeterWallpaper, index: usize) {
    let palette = PALETTES[index.min(PALETTES.len() - 1)];
    wallpaper.top = palette.background.mix(Color::BLACK, 0.28).with_alpha(1.0);
    wallpaper.bottom = palette.surface.mix(palette.accent, 0.08).with_alpha(1.0);
    wallpaper.accent_glow = palette.accent.scale_alpha(0.55);
}

/// Small helper used by the settings UI for the bar position label.
#[must_use]
pub fn bar_position_label(position: BarPosition) -> &'static str {
    match position {
        BarPosition::Top => "Alto",
        BarPosition::Bottom => "Basso",
    }
}
