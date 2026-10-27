//! Built-in screensaver: a procedural liquid environment that appears when the
//! session has been idle for a while.
//!
//! It is deliberately *not* a lock screen: the compositor reports activity
//! through `ext-idle-notify-v1` and any input dismisses it. It is not a video
//! either: the room (fog and caustics), the liquid bodies and the light are all
//! evaluated per pixel by `shaders/fluid.wgsl`, so the scene keeps evolving
//! without ever repeating a frame.
//!
//! Entering is a burn-in: the bright structures appear first, additively over
//! the live session; the bodies follow; the room closes in last. Leaving runs
//! the same process backwards. The arrow keys switch scenario, so the screen
//! stays "the usual environment" instead of turning into a slideshow.

use crate::scene_script::SceneScripts;
use lard_core::theme::ScreensaverEffect;
use lard_core::{Color, Config, Point, Rect, Size};
use lard_render::{Fluid, FluidMode, Scene, TextAlign, TextStyle};
use std::time::{Duration, Instant};

/// Seconds the exit hint stays at full opacity.
const HINT_FULL_SECS: f32 = 6.0;
/// Seconds a scenario crossfade takes. Long on purpose: a fast crossfade
/// reads as scenes flicking past each other.
const BLEND_SECS: f32 = 2.6;

/// What the screensaver is showing: one of the built-in looks or a user scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneKind {
    Builtin(ScreensaverEffect),
    Script(usize),
}

impl SceneKind {
    fn fluid(self) -> FluidMode {
        match self {
            Self::Builtin(effect) => fluid_mode(effect),
            // Scripted scenes draw their own shapes over the bare room.
            Self::Script(_) => FluidMode::Empty,
        }
    }

    /// Position in the flat list the arrow keys walk through.
    fn position(self, scripts: usize) -> usize {
        match self {
            Self::Builtin(effect) => ScreensaverEffect::ALL
                .iter()
                .position(|candidate| *candidate == effect)
                .unwrap_or(0),
            Self::Script(index) => ScreensaverEffect::ALL.len() + index.min(scripts),
        }
    }

    fn from_position(position: usize, _scripts: usize) -> Self {
        if position < ScreensaverEffect::ALL.len() {
            Self::Builtin(ScreensaverEffect::ALL[position])
        } else {
            Self::Script(position - ScreensaverEffect::ALL.len())
        }
    }
}

pub struct Screensaver {
    open: bool,
    /// Wall clock of the activation and of the dismissal: the burn-in is timed
    /// on real seconds, not on frames, so a slow compositor cannot stretch it.
    opened_at: Option<Instant>,
    closed_at: Option<Instant>,
    /// Reveal value when the dismissal started (the exit ramp picks up there).
    reveal_at_close: f32,
    /// 0..1 burn-in progress. Entry and exit run the same ramp in opposite
    /// directions, so leaving is the exact reverse of entering.
    reveal: f32,
    /// Seconds the scene has been running, for the animation clock.
    elapsed: f32,
    /// Scenario currently on screen.
    mode: SceneKind,
    seed: f32,
    /// Scenario being faded in.
    next_mode: SceneKind,
    next_seed: f32,
    /// 0..1 crossfade between the two scenarios (0 = single shot).
    blend: f32,
    blend_started: Option<Instant>,
    /// Small xorshift state: no dependency, deterministic per activation.
    rng: u32,
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
        Self {
            open: false,
            opened_at: None,
            closed_at: None,
            reveal_at_close: 0.0,
            reveal: 0.0,
            elapsed: 0.0,
            mode: SceneKind::Builtin(ScreensaverEffect::Liquid),
            seed: 0.0,
            next_mode: SceneKind::Builtin(ScreensaverEffect::Liquid),
            next_seed: 0.0,
            blend: 0.0,
            blend_started: None,
            rng: nanos | 1,
        }
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

    /// Returns true when the state changed and the caller must act.
    pub fn open(
        &mut self,
        settings: &lard_core::theme::ScreensaverConfig,
        scripts: &[String],
    ) -> bool {
        if self.open {
            return false;
        }
        let entrance = settings.entrance_secs().clamp(0.15, 2.5);
        // Reopening while the exit fade is still running continues the same
        // scene from where it was: picking a new random one here looked like
        // the scenes swapping every time the session blinked.
        let resuming = self.reveal > 0.001;
        self.open = true;
        self.closed_at = None;
        self.blend = 0.0;
        self.blend_started = None;
        if resuming {
            self.opened_at = Some(Instant::now() - Duration::from_secs_f32(self.reveal * entrance));
            return true;
        }
        // The burn-in starts on the first *drawn* frame: creating the layer
        // surface and waiting for the compositor can take a second, and the
        // entrance would play behind an invisible surface.
        self.opened_at = None;
        self.elapsed = 0.0;
        self.reveal = 0.0;
        self.seed = self.next_random();
        self.next_seed = self.seed;
        let count = ScreensaverEffect::ALL.len() + scripts.len();
        self.mode = if settings.random && count > 0 {
            SceneKind::from_position(
                (self.next_random() * count as f32) as usize % count,
                scripts.len(),
            )
        } else {
            SceneKind::Builtin(settings.effect)
        };
        self.next_mode = self.mode;
        true
    }

