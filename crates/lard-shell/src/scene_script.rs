//! User-authored screensaver scenes, written in Rhai.
//!
//! A scene is a small script in `~/.config/lard/scenes/*.rhai` that returns a
//! list of drawing commands for the current frame. The engine is sandboxed: no
//! file system, no network, no modules, a hard operation budget per frame. The
//! API is deliberately tiny (discs, rings, rectangles) because it draws on top
//! of the procedural room the renderer already provides.
//!
//! ```rhai
//! fn name() { "Orbite" }
//! fn draw(t, w, h) {
//!     let out = [];
//!     for i in 0..12 {
//!         let angle = t * 0.4 + i * 0.523;
//!         out.push(#{ kind: "disc", x: w / 2.0 + cos(angle) * 220.0,
//!                     y: h / 2.0 + sin(angle) * 120.0, r: 26.0,
//!                     color: "accent", alpha: 0.35 });
//!     }
//!     out
//! }
//! ```

use lard_core::theme::Theme;
use lard_core::{Color, Rect, Size};
use lard_render::Scene;
use rhai::{Array, Dynamic, Engine, Scope, AST};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Upper bound of drawing commands accepted from one frame of a script.
const MAX_COMMANDS: usize = 4000;
/// How often the folder is rescanned while the screensaver is open.
const RESCAN_EVERY: Duration = Duration::from_secs(3);

pub struct ScriptScene {
    pub name: String,
    pub path: PathBuf,
    ast: AST,
}

pub struct SceneScripts {
    dir: PathBuf,
    engine: Engine,
    scenes: Vec<ScriptScene>,
    last_scan: Option<Instant>,
    last_error: Option<String>,
    /// Cached drawing commands, kept between frames to avoid reallocating.
    commands: Vec<Command>,
}

#[derive(Clone, Debug)]
struct Command {
    rect: Rect,
    radius: f32,
    color: Color,
    border: f32,
}

impl SceneScripts {
    #[must_use]
    pub fn new() -> Self {
        let dir = lard_core::Config::effective_path()
            .parent()
            .map_or_else(|| PathBuf::from("scenes"), |parent| parent.join("scenes"));
        let mut engine = Engine::new();
        // A frame of a scene is a few hundred operations; anything past this is
        // a bug in the script, not a scene.
        engine.set_max_operations(200_000);
        engine.set_max_call_levels(8);
        engine.set_max_array_size(MAX_COMMANDS);
        engine.set_max_string_size(4096);
        engine.set_max_map_size(64);
        // Rhai's own expression-depth limits are fine: a `#{ ... }` literal
        // inside a loop is already deeper than a small hand-tuned budget.

        Self {
            dir,
            engine,
            scenes: Vec::new(),
            last_scan: None,
            last_error: None,
            commands: Vec::with_capacity(256),
        }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.scenes.iter().map(|scene| scene.name.clone()).collect()
    }

