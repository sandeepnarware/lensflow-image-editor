//! Turning an [`Opened`] picture into a [`Renderer`].

use std::sync::Arc;

use crate::open::{Opened, Picture};
use lf_pipeline::{Gpu, PipelineError, Renderer};

/// The renderer for `opened`: an 8-bit picture takes the web-identical 8-bit path
/// ([`Renderer::new`]); a developed RAW uploads as half floats with mips and keeps its
/// headroom above 1.0 ([`Renderer::from_frame`], which uploads under the call's own error
/// scopes, so a device error during the upload is returned rather than only logged).
pub fn renderer_for(gpu: &Arc<Gpu>, opened: &Opened) -> Result<Renderer, PipelineError> {
    match &opened.picture {
        Picture::EightBit(img) => {
            Renderer::new(gpu.clone(), img, opened.full_width, opened.full_height)
        }
        Picture::Float(dev) => {
            Renderer::from_frame(gpu.clone(), &dev.frame, dev.full_width, dev.full_height)
        }
    }
}
