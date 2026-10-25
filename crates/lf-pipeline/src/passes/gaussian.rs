//! The web's `blur(from, to, tmp, sigma)`: the separable `GAUSSIAN` across into `tmp`,
//! then down into `to`, then `to`'s mips.

use crate::gpu::Gpu;
use crate::passes::UniformArena;
use crate::texture::{Target, generate_mips};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    step: [f32; 2],
    sigma: f32,
    _pad: f32,
}

/// Blurs `from` into `to` through `tmp`; `sigma` is in texels of `to`.
pub fn blur(gpu: &Gpu, from: &Target, to: &Target, tmp: &Target, sigma: f32) {
    let runs = [(from, tmp, 1.0, 0.0), (tmp, to, 0.0, 1.0)];
    let params = runs.map(|(_, dst, dx, dy)| Params {
        step: [dx / dst.width as f32, dy / dst.height as f32],
        sigma,
        _pad: 0.0,
    });
    let uniforms = UniformArena::new(gpu, "gaussian params", &params);
    let pass = &gpu.pipelines.gaussian;
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gaussian"),
        });
    for ((src, dst, _, _), &offset) in runs.into_iter().zip(&uniforms.offsets) {
        let bind = pass.bind(
            gpu,
            &src.view,
            gpu.samplers.for_target(src),
            Some(uniforms.binding()),
        );
        pass.draw(&mut encoder, &dst.level_view(0), dst.format, &bind, offset);
    }
    gpu.queue.submit([encoder.finish()]);
    generate_mips(gpu, to);
}
