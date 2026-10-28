//! CPU/GPU develop pipeline: a headless wgpu device, render targets with mip chains,
//! and the passes ported from the web editor's WebGL renderer.

pub mod brush;
pub mod curve_texture;
pub mod error;
pub mod gpu;
pub mod lut;
pub mod passes;
pub mod renderer;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod texture;

pub use error::PipelineError;
pub use gpu::Gpu;
pub use renderer::Renderer;
pub use texture::FloatFrame;
