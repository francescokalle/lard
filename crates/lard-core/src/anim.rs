//! Physics-based motion engine.
//!
//! Every animation in LARD goes through this module: bar, popovers, launcher,
//! OSD, notifications, lock screen. The model is a damped harmonic oscillator
//! solved in **closed form**, not integrated step by step, so a given motion is
//! identical at 30, 60, 120 and 144 Hz and it cannot explode after a stall.
//!
//! Springs are described the way designers describe motion (Apple's model):
//!
//! * `response` — how long the motion takes, in seconds (the period of the
//!   undamped oscillator, i.e. the time of one full swing);
//! * `damping ratio` — how much it bounces. `1.0` is critically damped (fastest
//!   approach without overshoot), `0.0` never stops, `0.65` bounces visibly.
//!
//! Internally they become `stiffness`/`damping`/`mass`, which is what the
//! configuration file stores, so both vocabularies describe the same spring:
//!
//! ```text
//! ω = 2π / response          stiffness = ω² · mass
//! ζ = damping / (2√(k·m))    damping   = 2ζ · ω · mass
//! ```
//!
//! Two properties make animations feel alive rather than scripted:
//!
//! * **interruptions keep momentum** — retargeting a running spring preserves
//!   its velocity, so reversing mid-flight curves smoothly instead of restarting
//!   from zero ([`Spring::set_target`]).
//! * **gesture handoff** — a drag can be released with the exact velocity of the
//!   finger ([`Spring::set_target_with_velocity`], [`project_velocity`]), which
//!   is what removes the seam between dragging and settling.

use crate::color::Color;
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Full turn, used to convert a `response` into an angular frequency.
const TAU: f32 = std::f32::consts::TAU;

/// Parameters of a spring.
///
/// `stiffness`, `damping` and `mass` are what the configuration file stores;
/// [`SpringConfig::response`] and [`SpringConfig::damping_ratio`] expose the
/// same spring in designer terms.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpringConfig {
    /// Restoring force. Higher = faster.
    pub stiffness: f32,
    /// Friction. Higher = less oscillation.
    pub damping: f32,
    /// Mass of the animated body.
    pub mass: f32,
    /// Distance below which the spring is considered at rest (in px).
    pub rest_distance: f32,
    /// Velocity below which the spring is considered at rest (px/s).
    pub rest_velocity: f32,
}

