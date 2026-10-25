//! The web's `pass('copy', dst, { uTex: src })`.

use crate::gpu::Gpu;
use crate::passes::UniformArena;
use crate::texture::Target;

/// `copy.wgsl`'s `CopyParams`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    /// One destination pixel in UV: the derivatives `texture()` would take implicitly.
    pixel: [f32; 2],
    _pad: [f32; 2],
}

/// Draws `src` into `dst` (resampled by the sampler WebGL would use, opaque).
pub fn run(gpu: &Gpu, src: &Target, dst: &Target) {
    let pass = &gpu.pipelines.copy;
    let params = Params {
        pixel: [1.0 / dst.width as f32, 1.0 / dst.height as f32],
        _pad: [0.0; 2],
    };
    let uniforms = UniformArena::new(gpu, "copy params", &[params]);
    let bind = pass.bind(
        gpu,
        &src.view,
        gpu.samplers.for_target(src),
        Some(uniforms.binding()),
    );
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("copy"),
        });
    pass.draw(
        &mut encoder,
        &dst.level_view(0),
        dst.format,
        &bind,
        uniforms.offsets[0],
    );
    gpu.queue.submit([encoder.finish()]);
    crate::texture::generate_mips(gpu, dst);
}
