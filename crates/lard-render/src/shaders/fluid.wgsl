// Full-screen scene of the built-in screensaver.
//
// Everything is procedural: a volumetric "room" of drifting fog and caustics,
// a soft liquid haze behind it, and the liquid-glass bodies of the current
// scenario in front. There are no sprites, no floating bubbles and no video:
// the whole environment is evaluated per pixel, so it keeps evolving without
// ever repeating.
//
// The entrance is a burn-in: the bright structures appear first, additively
// over the live session; the bodies follow; the room closes in last. Leaving
// runs the same process backwards.

struct Fluid {
    screen: vec2<f32>,
    time: f32,
    mode: u32,
    // seed_a, seed_b, crossfade, reveal
    variants: vec4<f32>,
    // mode_a, mode_b, unused, unused
    modes: vec4<u32>,
    deep: vec4<f32>,
    accent: vec4<f32>,
    light: vec4<f32>,
    // speed, intensity, fade, veil
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> f: Fluid;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // One oversized triangle covers the viewport with a single draw.
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    let xy = corners[index];
    var out: VsOut;
    out.position = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
    return out;
}

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, vec3<f32>(p3.y, p3.z, p3.x) + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let fr = fract(p);
    let u = fr * fr * (3.0 - 2.0 * fr);
    let a = hash21(cell);
    let b = hash21(cell + vec2<f32>(1.0, 0.0));
    let c = hash21(cell + vec2<f32>(0.0, 1.0));
    let d = hash21(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(start: vec2<f32>) -> f32 {
    var p = start;
    var value = 0.0;
    var amp = 0.5;
    for (var i = 0; i < 3; i = i + 1) {
        value += amp * vnoise(p);
        p = mat2x2<f32>(0.80, -0.60, 0.60, 0.80) * p * 2.07 + vec2<f32>(1.7, 9.2);
        amp *= 0.5;
    }
    return value;
}

fn hue_rotate(color: vec3<f32>, angle: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735, 0.57735, 0.57735);
    let c = cos(angle);
    return color * c + cross(k, color) * sin(angle) + k * dot(k, color) * (1.0 - c);
}

/// Centre of the i-th control point of the liquid chain. The seed rearranges
/// the whole composition: same liquid, different shot.
fn node(i: f32, t: f32, seed: f32) -> vec2<f32> {
    let phase = i * 0.8976 + t * 0.155 + seed * 6.2831 + sin(seed * 13.7 + i) * 0.9;
    let r = 0.36 + 0.13 * sin(t * 0.19 + i * 1.7 + seed * 4.1);
    return vec2<f32>(
        cos(phase) * r * 1.42,
        sin(phase * 1.21 + t * 0.11 + seed * 2.7) * r * 0.60,
    );
}

/// Metaball height field of a chain of capsules: the merged surface is
/// `field == 1`. Capsules instead of lone blobs are what make the result read
/// as a flowing liquid, and the potential is smooth everywhere, so the normals
/// give rounded volumes.
fn field(p: vec2<f32>, t: f32, seed: f32) -> f32 {
    var sum = 0.0;
    var a = node(0.0, t, seed);
    for (var i = 1; i <= 9; i = i + 1) {
        let b = node(f32(i), t, seed);
        let radius = 0.048 + 0.014 * sin(t * 0.31 + f32(i) * 2.1 + seed * 5.0);
        let pa = p - a;
        let ba = b - a;
        let along = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-5), 0.0, 1.0);
        let off = pa - ba * along;
        sum += (radius * radius) / (dot(off, off) + 0.0011);
        a = b;
    }
    return sum;
}

/// Shaded body: premultiplied colour, coverage and the brightness used to
/// order the burn-in.
struct Body {
    color: vec3<f32>,
    alpha: f32,
    light: f32,
};

