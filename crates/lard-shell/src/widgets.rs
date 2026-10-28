//! Shared view helpers (single-threaded, allocation-light).

use lard_core::{Color, Rect};

/// Corner of the island a card grows from. While the card inflates the
/// anchor stays pinned to that corner, so the card never slides away from
/// the island it came out of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MorphAnchor {
    Left,
    Center,
    Right,
}

impl MorphAnchor {
    /// The x coordinate of the anchor inside a rectangle.
    #[must_use]
    pub fn x(self, rect: Rect) -> f32 {
        match self {
            MorphAnchor::Left => rect.x,
            MorphAnchor::Center => rect.x + rect.w * 0.5,
            MorphAnchor::Right => rect.x + rect.w,
        }
    }
}

/// Morph between two rectangles keeping the anchor corner pinned: the card
/// inflates out of the island instead of travelling to its final spot.
#[must_use]
pub fn morph_rect(from: Rect, to: Rect, t: f32, anchor: MorphAnchor) -> Rect {
    let t = t.clamp(0.0, 1.0);
    let width = from.w + (to.w - from.w) * t;
    let height = from.h + (to.h - from.h) * t;
    let x = anchor.x(from) + (anchor.x(to) - anchor.x(from)) * t;
    let y = from.y + (to.y - from.y) * t;
    let left = match anchor {
        MorphAnchor::Left => x,
        MorphAnchor::Center => x - width * 0.5,
        MorphAnchor::Right => x - width,
    };
    Rect::new(left, y, width, height)
}

/// Fast approximation used for layout only. Actual glyph positions come from
/// cosmic-text; a few pixels of slack are absorbed by padding.
///
/// The printable ASCII range uses the real advances of the bundled UI font,
/// so a label that fits is not cut and a label that does not fit is cut at
/// the right place. Other scripts fall back to a rough average.
#[must_use]
pub fn approx_width(text: &str, size: f32) -> f32 {
    let advances = lard_render::text::sans_advances();
    let scale = size / 100.0;
    text.chars()
        .map(|c| {
            let em = if (' '..='~').contains(&c) {
                advances[(c as u8 - 32) as usize]
            } else {
                55.0
            };
            em * scale
        })
        .sum::<f32>()
}

/// Alpha multiplier for frosted overlay panels. When compositor-side blur is
/// disabled, fall back to a nearly opaque fill so text stays legible.
///
/// `translucent` is the per-surface reference alpha; the user facing
/// `theme.overlay_opacity` scales all of them around the 0.64 reference.
/// The frost level decides how much of the blurred backdrop is let through:
/// an opaque panel colour would hide the blur behind it.
#[must_use]
pub fn panel_alpha(config: &lard_core::Config, translucent: f32) -> f32 {
    if config.theme.blur {
        let glass = config.theme.frost.clamp(0.0, 1.0);
        let alpha = config.theme.overlay_opacity * translucent / 0.64 * (1.0 - glass * 0.35);
        alpha.clamp(0.05, 1.0)
    } else {
        0.98
    }
}

#[must_use]
pub fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars && max_chars > 1 {
        out.pop();
        out.push('…');
    }
    out
}

#[must_use]
pub fn battery_color(percent: u8, theme: &lard_core::theme::Theme) -> Color {
    if percent <= 12 {
        theme.danger
    } else if percent <= 25 {
        theme.warning
    } else {
        theme.success
    }
}

