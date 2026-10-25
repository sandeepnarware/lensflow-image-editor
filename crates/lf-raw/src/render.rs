//! Turning an [`Opened`] picture into a [`Renderer`].

use std::sync::Arc;

use crate::open::{Opened, Picture};
use lf_pipeline::texture::Source;
use lf_pipeline::{Gpu, PipelineError, Renderer};

/// The renderer for `opened`: an 8-bit picture takes the web-identical 8-bit path
/// ([`Renderer::new`]); a developed RAW uploads as half floats with mips and keeps its
/// headroom above 1.0 ([`Renderer::from_texture`]).
pub fn renderer_for(gpu: &Arc<Gpu>, opened: &Opened) -> Result<Renderer, PipelineError> {
    match &opened.picture {
        Picture::EightBit(img) => {
            Renderer::new(gpu.clone(), img, opened.full_width, opened.full_height)
        }
        Picture::Float(dev) => {
            let source = Source::upload_f16(gpu, &dev.frame, true)?;
            Renderer::from_texture(gpu.clone(), source, dev.full_width, dev.full_height)
        }
    }
}