/// A single liquid body: premultiplied colour and alpha of the chain.
fn liquid_layer(p: vec2<f32>, t: f32, mode: u32, seed: f32, intensity: f32) -> Body {
        let height = field(p, t, seed);
        let eps = 0.02;
        let dx = field(p + vec2<f32>(eps, 0.0), t, seed) - height;
        let dy = field(p + vec2<f32>(0.0, eps), t, seed) - height;
        // Antialiasing: the iso-line of the potential is only one pixel wide
        // in field units, which is `gradient / screen height`. Normalising the
        // threshold by that width gives a clean edge whatever the slope, while
        // the fixed epsilon used before produced the stair steps.
        let gradient = max(length(vec2<f32>(dx, dy)) / eps, 0.0005);
        let aa = clamp(gradient / max(f.screen.y, 1.0), 0.0004, 0.08);
        let inside = smoothstep(1.0 - aa, 1.0 + aa, height);
        let normal = normalize(vec3<f32>(-dx / eps * 0.055, -dy / eps * 0.055, 1.0));
        let facing = clamp(normal.z, 0.0, 1.0);
        let fresnel = pow(1.0 - facing, 2.4);
        // Light slightly above the viewer: a lateral light leaves a dark pit
        // in the middle of every volume (the normal there points straight up).
        let key = normalize(vec3<f32>(-0.10, 0.12, 0.99));
        let specular = pow(clamp(dot(normal, key), 0.0, 1.0), 22.0) * inside;

        var color = vec3<f32>(0.0);
        if (mode == 0u) {
            // Liquid: gold-filled glass. The alpha stays below one on purpose:
            // the room must read through the volumes.
            color = f.accent.rgb * (0.10 + 0.30 * facing + 0.52 * fresnel * fresnel);
            color += f.accent.rgb * fresnel * fresnel * 0.55 * (0.4 + 0.6 * intensity);
            color += f.light.rgb * specular * (0.22 + 0.38 * intensity);
        } else {
            // Mercury: the surface mirrors a bright room. A horizon line
            // across every volume is what sells polished chrome.
            let horizon = smoothstep(-0.65, 0.45, normal.y * 2.0 + 0.16);
            color = mix(vec3<f32>(0.012, 0.015, 0.020), f.light.rgb, horizon * (0.26 + 0.46 * facing));
            color += f.accent.rgb * fresnel * 0.45;
            color += f.light.rgb * specular * (0.60 + 0.80 * intensity) * 1.6;
        }
        let halo = exp(-abs(height - 1.0) * 26.0) * (1.0 - inside);
        let alpha = clamp(inside * 0.72 + halo * 0.26, 0.0, 1.0);
        let premul = color * inside + f.accent.rgb * halo * (0.10 + 0.30 * intensity);
        let light = max(max(color.r, color.g), color.b);
        return Body(premul, alpha, light);
}

