// Upscale the half-resolution fluid into the output.

@group(0) @binding(0) var fluid: texture_2d<f32>;
@group(0) @binding(1) var fluid_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The fluid target already holds premultiplied colour, so the alpha goes
    // through untouched: that is what the entry/exit fade rides on.
    return textureSample(fluid, fluid_sampler, in.uv);
}
