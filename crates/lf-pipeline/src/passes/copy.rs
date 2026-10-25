//! The web's `pass('copy', dst, { uTex: src })`.

use crate::gpu::Gpu;
use crate::texture::Target;

/// Draws `src` into `dst` (resampled by the sampler WebGL would use, opaque).
pub fn run(gpu: &Gpu, src: &Target, dst: &Target) {
    let pass = &gpu.pipelines.copy;
    let bind = pass.bind(gpu, &src.view, gpu.samplers.for_target(src), None);
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("copy"),
        });
    pass.draw(&mut encoder, &dst.level_view(0), &bind, 0);
    gpu.queue.submit([encoder.finish()]);
    crate::texture::generate_mips(gpu, dst);
}