/// Find a binary in `$PATH` without spawning a shell.
#[must_use]
pub fn command_exists(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

/// The advances come from the real font, so only kerning and hinting are
/// still missing: this small factor keeps the ellipsis in the right place.
const WIDTH_SAFETY: f32 = 1.03;

/// Truncate to a pixel budget using the same approximation as layout.
#[must_use]
pub fn truncate_to_width(text: &str, size: f32, max_width: f32) -> String {
    if approx_width(text, size) * WIDTH_SAFETY <= max_width {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut width = 0.0;
    let budget = (max_width - size * 0.8).max(0.0);
    for ch in text.chars() {
        let advance = approx_width(ch.encode_utf8(&mut [0; 4]), size) * WIDTH_SAFETY;
        if width + advance > budget {
            break;
        }
        out.push(ch);
        width += advance;
    }
    out.push('…');
    out
}

/// Small labelled meter (label, percentage, bar): the CPU/RAM rows shared by
/// the control center, the system monitor and the side panel.
pub fn meter(
    scene: &mut lard_render::Scene,
    rect: Rect,
    label: &str,
    percent: u8,
    config: &lard_core::Config,
    p: f32,
) {
    use lard_core::Point;
    use lard_render::{TextAlign, TextStyle};
    let theme = &config.theme;
    let fonts = &config.font;
    scene.text(
        label,
        Point::new(rect.x, rect.y),
        TextStyle {
            size: fonts.size_small,
            weight: fonts.weight_medium,
            ..Default::default()
        },
        theme.text_muted.scale_alpha(0.8 * p),
    );
    scene.text(
        format!("{}%", percent),
        Point::new(rect.x, rect.y),
        TextStyle {
            size: fonts.size_small,
            weight: fonts.weight_semibold,
            align: TextAlign::Right,
            max_width: Some(rect.w),
            ..Default::default()
        },
        theme.text.scale_alpha(0.95 * p),
    );
    let track = Rect::new(rect.x, rect.y + 18.0, rect.w, 6.0);
    scene.rect(track, theme.background.scale_alpha(0.75 * p), 3.0);
    let fill = track.w * f32::from(percent.min(100)) / 100.0;
    if fill > 0.0 {
        scene.rect(
            Rect::new(track.x, track.y, fill, track.h),
            theme.accent.scale_alpha(p),
            3.0,
        );
    }
}

/// The mini task manager: CPU and memory, load average, the two process
/// leaderboards and the uptime. Shared so the standalone panel and the control
/// center page never drift apart.
pub fn system_monitor(
    scene: &mut lard_render::Scene,
    panel: Rect,
    system: &lard_services::system::SystemStats,
    config: &lard_core::Config,
    p: f32,
) {
    use lard_core::Point;
    use lard_render::TextStyle;
    let theme = &config.theme;
    let fonts = &config.font;
    let width = panel.w - 36.0;
    meter(
        scene,
        Rect::new(panel.x + 18.0, panel.y + 34.0, width, 30.0),
        "CPU",
        system.cpu_percent,
        config,
        p,
    );
    scene.text(
        format!(
            "{} cores · load {:.2}  {:.2}  {:.2}",
            system.cpu_count, system.load[0], system.load[1], system.load[2]
        ),
        Point::new(panel.x + 18.0, panel.y + 64.0),
        TextStyle {
            size: fonts.size_small - 1.0,
            ..Default::default()
        },
        theme.text_muted.scale_alpha(0.72 * p),
    );
    meter(
        scene,
        Rect::new(panel.x + 18.0, panel.y + 92.0, width, 30.0),
        "RAM",
        system.memory_percent,
        config,
        p,
    );
    scene.text(
        format!(
            "{:.1} / {:.1} GB used",
            f32::from(system.memory_used_mb as u16) / 1024.0,
            f32::from(system.memory_total_mb as u16) / 1024.0
        ),
        Point::new(panel.x + 18.0, panel.y + 122.0),
        TextStyle {
            size: fonts.size_small - 1.0,
            ..Default::default()
        },
        theme.text_muted.scale_alpha(0.72 * p),
    );
    process_list(
        scene,
        Rect::new(panel.x, panel.y + 150.0, panel.w, 120.0),
        "Processes · CPU",
        &system.top_cpu,
        config,
        p,
    );
    process_list(
        scene,
        Rect::new(panel.x, panel.y + 278.0, panel.w, 120.0),
        "Processes · memory",
        &system.top_mem,
        config,
        p,
    );
    let uptime = system.uptime_secs;
    scene.text(
        format!("Up {}h {:02}m", uptime / 3600, (uptime % 3600) / 60),
        Point::new(panel.x + 18.0, panel.y + 404.0),
        TextStyle {
            size: fonts.size_small - 1.0,
            ..Default::default()
        },
        theme.text_muted.scale_alpha(0.6 * p),
    );
}

/// One line of a leaderboard: name on the left, CPU and memory on the right.
pub fn process_list(
    scene: &mut lard_render::Scene,
    rect: Rect,
    label: &str,
    processes: &[lard_services::system::ProcInfo],
    config: &lard_core::Config,
    p: f32,
) {
    use lard_core::Point;
    use lard_render::{TextAlign, TextStyle};
    let theme = &config.theme;
    let fonts = &config.font;
    scene.text(
        label,
        Point::new(rect.x + 18.0, rect.y),
        TextStyle {
            size: fonts.size_small - 1.0,
            weight: fonts.weight_semibold,
            ..Default::default()
        },
        theme.text_muted.scale_alpha(0.8 * p),
    );
    if processes.is_empty() {
        scene.text(
            "no processes",
            Point::new(rect.x + 18.0, rect.y + 20.0),
            TextStyle {
                size: fonts.size_small - 1.0,
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.5 * p),
        );
        return;
    }
    for (index, process) in processes.iter().enumerate() {
        let y = rect.y + 20.0 + index as f32 * 19.0;
        scene.text(
            truncate(&process.name, 24),
            Point::new(rect.x + 18.0, y),
            TextStyle {
                size: fonts.size_small,
                max_width: Some(rect.w - 190.0),
                wrap: false,
                ..Default::default()
            },
            theme.text.scale_alpha(0.9 * p),
        );
        scene.text(
            format!("{:.0}%", process.cpu_percent),
            Point::new(rect.x + 18.0, y),
            TextStyle {
                size: fonts.size_small,
                align: TextAlign::Right,
                max_width: Some(rect.w - 36.0 - 74.0),
                ..Default::default()
            },
            if process.cpu_percent >= 25.0 {
                theme.accent.scale_alpha(p)
            } else {
                theme.text.scale_alpha(0.8 * p)
            },
        );
        scene.text(
            format_quantity(process.memory_mb),
            Point::new(rect.x + 18.0, y),
            TextStyle {
                size: fonts.size_small,
                align: TextAlign::Right,
                max_width: Some(rect.w - 36.0),
                ..Default::default()
            },
            theme.text_muted.scale_alpha(0.75 * p),
        );
    }
}

/// Process memory with one decimal under 100 MB.
fn format_quantity(megabytes: f32) -> String {
    if megabytes >= 100.0 {
        format!("{megabytes:.0} MB")
    } else {
        format!("{megabytes:.1} MB")
    }
}
