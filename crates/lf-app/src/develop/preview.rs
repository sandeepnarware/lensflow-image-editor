//! The preview canvas: a wgpu texture the photo's `Renderer` draws into and egui shows
//! directly (registered with egui-wgpu as a native texture), so the interactive path has no
//! readback. The canvas is the size of the photo's on-screen rectangle in physical pixels,
//! remade when that size changes (the texture id stays; egui is pointed at the new view).
//!
//! On a software adapter the develop shader runs on the CPU, a pixel at a time, so the
//! canvas is capped at [`SOFTWARE_EDGE`] long edge ([`canvas_size`]). egui stretches the
//! smaller canvas over the same rectangle with bilinear filtering: the framing, the
//! histogram and every edit are the same, only the preview's sharpness differs, and the
//! per-frame cost of a drag falls by the square of the reduction.

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use lf_pipeline::texture::Target;
use lf_pipeline::{Gpu, PipelineError};

/// The longest edge of a preview canvas on a software adapter (WARP, lavapipe,
/// SwiftShader) whose 1400×900 window would otherwise ask for a million CPU-shaded pixels
/// per frame of a drag.
pub const SOFTWARE_EDGE: u32 = 1024;

/// The longest edge of a preview canvas: the device's limit, or [`SOFTWARE_EDGE`] when the
/// device rasterises on the CPU.
fn canvas_cap(gpu: &Gpu) -> u32 {
    let limit = gpu.max_edge();
    if gpu.is_software() {
        SOFTWARE_EDGE.min(limit)
    } else {
        limit
    }
}

/// `(width, height)` scaled down by one factor so its longest edge is at most `cap` (never
/// scaled up: a small photo is drawn at its own size). Aspect ratio is kept to the pixel.
pub fn cap_size(width: u32, height: u32, cap: u32) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    let longest = width.max(height);
    if longest <= cap.max(1) {
        return (width, height);
    }
    let k = f64::from(cap.max(1)) / f64::from(longest);
    (
        ((f64::from(width) * k).round() as u32).max(1),
        ((f64::from(height) * k).round() as u32).max(1),
    )
}

/// The canvas for a preview shown in `width`×`height` physical pixels: that size, capped by
/// [`canvas_cap`] for the device.
pub fn canvas_size(gpu: &Gpu, width: f64, height: f64) -> (u32, u32) {
    let w = (width.round() as u32).max(1);
    let h = (height.round() as u32).max(1);
    cap_size(w, h, canvas_cap(gpu))
}

pub struct Preview {
    render_state: RenderState,
    canvas: Option<Target>,
    texture: Option<egui::TextureId>,
}

impl Preview {
    /// A preview that registers its canvas with egui's renderer in `render_state`.
    pub fn new(render_state: RenderState) -> Preview {
        Preview {
            render_state,
            canvas: None,
            texture: None,
        }
    }

    /// The canvas at `width`×`height` (physical pixels), and egui's id for it. Returns true
    /// in `.2` when the canvas was (re)made, so it needs drawing.
    pub fn ensure(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
    ) -> Result<(&Target, egui::TextureId, bool), PipelineError> {
        let (width, height) = (width.max(1), height.max(1));
        let fresh = !matches!(&self.canvas, Some(c) if c.width == width && c.height == height);
        if fresh {
            // `Rgba8Unorm` holding display-encoded values: what egui samples ("normal",
            // not sRGB-aware, textures).
            let canvas = Target::new(gpu, width, height, false)?;
            let mut renderer = self.render_state.renderer.write();
            let id = match self.texture {
                Some(id) => {
                    renderer.update_egui_texture_from_wgpu_texture(
                        &self.render_state.device,
                        canvas.view(),
                        wgpu::FilterMode::Linear,
                        id,
                    );
                    id
                }
                None => renderer.register_native_texture(
                    &self.render_state.device,
                    canvas.view(),
                    wgpu::FilterMode::Linear,
                ),
            };
            drop(renderer);
            // The old canvas is dropped (not destroyed): work already submitted keeps it.
            self.canvas = Some(canvas);
            self.texture = Some(id);
        }
        let canvas = self.canvas.as_ref().expect("made above");
        let id = self.texture.expect("registered above");
        Ok((canvas, id, fresh))
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        if let Some(id) = self.texture.take() {
            self.render_state.renderer.write().free_texture(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_canvas_is_capped_by_its_longest_edge() {
        assert_eq!(cap_size(1200, 800, 1024), (1024, 683), "landscape");
        assert_eq!(cap_size(800, 1200, 1024), (683, 1024), "portrait");
        assert_eq!(cap_size(1024, 700, 1024), (1024, 700), "already inside");
        assert_eq!(cap_size(300, 200, 1024), (300, 200), "never enlarged");
        // A square cap keeps a square.
        assert_eq!(cap_size(2000, 2000, 1000), (1000, 1000));
        // Degenerate sizes and caps still make a canvas.
        assert_eq!(cap_size(0, 0, 1024), (1, 1));
        assert_eq!(
            cap_size(4000, 10, 100),
            (100, 1),
            "a one-pixel side stays one"
        );
    }
}
