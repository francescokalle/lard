// Orbite — sfere di vetro che ruotano attorno al centro.
//
// API: draw(t, w, h) restituisce un array di comandi.
//   #{ kind: "disc", x, y, r, color, alpha }        disco di vetro (glow)
//   #{ kind: "ring", x, y, r, width, color, alpha } anello
//   #{ kind: "rect", x, y, w, h, radius, color, alpha }
// color: "accent" | "text" | "muted" | "danger" | "white" | "#RRGGBB" | "#RRGGBBAA"
// t = secondi dall'ingresso, w/h = dimensioni in pixel.

fn name() {
    "Orbite"
}

fn draw(t, w, h) {
    let out = [];
    let cx = w / 2.0;
    let cy = h / 2.0;
    for i in 0..14 {
        let i = i * 1.0;
        let angle = t * (0.18 + 0.02 * i) + i * 0.449;
        let orbit = h * (0.10 + 0.035 * (i % 4));
        let size = 10.0 + 5.0 * (i % 3);
        out.push(#{
            kind: "disc",
            x: cx + cos(angle) * orbit * 1.6,
            y: cy + sin(angle * 1.13) * orbit * 0.75,
            r: size,
            color: "accent",
            alpha: 0.30,
        });
        out.push(#{
            kind: "ring",
            x: cx + cos(angle) * orbit * 1.6,
            y: cy + sin(angle * 1.13) * orbit * 0.75,
            r: size,
            width: 1.2,
            color: "text",
            alpha: 0.22,
        });
    }
    out
}
