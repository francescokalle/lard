//! Motion engine report: prints the presets and writes `docs/motion-springs.svg`.
//!
//! Run on the build box with:
//!
//! ```text
//! cargo run --release --example motion -p lard-core
//! ```

use lard_core::anim::{CubicBezier, Spring, SpringConfig};

/// One preset: name, spring and the stroke used in the chart.
const PRESETS: [(&str, SpringConfig, &str); 6] = [
    ("strict", SpringConfig::STRICT, "#8AB4FF"),
    ("snappy", SpringConfig::SNAPPY, "#7BD88F"),
    ("smooth", SpringConfig::SMOOTH, "#FFD166"),
    ("bouncy", SpringConfig::BOUNCY, "#EF7C8E"),
    ("gentle", SpringConfig::GENTLE, "#C79BFF"),
    ("glide", SpringConfig::GLIDE, "#66D9E8"),
];

const SAMPLE_HZ: f32 = 240.0;

fn main() {
    println!(
        "{:<8} {:>9} {:>7} {:>10} {:>10} {:>8}",
        "preset", "response", "zeta", "overshoot", "settled", "frames"
    );
    let mut curves = Vec::new();
    for (name, config, color) in PRESETS {
        let samples = trace(config, 1.2);
        let settled = settle_time(&samples);
        println!(
            "{:<8} {:>8.2}s {:>7.2} {:>9.1}% {:>9.2}s {:>8}",
            name,
            config.response(),
            config.damping_ratio(),
            config.overshoot() * 100.0,
            settled,
            (settled * 120.0).ceil() as u32
        );
        curves.push((name, config, color, samples));
    }

    // Interruption: retarget mid-flight, keep the velocity, no jump.
    let mut spring = Spring::new(0.0, SpringConfig::SNAPPY);
    spring.set_target(1.0);
    let mut before = 0.0;
    for _ in 0..18 {
        spring.update(1.0 / 120.0);
        before = spring.value();
    }
    let velocity = spring.velocity();
    spring.set_target(0.35);
    println!(
        "\nretarget a 1.0 -> 0.35 after 150 ms: x = {before:.3}, v = {velocity:.1} px/s (unchanged), \
         next frame x = {:.3}",
        {
            spring.update(1.0 / 120.0);
            spring.value()
        }
    );

    // Raw samples, so the chart can also be rendered without an SVG viewer.
    let mut csv = String::from("preset,t,value,velocity\n");
    for (name, _, _, samples) in &curves {
        for (t, value, velocity) in samples {
            csv.push_str(&format!("{name},{t:.5},{value:.6},{velocity:.3}\n"));
        }
    }
    let _ = std::fs::write("docs/motion-springs.csv", csv);

    let svg = chart(&curves);
    let _ = std::fs::create_dir_all("docs");
    std::fs::write("docs/motion-springs.svg", svg).expect("write docs/motion-springs.svg");
    println!("chart: docs/motion-springs.svg");
}

/// Position over time for a step to 1.0, sampled at 240 Hz.
fn trace(config: SpringConfig, seconds: f32) -> Vec<(f32, f32, f32)> {
    let mut spring = Spring::new(0.0, config);
    spring.set_target(1.0);
    let dt = 1.0 / SAMPLE_HZ;
    let mut out = Vec::with_capacity((seconds * SAMPLE_HZ) as usize + 2);
    out.push((0.0, 0.0, 0.0));
    let mut t = 0.0;
    while t < seconds {
        spring.update(dt);
        t += dt;
        out.push((t, spring.value(), spring.velocity()));
    }
    out
}

/// Last moment the spring was still visibly moving.
fn settle_time(samples: &[(f32, f32, f32)]) -> f32 {
    let mut last_moving = 0;
    for (index, &(_, value, velocity)) in samples.iter().enumerate() {
        if (value - 1.0).abs() > 0.002 || velocity.abs() > 1.0 {
            last_moving = index;
        }
    }
    samples.get(last_moving + 1).map_or(0.0, |next| next.0)
}

