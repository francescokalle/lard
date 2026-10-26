// Full-screen backdrop and objects of the built-in screensaver.
//
// The screensaver is not a video: it is the desktop itself, frosted, with the
// shell's liquid-glass objects floating over it. This shader draws the frosted
// wallpaper (sampled from the image atlas and blurred in place) and the liquid
// bodies of the current scenario. Small glass droplets are drawn by the shell
// on top.
//
// The entrance is a burn-in: the bright structures of the objects appear first,
// additively over the live session; the bodies follow; the frosted backdrop
// closes the scene last. Leaving runs the same process backwards.

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
    // Wallpaper cell in the image atlas: u0, v0, u1, v1.
    bg_uv: vec4<f32>,
    // has_wallpaper, blur radius, unused, unused
    bg_info: vec4<f32>,
};

@group(0) @binding(0) var<uniform> f: Fluid;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

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

fn hue_rotate(color: vec3<f32>, angle: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735, 0.57735, 0.57735);
    let c = cos(angle);
    return color * c + cross(k, color) * sin(angle) + k * dot(k, color) * (1.0 - c);
}

/// Sample the wallpaper cell with a small ring blur. The cell is low
/// resolution already, so nine taps are enough for a convincing frost.
fn wallpaper(uv: vec2<f32>, blur: f32) -> vec3<f32> {
    let a = f.bg_uv.xy;
    let b = f.bg_uv.zw;
    let sample_uv = a + uv * (b - a);
    var acc = textureSample(atlas, atlas_sampler, sample_uv).rgb;
    if (blur > 0.00001) {
        for (var i = 0; i < 8; i = i + 1) {
            let angle = f32(i) * 0.785398;
            let offset = vec2<f32>(cos(angle), sin(angle)) * blur;
            acc += textureSample(
                atlas,
                atlas_sampler,
                clamp(sample_uv + offset * vec2<f32>(b.x - a.x, b.y - a.y) * 6.0, a, b),
            ).rgb;
        }
        acc /= 9.0;
    }
    return acc;
}

/// Centre of the i-th control point of the liquid chain. The seed rearranges
/// the whole composition: same room, different arrangement.
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
/// as a flowing liquid rather than a handful of floating bubbles, and the
/// potential is smooth everywhere, so the normals give rounded volumes.
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
    // Two fatter droplets drift through the chain and merge with it.
    for (var k = 0; k < 2; k = k + 1) {
        let fk = f32(k);
        let phase = fk * 3.1 + t * 0.24 + seed * 5.9 + fk * seed;
        let c = vec2<f32>(cos(phase) * 0.95, sin(phase * 0.83 + 1.7 + seed) * 0.34);
        let radius = 0.085 + 0.015 * sin(t * 0.27 + fk + seed * 3.0);
        sum += (radius * radius) / (dot(p - c, p - c) + 0.0011);
    }
    return sum;
}