    /// While the scene is on screen a frame is needed even if the compositor
    /// stops sending frame callbacks (it does for a fully transparent buffer):
    /// a short timer keeps the burn-in and the animation moving.
    #[must_use]
    pub fn next_deadline(&self) -> Option<std::time::Instant> {
        self.wants_surface()
            .then(|| std::time::Instant::now() + Duration::from_millis(16))
    }

    pub fn close(&mut self) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        self.closed_at = Some(Instant::now());
        self.reveal_at_close = self.reveal;
        true
    }

    /// Move to the previous/next scenario, with a crossfade. `delta` is -1 or
    /// +1; the arrow keys drive it.
    pub fn cycle(&mut self, delta: i32, scripts: &[String]) {
        if !self.open || delta == 0 {
            return;
        }
        let count = (ScreensaverEffect::ALL.len() + scripts.len()) as i32;
        if count == 0 {
            return;
        }
        let index = self.next_mode.position(scripts.len()) as i32;
        let next = (index + delta).rem_euclid(count) as usize;
        self.next_mode = SceneKind::from_position(next, scripts.len());
        self.next_seed = self.next_random();
        self.blend = 0.001;
        self.blend_started = Some(Instant::now());
    }

    /// Advance the burn-in and the crossfade. Returns true while a frame is
    /// needed, which keeps the scene animating at the frame rate.
    pub fn update(&mut self, _dt: f32, settings: &lard_core::theme::ScreensaverConfig) -> bool {
        let entrance = settings.entrance_secs().clamp(0.15, 2.5);
        let now = Instant::now();
        if self.open {
            if let Some(opened) = self.opened_at {
                let seconds = now.duration_since(opened).as_secs_f32();
                self.elapsed = seconds;
                self.reveal = (seconds / entrance).min(1.0);
            }
        } else if let Some(closed) = self.closed_at {
            let seconds = now.duration_since(closed).as_secs_f32();
            self.reveal = (self.reveal_at_close - seconds / entrance).max(0.0);
        }

        if let Some(started) = self.blend_started {
            self.blend = (now.duration_since(started).as_secs_f32() / BLEND_SECS).min(1.0);
            if self.blend >= 1.0 {
                // The incoming scenario becomes the current one: the pixels do
                // not change, only which side of the crossfade they come from.
                self.blend = 0.0;
                self.blend_started = None;
                self.mode = self.next_mode;
                self.seed = self.next_seed;
            }
        }
        self.wants_surface()
    }

    pub fn build_with(
        &mut self,
        scene: &mut Scene,
        size: Size,
        config: &Config,
        clock: &str,
        date: &str,
        scripts: &mut SceneScripts,
    ) {
        let theme = &config.theme;
        let fonts = &config.font;
        let settings = &config.screensaver;
        // First frame actually on screen: the burn-in clock starts here.
        if self.open && self.opened_at.is_none() {
            self.opened_at = Some(std::time::Instant::now());
        }
        let reveal = self.reveal.clamp(0.0, 1.0);
        if reveal <= 0.001 {
            return;
        }
        scene.fluid(Fluid {
            mode: self.mode.fluid(),
            mode_b: self.next_mode.fluid(),
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
            veil: settings.veil,
            dust: settings.dust,
            fade: 1.0,
        });

        // Scripted scenes paint their own shapes over the bare room. During a
        // crossfade the outgoing and incoming scenes are both drawn, each with
        // its share of the fade.
        let fade = if self.blend_started.is_some() {
            self.blend
        } else {
            0.0
        };
        if let SceneKind::Script(index) = self.mode {
            scripts.draw(
                index,
                scene,
                size,
                self.elapsed,
                theme,
                fonts.size_small,
                1.0 - fade,
            );
        }
        if fade > 0.0
            && let SceneKind::Script(index) = self.next_mode
        {
            scripts.draw(index, scene, size, self.elapsed, theme, fonts.size_small, fade);
        }

        if !settings.show_clock {
            return;
        }
        // The interface arrives once the room is closing in, rising a few
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
}