fn chart(curves: &[(&str, SpringConfig, &str, Vec<(f32, f32, f32)>)]) -> String {
    let width = 900.0;
    let height = 640.0;
    let left = 70.0;
    let right = width - 30.0;
    let span = 1.2;

    let top = 70.0;
    let top_h = 230.0;
    let bottom = 380.0;
    let bottom_h = 190.0;

    // Auto-scale the velocity panel: a unit step moves a few pixels per
    // second, so a hard-coded range would flatten every curve on the axis.
    let mut velocity_lo = 0.0f32;
    let mut velocity_hi = 0.0f32;
    for (_, _, _, samples) in curves {
        for &(_, _, velocity) in samples {
            velocity_lo = velocity_lo.min(velocity);
            velocity_hi = velocity_hi.max(velocity);
        }
    }
    let velocity_pad = (velocity_hi - velocity_lo).max(0.1) * 0.12;
    let velocity_lo = velocity_lo - velocity_pad;
    let velocity_hi = velocity_hi + velocity_pad;

    let x = |t: f32| left + (t / span).clamp(0.0, 1.0) * (right - left);
    // Position panel: -0.1 .. 1.15 mapped top-down.
    let y_top = |value: f32| top + top_h - ((value + 0.1) / 1.25).clamp(0.0, 1.0) * top_h;
    let y_bottom = |value: f32| {
        bottom + bottom_h - ((value - velocity_lo) / (velocity_hi - velocity_lo)).clamp(0.0, 1.0) * bottom_h
    };

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">\n"
    ));
    svg.push_str(
        "<rect width=\"100%\" height=\"100%\" fill=\"#12100E\"/>\n\
         <text x=\"70\" y=\"38\" fill=\"#F2EDE4\" font-family=\"Inter, sans-serif\" font-size=\"20\" font-weight=\"600\">LARD motion engine · spring step response</text>\n\
         <text x=\"70\" y=\"58\" fill=\"#9A938A\" font-family=\"Inter, sans-serif\" font-size=\"12\">response 0.2–0.75 s · damping ratio 0.65–1.0 · same trajectory at 30/60/120/144 Hz</text>\n",
    );

    // Grid + axes for both panels.
    for panel in [(top, top_h, "position"), (bottom, bottom_h, "velocity px/s (unit step)")] {
        let (panel_top, panel_h, label) = panel;
        svg.push_str(&format!(
            "<rect x=\"{left:.0}\" y=\"{panel_top:.0}\" width=\"{:.0}\" height=\"{panel_h:.0}\" fill=\"#191614\" stroke=\"#2A2521\"/>\n",
            right - left
        ));
        for step in 0..=6 {
            let t = span * step as f32 / 6.0;
            let gx = x(t);
            svg.push_str(&format!(
                "<line x1=\"{gx:.1}\" y1=\"{panel_top:.0}\" x2=\"{gx:.1}\" y2=\"{:.0}\" stroke=\"#221E1B\"/>\n",
                panel_top + panel_h
            ));
            svg.push_str(&format!(
                "<text x=\"{gx:.1}\" y=\"{:.0}\" fill=\"#6F6862\" font-family=\"Inter, sans-serif\" font-size=\"10\" text-anchor=\"middle\">{t:.1}s</text>\n",
                panel_top + panel_h + 16.0
            ));
        }
        svg.push_str(&format!(
            "<text x=\"{left:.0}\" y=\"{:.0}\" fill=\"#9A938A\" font-family=\"Inter, sans-serif\" font-size=\"12\">{label}</text>\n",
            panel_top - 10.0
        ));
    }

    // Target line and zero line.
    svg.push_str(&format!(
        "<line x1=\"{left:.0}\" y1=\"{:.1}\" x2=\"{right:.0}\" y2=\"{:.1}\" stroke=\"#4A4038\" stroke-dasharray=\"4 4\"/>\n",
        y_top(1.0),
        y_top(1.0)
    ));
    svg.push_str(&format!(
        "<text x=\"{:.0}\" y=\"{:.1}\" fill=\"#6F6862\" font-family=\"Inter, sans-serif\" font-size=\"10\" text-anchor=\"end\">target</text>\n",
        left - 8.0,
        y_top(1.0) + 3.0
    ));
    svg.push_str(&format!(
        "<line x1=\"{left:.0}\" y1=\"{:.1}\" x2=\"{right:.0}\" y2=\"{:.1}\" stroke=\"#4A4038\"/>\n",
        y_bottom(0.0),
        y_bottom(0.0)
    ));

    for (_name, _config, color, samples) in curves {
        // Position polyline.
        let mut points = String::new();
        for &(t, value, _) in samples {
            points.push_str(&format!("{:.1},{:.1} ", x(t), y_top(value)));
        }
        svg.push_str(&format!(
            "<polyline points=\"{points}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2.2\" stroke-linejoin=\"round\"/>\n"
        ));
        // Velocity polyline, lighter.
        let mut velocity_points = String::new();
        for &(t, _, velocity) in samples {
            velocity_points.push_str(&format!("{:.1},{:.1} ", x(t), y_bottom(velocity)));
        }
        svg.push_str(&format!(
            "<polyline points=\"{velocity_points}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.4\" opacity=\"0.55\"/>\n"
        ));
    }

    // Legend.
    let mut legend_x = left;
    for (name, config, color, _) in curves {
        svg.push_str(&format!(
            "<line x1=\"{legend_x:.0}\" y1=\"{:.0}\" x2=\"{:.0}\" y2=\"{:.0}\" stroke=\"{color}\" stroke-width=\"3\"/>\n",
            height - 44.0,
            legend_x + 22.0,
            height - 44.0
        ));
        svg.push_str(&format!(
            "<text x=\"{:.0}\" y=\"{:.0}\" fill=\"#C9C2B8\" font-family=\"Inter, sans-serif\" font-size=\"11\">{name} · ζ {:.2}</text>\n",
            legend_x + 28.0,
            height - 40.0,
            config.damping_ratio()
        ));
        legend_x += 132.0;
    }
    svg.push_str(&format!(
        "<text x=\"{left:.0}\" y=\"{:.0}\" fill=\"#6F6862\" font-family=\"Inter, sans-serif\" font-size=\"11\">dashed: ease-out cubic · linear reference</text>\n",
        height - 14.0
    ));
    // A CSS easing reference for comparison, so the engine can be judged against
    // what a duration-based animation would look like.
    let mut points = String::new();
    for step in 0..=120 {
        let t = step as f32 / 120.0;
        points.push_str(&format!(
            "{:.1},{:.1} ",
            x(t * span),
            y_top(CubicBezier::EASE_OUT.at(t))
        ));
    }
    svg.push_str(&format!(
        "<polyline points=\"{points}\" fill=\"none\" stroke=\"#5B534B\" stroke-width=\"1.2\" stroke-dasharray=\"5 5\"/>\n"
    ));
    svg.push_str("</svg>\n");
    svg
}