/// One scenario: returns premultiplied colour and alpha of the liquid bodies.
fn shot(p: vec2<f32>, t: f32, mode: u32, seed: f32, intensity: f32) -> vec4<f32> {
    if (mode == 0u || mode == 1u) {
        // Liquid / Mercury: molten volumes shaded from a metaball field.
        let height = field(p, t, seed);
        let inside = smoothstep(0.985, 1.015, height);
        let eps = 0.02;
        let dx = field(p + vec2<f32>(eps, 0.0), t, seed) - height;
        let dy = field(p + vec2<f32>(0.0, eps), t, seed) - height;
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
            // the frosted desktop must read through the volumes.
            color = f.accent.rgb * (0.16 + 0.34 * facing + 0.30 * fresnel);
            color += f.accent.rgb * fresnel * fresnel * 0.55 * (0.4 + 0.6 * intensity);
            color += f.light.rgb * specular * (0.22 + 0.38 * intensity);
        } else {
            // Mercury: the surface mirrors a bright room. A horizon line
            // across every volume is what sells polished chrome.
            let horizon = smoothstep(-0.65, 0.45, normal.y * 2.0 + 0.16);
            color = mix(f.deep.rgb * 2.0, f.light.rgb, horizon * (0.30 + 0.55 * facing));
            color += f.accent.rgb * fresnel * 0.45;
            color += f.light.rgb * specular * (0.60 + 0.80 * intensity) * 1.6;
        }
        let halo = exp(-abs(height - 1.0) * 26.0) * (1.0 - inside);
        let alpha = clamp(inside * 0.86 + halo * 0.30, 0.0, 1.0);
        let premul = color * inside + f.accent.rgb * halo * (0.10 + 0.30 * intensity);
        return vec4<f32>(premul, alpha);
    }

    // Aurora: three drifting curtains of light above the room.
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
    return vec4<f32>(color, alpha);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let res = max(f.screen, vec2<f32>(1.0, 1.0));
    let aspect = clamp(res.x / res.y, 0.5, 4.0);
    let t = f.time * max(f.params.x, 0.05);
    let intensity = clamp(f.params.y, 0.0, 1.0);
    let fade = clamp(f.params.z, 0.0, 1.0);
    let veil = clamp(f.params.w, 0.0, 1.0);
    let seed_a = f.variants.x;
    let seed_b = f.variants.y;
    let blend = clamp(f.variants.z, 0.0, 1.0);
    let progress = clamp(f.variants.w, 0.0, 1.0);
    let uv = in.uv;
    let p = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);

    // ---- objects of the current scenario (crossfaded between two shots) ----
    var body = shot(p, t, f.modes.x, seed_a, intensity);
    if (blend > 0.001) {
        let other = shot(p, t, f.modes.y, seed_b, intensity);
        body = mix(body, other, smoothstep(0.0, 1.0, blend));
    }

    // Burn-in of the objects: bright structures first, additively, then the
    // bodies. `appear` works on the unpremultiplied brightness of each pixel.
    let body_luma = dot(body.rgb / max(body.a, 0.001), vec3<f32>(0.2126, 0.7152, 0.0722));
    let start = 0.86 - progress * 1.10;
    let appear = smoothstep(start, start + 0.18, body_luma) * smoothstep(0.0, 0.02, progress);
    let body_fill = smoothstep(0.06, 0.45, progress);
    let glow = 1.0 + (1.0 - body_fill) * 1.65;
    let body_alpha = body.a * appear * body_fill;
    var body_rgb = body.rgb * appear * glow;

    // Light from the bodies is composited additively over the room: that is
    // what makes them look like they are burning into the screen.
    body_rgb += body.rgb * appear * (1.0 - body_fill) * 0.9 * max(0.0, 1.0 - body_alpha);

    // ---- the room: frosted wallpaper, closing in last ----
    var room = f.deep.rgb;
    if (f.bg_info.x > 0.5) {
        let blur = mix(0.00002, f.bg_info.y, smoothstep(0.35, 1.0, progress));
        room = wallpaper(uv, blur);
    }
    room = mix(room, f.deep.rgb, veil);
    // A soft vignette and a darker foot so the clock stays readable.
    let vig = smoothstep(1.45, 0.10, length(p * vec2<f32>(0.82, 1.0)));
    room *= mix(1.0, 0.55, 1.0 - vig) * mix(1.0, 0.62, smoothstep(0.55, 1.0, uv.y) * 0.9);

    let room_alpha = smoothstep(0.50, 1.0, progress);
    let grain = hash21(floor(uv * res) + vec2<f32>(t * 7.0, t * 11.0)) - 0.5;
    body_rgb += grain * 0.0025;

    // Premultiplied composite: the room is behind, the objects float on top of
    // it. During the burn-in the room is still transparent, so the objects
    // burn in over the live session; at the end the room closes in under them.
    let out_alpha = body_alpha + room_alpha * (1.0 - body_alpha);
    let out_rgb = body_rgb + room * room_alpha * (1.0 - body_alpha);
    return vec4<f32>(max(out_rgb, vec3<f32>(0.0)) * fade, clamp(out_alpha, 0.0, 1.0) * fade);
}
