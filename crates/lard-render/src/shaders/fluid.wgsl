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
    // dust, unused, unused, unused
    extra: vec4<f32>,
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

/// Seven independent metaballs drifting on slow orbits: they merge when they
/// meet and split when they part, like a lava lamp.
fn metaballs(p: vec2<f32>, t: f32, seed: f32) -> f32 {
    var sum = 0.0;
    for (var i = 0; i < 7; i = i + 1) {
        let fi = f32(i);
        let phase = fi * 2.39996 + seed * 6.2831;
        let orbit = 0.20 + 0.11 * fi;
        let speed = 0.17 + 0.026 * fi;
        let center = vec2<f32>(
            cos(t * speed + phase) * orbit * 1.45,
            sin(t * speed * 0.83 + phase * 1.7) * orbit * 0.72,
        );
        let radius = 0.125 + 0.035 * sin(t * 0.29 + phase * 2.1);
        let delta = p - center;
        sum += (radius * radius) / (dot(delta, delta) + 0.0012);
    }
    return sum;
}


/// Gas density: domain-warped fbm, the classical volumetric cloud.
fn gas_density(p: vec2<f32>, t: f32, seed: f32) -> f32 {
    let warp = vec2<f32>(
        sin(p.y * 1.35 + t * 0.17 + seed * 3.0),
        cos(p.x * 1.15 - t * 0.14 + seed * 2.0),
    ) * 0.55;
    return fbm(p * 1.05 + warp + vec2<f32>(t * 0.018, -t * 0.013));
}

/// Gas: soft clouds lit from above, with a bright scattering rim.
fn gas_layer(p: vec2<f32>, t: f32, seed: f32, intensity: f32) -> vec4<f32> {
    let height = gas_density(p, t, seed);
    let eps = 0.02;
    let dx = gas_density(p + vec2<f32>(eps, 0.0), t, seed) - height;
    let dy = gas_density(p + vec2<f32>(0.0, eps), t, seed) - height;
    let normal = normalize(vec3<f32>(-dx / eps * 0.18, -dy / eps * 0.18, 1.0));
    let facing = clamp(normal.z, 0.0, 1.0);
    let key = normalize(vec3<f32>(-0.25, 0.55, 0.80));
    let lit = 0.30 + 0.70 * clamp(dot(normal, key), 0.0, 1.0);
    let density = smoothstep(0.30, 0.62, height);
    let rim = pow(1.0 - facing, 2.5);

    var color = f.accent.rgb * density * lit * 0.85;
    color += f.light.rgb * rim * density * (0.20 + 0.30 * intensity);
    color += f.accent.rgb * smoothstep(0.52, 0.95, height) * 0.12;
    let alpha = clamp(density * 0.85, 0.0, 1.0);
    return vec4<f32>(color * alpha, alpha);
}

/// Bright dust: a hash grid of slow particles, used both as a scene and as a
/// layer over the others.
fn dust(uv: vec2<f32>, t: f32, scale: f32) -> f32 {
    let q = uv * scale + vec2<f32>(t * 0.012, -t * 0.009);
    let cell = floor(q);
    let local = fract(q) - 0.5;
    var sum = 0.0;
    for (var i = 0; i < 9; i = i + 1) {
        let offset = vec2<f32>(f32(i % 3) - 1.0, f32(i / 3) - 1.0);
        let key = cell + offset;
        let seed = hash21(key);
        if (seed < 0.42) {
            continue;
        }
        let jitter = vec2<f32>(hash21(key + 3.1), hash21(key + 7.7)) - 0.5;
        let delta = local - offset - jitter * 0.7;
        let twinkle = 0.35 + 0.65 * (0.5 + 0.5 * sin(t * 1.6 + seed * 40.0));
        let size = 0.035 + 0.05 * fract(seed * 7.3);
        sum += smoothstep(size, 0.0, length(delta)) * twinkle;
    }
    return sum;
}

/// Waves: nine filaments of light undulating like an oscilloscope wall.
fn waves_scene(p: vec2<f32>, t: f32, intensity: f32) -> vec3<f32> {
    var color = vec3<f32>(0.0);
    for (var i = 0; i < 9; i = i + 1) {
        let fi = f32(i);
        let base = -0.44 + fi * 0.11;
        let amplitude = 0.030 + 0.012 * sin(fi * 1.7 + t * 0.3);
        let wave = amplitude * sin(p.x * 2.7 + t * (0.65 + fi * 0.11) + fi * 0.9)
            + 0.010 * sin(p.x * 5.3 - t * 0.9 + fi * 1.3);
        let d = abs(p.y - (base + wave));
        let line = exp(-d * d * 14000.0);
        let weight = 0.35 + 0.12 * sin(fi * 2.1 + t * 0.5);
        color += (f.accent.rgb * 0.75 + f.light.rgb * 0.25) * line * weight;
    }
    return color * (0.55 + 0.75 * intensity);
}

