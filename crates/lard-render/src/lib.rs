//! Hardware-accelerated renderer for lard.
//!
//! Everything on screen is drawn with two instanced pipelines:
//! * rounded rectangles / borders / soft shadows via a signed-distance field;
//! * glyphs from a single R8 atlas.
//!
//! There is no per-frame heap allocation once the scene reaches its steady
//! size, and idle surfaces are never redrawn.

pub mod gpu;
pub mod renderer;
pub mod scene;
pub mod surface;
pub mod text;

pub use gpu::Gpu;
pub use renderer::{RenderStats, Renderer};
pub use scene::{Corners, Fluid, FluidMode, FontFamily, Scene, Shadow, TextAlign, TextStyle};
pub use surface::RenderSurface;

#[cfg(test)]
mod shader_tests {
    /// The shaders are compiled at runtime, so a typo would only show up as a
    /// blank screen on the device. Parse every one of them in tests instead.
    #[test]
    fn every_shader_parses() {
        for (name, source) in [
            ("rect", include_str!("shaders/rect.wgsl")),
            ("text", include_str!("shaders/text.wgsl")),
            ("image", include_str!("shaders/image.wgsl")),
            ("fluid", include_str!("shaders/fluid.wgsl")),
            ("fluid_resolve", include_str!("shaders/fluid_resolve.wgsl")),
        ] {
            naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name}.wgsl: {}", error.emit_to_string(source)));
        }
    }
}
