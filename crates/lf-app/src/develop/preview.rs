//! The preview canvas: a wgpu texture the photo's `Renderer` draws into and egui shows
//! directly (registered with egui-wgpu as a native texture), so the interactive path has no
//! readback. The canvas is the size of the photo's on-screen rectangle in physical pixels,
//! remade when that size changes (the texture id stays; egui is pointed at the new view).

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use lf_pipeline::texture::Target;
use lf_pipeline::{Gpu, PipelineError};

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