/// Grid: a perspective floor/ceiling grid running to the horizon.
fn grid_scene(uv: vec2<f32>, t: f32, intensity: f32) -> vec3<f32> {
    let horizon = 0.54;
    let dy = uv.y - horizon;
    let depth = 1.0 / max(abs(dy), 0.008);
    let lateral = (uv.x - 0.5) * depth;
    let vertical_line = abs(fract(lateral * 1.6) - 0.5);
    let depth_line = abs(fract(depth * 1.5 - t * 0.55) - 0.5);
    // Antialias the lines on their own screen-space width: far away a line is
    // thinner than a pixel, and a fixed threshold would break into dots.
    let vertical_width = clamp(fwidth(vertical_line) * 1.2, 0.001, 0.12);
    let depth_width = clamp(fwidth(depth_line) * 1.2, 0.001, 0.12);
    let lines = (1.0 - smoothstep(0.0, vertical_width, vertical_line))
        + (1.0 - smoothstep(0.0, depth_width, depth_line));
    let reach = smoothstep(0.0, 0.18, abs(dy)) * clamp(2.2 - depth * 0.05, 0.0, 1.0);
    let glow = exp(-abs(dy) * 30.0);
    var color = f.accent.rgb * (lines * reach * 0.55 + glow * 0.35);
    color += f.light.rgb * glow * 0.45;
    return color * (0.6 + 0.7 * intensity);
}

/// Dust: a slow nebula of bright grains with depth.
fn dust_scene(uv: vec2<f32>, t: f32, intensity: f32) -> vec3<f32> {
    let near = dust(uv, t, 5.0) * 0.85;
    let far = dust(uv * 1.9 + vec2<f32>(0.3, 0.1), t * 1.4, 9.0) * 0.45;
    let haze = smoothstep(0.35, 0.85, sin(uv.x * 1.7 + t * 0.17) * 0.5 + 0.5) * 0.05;
    return (f.accent.rgb * 0.8 + f.light.rgb * 0.2) * (near + far + haze) * (0.55 + 0.6 * intensity);
}

/// The liquid body: one metaball field, shaded per scenario.
fn liquid_layer(
    p: vec2<f32>,
    t: f32,
    style: u32,
    seed: f32,
    intensity: f32,
) -> vec4<f32> {
    let q = p;
    let height = metaballs(q, t, seed);
    let eps = 0.02;
    let dx = metaballs(q + vec2<f32>(eps, 0.0), t, seed) - height;
    let dy = metaballs(q + vec2<f32>(0.0, eps), t, seed) - height;
    // Antialiasing: the iso-line of the potential is one pixel wide in field
    // units, i.e. `gradient / screen height`. Normalising the threshold by
    // that width keeps the edge clean whatever the slope.
    let gradient = max(length(vec2<f32>(dx, dy)) / eps, 0.0005);
    let aa = clamp(gradient / max(f.screen.y, 1.0), 0.0004, 0.08);
    let inside = smoothstep(1.0 - aa, 1.0 + aa, height);
    let normal = normalize(vec3<f32>(-dx / eps * 0.055, -dy / eps * 0.055, 1.0));
    let facing = clamp(normal.z, 0.0, 1.0);
    let fresnel = pow(1.0 - facing, 2.4);
    // Light slightly above the viewer: a lateral light leaves a dark pit in
    // the middle of every volume (the normal there points straight up).
    let key = normalize(vec3<f32>(-0.10, 0.12, 0.99));
    let specular = pow(clamp(dot(normal, key), 0.0, 1.0), 22.0) * inside;

    var color = vec3<f32>(0.0);
    if (style == 1u) {
        // Mercury: the surface mirrors a bright room. A horizon line across
        // every volume is what sells polished chrome.
        let horizon = smoothstep(-0.65, 0.45, normal.y * 2.0 + 0.16);
        // Neutral dark instead of the theme black: chrome must not look brown
        // when the palette is warm.
        let chrome = mix(
            vec3<f32>(0.012, 0.015, 0.020),
            f.light.rgb,
            horizon * (0.26 + 0.46 * facing),
        );
        color = mix(f.deep.rgb, chrome, inside);
        color += f.accent.rgb * fresnel * 0.45 * inside;
        color += f.light.rgb * specular * (0.60 + 0.80 * intensity) * 1.6;
    } else {
        // Liquid and vortex: gold-filled glass. The alpha stays below one on
        // purpose: the room must read through the volumes.
        color = f.accent.rgb * (0.10 + 0.30 * facing + 0.52 * fresnel * fresnel);
        color += f.accent.rgb * fresnel * fresnel * 0.55 * (0.4 + 0.6 * intensity);
        color += f.light.rgb * specular * (0.22 + 0.38 * intensity);
    }

    let halo = exp(-abs(height - 1.0) * 26.0) * (1.0 - inside);
    let alpha = clamp(inside * 0.72 + halo * 0.26, 0.0, 1.0);
    let premul = color * inside + f.accent.rgb * halo * (0.10 + 0.30 * intensity);
    return vec4<f32>(premul, alpha);
}

