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
