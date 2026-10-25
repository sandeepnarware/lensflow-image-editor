//! The web's `blur(from, to, tmp, sigma)`: the separable `GAUSSIAN` across into `tmp`,
//! then down into `to`, then `to`'s mips.

use crate::gpu::Gpu;
use crate::texture::{Target, generate_mips};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    step: [f32; 2],
    sigma: f32,
    _pad: f32,
}

const PARAMS_SIZE: u64 = std::mem::size_of::<Params>() as u64;

/// Blurs `from` into `to` through `tmp`; `sigma` is in texels of `to`.
pub fn blur(gpu: &Gpu, from: &Target, to: &Target, tmp: &Target, sigma: f32) {
    // Both runs' uniforms live in one buffer, one aligned slot each.
    let slot = (gpu.limits.min_uniform_buffer_offset_alignment as u64).max(PARAMS_SIZE);
    let runs = [(from, tmp, 1.0, 0.0), (tmp, to, 0.0, 1.0)];
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gaussian params"),
        size: slot * runs.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let pass = &gpu.pipelines.gaussian;
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gaussian"),
        });
    for (i, (src, dst, dx, dy)) in runs.into_iter().enumerate() {
        let params = Params {
            step: [dx / dst.width as f32, dy / dst.height as f32],
            sigma,
            _pad: 0.0,
        };
        let offset = slot * i as u64;
        gpu.queue
            .write_buffer(&buffer, offset, bytemuck::bytes_of(&params));
        let bind = pass.bind(
            gpu,
            &src.view,
            gpu.samplers.for_target(src),
            Some((&buffer, PARAMS_SIZE)),
        );
        pass.draw(&mut encoder, &dst.level_view(0), &bind, offset as u32);
    }
    gpu.queue.submit([encoder.finish()]);
    generate_mips(gpu, to);
}