/// One scenario, as premultiplied colour and coverage of its "body" layer.
fn shot(p: vec2<f32>, t: f32, mode: u32, uv: vec2<f32>, seed: f32, intensity: f32) -> vec4<f32> {
    if (mode == 0u || mode == 1u) {
        // Liquid and its chrome twin: the metaball body.
        return liquid_layer(p, t, mode, seed, intensity);
    }
    if (mode == 3u) {
        let color = waves_scene(p, t, intensity);
        return vec4<f32>(color, clamp(length(color) * 0.6, 0.0, 0.55));
    }
    if (mode == 4u) {
        let color = grid_scene(uv, t, intensity);
        return vec4<f32>(color, clamp(length(color) * 0.7, 0.0, 0.6));
    }
    if (mode == 5u) {
        let color = dust_scene(uv, t, intensity);
        return vec4<f32>(color, clamp(length(color) * 0.8, 0.0, 0.65));
    }
    if (mode == 6u) {
        return gas_layer(p, t, seed, intensity);
    }
    if (mode == 7u) {
        // Scripted scene: only the room, the shapes come from the script.
        return vec4<f32>(0.0);
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
    return vec4<f32>(color, clamp(alpha, 0.0, 1.0));
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

    // ---- backdrop -----------------------------------------------------------
    // Deliberately plain: the scenes are the subject. Just a dark gradient with
    // a vignette and a slightly darker foot for the clock.
    var room = mix(f.deep.rgb * 0.55, f.deep.rgb * 1.25, uv.y);
    room *= 1.0 - veil * 0.55;
    let vig = smoothstep(1.45, 0.10, length(p * vec2<f32>(0.82, 1.0)));
    room *= mix(1.0, 0.55, 1.0 - vig) * mix(1.0, 0.66, smoothstep(0.55, 1.0, uv.y) * 0.9);

    // ---- objects of the current scenario (crossfaded between two shots) ----
    var body = shot(p, t, f.modes.x, uv, seed_a, intensity);
    if (blend > 0.001) {
        let other = shot(p, t, f.modes.y, uv, seed_b, intensity);
        body = mix(body, other, smoothstep(0.0, 1.0, blend));
    }


    // Burn-in of the objects: bright structures first, additively, then the
    // bodies. `appear` works on the unpremultiplied brightness of each pixel.
    // Brightness of the body, for the burn-in order. Premultiplied is fine:
    // what matters is ranking the emissive parts before the dark ones.
    let body_luma = max(max(body.r, body.g), body.b);
    let start = 0.86 - progress * 1.10;
    let appear = smoothstep(start, start + 0.18, body_luma) * smoothstep(0.0, 0.02, progress);
    let body_fill = smoothstep(0.06, 0.45, progress);
    let glow = 1.0 + (1.0 - body_fill) * 1.65;
    let body_alpha = body.a * appear * body_fill;
    var body_rgb = body.rgb * appear * glow;
    // Light from the bodies is composited additively over the room: that is
    // what makes them look like they are burning into the screen.
    body_rgb += body.rgb * appear * (1.0 - body_fill) * 0.9 * max(0.0, 1.0 - body_alpha);

    // The room closes in last, under the objects.
    let room_alpha = smoothstep(0.50, 1.0, progress);


    let out_alpha = body_alpha + room_alpha * (1.0 - body_alpha);
    var out_rgb = body_rgb + room * room_alpha * (1.0 - body_alpha);

    // Layer on top of every scene: slow bright dust. Additive, so it reads as
    // light in the room instead of another object.
    let dust_layer = dust(uv, t * 0.7, 11.0) * f.extra.x * 0.5;
    out_rgb += (f.accent.rgb * 0.75 + f.light.rgb * 0.25) * dust_layer * fade;
    return vec4<f32>(max(out_rgb, vec3<f32>(0.0)) * fade, clamp(out_alpha, 0.0, 1.0) * fade);
}
