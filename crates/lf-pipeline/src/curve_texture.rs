//! The 256×1 `RGBA8` tone-curve texture the develop pass looks up (the web's `this.curve`).

use crate::error::PipelineError;
use crate::gpu::Gpu;
use crate::texture::Target;
use lf_settings::Curves;
use lf_settings::curve::pack_curves;

pub struct CurveTexture {
    pub target: Target,
    /// The curves last uploaded (the web's `curveKey`); `None` until the first upload.
    key: Option<Curves>,
}

impl CurveTexture {
    /// A texture holding the identity curves, as the web's constructor uploads.
    pub fn new(gpu: &Gpu) -> Result<CurveTexture, PipelineError> {
        let target = Target::new(gpu, 256, 1, false)?;
        write(gpu, &target, &pack_curves(&Curves::identity()));
        Ok(CurveTexture { target, key: None })
    }

    /// Forget what was uploaded, so the next [`CurveTexture::update`] uploads again.
    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// Uploads `curves` when they differ from the last upload. Returns whether it uploaded.
    pub fn update(&mut self, gpu: &Gpu, curves: &Curves) -> bool {
        if self.key.as_ref() == Some(curves) {
            return false;
        }
        write(gpu, &self.target, &pack_curves(curves));
        self.key = Some(curves.clone());
        true
    }
}

fn write(gpu: &Gpu, target: &Target, data: &[u8; 1024]) {
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &target.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(1024),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 256,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
}
