//! Frame-rate independent motion.
//!
//! The core is an analytic damped harmonic oscillator. Unlike naive Euler
//! integration it produces identical trajectories at 60 Hz, 120 Hz and 144 Hz,
//! and it cannot explode with a long frame time. This is what keeps animations
//! fluid without a fixed-timestep accumulator or per-frame allocations.

use crate::color::Color;
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Parameters of a spring.
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
    pub const SNAPPY: Self = Self {
        stiffness: 420.0,
        damping: 41.0,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    pub const GENTLE: Self = Self {
        stiffness: 200.0,
        damping: 28.3,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };
    pub const STIFF: Self = Self {
        stiffness: 900.0,
        damping: 60.0,
        mass: 1.0,
        rest_distance: 0.05,
        rest_velocity: 0.5,
    };

    #[must_use]
    pub fn with_damping(mut self, damping: f32) -> Self {
        self.damping = damping;
        self
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

    #[must_use]
    pub fn at(value: f32) -> Self {
        Self::new(value, SpringConfig::default())
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

    /// Retarget while preserving velocity. This is what makes interrupted
    /// animations feel natural instead of restarting from zero.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump immediately, cancelling any motion.
    pub fn snap_to(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() <= self.config.rest_distance
            && self.velocity.abs() <= self.config.rest_velocity
    }

    /// Integrate by `dt` seconds using the closed-form solution.
    ///
    /// Returns `true` while the value is still moving.
    pub fn update(&mut self, dt: f32) -> bool {
        if dt <= f32::EPSILON {
            return !self.is_settled();
        }
        let dt = dt.min(0.1);
        let k = self.config.stiffness.max(1.0);
        let m = self.config.mass.max(0.01);
        let c = self.config.damping.max(0.0);
        let omega = (k / m).sqrt();
        let zeta = c / (2.0 * (k * m).sqrt());

        let x0 = self.value - self.target;
        let v0 = self.velocity;
        let x;
        let v;

        if zeta < 1.0 - 1e-4 {
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let a = zeta * omega;
            let decay = (-a * dt).exp();
            let (sin, cos) = (wd * dt).sin_cos();
            let b = (v0 + a * x0) / wd;
            x = decay * (x0 * cos + b * sin);
            v = decay * (v0 * cos - ((a * v0 + omega * omega * x0) / wd) * sin);
        } else if zeta <= 1.0 + 1e-4 {
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

    pub fn snap_to(&mut self, value: (f32, f32)) {
        self.x.snap_to(value.0);
        self.y.snap_to(value.1);
    }

    #[must_use]
    pub fn value(&self) -> (f32, f32) {
        (self.x.value(), self.y.value())
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

    pub fn update(&mut self, dt: f32) -> bool {
        let mut active = false;
        for channel in &mut self.channels {
            active |= channel.update(dt);
        }
        active
    }
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
    pub const EASE_OUT: Self = Self::new(0.16, 1.0, 0.3, 1.0);
    pub const EASE_IN_OUT: Self = Self::new(0.65, 0.0, 0.35, 1.0);
    pub const LINEAR: Self = Self::new(0.0, 0.0, 1.0, 1.0);

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
    fn spring_is_frame_rate_independent() {
        let mut a = Spring::at(0.0);
        let mut b = Spring::at(0.0);
        a.set_target(1.0);
        b.set_target(1.0);
        for _ in 0..60 {
            a.update(1.0 / 60.0);
        }
        for _ in 0..120 {
            b.update(1.0 / 120.0);
        }
        assert!(approx(a.value(), b.value(), 0.01));
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
    fn bezier_bounds() {
        let e = CubicBezier::EASE_OUT;
        assert!(approx(e.at(0.0), 0.0, 1e-4));
        assert!(approx(e.at(1.0), 1.0, 1e-4));
        assert!(e.at(0.5) > 0.5);
    }
}
