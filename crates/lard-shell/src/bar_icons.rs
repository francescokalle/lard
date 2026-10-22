//! Custom glyphs for the bar islands.
//!
//! The Material font gives thin, outlined icons that do not match the solid
//! and rounded look of the rest of the shell. These are drawn with the SDF
//! primitives instead: they stay sharp at any scale, follow the theme colour
//! exactly and can be as chunky as the user wants.

use lard_core::Color;
use lard_core::Rect;
use lard_render::Scene;

/// Launcher: a 2x2 grid of rounded squares.
pub fn launcher(scene: &mut Scene, rect: Rect, color: Color) {
    let unit = rect.w * 0.42;
    let gap = (rect.w - unit * 2.0) / 3.0;
    for row in 0..2 {
        for col in 0..2 {
            let x = rect.x + gap + col as f32 * (unit + gap);
            let y = rect.y + gap + row as f32 * (unit + gap);
            scene.rect(Rect::new(x, y, unit, unit), color, unit * 0.32);
        }
    }
}

/// Grouped control center: three sliders with their knobs.
pub fn control_center(scene: &mut Scene, rect: Rect, color: Color) {
    let line_h = rect.h * 0.13;
    let knob = rect.h * 0.30;
    // The track length and the knob position of every slider.
    let tracks = [0.94, 0.74, 0.86];
    let knobs = [0.28, 0.60, 0.44];
    for index in 0..3 {
        let y = rect.y + rect.h * (0.14 + 0.36 * index as f32);
        let width = rect.w * tracks[index];
        scene.rect(Rect::new(rect.x, y, width, line_h), color, line_h * 0.5);
        let knob_x = rect.x + rect.w * knobs[index];
        scene.rect(
            Rect::new(knob_x - knob * 0.5, y - (knob - line_h) * 0.5, knob, knob),
            color,
            knob * 0.5,
        );
    }
}

/// Notification centre: a filled bell.
pub fn bell(scene: &mut Scene, rect: Rect, color: Color) {
    let w = rect.w;
    let h = rect.h;
    // The dome of the bell.
    scene.rect(
        Rect::new(rect.x + w * 0.20, rect.y + h * 0.02, w * 0.60, h * 0.60),
        color,
        w * 0.30,
    );
    // The rim, a bit wider than the dome.
    scene.rect(
        Rect::new(rect.x + w * 0.02, rect.y + h * 0.54, w * 0.96, h * 0.20),
        color,
        h * 0.10,
    );
    // The clapper.
    scene.rect(
        Rect::new(rect.x + w * 0.40, rect.y + h * 0.79, w * 0.20, h * 0.19),
        color,
        w * 0.10,
    );
}
