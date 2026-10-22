// Rounded-rectangle SDF pipeline.
// One instanced draw call handles panels, pills, borders and soft shadows.

struct Globals {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect: vec4<f32>,          // x, y, w, h in physical pixels
    @location(1) color: vec4<f32>,         // straight linear RGBA
    @location(2) radii: vec4<f32>,         // tl, tr, br, bl
    @location(3) params: vec4<f32>,        // border_width, softness, opacity, mode
    @location(4) border_color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) radii: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) border_color: vec4<f32>,
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
    out.local = pixel - (instance.rect.xy + instance.rect.zw * 0.5);
    out.half_size = instance.rect.zw * 0.5;
    out.radii = instance.radii;
    out.color = instance.color;
    out.params = instance.params;
    out.border_color = instance.border_color;
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
    let d = rounded_box_sdf(in.local, in.half_size, in.radii);
    let opacity = clamp(in.params.z, 0.0, 1.0);
    let mode = in.params.w;

    if (mode > 1.5) {
        // Elliptical glow: radial falloff inside the rectangle. This is what
        // keeps a large low-alpha wallpaper blob from looking like a quad.
        let radius = length(in.local / max(in.half_size, vec2<f32>(1.0)));
        let coverage = 1.0 - smoothstep(0.0, 1.0, radius);
        let a = in.color.a * coverage * opacity;
        return vec4<f32>(in.color.rgb * a, a);
    }

    if (mode > 0.5) {
        // Soft shadow: full inside, smooth falloff outside. The quad is
        // padded by `params.x`; shrink the SDF back to the panel shape so
        // the falloff is actually rasterised instead of clipped.
        let pad = max(in.params.x, 0.0);
        let shape_half = max(in.half_size - vec2<f32>(pad), vec2<f32>(0.5));
        let shape_d = rounded_box_sdf(in.local, shape_half, in.radii);
        let soft = max(in.params.y, 1.0);
        let coverage = 1.0 - smoothstep(-soft, soft, shape_d);
        let a = in.color.a * coverage * opacity;
        return vec4<f32>(in.color.rgb * a, a);
    }

    let aa = max(fwidth(d), 0.0001) * 0.75;
    let fill_coverage = 1.0 - smoothstep(-aa, aa, d);
    let border_width = in.params.x;

    if (border_width > 0.01 && in.border_color.a > 0.0) {
        let inner_d = d + border_width;
        let inner_coverage = 1.0 - smoothstep(-aa, aa, inner_d);
        let border_coverage = clamp(fill_coverage - inner_coverage, 0.0, 1.0);

        let a_fill = in.color.a * inner_coverage * opacity;
        let a_border = in.border_color.a * border_coverage * opacity;
        let a = a_border + a_fill * (1.0 - a_border);
        let rgb = (in.border_color.rgb * a_border + in.color.rgb * a_fill * (1.0 - a_border))
            / max(a, 0.00001);
        return vec4<f32>(rgb * a, a);
    }

    let a = in.color.a * fill_coverage * opacity;
    return vec4<f32>(in.color.rgb * a, a);
}
