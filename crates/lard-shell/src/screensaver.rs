//! Built-in screensaver: the desktop itself, frosted, with the shell's liquid
//! glass objects floating over it.
//!
//! It is deliberately *not* a lock screen: the compositor reports activity
//! through `ext-idle-notify-v1` and any input dismisses it. It is not a video
//! either: the wallpaper is the room, the liquid bodies are drawn by the
//! renderer and the droplets are a small physical simulation updated here. The
//! arrow keys move between scenarios, so the screen keeps being "the same
//! environment" instead of a slideshow.
//!
//! Entering is a burn-in: the bright structures of the objects appear first,
//! additively over the live session; the bodies follow; the frosted backdrop
//! closes the scene last. Leaving runs the same process backwards.

use lard_core::theme::{ScreensaverConfig, ScreensaverEffect};
use lard_core::{Color, Config, Point, Rect, Size};
use lard_render::{Fluid, FluidMode, Scene, TextAlign, TextStyle};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Result of the wallpaper decode thread: the file and its pixels (empty when
/// the image could not be read).
type WallpaperSlot = Arc<Mutex<Option<(PathBuf, Vec<u8>)>>>;

/// Image-atlas slot of the frosted wallpaper. Fixed so re-uploading the same
/// bitmap is a no-op in the renderer.
/// Cell 64 of the image atlas: app icons start at 128, wallpaper thumbs at 0.
const WALLPAPER_ID: u32 = 0x00C0_0040;
const WALLPAPER_W: u32 = 128;
const WALLPAPER_H: u32 = 72;
const DROPLETS: usize = 9;
/// Seconds the exit hint stays at full opacity.
const HINT_FULL_SECS: f32 = 6.0;
/// Seconds a scenario crossfade takes.
const BLEND_SECS: f32 = 1.4;

pub struct Screensaver {
    open: bool,
    /// 0..1 burn-in progress. Entry and exit run the same ramp in opposite
    /// directions, so leaving is the exact reverse of entering.
    reveal: f32,
    /// Seconds the scene has been running, for the animation clock.
    elapsed: f32,
    /// Scenario currently on screen.
    mode: ScreensaverEffect,
    seed: f32,
    /// Scenario being faded in.
    next_mode: ScreensaverEffect,
    next_seed: f32,
    /// 0..1 crossfade between the two scenarios (0 = single shot).
    blend: f32,
    /// Small xorshift state: no dependency, deterministic per activation.
    rng: u32,
    /// Decoded wallpaper (path + pixels) and the file being looked for.
    wallpaper: Option<(PathBuf, Vec<u8>)>,
    wallpaper_requested: Option<PathBuf>,
    wallpaper_slot: WallpaperSlot,
    /// Glass marbles drifting over the scene.
    droplets: [Droplet; DROPLETS],
}

/// A glass marble floating over the frosted desktop. Positions are normalized
/// so the scene scales with any output.
#[derive(Clone, Copy, Debug)]
struct Droplet {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// Radius as a fraction of the output height.
    radius: f32,
    /// Phase of the slow dither, so they never move in lockstep.
    phase: f32,
}

impl Default for Screensaver {
    fn default() -> Self {
        Self::new()
    }
}

impl Screensaver {
    #[must_use]
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9, |d| d.subsec_nanos() ^ d.as_secs() as u32);
        let mut saver = Self {
            open: false,
            reveal: 0.0,
            elapsed: 0.0,
            mode: ScreensaverEffect::Liquid,
            seed: 0.0,
            next_mode: ScreensaverEffect::Liquid,
            next_seed: 0.0,
            blend: 0.0,
            rng: nanos | 1,
            wallpaper: None,
            wallpaper_requested: None,
            wallpaper_slot: Arc::new(Mutex::new(None)),
            droplets: [Droplet {
                x: 0.5,
                y: 0.5,
                vx: 0.0,
                vy: 0.0,
                radius: 0.02,
                phase: 0.0,
            }; DROPLETS],
        };
        saver.spawn_droplets();
        saver
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the overlay surface must exist. It stays alive for the exit
    /// burn-in, so the surface disappears only once the scene has dissolved.
    #[must_use]
    pub fn wants_surface(&self) -> bool {
        self.open || self.reveal > 0.001
    }

    fn next_random(&mut self) -> f32 {
        // xorshift32: cheap and plenty for picking a composition.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x & 0xFFFF) as f32 / 65535.0
    }

    fn spawn_droplets(&mut self) {
        for index in 0..DROPLETS {
            let x = 0.12 + self.next_random() * 0.76;
            let y = 0.14 + self.next_random() * 0.72;
            let angle = self.next_random() * std::f32::consts::TAU;
            let speed = 0.008 + self.next_random() * 0.022;
            self.droplets[index] = Droplet {
                x,
                y,
                vx: angle.cos() * speed,
                vy: angle.sin() * speed,
                radius: 0.012 + self.next_random() * 0.022,
                phase: self.next_random() * std::f32::consts::TAU,
            };
        }
    }

    /// Returns true when the state changed and the caller must act.
    pub fn open(&mut self, settings: &ScreensaverConfig) -> bool {
        if self.open {
            return false;
        }
        self.open = true;
        self.elapsed = 0.0;
        self.reveal = 0.0;
        self.blend = 0.0;
        self.seed = self.next_random();
        self.next_seed = self.seed;
        self.mode = if settings.random {
            ScreensaverEffect::ALL[(self.next_random() * 3.0) as usize % 3]
        } else {
            settings.effect
        };
        self.next_mode = self.mode;
        self.spawn_droplets();
        true
    }

    pub fn close(&mut self) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        true
    }

    /// Move to the previous/next scenario, with a crossfade. `delta` is -1 or
    /// +1; the arrow keys drive it.
    pub fn cycle(&mut self, delta: i32) {
        if !self.open || delta == 0 {
            return;
        }
        let count = ScreensaverEffect::ALL.len() as i32;
        let index = ScreensaverEffect::ALL
            .iter()
            .position(|effect| *effect == self.next_mode)
            .unwrap_or(0) as i32;
        let next = (index + delta).rem_euclid(count) as usize;
        self.next_mode = ScreensaverEffect::ALL[next];
        self.next_seed = self.next_random();
        self.blend = 0.001;
    }

    /// Advance the burn-in, the crossfade and the droplets. Returns true while
    /// a frame is needed, which keeps the scene animating at the frame rate.
    pub fn update(&mut self, dt: f32, settings: &ScreensaverConfig) -> bool {
        if dt <= 0.0 {
            return self.wants_surface();
        }
        let dt = dt.min(0.1);
        let entrance = settings.entrance_secs().clamp(0.15, 2.5);
        let step = (dt / entrance).clamp(0.0, 1.0);
        if self.open {
            self.reveal = (self.reveal + step).min(1.0);
            self.elapsed += dt;
            self.move_droplets(dt);
        } else {
            self.reveal = (self.reveal - step).max(0.0);
        }

        if self.blend > 0.0 {
            self.blend += dt / BLEND_SECS;
            if self.blend >= 1.0 {
                // The incoming scenario becomes the current one: the pixels do
                // not change, only which side of the crossfade they come from.
                self.blend = 0.0;
                self.mode = self.next_mode;
                self.seed = self.next_seed;
            }
        }
        self.wants_surface()
    }

    /// Tiny physical simulation: slow orbital drift, soft collisions between
    /// droplets and elastic bounces off the edges of the safe area.
    fn move_droplets(&mut self, dt: f32) {
        let margin = 0.05;
        for index in 0..DROPLETS {
            let (x1, y1, r1) = {
                let d = &self.droplets[index];
                (d.x, d.y, d.radius)
            };
            for other in (index + 1)..DROPLETS {
                let (x2, y2, r2) = {
                    let d = &self.droplets[other];
                    (d.x, d.y, d.radius)
                };
                let dx = x2 - x1;
                let dy = (y2 - y1) * 0.75;
                let distance = (dx * dx + dy * dy).sqrt().max(1e-4);
                let min = r1 + r2 + 0.03;
                if distance < min {
                    let push = (min - distance) / min * 0.9 * dt;
                    let nx = dx / distance * push;
                    let ny = dy / distance * push;
                    self.droplets[index].vx -= nx;
                    self.droplets[index].vy -= ny;
                    self.droplets[other].vx += nx;
                    self.droplets[other].vy += ny;
                }
            }
        }

        for droplet in &mut self.droplets {
            let phase = droplet.phase + self.elapsed * 0.21;
            // Gentle orbit around the centre, plus a slow dither.
            let ax = -(droplet.y - 0.5) * 0.02 + phase.sin() * 0.004;
            let ay = (droplet.x - 0.5) * 0.02 + phase.cos() * 0.004;
            droplet.vx += ax * dt;
            droplet.vy += ay * dt;
            // Viscosity: glass marbles in a thick liquid, not billiard balls.
            let damping = (1.0 - 0.35 * dt).max(0.0);
            droplet.vx *= damping;
            droplet.vy *= damping;
            droplet.x += droplet.vx * dt;
            droplet.y += droplet.vy * dt;

            let r = droplet.radius;
            if droplet.x < margin + r {
                droplet.x = margin + r;
                droplet.vx = droplet.vx.abs() * 0.85;
            } else if droplet.x > 1.0 - margin - r {
                droplet.x = 1.0 - margin - r;
                droplet.vx = -droplet.vx.abs() * 0.85;
            }
            if droplet.y < margin + r {
                droplet.y = margin + r;
                droplet.vy = droplet.vy.abs() * 0.85;
            } else if droplet.y > 1.0 - margin - r {
                droplet.y = 1.0 - margin - r;
                droplet.vy = -droplet.vy.abs() * 0.85;
            }
        }
    }

    /// Decode the wallpaper on a worker thread the first time it is needed:
    /// the shell thread must never block on a 4K JPEG, and the frosted backdrop
    /// only appears at the end of the burn-in anyway.
    /// Start decoding the configured wallpaper in the background. Called once
    /// at shell start-up so the first activation already has its room.
    pub fn request_wallpaper(&mut self, config: &Config) {
        let want = PathBuf::from(&config.theme.wallpaper);
        if want.as_os_str().is_empty()
            || self.wallpaper.as_ref().map(|(path, _)| path) == Some(&want)
            || self.wallpaper_requested.as_ref() == Some(&want)
        {
            return;
        }
        self.wallpaper_requested = Some(want.clone());
        if !want.exists() {
            return;
        }
        let slot = Arc::clone(&self.wallpaper_slot);
        std::thread::Builder::new()
            .name("lard-saver-wallpaper".into())
            .spawn(move || {
                let rgba = image::open(&want)
                    .ok()
                    .map(|img| {
                        img.resize_exact(
                            WALLPAPER_W,
                            WALLPAPER_H,
                            image::imageops::FilterType::Triangle,
                        )
                        .to_rgba8()
                        .into_raw()
                    })
                    .unwrap_or_default();
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some((want, rgba));
                }
            })
            .ok();
    }

    fn ensure_wallpaper(&mut self, scene: &mut Scene, config: &Config) {
        if let Ok(mut slot) = self.wallpaper_slot.lock()
            && let Some((path, rgba)) = slot.take()
        {
            if rgba.is_empty() {
                self.wallpaper = None;
            } else {
                self.wallpaper = Some((path.clone(), rgba));
            }
            self.wallpaper_requested = Some(path);
        }
        // The scene is rebuilt every frame and the renderer deduplicates by id,
        // so handing it the bitmap every time keeps the atlas correct without
        // the shell having to know when a frame was actually presented.
        if let Some((path, rgba)) = &self.wallpaper {
            let _ = path;
            scene.upload_image(WALLPAPER_ID, WALLPAPER_W, WALLPAPER_H, rgba.clone());
        }
        self.request_wallpaper(config);
    }

    pub fn build(
        &mut self,
        scene: &mut Scene,
        size: Size,
        config: &Config,
        clock: &str,
        date: &str,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let settings = &config.screensaver;
        let reveal = self.reveal.clamp(0.0, 1.0);
        if reveal <= 0.001 {
            return;
        }
        self.ensure_wallpaper(scene, config);

        scene.fluid(Fluid {
            mode: fluid_mode(self.mode),
            mode_b: fluid_mode(self.next_mode),
            time: self.elapsed,
            speed: settings.speed,
            intensity: settings.intensity,
            seed_a: self.seed,
            seed_b: self.next_seed,
            mix: self.blend,
            reveal,
            deep: theme.background,
            accent: theme.accent,
            // Pure white turns gold into grey fog: keep the highlight warm.
            light: theme.accent.mix(Color::WHITE, 0.55),
            wallpaper: self.wallpaper.as_ref().map(|_| WALLPAPER_ID),
            veil: settings.veil,
            fade: 1.0,
        });

        // Droplets belong to the subjects: they burn in with the bodies.
        let subject = smoothstep(0.22, 0.62, reveal);
        if subject > 0.01 {
            self.draw_droplets(scene, size, theme, subject);
        }

        if !settings.show_clock {
            return;
        }
        // The interface arrives once the backdrop is closing in, rising a few
        // pixels into place; on the way out it leaves the same way.
        let ui = smoothstep(0.62, 0.92, reveal);
        if ui <= 0.001 {
            return;
        }
        let slide = (1.0 - ui) * 18.0;
        let pad = (size.w * 0.03).clamp(28.0, 72.0);
        let clock_size = (size.h * 0.115).clamp(44.0, 104.0);
        let clock_y = size.h - pad - clock_size * 1.30 + slide;

        scene.text(
            "LARD",
            Point::new(pad, pad * 0.72),
            TextStyle {
                size: (fonts.size_small + 1.0).max(11.0),
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(0.45 * ui),
        );
        scene.rect(
            Rect::new(pad, pad * 0.72 + 20.0, 26.0, 1.5),
            theme.accent.scale_alpha(0.55 * ui),
            0.75,
        );
        scene.text(
            clock,
            Point::new(pad - 2.0, clock_y),
            TextStyle {
                size: clock_size,
                weight: fonts.weight_semibold,
                ..Default::default()
            },
            theme.text.scale_alpha(ui),
        );
        scene.text(
            date,
            Point::new(pad, size.h - pad - fonts.size_body + slide * 0.6),
            TextStyle {
                size: fonts.size_body,
                weight: fonts.weight_regular,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.75 * ui),
        );

        // The hint fades back once the user has had time to read it.
        let hint = if self.elapsed < HINT_FULL_SECS {
            1.0
        } else {
            (1.0 - (self.elapsed - HINT_FULL_SECS) / 1.5).clamp(0.28, 1.0)
        };
        let hint_in = smoothstep(0.9, 1.0, reveal);
        scene.text(
            "← → scenario · premi un tasto per uscire",
            Point::new(pad, size.h - pad - fonts.size_small),
            TextStyle {
                size: fonts.size_small,
                align: TextAlign::Right,
                max_width: Some(size.w - pad * 2.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.42 * hint_in * hint),
        );
    }

    fn draw_droplets(&self, scene: &mut Scene, size: Size, theme: &lard_core::theme::Theme, p: f32) {
        for droplet in &self.droplets {
            let r = (droplet.radius * size.h).max(3.0);
            let cx = droplet.x * size.w;
            let cy = droplet.y * size.h;
            let body = Rect::new(cx - r, cy - r, r * 2.0, r * 2.0);
            // Contact shadow: the marble is floating *over* the frosted room.
            scene.glow(
                Rect::new(body.x + r * 0.22, body.y + r * 0.55, r * 2.2, r * 2.0),
                Color::BLACK.with_alpha(0.30 * p),
                0.0,
            );
            // Glass: a faint accent tint, a white rim and a specular highlight.
            scene.rect(body, theme.accent.with_alpha(0.10 * p), r);
            scene.rect(body, theme.text.with_alpha(0.05 * p), r);
            scene.bordered(
                body,
                Color::TRANSPARENT,
                r,
                1.2,
                theme.text.with_alpha(0.28 * p),
            );
            let spec = r * 0.62;
            scene.glow(
                Rect::new(
                    cx - spec * 1.05,
                    cy - spec * 1.25,
                    spec * 1.15,
                    spec * 1.05,
                ),
                Color::WHITE.with_alpha(0.62 * p),
                0.0,
            );
            // Caustic light focused through the bottom of the marble.
            scene.glow(
                Rect::new(cx - spec * 0.45, cy + spec * 0.45, spec, spec * 0.7),
                theme.accent.with_alpha(0.28 * p),
                0.0,
            );
        }
    }
}

fn fluid_mode(effect: ScreensaverEffect) -> FluidMode {
    match effect {
        ScreensaverEffect::Liquid => FluidMode::Liquid,
        ScreensaverEffect::Mercury => FluidMode::Mercury,
        ScreensaverEffect::Aurora => FluidMode::Aurora,
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-5)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ScreensaverConfig {
        ScreensaverConfig {
            random: false,
            ..ScreensaverConfig::default()
        }
    }

    #[test]
    fn entry_and_exit_are_symmetric() {
        let settings = settings();
        let mut saver = Screensaver::new();
        assert!(saver.open(&settings));
        let mut frames_in = 0;
        while saver.reveal < 1.0 && frames_in < 400 {
            saver.update(1.0 / 60.0, &settings);
            frames_in += 1;
        }
        assert_eq!(saver.reveal, 1.0);
        assert!(saver.wants_surface());

        assert!(saver.close());
        let mut frames_out = 0;
        while saver.reveal > 0.0 && frames_out < 400 {
            saver.update(1.0 / 60.0, &settings);
            frames_out += 1;
        }
        assert_eq!(saver.reveal, 0.0);
        assert!(!saver.wants_surface());
        assert!((frames_in as i32 - frames_out as i32).abs() <= 1);
    }

    #[test]
    fn a_settled_scene_still_requests_frames_while_open() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings);
        for _ in 0..160 {
            saver.update(1.0 / 60.0, &settings);
        }
        assert_eq!(saver.reveal, 1.0);
        assert!(saver.update(1.0 / 60.0, &settings));
    }

    #[test]
    fn random_mode_changes_the_composition_on_every_open() {
        let settings = ScreensaverConfig {
            random: true,
            ..settings()
        };
        let mut saver = Screensaver::new();
        let mut seeds = std::collections::HashSet::new();
        for _ in 0..8 {
            saver.open(&settings);
            seeds.insert(saver.seed.to_bits());
            saver.close();
            for _ in 0..50 {
                saver.update(1.0 / 60.0, &settings);
            }
        }
        assert!(seeds.len() >= 6, "seeds barely changed: {seeds:?}");
    }

    #[test]
    fn arrows_move_between_scenarios_in_both_directions() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings);
        let start = saver.next_mode;
        saver.cycle(1);
        let forward = saver.next_mode;
        assert_ne!(forward, start);
        saver.cycle(-1);
        assert_eq!(saver.next_mode, start);
        saver.cycle(-1);
        let back = saver.next_mode;
        assert_ne!(back, start);
        // One more step forward comes back around to the start.
        saver.cycle(1);
        assert_eq!(saver.next_mode, start);
    }

    #[test]
    fn a_scenario_change_is_a_crossfade_not_a_cut() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings);
        for _ in 0..160 {
            saver.update(1.0 / 60.0, &settings);
        }
        let before = saver.mode;
        saver.cycle(1);
        assert!(saver.blend > 0.0 && saver.blend < 0.5);
        // The outgoing scenario is still the current one during the fade.
        assert_eq!(saver.mode, before);
        for _ in 0..120 {
            saver.update(1.0 / 60.0, &settings);
        }
        assert_eq!(saver.blend, 0.0);
        assert_eq!(saver.mode, saver.next_mode);
        assert_ne!(saver.mode, before);
    }

    #[test]
    fn droplets_stay_inside_the_safe_area() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings);
        // Shove one into a corner: the simulation must pull it back.
        saver.droplets[0].x = 0.0;
        saver.droplets[0].y = 1.0;
        saver.droplets[0].vx = -1.0;
        saver.droplets[0].vy = 1.0;
        for _ in 0..600 {
            saver.update(1.0 / 60.0, &settings);
        }
        for droplet in &saver.droplets {
            assert!(droplet.x > 0.0 && droplet.x < 1.0, "{droplet:?}");
            assert!(droplet.y > 0.0 && droplet.y < 1.0, "{droplet:?}");
        }
    }

    #[test]
    fn fixed_mode_keeps_the_configured_scenario() {
        let settings = ScreensaverConfig {
            random: false,
            effect: ScreensaverEffect::Mercury,
            ..settings()
        };
        let mut saver = Screensaver::new();
        for _ in 0..5 {
            saver.open(&settings);
            assert_eq!(saver.mode, ScreensaverEffect::Mercury);
            saver.close();
            saver.update(1.0 / 60.0, &settings);
        }
    }
}