    /// Create the folder and a couple of examples the first time it is used.
    pub fn install_examples(&mut self) {
        if self.dir.join("orbite.rhai").exists() || !self.dir.exists() {
            // Only touch a folder that either exists (leave the user alone) or
            // is missing entirely (first run: seed the examples).
            if self.dir.exists() {
                return;
            }
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let examples = [
            ("orbite.rhai", include_str!("../../../config/scenes/orbite.rhai")),
            (
                "nebulosa.rhai",
                include_str!("../../../config/scenes/nebulosa.rhai"),
            ),
        ];
        for (name, body) in examples {
            let path = self.dir.join(name);
            if !path.exists() {
                let _ = std::fs::write(path, body);
            }
        }
    }

    /// Rescan the folder if enough time has passed (a script can be edited
    /// while the screensaver is open and will be picked up).
    pub fn refresh(&mut self) {
        let now = Instant::now();
        if self
            .last_scan
            .is_some_and(|last| now.duration_since(last) < RESCAN_EVERY)
        {
            return;
        }
        self.last_scan = Some(now);
        self.reload();
    }

    pub fn reload(&mut self) {
        self.scenes.clear();
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = read
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "rhai"))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            match self.engine.compile(&source) {
                Ok(ast) => {
                    let name = self
                        .engine
                        .call_fn::<String>(&mut Scope::new(), &ast, "name", ())
                        .ok()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| {
                            path.file_stem()
                                .and_then(|stem| stem.to_str())
                                .unwrap_or("scena")
                                .to_string()
                        });
                    self.scenes.push(ScriptScene { name, path, ast });
                }
                Err(error) => {
                    let message = format!("{}: {error}", path.display());
                    if self.last_error.as_deref() != Some(&message) {
                        // Also on stderr: the previews are headless tools without
                        // a tracing subscriber.
                        eprintln!("lard: scena non compilata: {message}");
                        tracing::warn!(%message, "cannot compile screensaver scene");
                        self.last_error = Some(message);
                    }
                }
            }
        }
    }

    /// Draw the scene at `index` for the given frame. `alpha` scales the whole
    /// scene (used by the burn-in and by the crossfade).
    pub fn draw(
        &mut self,
        index: usize,
        scene: &mut Scene,
        size: Size,
        time: f32,
        theme: &Theme,
        font_size: f32,
        alpha: f32,
    ) {
        if alpha <= 0.002 {
            return;
        }
        let Some(script) = self.scenes.get(index) else {
            return;
        };
        self.commands.clear();
        let mut scope = Scope::new();
        let result: Result<Dynamic, _> = self.engine.call_fn(
            &mut scope,
            &script.ast,
            "draw",
            (f64::from(time), f64::from(size.w), f64::from(size.h)),
        );
        match result {
            Ok(value) => {
                let array = value.cast::<Array>();
                if array.is_empty() {
                    // Not an array: report once and show the script name so the
                    // user knows the scene loaded but returned nothing useful.
                    let message = format!("{}: draw() non ha restituito un array", script.path.display());
                    if self.last_error.as_deref() != Some(&message) {
                        tracing::warn!(%message, "screensaver scene");
                        self.last_error = Some(message);
                    }
                    return;
                }
                for (index, item) in array.into_iter().take(MAX_COMMANDS).enumerate() {
                    if let Some(command) = command_from(item, theme) {
                        self.commands.push(command);
                    } else if index == 0 {
                        let message = format!("{}: comando non valido", script.path.display());
                        if self.last_error.as_deref() != Some(&message) {
                            tracing::warn!(%message, "screensaver scene");
                            self.last_error = Some(message);
                        }
                    }
                }
            }
            Err(error) => {
                let message = format!("{}: {error}", script.path.display());
                if self.last_error.as_deref() != Some(&message) {
                    eprintln!("lard: scena: {message}");
                    tracing::warn!(%message, "screensaver scene");
                    self.last_error = Some(message);
                }
                return;
            }
        }

        for command in &self.commands {
            let a = command.color.a * alpha;
            if a <= 0.002 {
                continue;
            }
            let color = command.color.with_alpha(a);
            if command.border > 0.01 {
                scene.bordered(command.rect, Color::TRANSPARENT, command.radius, command.border, color);
            } else if command.radius >= command.rect.w.min(command.rect.h) * 0.5 - 0.01
                && command.rect.w - command.rect.h < 0.02
            {
                // Perfectly round: a radial glow reads as glass instead of a disc.
                scene.glow(command.rect, color, 0.0);
            } else {
                scene.rect(command.rect, color, command.radius);
            }
        }

        let _ = font_size;
    }

    /// Name of the scene at `index`, for the settings list.
    #[must_use]
    pub fn name(&self, index: usize) -> Option<&str> {
        self.scenes.get(index).map(|scene| scene.name.as_str())
    }
}

impl Default for SceneScripts {
    fn default() -> Self {
        Self::new()
    }
}