/// One scenario: two liquid masses at different depths, or the aurora.
fn shot(p: vec2<f32>, t: f32, mode: u32, seed: f32, intensity: f32) -> Body {
    if (mode == 0u || mode == 1u) {
        // Near mass, plus a larger, slower one behind it: the room reads as
        // full of liquid instead of a single blob on a table.
        let front = liquid_layer(p, t, mode, seed, intensity);
        let back = liquid_layer(
            p * 0.84 + vec2<f32>(0.16, -0.12),
            t * 0.85,
            mode,
            seed + 0.41,
            intensity * 0.65,
        );
        let alpha = front.alpha + back.alpha * (1.0 - front.alpha);
        let rgb = front.color + back.color * 0.40 * (1.0 - front.alpha);
        let light = max(front.light, back.light * 0.50);
        return Body(rgb, alpha, light);
    }

    // Aurora: three drifting curtains of light.
    var color = vec3<f32>(0.0);
    var alpha = 0.0;
    for (var k = 0; k < 3; k = k + 1) {
        let fk = f32(k);
        let phase = fk * 1.73 + seed * 4.0;
        let axis = p.y * 4.2
            + 0.55 * sin(p.x * 1.55 + t * 0.33 + phase)
            + 0.22 * sin(p.x * 3.10 - t * 0.26 + phase * 2.0)
            + 0.22 - fk * 0.42;
        let offset = axis - 0.08 * sin(t * 0.21 + phase);
        let curtain = exp(-offset * offset * 7.0);
        let core = exp(-offset * offset * 42.0);
        let tint = hue_rotate(f.accent.rgb, 1.25 - fk * 1.15 + 0.20 * sin(t * 0.07));
        color += tint * curtain * (0.22 + 0.16 * intensity);
        color += f.light.rgb * core * 0.12 * intensity;
        alpha += curtain * (0.28 + 0.18 * intensity) + core * 0.12 * intensity;
    }
    let rays = 0.5 + 0.5 * sin(p.x * 26.0 + 3.0 * sin(p.y * 3.0 + t * 0.4) + t * 0.9);
    color *= 0.82 + 0.30 * rays;
    alpha = clamp(alpha, 0.0, 1.0);
    let light = max(max(color.r, color.g), color.b);
    return Body(color, alpha, light);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let res = max(f.screen, vec2<f32>(1.0, 1.0));
    let aspect = clamp(res.x / res.y, 0.5, 4.0);
    let t = f.time * max(f.params.x, 0.05);
    let intensity = clamp(f.params.y, 0.0, 1.0);
    let fade = clamp(f.params.z, 0.0, 1.0);
    let veil = clamp(f.params.w, 0.0, 0.95);
    let seed_a = f.variants.x;
    let seed_b = f.variants.y;
    let blend = clamp(f.variants.z, 0.0, 1.0);
    let progress = clamp(f.variants.w, 0.0, 1.0);
    let uv = in.uv;
    let p = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);

    // ---- room: volumetric fog and caustics, all procedural -----------------
    let warp = vec2<f32>(
        fbm(p * 0.85 + vec2<f32>(0.0, t * 0.021)),
        fbm(p * 0.85 + vec2<f32>(3.7, 1.1) - vec2<f32>(t * 0.018, 0.0)),
    ) - vec2<f32>(0.47, 0.47);
    let fog = fbm(p * 1.15 + warp * 1.7 + vec2<f32>(t * 0.013, -t * 0.011));
    // Cold graphite, lit by the palette accent: a dark room, not a flat black.
    var room = mix(
        vec3<f32>(0.008, 0.011, 0.016),
        vec3<f32>(0.028, 0.040, 0.055) + f.accent.rgb * 0.03,
        smoothstep(0.28, 0.80, fog),
    );
    // Caustics: thin sheets of light where the fog folds.
    let caustic = pow(smoothstep(0.62, 0.95, fog), 5.0);
    room += f.accent.rgb * caustic * (0.14 + 0.18 * intensity);
    // A slow, out-of-focus liquid shadow behind everything gives depth.
    let haze = smoothstep(0.30, 1.35, field(p * 0.55 + vec2<f32>(0.0, t * 0.01), t * 0.6, seed_a + 0.77));
    room += f.accent.rgb * haze * (0.035 + 0.035 * intensity);
    room *= 1.0 - veil;
    // Vignette and a darker foot so the clock stays readable.
    let vig = smoothstep(1.45, 0.10, length(p * vec2<f32>(0.82, 1.0)));
    room *= mix(1.0, 0.55, 1.0 - vig) * mix(1.0, 0.66, smoothstep(0.55, 1.0, uv.y) * 0.9);

    // ---- objects of the current scenario (crossfaded between two shots) ----
    var body = shot(p, t, f.modes.x, seed_a, intensity);
    if (blend > 0.001) {
        let other = shot(p, t, f.modes.y, seed_b, intensity);
        let k = smoothstep(0.0, 1.0, blend);
        body.color = mix(body.color, other.color, k);
        body.alpha = mix(body.alpha, other.alpha, k);
        body.light = mix(body.light, other.light, k);
    }

    // Burn-in of the objects: bright structures first, additively, then the
    // bodies. `appear` works on the unpremultiplied brightness of each pixel.
    let body_luma = body.light;
    let start = 0.86 - progress * 1.10;
    let appear = smoothstep(start, start + 0.18, body_luma) * smoothstep(0.0, 0.02, progress);
    let body_fill = smoothstep(0.06, 0.45, progress);
    let glow = 1.0 + (1.0 - body_fill) * 1.65;
    let body_alpha = body.alpha * appear * body_fill;
    var body_rgb = body.color * appear * glow;
    // Light from the bodies is composited additively over the room: that is
    // what makes them look like they are burning into the screen.
    body_rgb += body.color * appear * (1.0 - body_fill) * 0.9 * max(0.0, 1.0 - body_alpha);

    // The room closes in last, under the objects.
    let room_alpha = smoothstep(0.50, 1.0, progress);
    let grain = hash21(floor(uv * res) + vec2<f32>(t * 7.0, t * 11.0)) - 0.5;
    body_rgb += grain * 0.0025;


    let out_alpha = body_alpha + room_alpha * (1.0 - body_alpha);
    let out_rgb = body_rgb + room * room_alpha * (1.0 - body_alpha);
    return vec4<f32>(max(out_rgb, vec3<f32>(0.0)) * fade, clamp(out_alpha, 0.0, 1.0) * fade);
}
