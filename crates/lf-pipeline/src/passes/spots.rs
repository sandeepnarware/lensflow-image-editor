//! The web's `spotsPass`: heal and clone spots drawn from the source into a mipmapped target.

use crate::gpu::Gpu;
use crate::passes::UniformArena;
use crate::texture::{Target, generate_mips};
use lf_settings::{MAX_SPOTS, Spot, SpotMode};

/// `SPOTS`'s uniforms, laid out as `spots.wgsl`'s `Uniforms`. `uSpotB` is a `vec2` array in
/// the GLSL; here each entry is padded to a `vec4` (WGSL uniform arrays have a 16-byte stride).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    /// `uSpot`: target x, y, source x, y (UV).
    pub spot: [[f32; 4]; MAX_SPOTS],
    /// `uSpotB` in `xy`: radius (fraction of the width), mode (0 heal, 1 clone).
    pub spot_b: [[f32; 4]; MAX_SPOTS],
    /// `uSize`: photo size in pixels.
    pub size: [f32; 2],
    /// `uCount`.
    pub count: u32,
    pub _pad: u32,
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 1552);
const _: () = assert!(std::mem::offset_of!(Uniforms, spot) == 0);
const _: () = assert!(std::mem::offset_of!(Uniforms, spot_b) == 768);
const _: () = assert!(std::mem::offset_of!(Uniforms, size) == 1536);
const _: () = assert!(std::mem::offset_of!(Uniforms, count) == 1544);

impl Uniforms {
    /// `spotsPass`'s uniforms: the first [`MAX_SPOTS`] spots of a `width`×`height` photo.
    pub fn new(spots: &[Spot], width: u32, height: u32) -> Uniforms {
        let spots = &spots[..spots.len().min(MAX_SPOTS)];
        let mut u = Uniforms {
            spot: [[0.0; 4]; MAX_SPOTS],
            spot_b: [[0.0; 4]; MAX_SPOTS],
            size: [width as f32, height as f32],
            count: spots.len() as u32,
            _pad: 0,
        };
        for (i, p) in spots.iter().enumerate() {
            u.spot[i] = [p.x as f32, p.y as f32, p.sx as f32, p.sy as f32];
            let clone = if p.mode == SpotMode::Clone { 1.0 } else { 0.0 };
            u.spot_b[i] = [p.r as f32, clone, 0.0, 0.0];
        }
        u
    }
}

/// Draws `spots` over `source` into level 0 of `target` (the photo's size), then rebuilds
/// `target`'s mips.
pub fn run(gpu: &Gpu, source: &Target, target: &Target, spots: &[Spot], width: u32, height: u32) {
    let uniforms = Uniforms::new(spots, width, height);
    let arena = UniformArena::new(gpu, "spots uniforms", std::slice::from_ref(&uniforms));
    let pass = &gpu.pipelines.spots;
    let bind = pass.bind(
        gpu,
        &source.view,
        gpu.samplers.for_target(source),
        Some(arena.binding()),
    );
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("spots"),
        });
    pass.draw(&mut encoder, &target.level_view(0), &bind, arena.offsets[0]);
    gpu.queue.submit([encoder.finish()]);
    generate_mips(gpu, target);
}
