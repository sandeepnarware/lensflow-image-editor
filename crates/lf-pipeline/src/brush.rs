//! Desktop only: the brush masks' coverage, drawn on the CPU
//! ([`lf_settings::masks::rasterize`]) into one `R8` texture the develop pass samples. Each
//! mask has a layer (its place in the list) and the layers are stacked top to bottom, so one
//! binding serves all [`MAX_MASKS`]; a layer is the working photo's shape, its long edge at
//! most [`LAYER_EDGE`] (a brush is soft, so a finer map would not show).
//!
//! The coverage is drawn again only when the strokes change (or the photo's size does); a
//! photo with no brush mask keeps a 1×1 stand-in the develop pass never reads.

use crate::error::PipelineError;
use crate::gpu::Gpu;
use crate::texture::Target;
use lf_settings::EditSettings;
use lf_settings::masks::{MAX_MASKS, Masks, Shape, decode_coverage, image_weight, rasterize};

/// The layers stacked in the texture: one per mask, then the lens blur's depth
/// ([`lf_settings::lens_blur`]) in the last.
pub const LAYERS: usize = MAX_MASKS + 1;

/// The long edge of one mask's layer, in pixels.
pub const LAYER_EDGE: u32 = 512;

pub struct BrushTexture {
    pub target: Target,
    /// The strokes last drawn and the layer size; empty while nothing is drawn.
    key: String,
}

impl BrushTexture {
    /// The 1×1 stand-in.
    pub fn new(gpu: &Gpu) -> Result<BrushTexture, PipelineError> {
        Ok(BrushTexture {
            target: Target::new_with(gpu, 1, 1, false, wgpu::TextureFormat::R8Unorm)?,
            key: String::new(),
        })
    }

    /// Forget what was drawn, so the next [`BrushTexture::update`] draws again.
    pub fn invalidate(&mut self) {
        self.key.clear();
    }

    /// The layer size for a `width`×`height` working photo: its shape, the long edge
    /// [`LAYER_EDGE`] at most and short enough for eight to stack within the device's limit.
    pub fn layer_size(gpu: &Gpu, width: u32, height: u32) -> (u32, u32) {
        let limit = gpu.max_edge() / LAYERS as u32;
        let long = LAYER_EDGE.min(limit).min(width.max(height)).max(1);
        let k = f64::from(long) / f64::from(width.max(height).max(1));
        let side = |v: u32| ((f64::from(v) * k).round() as u32).max(1);
        (side(width), side(height))
    }

    /// Draws the brush masks of `s` for a `width`×`height` working photo when they changed.
    pub fn update(
        &mut self,
        gpu: &Gpu,
        s: &EditSettings,
        width: u32,
        height: u32,
    ) -> Result<(), PipelineError> {
        let masks = Masks::from_settings(s);
        let blur = lf_settings::lens_blur::LensBlur::from_settings(s);
        if blur.is_none()
            && !masks
                .0
                .iter()
                .any(|m| matches!(m.shape, Shape::Brush { .. } | Shape::Image { .. }))
        {
            return Ok(());
        }
        let (lw, lh) = BrushTexture::layer_size(gpu, width, height);
        // Each layer's source: brush strokes, or an AI mask's picture.
        let layers: Vec<Option<&Shape>> = masks
            .0
            .iter()
            .map(|m| match &m.shape {
                s @ (Shape::Brush { .. } | Shape::Image { .. }) => Some(s),
                _ => None,
            })
            .collect();
        let depth_key = blur.as_ref().map(|b| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (b.width, b.height, &b.depth).hash(&mut h);
            h.finish()
        });
        let key = format!(
            "{lw}x{lh}:{depth_key:?}:{}",
            serde_json::to_string(&layers).expect("shapes serialise")
        );

        if key == self.key {
            return Ok(());
        }
        let rows = lh * LAYERS as u32;
        if (self.target.width, self.target.height) != (lw, rows) {
            let old = std::mem::replace(
                &mut self.target,
                Target::new_with(gpu, lw, rows, false, wgpu::TextureFormat::R8Unorm)?,
            );
            old.texture.destroy();
        }
        let layer = (lw * lh) as usize;
        let mut data = vec![0u8; layer * LAYERS];
        if let Some((b, values)) = blur.as_ref().and_then(|b| b.values().map(|v| (b, v))) {
            resample(
                b.width,
                b.height,
                &values,
                lw,
                lh,
                &mut data[MAX_MASKS * layer..],
            );
        }
        for (i, shape) in layers.iter().enumerate().take(MAX_MASKS) {
            let out = &mut data[i * layer..(i + 1) * layer];
            match shape {
                Some(Shape::Brush { strokes }) => {
                    out.copy_from_slice(&rasterize(strokes, lw as usize, lh as usize));
                }
                Some(Shape::Image {
                    width,
                    height,
                    data: coded,
                    ..
                }) => {
                    if let Some(values) = decode_coverage(coded, (*width * *height) as usize) {
                        resample(*width, *height, &values, lw, lh, out);
                    }
                }
                _ => {}
            }
        }

        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(lw),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: lw,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        self.key = key;
        Ok(())
    }
}

/// An AI mask's `width` × `height` coverage drawn into a `lw` × `lh` layer (bilinear).
fn resample(width: u32, height: u32, values: &[u8], lw: u32, lh: u32, out: &mut [u8]) {
    for y in 0..lh {
        for x in 0..lw {
            let p = (
                (f64::from(x) + 0.5) / f64::from(lw),
                (f64::from(y) + 0.5) / f64::from(lh),
            );
            let w = image_weight(width, height, values, p);
            out[(y * lw + x) as usize] = (w * 255.0).round() as u8;
        }
    }
}