fn command_from(value: Dynamic, theme: &Theme) -> Option<Command> {
    let map = value.cast::<rhai::Map>();
    if map.is_empty() {
        return None;
    }
    let number = |key: &str, default: f64| -> f32 {
        map.get(key)
            .and_then(|value| value.as_float().ok())
            .unwrap_or(default) as f32
    };
    let text = |key: &str| -> Option<String> {
        map.get(key)
            .and_then(|value| value.clone().into_string().ok())
    };

    let kind = text("kind").unwrap_or_else(|| "disc".into());
    let color = color_from(text("color").as_deref(), theme);
    let alpha = number("alpha", 1.0).clamp(0.0, 1.0);
    let color = color.with_alpha(color.a * alpha);
    let radius = number("r", 0.0).max(0.0);

    match kind.as_str() {
        "disc" | "glow" => {
            let x = number("x", 0.0);
            let y = number("y", 0.0);
            if radius <= 0.5 {
                return None;
            }
            Some(Command {
                rect: Rect::new(x - radius, y - radius, radius * 2.0, radius * 2.0),
                radius,
                color,
                border: 0.0,
            })
        }
        "ring" => {
            let x = number("x", 0.0);
            let y = number("y", 0.0);
            if radius <= 0.5 {
                return None;
            }
            Some(Command {
                rect: Rect::new(x - radius, y - radius, radius * 2.0, radius * 2.0),
                radius,
                color,
                border: number("width", 1.5).max(0.5),
            })
        }
        "rect" => {
            let w = number("w", 0.0).max(0.0);
            let h = number("h", 0.0).max(0.0);
            if w <= 0.5 || h <= 0.5 {
                return None;
            }
            Some(Command {
                rect: Rect::new(number("x", 0.0), number("y", 0.0), w, h),
                radius: number("radius", 0.0).max(0.0),
                color,
                border: 0.0,
            })
        }
        _ => None,
    }
}

fn color_from(name: Option<&str>, theme: &Theme) -> Color {
    match name.unwrap_or("accent").trim() {
        "accent" => theme.accent,
        "text" => theme.text,
        "muted" | "text_muted" => theme.text_muted,
        "danger" => theme.danger,
        "warning" => theme.warning,
        "success" => theme.success,
        "white" => Color::WHITE,
        "black" => Color::BLACK,
        other => Color::from_hex(other).unwrap_or(theme.accent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_returns_draw_commands() {
        let mut scripts = SceneScripts::new();
        scripts.dir = std::env::temp_dir().join(format!("lard-scene-{}", std::process::id()));
        std::fs::create_dir_all(&scripts.dir).unwrap();
        std::fs::write(
            scripts.dir.join("test.rhai"),
            r##"
            fn name() { "Prova" }
            fn draw(t, w, h) {
                [#{ kind: "ring", x: w / 2.0, y: h / 2.0, r: 40.0, width: 2.0, color: "#ffcc00" }]
            }
            "##,
        )
        .unwrap();
        scripts.reload();
        assert_eq!(scripts.names(), vec!["Prova".to_string()]);

        let mut scene = Scene::new();
        scripts.draw(
            0,
            &mut scene,
            Size::new(1280.0, 800.0),
            1.0,
            &Theme::default(),
            11.0,
            1.0,
        );
        assert!(!scene.is_empty(), "the scene drew nothing");
        let _ = std::fs::remove_dir_all(&scripts.dir);
    }

    #[test]
    fn a_broken_script_is_skipped() {
        let mut scripts = SceneScripts::new();
        scripts.dir = std::env::temp_dir().join(format!("lard-broken-{}", std::process::id()));
        std::fs::create_dir_all(&scripts.dir).unwrap();
        std::fs::write(scripts.dir.join("broken.rhai"), "fn draw( {").unwrap();
        scripts.reload();
        assert!(scripts.is_empty());
        let _ = std::fs::remove_dir_all(&scripts.dir);
    }

    #[test]
    fn endless_scripts_are_stopped() {
        let mut scripts = SceneScripts::new();
        scripts.dir = std::env::temp_dir().join(format!("lard-loop-{}", std::process::id()));
        std::fs::create_dir_all(&scripts.dir).unwrap();
        std::fs::write(
            scripts.dir.join("loop.rhai"),
            "fn draw(t, w, h) { loop { } }",
        )
        .unwrap();
        scripts.reload();
        let mut scene = Scene::new();
        // Must return (the operation budget fires) instead of hanging.
        scripts.draw(
            0,
            &mut scene,
            Size::new(100.0, 100.0),
            0.0,
            &Theme::default(),
            11.0,
            1.0,
        );
        let _ = std::fs::remove_dir_all(&scripts.dir);
    }
}