fn fluid_mode(effect: ScreensaverEffect) -> FluidMode {
    match effect {
        ScreensaverEffect::Liquid => FluidMode::Liquid,
        ScreensaverEffect::Mercury => FluidMode::Mercury,
        ScreensaverEffect::Aurora => FluidMode::Aurora,
        ScreensaverEffect::Gas => FluidMode::Gas,
        // Removed scenes and legacy names fall back to a surviving look.
        ScreensaverEffect::Waves | ScreensaverEffect::Grid | ScreensaverEffect::Tunnel => {
            FluidMode::Aurora
        }
        ScreensaverEffect::Dust | ScreensaverEffect::Vortex | ScreensaverEffect::Storm => {
            FluidMode::Liquid
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-5)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lard_core::theme::ScreensaverConfig;

    fn settings() -> ScreensaverConfig {
        ScreensaverConfig {
            random: false,
            ..ScreensaverConfig::default()
        }
    }

    fn wait(millis: u64) {
        std::thread::sleep(std::time::Duration::from_millis(millis));
    }

    #[test]
    fn entry_and_exit_are_symmetric() {
        let settings = ScreensaverConfig {
            entrance_ms: 300,
            ..ScreensaverConfig::default()
        };
        let mut saver = Screensaver::new();
        assert!(saver.open(&settings, &[]));
        // The burn-in is timed on the wall clock, not on frames.
        wait(360);
        saver.update(1.0 / 60.0, &settings);
        assert_eq!(saver.reveal, 1.0, "entrance did not finish in time");
        assert!(saver.wants_surface());

        assert!(saver.close());
        wait(150);
        saver.update(1.0 / 60.0, &settings);
        let middle = saver.reveal;
        assert!(middle > 0.0 && middle < 1.0, "exit is not gradual: {middle}");
        wait(220);
        saver.update(1.0 / 60.0, &settings);
        assert_eq!(saver.reveal, 0.0);
        assert!(!saver.wants_surface());
    }

    #[test]
    fn a_settled_scene_still_requests_frames_while_open() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings, &[]);
        wait(60);
        saver.update(1.0 / 60.0, &settings);
        assert!(saver.update(1.0 / 60.0, &settings));
    }

    #[test]
    fn random_mode_changes_the_composition_on_every_open() {
        let settings = ScreensaverConfig {
            random: true,
            entrance_ms: 300,
            ..settings()
        };
        let mut saver = Screensaver::new();
        let mut seeds = std::collections::HashSet::new();
        for _ in 0..8 {
            saver.open(&settings, &[]);
            seeds.insert(saver.seed.to_bits());
            saver.close();
            // Let the exit fade finish: reopening mid-fade keeps the scene.
            wait(360);
            saver.update(1.0 / 60.0, &settings);
        }
        assert!(seeds.len() >= 6, "seeds barely changed: {seeds:?}");
    }

    #[test]
    fn reopening_mid_fade_keeps_the_scene() {
        let settings = ScreensaverConfig {
            random: true,
            entrance_ms: 300,
            ..settings()
        };
        let mut saver = Screensaver::new();
        saver.open(&settings, &[]);
        let scene = saver.mode;
        let seed = saver.seed;
        saver.close();
        wait(80);
        assert!(saver.open(&settings, &[]));
        assert_eq!(saver.mode, scene, "the scene changed during the exit fade");
        assert_eq!(saver.seed, seed);
    }

    #[test]
    fn arrows_move_between_scenarios_in_both_directions() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings, &[]);
        let start = saver.next_mode;
        saver.cycle(1, &[]);
        let forward = saver.next_mode;
        assert_ne!(forward, start);
        saver.cycle(-1, &[]);
        assert_eq!(saver.next_mode, start);
        saver.cycle(-1, &[]);
        let back = saver.next_mode;
        assert_ne!(back, start);
        saver.cycle(1, &[]);
        assert_eq!(saver.next_mode, start);
    }

    #[test]
    fn a_scenario_change_is_a_crossfade_not_a_cut() {
        let settings = settings();
        let mut saver = Screensaver::new();
        saver.open(&settings, &[]);
        let before = saver.mode;
        saver.cycle(1, &[]);
        while saver.mode == before {
            saver.update(1.0 / 60.0, &settings);
            if saver.blend == 0.0 && saver.mode == before {
                // Still fading: the outgoing scenario is the current one.
                wait(40);
            }
        }
        assert_eq!(saver.blend, 0.0);
        assert_eq!(saver.mode, saver.next_mode);
        assert_ne!(saver.mode, before);
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
            saver.open(&settings, &[]);
            assert_eq!(saver.mode, SceneKind::Builtin(ScreensaverEffect::Mercury));
            saver.close();
            saver.update(1.0 / 60.0, &settings);
        }
    }
}
