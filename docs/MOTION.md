# Motion engine

Everything that moves in LARD — bar, popovers, launcher, OSD, notifications,
lock screen — goes through `lard-core/src/anim.rs`. It is not a set of
duration-based `lerp`s: it is a **damped harmonic oscillator solved in closed
form**.

```
x(t) = target + e^(-ζωt) · (x₀·cos(ω_d t) + b·sin(ω_d t))
```

Practical consequences:

- **same trajectory at 30, 60, 120 and 144 Hz**: no fixed-step accumulator, no
  `dt` to tune;
- **it cannot explode** after a stall (suspend, compositor hiccup): `dt` is
  clamped to 100 ms and the solution stays valid;
- **it can be interrupted without a seam**: changing the target mid-flight keeps
  position *and* velocity, so the motion curves instead of restarting from zero;
- **it lands exactly on the target** (rest thresholds on distance and velocity)
  and stops asking for frames.

## Describing a spring

Two vocabularies for the same thing. The config one is physical, the code one is
the designer-facing model (Apple's):

| config (`config.toml`) | code |
| --- | --- |
| `stiffness`, `damping`, `mass` | `SpringConfig::from_response(response, damping_ratio)` |
| — | `response()` = 2π/ω, one full swing in seconds |
| — | `damping_ratio()` = ζ, 1.0 = no bounce |

`ζ < 1` overshoots (`ζ 0.65` ≈ 7 %), `ζ = 1` is critical, `ζ > 1` arrives slowly
without ever passing the target. Duration is set by `response`.

## Presets

| preset | response | ζ | overshoot | settles in | frames @120 Hz |
| --- | --- | --- | --- | --- | --- |
| `STRICT` | 0.20 s | 1.00 | 0 % | 0.19 s | 23 |
| `SNAPPY` | 0.30 s | 0.90 | 0.2 % | 0.24 s | 29 |
| `SMOOTH` | 0.45 s | 1.00 | 0 % | 0.35 s | 43 |
| `BOUNCY` | 0.45 s | 0.65 | 6.8 % | 0.36 s | 44 |
| `GENTLE` | 0.60 s | 1.00 | 0 % | 0.45 s | 55 |
| `GLIDE` | 0.75 s | 0.85 | 0.6 % | 0.47 s | 57 |

Numbers produced by `cargo run --release --example motion -p lard-core`, which
also writes `docs/motion-springs.svg` (position and velocity over time).

What they are used for, by default:

- `animation.panel` → `SMOOTH` (popover, control center: short glide, no bounce);
- `animation.launcher` → `SMOOTH` (the card is large, the motion is longer);
- `animation.osd` → `STRICT` (volume/brightness must answer immediately);
- `animation.notification` → `BOUNCY` (they arrive with a 7 % bounce).

## Exit faster than entry

A dismissal must *read* as faster than the presentation (Apple's guidance): the
launcher closes with `response × 0.8` while keeping the same shape, and reopens
with the configured preset.

## Entry starts when the surface is on screen

A panel is a layer surface: the compositor creates and configures it a few
frames after `open()`. Starting the spring there would burn most of the
animation while the surface is still being mapped, and the panel would appear
fully formed. The shell therefore calls `start_entrance()` (on the launcher,
popovers, control center, settings, wallpapers and toasts) on the **first
configure** of the surface.

## API

```rust
use lard_core::anim::{Spring, SpringConfig, Springs, project_velocity};

// A value chasing a target.
let mut x = Spring::new(0.0, SpringConfig::SNAPPY);
x.set_target(1.0);
x.update(dt);                 // -> true while it moves
x.value();                    // position
x.fraction(0.0);              // 0..1 for morphs/scrims, already clamped

// Interruption with continuity: no jump.
x.set_target(0.0);

// Gesture release: the finger velocity becomes the initial velocity.
x.set_target_with_velocity(1.0, finger_px_per_second);

// Project a fling (where it would come to rest).
let landing = project_velocity(finger_px_per_second, 8.0);

// Several channels at once (position + scale, or RGBA).
let mut t = Springs::<3>::new([0.0, 0.0, 1.0], SpringConfig::BOUNCY);
t.set_target([10.0, 20.0, 1.1]);
```

`CubicBezier` is still available for the few things that are not physical
(scrims, background gradients).

## Reduced motion

`theme.animation.reduce_motion = true` makes every widget call `snap_to` instead
of animating, and `SpringConfig::reduced()` returns a practically instant spring
for code that builds the animation by hand.
