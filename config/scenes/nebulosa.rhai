// Nebulosa — veli concentrici che respirano, con un nucleo caldo.
//
// Vedi orbite.rhai per il riferimento dell'API.

fn name() {
    "Nebulosa"
}

fn draw(t, w, h) {
    let out = [];
    let cx = w / 2.0;
    let cy = h / 2.0;
    let pulse = 1.0 + 0.06 * sin(t * 0.5);

    for i in 0..7 {
        let i = i * 1.0;
        let radius = (90.0 + i * 46.0) * pulse;
        out.push(#{
            kind: "ring",
            x: cx,
            y: cy,
            r: radius,
            width: 1.0 + 0.5 * (i % 2.0),
            color: "accent",
            alpha: 0.16 - i * 0.015,
        });
    }

    for i in 0..26 {
        let i = i * 1.0;
        let angle = t * 0.12 + i * 0.733;
        let radius = 60.0 + i * 13.0;
        out.push(#{
            kind: "disc",
            x: cx + cos(angle) * radius,
            y: cy + sin(angle * 0.87) * radius * 0.62,
            r: 3.0 + 2.5 * (i % 3.0),
            color: "text",
            alpha: 0.20,
        });
    }

    out
}