impl SpringConfig {
    /// Very fast and with no bounce: taps, toggles, focus rings.
    pub const STRICT: Self = Self {
        stiffness: 986.96,
        damping: 62.83,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    /// The default for panels and overlays: decisive, no overshoot.
    pub const SNAPPY: Self = Self {
        stiffness: 438.65,
        damping: 37.70,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    /// Long, calm motions (large cards, page transitions).
    pub const SMOOTH: Self = Self {
        stiffness: 194.96,
        damping: 27.93,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    /// Playful: overshoots by ~7%. For things that "pop" (icons, badges).
    pub const BOUNCY: Self = Self {
        stiffness: 194.96,
        damping: 18.15,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    /// Slow and relaxed, for surfaces that should not draw attention.
    pub const GENTLE: Self = Self {
        stiffness: 109.66,
        damping: 20.94,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    /// Very long glide: wallpaper changes, background cross-fades.
    pub const GLIDE: Self = Self {
        stiffness: 70.18,
        damping: 14.24,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    /// Kept for the older configuration: a stiff, slightly slower spring.
    pub const STIFF: Self = Self {
        stiffness: 900.0,
        damping: 60.0,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };

    /// No motion at all: used when the user asks for reduced motion.
    #[must_use]
    pub fn instant() -> Self {
        Self {
            stiffness: 3.0e6,
            damping: 4000.0,
            mass: 1.0,
            rest_distance: 0.5,
            rest_velocity: 4.0,
        }
    }

    /// Spring from the designer-facing pair.
    #[must_use]
    pub fn from_response(response: f32, damping_ratio: f32) -> Self {
        let omega = TAU / response.max(1.0e-3);
        Self {
            stiffness: omega * omega,
            damping: 2.0 * damping_ratio.max(0.0) * omega,
            mass: 1.0,
            rest_distance: 0.05,
            rest_velocity: 0.5,
        }
    }

    /// Spring from a stiffness plus the dimensionless damping ratio.
    #[must_use]
    pub fn from_stiffness(stiffness: f32, damping_ratio: f32) -> Self {
        let stiffness = stiffness.max(1.0e-3);
        let omega = stiffness.sqrt();
        Self {
            stiffness,
            damping: 2.0 * damping_ratio.max(0.0) * omega,
            mass: 1.0,
            rest_distance: 0.05,
            rest_velocity: 0.5,
        }
    }

    /// Angular frequency of the undamped oscillator (rad/s).
    #[must_use]
    pub fn natural_frequency(&self) -> f32 {
        (self.stiffness.max(1.0e-3) / self.mass.max(1.0e-3)).sqrt()
    }

    /// Duration of one full swing, in seconds.
    #[must_use]
    pub fn response(&self) -> f32 {
        TAU / self.natural_frequency()
    }

    /// Dimensionless damping: 1.0 = critical, below = bouncy, above = sluggish.
    #[must_use]
    pub fn damping_ratio(&self) -> f32 {
        let denom = 2.0 * (self.stiffness.max(1.0e-3) * self.mass.max(1.0e-3)).sqrt();
        self.damping.max(0.0) / denom
    }

    #[must_use]
    pub fn with_response(mut self, response: f32) -> Self {
        self.stiffness = (TAU / response.max(1.0e-3)).powi(2) * self.mass;
        self
    }

    #[must_use]
    pub fn with_damping_ratio(mut self, ratio: f32) -> Self {
        self.damping = 2.0 * ratio.max(0.0) * (self.stiffness.max(1.0e-3) * self.mass.max(1.0e-3)).sqrt();
        self
    }

    /// `bounce = 1 - damping_ratio`: 0 is flat, 0.3 is playful.
    #[must_use]
    pub fn with_bounce(self, bounce: f32) -> Self {
        self.with_damping_ratio(1.0 - bounce)
    }

    #[must_use]
    pub fn with_damping(mut self, damping: f32) -> Self {
        self.damping = damping;
        self
    }

    #[must_use]
    pub fn with_mass(mut self, mass: f32) -> Self {
        self.mass = mass.max(1.0e-3);
        self
    }

    #[must_use]
    pub fn with_rest(mut self, distance: f32, velocity: f32) -> Self {
        self.rest_distance = distance.max(0.0);
        self.rest_velocity = velocity.max(0.0);
        self
    }

    /// How long a step (from rest at `distance = 1`) takes to get within
    /// `rest_distance` of the target.
    ///
    /// Used by tests and by the demo to compare presets; the shell itself never
    /// needs it, because a spring knows when it has settled.
    #[must_use]
    pub fn settle_time(&self) -> f32 {
        let omega = self.natural_frequency().max(1.0e-3);
        let zeta = self.damping_ratio();
        let rest = self.rest_distance.max(1.0e-4);
        if zeta < 1.0 - 1.0e-3 {
            let damped = omega * (1.0 - zeta * zeta).sqrt();
            let amplitude = (1.0 + (zeta * omega / damped).powi(2)).sqrt();
            (amplitude / rest).ln() / (zeta * omega).max(1.0e-4)
        } else if zeta > 1.0 + 1.0e-3 {
            let slow = omega * (zeta - (zeta * zeta - 1.0).sqrt());
            (1.0 / rest).ln() / slow.max(1.0e-4)
        } else {
            // Critical: x(t) = (1 + ωt)e^{-ωt}; solve (1 + u)e^{-u} = rest.
            let mut u = 2.0f32;
            for _ in 0..24 {
                let f = (1.0 + u) * (-u).exp() - rest;
                let df = -u * (-u).exp();
                if df.abs() < 1.0e-9 {
                    break;
                }
                u = (u - f / df).clamp(0.0, 200.0);
            }
            u / omega
        }
    }

    /// Overshoot of a step response as a fraction of the distance (0 = none).
    #[must_use]
    pub fn overshoot(&self) -> f32 {
        let zeta = self.damping_ratio();
        if zeta < 1.0 - 1.0e-3 {
            (-std::f32::consts::PI * zeta / (1.0 - zeta * zeta).sqrt()).exp()
        } else {
            0.0
        }
    }

    /// The same spring, but effectively instant.
    #[must_use]
    pub fn reduced(&self) -> Self {
        Self::instant().with_rest(self.rest_distance, self.rest_velocity)
    }
}

impl Default for SpringConfig {
    fn default() -> Self {
        Self::SNAPPY
    }
}

/// A single scalar spring.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    value: f32,
    target: f32,
    velocity: f32,
    config: SpringConfig,
}

impl Spring {
    #[must_use]
    pub fn new(value: f32, config: SpringConfig) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
            config,
        }
    }

    /// A spring at rest on `value`, with the default feel.
    #[must_use]
    pub fn at(value: f32) -> Self {
        Self::new(value, SpringConfig::default())
    }

    /// A spring that starts on `from` and is already travelling to `to`.
    #[must_use]
    pub fn towards(from: f32, to: f32, config: SpringConfig) -> Self {
        let mut spring = Self::new(from, config);
        spring.target = to;
        spring
    }

    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    #[must_use]
    pub fn target(&self) -> f32 {
        self.target
    }

    #[must_use]
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    #[must_use]
    pub fn config(&self) -> SpringConfig {
        self.config
    }

    pub fn set_config(&mut self, config: SpringConfig) {
        self.config = config;
    }

    /// Change both the target and the feel mid-flight, keeping the momentum.
    pub fn animate_to(&mut self, target: f32, config: SpringConfig) {
        self.config = config;
        self.set_target(target);
    }

    /// Retarget while preserving velocity. This is what makes interrupted
    /// animations feel natural instead of restarting from zero.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Retarget and take the velocity of a gesture (px/s) as the new motion's
    /// starting velocity. Flinging a card away lands exactly like a finger
    /// flick, with no seam between dragging and settling.
    pub fn set_target_with_velocity(&mut self, target: f32, velocity: f32) {
        self.target = target;
        self.velocity = velocity;
    }

    /// Jump immediately, cancelling any motion.
    pub fn snap_to(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Set position and velocity at once (e.g. while the user drags).
    pub fn set_value_velocity(&mut self, value: f32, velocity: f32) {
        self.value = value;
        self.velocity = velocity;
    }

    /// 0..1 position between `from` and the target, clamped: convenient for
    /// progress-style animations (scrims, morphs) that must not overshoot.
    #[must_use]
    pub fn fraction(&self, from: f32) -> f32 {
        let span = self.target - from;
        if span.abs() < 1.0e-6 {
            return if self.is_settled() { 1.0 } else { 0.0 };
        }
        ((self.value - from) / span).clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() <= self.config.rest_distance
            && self.velocity.abs() <= self.config.rest_velocity
    }

    /// Rough time until this spring comes to rest, from its current state.
    #[must_use]
    pub fn settle_in(&self) -> f32 {
        let decay = self.config.damping_ratio() * self.config.natural_frequency();
        if decay <= 1.0e-4 {
            return f32::INFINITY;
        }
        let distance = (self.value - self.target).abs().max(self.config.rest_distance.max(1.0e-4));
        (distance / self.config.rest_distance.max(1.0e-4)).ln() / decay
    }

    /// Integrate by `dt` seconds using the closed-form solution.
    ///
    /// Returns `true` while the value is still moving.
    pub fn update(&mut self, dt: f32) -> bool {
        if !dt.is_finite() || dt <= f32::EPSILON {
            return !self.is_settled();
        }
        // A stalled frame (suspend, compositor hiccup) must not teleport the
        // animation: the analytic solution handles any dt, but the eye does not.
        let dt = dt.min(0.1);
        let k = self.config.stiffness.max(1.0e-3);
        let m = self.config.mass.max(1.0e-3);
        let c = self.config.damping.max(0.0);
        let omega = (k / m).sqrt();
        let zeta = c / (2.0 * (k * m).sqrt());

        let x0 = self.value - self.target;
        let v0 = self.velocity;
        let x;
        let v;

        if zeta < 1.0 - 1.0e-4 {
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let a = zeta * omega;
            let decay = (-a * dt).exp();
            let (sin, cos) = (wd * dt).sin_cos();
            let b = (v0 + a * x0) / wd;
            x = decay * (x0 * cos + b * sin);
            v = decay * (v0 * cos - ((a * v0 + omega * omega * x0) / wd) * sin);
        } else if zeta <= 1.0 + 1.0e-4 {
            let decay = (-omega * dt).exp();
            let b = v0 + omega * x0;
            x = decay * (x0 + b * dt);
            v = decay * (v0 - omega * b * dt);
        } else {
            let s = (zeta * zeta - 1.0).sqrt();
            let r1 = -omega * (zeta - s);
            let r2 = -omega * (zeta + s);
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            let e1 = (r1 * dt).exp();
            let e2 = (r2 * dt).exp();
            x = c1 * e1 + c2 * e2;
            v = c1 * r1 * e1 + c2 * r2 * e2;
        }

        if !x.is_finite() || !v.is_finite() {
            self.snap_to(self.target);
            return false;
        }

        self.value = self.target + x;
        self.velocity = v;

        if (self.value - self.target).abs() <= self.config.rest_distance
            && self.velocity.abs() <= self.config.rest_velocity
        {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

/// `N` independent springs sharing one feel.
///
/// Used for transforms (position + scale) and colors, so a whole element moves
/// as one object instead of drifting because two channels were tweaked apart.
#[derive(Clone, Copy, Debug)]
pub struct Springs<const N: usize> {
    values: [f32; N],
    targets: [f32; N],
    velocities: [f32; N],
    config: SpringConfig,
}

impl<const N: usize> Springs<N> {
    #[must_use]
    pub fn new(values: [f32; N], config: SpringConfig) -> Self {
        Self {
            values,
            targets: values,
            velocities: [0.0; N],
            config,
        }
    }

    pub fn set_target(&mut self, targets: [f32; N]) {
        self.targets = targets;
    }

    pub fn set_target_with_velocity(&mut self, targets: [f32; N], velocities: [f32; N]) {
        self.targets = targets;
        self.velocities = velocities;
    }

    pub fn snap_to(&mut self, values: [f32; N]) {
        self.values = values;
        self.targets = values;
        self.velocities = [0.0; N];
    }

    pub fn set_config(&mut self, config: SpringConfig) {
        self.config = config;
    }

    #[must_use]
    pub fn values(&self) -> [f32; N] {
        self.values
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        (0..N).all(|i| {
            (self.values[i] - self.targets[i]).abs() <= self.config.rest_distance
                && self.velocities[i].abs() <= self.config.rest_velocity
        })
    }

    /// Step every channel; returns `true` while any of them still moves.
    pub fn update(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for i in 0..N {
            let mut spring = Spring {
                value: self.values[i],
                target: self.targets[i],
                velocity: self.velocities[i],
                config: self.config,
            };
            moving |= spring.update(dt);
            self.values[i] = spring.value;
            self.velocities[i] = spring.velocity;
        }
        moving
    }
}

/// Two independent springs, convenient for positions.
#[derive(Clone, Copy, Debug)]
pub struct Spring2 {
    pub x: Spring,
    pub y: Spring,
}

impl Spring2 {
    #[must_use]
    pub fn new(value: (f32, f32), config: SpringConfig) -> Self {
        Self {
            x: Spring::new(value.0, config),
            y: Spring::new(value.1, config),
        }
    }

    pub fn set_target(&mut self, target: (f32, f32)) {
        self.x.set_target(target.0);
        self.y.set_target(target.1);
    }

    pub fn set_target_with_velocity(&mut self, target: (f32, f32), velocity: (f32, f32)) {
        self.x.set_target_with_velocity(target.0, velocity.0);
        self.y.set_target_with_velocity(target.1, velocity.1);
    }

    pub fn snap_to(&mut self, value: (f32, f32)) {
        self.x.snap_to(value.0);
        self.y.snap_to(value.1);
    }

    #[must_use]
    pub fn value(&self) -> (f32, f32) {
        (self.x.value(), self.y.value())
    }

    #[must_use]
    pub fn velocity(&self) -> (f32, f32) {
        (self.x.velocity(), self.y.velocity())
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.x.is_settled() && self.y.is_settled()
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let a = self.x.update(dt);
        let b = self.y.update(dt);
        a || b
    }
}

/// A color whose components are animated in sRGB space.
#[derive(Clone, Copy, Debug)]
pub struct AnimatedColor {
    channels: [Spring; 4],
}

impl AnimatedColor {
    #[must_use]
    pub fn new(color: Color, config: SpringConfig) -> Self {
        Self {
            channels: [
                Spring::new(color.r, config),
                Spring::new(color.g, config),
                Spring::new(color.b, config),
                Spring::new(color.a, config),
            ],
        }
    }

    pub fn set_target(&mut self, color: Color) {
        self.channels[0].set_target(color.r);
        self.channels[1].set_target(color.g);
        self.channels[2].set_target(color.b);
        self.channels[3].set_target(color.a);
    }

    pub fn snap_to(&mut self, color: Color) {
        self.channels[0].snap_to(color.r);
        self.channels[1].snap_to(color.g);
        self.channels[2].snap_to(color.b);
        self.channels[3].snap_to(color.a);
    }

    #[must_use]
    pub fn value(&self) -> Color {
        Color::rgba(
            self.channels[0].value(),
            self.channels[1].value(),
            self.channels[2].value(),
            self.channels[3].value(),
        )
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.channels.iter().all(Spring::is_settled)
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let mut active = false;
        for channel in &mut self.channels {
            active |= channel.update(dt);
        }
        active
    }
}

/// Where a fling would come to rest, given the velocity at release.
///
/// Apple's momentum projection: a scroll view or a card thrown by the finger
/// should land where the finger "aimed", not where it stopped. Used to pick the
/// target of a gesture release in the launcher and in the dock.
#[must_use]
pub fn project_velocity(velocity: f32, deceleration: f32) -> f32 {
    let deceleration = deceleration.max(0.01);
    velocity * velocity / (2.0 * deceleration)
}

/// CSS-style cubic Bézier for the few motions that are not spring-driven
/// (scrim fades, wallpaper gradients).
#[derive(Clone, Copy, Debug)]
pub struct CubicBezier {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

impl CubicBezier {
    pub const LINEAR: Self = Self::new(0.0, 0.0, 1.0, 1.0);
    pub const EASE_IN: Self = Self::new(0.42, 0.0, 1.0, 1.0);
    pub const EASE_OUT: Self = Self::new(0.16, 1.0, 0.3, 1.0);
    pub const EASE_IN_OUT: Self = Self::new(0.65, 0.0, 0.35, 1.0);
    /// Google's "emphasized" curve: slow start, long, soft landing.
    pub const EMPHASIZED: Self = Self::new(0.2, 0.0, 0.0, 1.0);

    #[must_use]
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn sample_x(&self, t: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * t * self.x1 + 3.0 * u * t * t * self.x2 + t * t * t
    }

    fn sample_y(&self, t: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * t * self.y1 + 3.0 * u * t * t * self.y2 + t * t * t
    }

    fn sample_dx(&self, t: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * self.x1 + 6.0 * u * t * (self.x2 - self.x1) + 3.0 * t * t * (1.0 - self.x2)
    }

    #[must_use]
    pub fn at(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let mut t = x;
        for _ in 0..6 {
            let error = self.sample_x(t) - x;
            if error.abs() < 1e-5 {
                break;
            }
            let dx = self.sample_dx(t);
            if dx.abs() < 1e-6 {
                break;
            }
            t = (t - error / dx).clamp(0.0, 1.0);
        }
        self.sample_y(t)
    }
}

/// dt / FPS measurement with a light exponential moving average.
#[derive(Debug)]
pub struct FrameClock {
    last: Instant,
    dt: f32,
    fps: f32,
    frames: u64,
}

impl FrameClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            dt: 1.0 / 60.0,
            fps: 60.0,
            frames: 0,
        }
    }

    /// Tick once per presented frame. Long stalls are clamped so a suspended
    /// laptop does not teleport every animation.
    pub fn tick(&mut self) -> f32 {
        let now = Instant::now();
        let raw = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.dt = raw.clamp(0.0, 0.1);
        self.frames = self.frames.wrapping_add(1);
        if raw > 0.0 {
            let instant_fps = 1.0 / raw;
            self.fps += (instant_fps - self.fps) * 0.08;
        }
        self.dt
    }

    #[must_use]
    pub fn dt(&self) -> f32 {
        self.dt
    }

    #[must_use]
    pub fn fps(&self) -> f32 {
        self.fps
    }

    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    const PRESETS: [(&str, SpringConfig); 6] = [
        ("strict", SpringConfig::STRICT),
        ("snappy", SpringConfig::SNAPPY),
        ("smooth", SpringConfig::SMOOTH),
        ("bouncy", SpringConfig::BOUNCY),
        ("gentle", SpringConfig::GENTLE),
        ("glide", SpringConfig::GLIDE),
    ];

    /// Step a spring to `1.0` for exactly `seconds` of wall clock and return
    /// (value, velocity). Whole steps only, so every rate lands on the same
    /// instant when `seconds` is a multiple of the intervals.
    fn run(config: SpringConfig, seconds: f32, hz: f32) -> (f32, f32) {
        let mut spring = Spring::new(0.0, config);
        spring.set_target(1.0);
        let dt = 1.0 / hz;
        let steps = (seconds * hz).round() as usize;
        for _ in 0..steps {
            spring.update(dt);
        }
        (spring.value(), spring.velocity())
    }

    #[test]
    fn spring_reaches_target() {
        let mut s = Spring::at(0.0);
        s.set_target(100.0);
        for _ in 0..240 {
            s.update(1.0 / 120.0);
        }
        assert!(approx(s.value(), 100.0, 0.1), "value = {}", s.value());
        assert!(s.is_settled());
    }

    #[test]
    fn presets_describe_their_parameters() {
        let strict = SpringConfig::STRICT;
        assert!(approx(strict.response(), 0.20, 0.005), "{}", strict.response());
        assert!(approx(strict.damping_ratio(), 1.0, 0.01));

        let snappy = SpringConfig::SNAPPY;
        assert!(approx(snappy.response(), 0.30, 0.005), "{}", snappy.response());
        assert!(approx(snappy.damping_ratio(), 0.90, 0.01));

        let bouncy = SpringConfig::BOUNCY;
        assert!(bouncy.damping_ratio() < 0.7);
        assert!(bouncy.overshoot() > 0.05);
    }

    #[test]
    fn response_round_trips() {
        for (name, config) in PRESETS {
            let rebuilt = SpringConfig::from_response(config.response(), config.damping_ratio());
            assert!(
                approx(rebuilt.stiffness, config.stiffness, config.stiffness * 0.01),
                "{name}: {} vs {}",
                rebuilt.stiffness,
                config.stiffness
            );
            assert!(
                approx(rebuilt.damping, config.damping, config.damping * 0.01),
                "{name}: {} vs {}",
                rebuilt.damping,
                config.damping
            );
        }
    }

    #[test]
    fn spring_is_frame_rate_independent() {
        // The same wall-clock trajectory at 30, 60, 120 and 144 Hz. Sampled
        // before anything settles: once a spring snaps to its target the value
        // is exact by construction and the comparison would be trivial.
        for (name, config) in PRESETS {
            // No "settled" snapping in this comparison: the rest thresholds
            // truncate the tail differently at each rate, and what is measured
            // here is the solver itself.
            let config = config.with_rest(1.0e-7, 1.0e-9);
            // 0.5 s is a whole number of frames at 30, 60, 120 and 144 Hz.
            let reference = run(config, 0.5, 144.0);
            for hz in [30.0, 60.0, 120.0] {
                let other = run(config, 0.5, hz);
                assert!(
                    (reference.0 - other.0).abs() < 1.0e-3,
                    "{name} at {hz} Hz: {} vs {}",
                    other.0,
                    reference.0
                );
                assert!(
                    (reference.1 - other.1).abs() < 0.5,
                    "{name} at {hz} Hz velocity: {} vs {}",
                    other.1,
                    reference.1
                );
            }
        }
    }

    #[test]
    fn no_overshoot_when_critically_damped() {
        for (name, config) in PRESETS {
            if config.damping_ratio() >= 1.0 {
                let (value, _) = run(config, 2.0, 120.0);
                assert!(value <= 1.0001, "{name} overshot: {value}");
            }
        }
    }

    #[test]
    fn overshoot_matches_the_analytic_formula() {
        let config = SpringConfig::from_response(0.45, 0.65);
        let mut spring = Spring::new(0.0, config);
        spring.set_target(1.0);
        let mut peak = 0.0f32;
        for _ in 0..480 {
            spring.update(1.0 / 240.0);
            peak = peak.max(spring.value());
        }
        let measured = peak - 1.0;
        assert!(
            approx(measured, config.overshoot(), 0.02),
            "measured {measured}, formula {}",
            config.overshoot()
        );
    }

    #[test]
    fn settle_time_estimate_is_close() {
        for (name, config) in PRESETS {
            let estimate = config.settle_time();
            let mut spring = Spring::new(0.0, config);
            spring.set_target(1.0);
            let dt = 1.0 / 240.0;
            let mut elapsed = 0.0;
            let mut settled_at = f32::INFINITY;
            for step in 0..(240 * 3) {
                if !spring.update(dt) && step > 0 {
                    settled_at = elapsed;
                    break;
                }
                elapsed += dt;
            }
            assert!(
                settled_at <= estimate * 1.6 + 0.05,
                "{name}: settled after {settled_at}s, estimate {estimate}s"
            );
        }
    }

    #[test]
    fn retarget_preserves_momentum() {
        let mut s = Spring::at(0.0);
        s.set_target(1.0);
        s.update(1.0 / 60.0);
        let v = s.velocity();
        s.set_target(0.0);
        assert!(approx(s.velocity(), v, f32::EPSILON));
    }

    #[test]
    fn interrupting_mid_flight_is_continuous() {
        let mut s = Spring::at(0.0);
        s.set_target(100.0);
        for _ in 0..10 {
            s.update(1.0 / 120.0);
        }
        let before = (s.value(), s.velocity());
        s.set_target(-50.0);
        // No jump: position and momentum are untouched by the retarget. The
        // body keeps drifting forward for a frame or two (it has momentum),
        // then the new target wins.
        assert_eq!((s.value(), s.velocity()), before);
        for _ in 0..36 {
            s.update(1.0 / 120.0);
        }
        assert!(s.value() < before.0, "value = {}", s.value());
        assert!(s.velocity() < 0.0);
    }

    #[test]
    fn gesture_velocity_is_adopted() {
        let mut s = Spring::at(0.0);
        s.set_target_with_velocity(500.0, 3200.0);
        assert!(approx(s.velocity(), 3200.0, f32::EPSILON));
        s.update(1.0 / 120.0);
        assert!(s.value() > 20.0, "value = {}", s.value());
    }

    #[test]
    fn projected_fling_distance() {
        // 2000 px/s released with the usual deceleration.
        let travel = project_velocity(2000.0, 8.0);
        assert!(approx(travel, 250_000.0, 1.0), "{travel}");
        assert!((project_velocity(0.0, 8.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn multi_spring_moves_every_channel() {
        let mut s = Springs::<3>::new([0.0, 0.0, 0.0], SpringConfig::SNAPPY);
        s.set_target([1.0, 2.0, 3.0]);
        for _ in 0..240 {
            s.update(1.0 / 120.0);
        }
        assert!(s.is_settled());
        let values = s.values();
        assert!(approx(values[0], 1.0, 0.01));
        assert!(approx(values[1], 2.0, 0.01));
        assert!(approx(values[2], 3.0, 0.01));
    }

    #[test]
    fn reduced_motion_is_instant() {
        let mut s = Spring::new(0.0, SpringConfig::instant());
        s.set_target(100.0);
        s.update(1.0 / 60.0);
        assert!(approx(s.value(), 100.0, 0.6), "value = {}", s.value());
        assert!(s.is_settled());
    }

    #[test]
    fn long_stall_does_not_explode() {
        let mut s = Spring::new(0.0, SpringConfig::BOUNCY);
        s.set_target(10.0);
        // A 5 s stall (suspend, compositor hiccup) is clamped to 0.1 s: the
        // animation continues from where the eye left it instead of teleporting.
        s.update(5.0);
        assert!(s.value().is_finite());
        assert!(s.value() > 0.0 && s.value() < 12.0, "value = {}", s.value());
        for _ in 0..120 {
            s.update(1.0 / 120.0);
        }
        assert!(s.is_settled());
        assert!(approx(s.value(), 10.0, 0.01));
    }

    #[test]
    fn color_lands_exactly_on_target() {
        let mut color = AnimatedColor::new(Color::rgba(0.0, 0.0, 0.0, 1.0), SpringConfig::SNAPPY);
        color.set_target(Color::rgba(1.0, 0.5, 0.25, 1.0));
        for _ in 0..240 {
            color.update(1.0 / 120.0);
        }
        let value = color.value();
        assert!(approx(value.r, 1.0, 0.01));
        assert!(approx(value.g, 0.5, 0.01));
        assert!(approx(value.b, 0.25, 0.01));
        assert!(color.is_settled());
    }

    #[test]
    fn bezier_bounds() {
        let e = CubicBezier::EASE_OUT;
        assert!(approx(e.at(0.0), 0.0, 1e-4));
        assert!(approx(e.at(1.0), 1.0, 1e-4));
        assert!(e.at(0.5) > 0.5);
        assert!(approx(CubicBezier::LINEAR.at(0.25), 0.25, 1e-3));
    }
}
