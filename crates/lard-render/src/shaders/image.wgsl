// Rounded image-quad pipeline: samples the RGBA image atlas with soft corners.

struct Globals {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

@group(1) @binding(0) var image_atlas: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct Instance {
    @location(0) rect: vec4<f32>,   // x, y, w, h in physical pixels
    @location(1) uv: vec4<f32>,     // u0, v0, u1, v1
    @location(2) radii: vec4<f32>,  // tl, tr, br, bl
    @location(3) alpha: vec4<f32>,  // x = opacity
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) radii: vec4<f32>,
    @location(4) alpha: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_index];
    let pixel = instance.rect.xy + corner * instance.rect.zw;
    let ndc = vec2<f32>(
        pixel.x / globals.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / globals.screen.y * 2.0,
    );

    var out: VsOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = mix(instance.uv.xy, instance.uv.zw, corner);
    out.local = pixel - (instance.rect.xy + instance.rect.zw * 0.5);
    out.half_size = instance.rect.zw * 0.5;
    out.radii = instance.radii;
    out.alpha = instance.alpha;
    return out;
}

fn rounded_box_sdf(p: vec2<f32>, half_size: vec2<f32>, radii: vec4<f32>) -> f32 {
    var radius = radii.x;
    let right = p.x > 0.0;
    let bottom = p.y > 0.0;
    if (right && !bottom) {
        radius = radii.y;
    } else if (right && bottom) {
        radius = radii.z;
    } else if (!right && bottom) {
        radius = radii.w;
    }
    radius = clamp(radius, 0.0, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(image_atlas, image_sampler, in.uv);
    let d = rounded_box_sdf(in.local, in.half_size, in.radii);
    let aa = max(fwidth(d), 0.0001) * 0.75;
    let coverage = 1.0 - smoothstep(-aa, aa, d);
    let opacity = clamp(in.alpha.x, 0.0, 1.0);
    let a = tex.a * coverage * opacity;
    return vec4<f32>(tex.rgb * a, a);
}
