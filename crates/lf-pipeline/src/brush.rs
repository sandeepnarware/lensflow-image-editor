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
use lf_settings::masks::{MAX_MASKS, Masks, Shape, rasterize};

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
        let limit = gpu.max_edge() / MAX_MASKS as u32;
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
        if !masks
            .0
            .iter()
            .any(|m| matches!(m.shape, Shape::Brush { .. }))
        {
            return Ok(());
        }
        let (lw, lh) = BrushTexture::layer_size(gpu, width, height);
        let layers: Vec<Option<&Vec<_>>> = masks
            .0
            .iter()
            .map(|m| match &m.shape {
                Shape::Brush { strokes } => Some(strokes),
                _ => None,
            })
            .collect();
        let key = format!(
            "{lw}x{lh}:{}",
            serde_json::to_string(&layers).expect("strokes serialise")
        );
        if key == self.key {
            return Ok(());
        }
        let rows = lh * MAX_MASKS as u32;
        if (self.target.width, self.target.height) != (lw, rows) {
            let old = std::mem::replace(
                &mut self.target,
                Target::new_with(gpu, lw, rows, false, wgpu::TextureFormat::R8Unorm)?,
            );
            old.texture.destroy();
        }
        let layer = (lw * lh) as usize;
        let mut data = vec![0u8; layer * MAX_MASKS];
        for (i, strokes) in layers.iter().enumerate().take(MAX_MASKS) {
            if let Some(strokes) = strokes {
                data[i * layer..(i + 1) * layer].copy_from_slice(&rasterize(
                    strokes,
                    lw as usize,
                    lh as usize,
                ));
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
