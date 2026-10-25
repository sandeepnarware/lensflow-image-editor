//! The web's `developPass`: every adjustment, geometry and effects, straight to the output.

use crate::gpu::Gpu;
use crate::passes::{FullscreenPass, UniformArena, sampler_entry, texture_entry, uniform_entry};
use crate::texture::Target;
use lf_settings::EditSettings;
use lf_settings::geometry::Affine;
use lf_settings::shader_values::shader_values;

/// `DEVELOP`'s uniforms, laid out as `develop.wgsl`'s `Uniforms` (WGSL uniform layout: each
/// `vec3` followed by a scalar fills a 16-byte row; arrays have a 16-byte stride). Field `x`
/// is the GLSL's `uX`; values carry `developPass`'s unit conversions.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    /// Output UV → photo UV: three columns (the web's column-major upload), each padded to 16 bytes.
    pub matrix: [[f32; 4]; 3],
    pub view: [f32; 4],
    pub img_size: [f32; 2],
    pub out_size: [f32; 2],
    pub scale: f32,
    pub flip_y: u32,
    pub develop: u32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub grade_balance: f32,
    pub wb: [f32; 3],
    pub clarity: f32,
    pub grade_sh: [f32; 3],
    /// `uTexture` (renamed: `texture` reads as a type in WGSL).
    pub texture_amount: f32,
    pub grade_mid: [f32; 3],
    pub dehaze: f32,
    pub grade_hi: [f32; 3],
    pub sharpen: f32,
    pub noise: f32,
    pub color_noise: f32,
    pub vignette: f32,
    pub vignette_mid: f32,
    pub grain: f32,
    pub grain_size: f32,
    pub bw: u32,
    pub sharp_on: u32,
    pub tex_on: u32,
    pub _pad: u32,
    /// One output pixel in canvas UV (`1 / target size`): the derivatives `texture()` would
    /// take implicitly (WGSL only; see `develop.wgsl`).
    pub pixel: [f32; 2],
    /// `uHsl`: hue, saturation, luminance per band (`w` unused).
    pub hsl: [[f32; 4]; 8],
    /// `uBandHue` in `x`.
    pub band_hue: [[f32; 4]; 8],
    /// `uBandWidth` in `x`.
    pub band_width: [[f32; 4]; 8],
}

// The WGSL layout, checked where it is easiest to get wrong: the first vec3, the WGSL-only
// `pixel` and the three 16-byte-stride arrays.
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<Uniforms>() == 624);
    assert!(offset_of!(Uniforms, wb) == 128);
    assert!(offset_of!(Uniforms, pixel) == 232);
    assert!(offset_of!(Uniforms, hsl) == 240);
    assert!(offset_of!(Uniforms, band_hue) == 240 + 8 * 16);
    assert!(offset_of!(Uniforms, band_width) == 240 + 2 * 8 * 16);
};

/// Where the develop pass draws from and how (the arguments of `developPass` that are not
/// settings).
pub struct Frame {
    /// Output UV → photo UV (`outputToSource`).
    pub matrix: Affine,
    /// Output size at full resolution, unrounded.
    pub out: [f64; 2],
    /// The part of the output shown: x, y, w, h.
    pub view: [f64; 4],
    pub develop: bool,
    pub flip: bool,
    /// Working photo size.
    pub img_size: [u32; 2],
    /// Working size ÷ full size.
    pub scale: f64,
    /// Size of the target drawn into (the canvas).
    pub target: [u32; 2],
}

impl Uniforms {
    /// `developPass`'s uniforms. `sharp_ready`/`texture_ready`: the detail blur targets exist.
    pub fn new(s: &EditSettings, f: &Frame, sharp_ready: bool, texture_ready: bool) -> Uniforms {
        let m = f.matrix.map(|v| v as f32);
        let v = shader_values(s);
        let p = |x: f64| (x / 100.0) as f32;
        let mut hsl = [[0.0f32; 4]; 8];
        let mut band_hue = [[0.0f32; 4]; 8];
        let mut band_width = [[0.0f32; 4]; 8];
        for i in 0..8 {
            hsl[i] = [v.hsl[i * 3], v.hsl[i * 3 + 1], v.hsl[i * 3 + 2], 0.0];
            band_hue[i][0] = v.band_hue[i];
            band_width[i][0] = v.band_width[i];
        }
        Uniforms {
            matrix: [
                [m[0], m[3], 0.0, 0.0],
                [m[1], m[4], 0.0, 0.0],
                [m[2], m[5], 1.0, 0.0],
            ],
            view: f.view.map(|x| x as f32),
            img_size: f.img_size.map(|x| x as f32),
            out_size: f.out.map(|x| x as f32),
            scale: f.scale as f32,
            flip_y: f.flip as u32,
            develop: f.develop as u32,
            exposure: s.exposure as f32,
            contrast: p(s.contrast),
            highlights: p(s.highlights),
            shadows: p(s.shadows),
            whites: p(s.whites),
            blacks: p(s.blacks),
            vibrance: p(s.vibrance),
            saturation: p(s.saturation),
            grade_balance: p(s.grading.balance),
            wb: v.wb,
            clarity: p(s.clarity),
            grade_sh: v.grade_sh,
            texture_amount: p(s.texture),
            grade_mid: v.grade_mid,
            dehaze: p(s.dehaze),
            grade_hi: v.grade_hi,
            sharpen: p(s.sharpen),
            noise: p(s.noise),
            color_noise: p(s.color_noise),
            vignette: p(s.vignette),
            vignette_mid: p(s.vignette_midpoint),
            grain: p(s.grain),
            grain_size: p(s.grain_size),
            bw: s.bw as u32,
            sharp_on: (s.sharpen > 0.0 && sharp_ready) as u32,
            tex_on: (s.texture != 0.0 && texture_ready) as u32,
            _pad: 0,
            pixel: [1.0 / f.target[0] as f32, 1.0 / f.target[1] as f32],
            hsl,
            band_hue,
            band_width,
        }
    }
}

/// The develop pipeline: six textures (bindings 0–5), a sampler for each (6–11) and the
/// uniforms (12).
pub(crate) fn pipeline(device: &wgpu::Device, source: &str) -> FullscreenPass {
    let mut entries: Vec<_> = (0..6).map(texture_entry).collect();
    entries.extend((6..12).map(sampler_entry));
    entries.push(uniform_entry(12));
    FullscreenPass::with_entries(device, "develop", source, "fs_develop", &entries)
}

/// The textures the develop pass reads, each sampled as WebGL would sample it.
pub struct Inputs<'a> {
    pub image: &'a Target,
    pub blur_m: &'a Target,
    pub blur_l: &'a Target,
    pub blur_s: &'a Target,
    pub blur_t: &'a Target,
    /// The 256×1 curve texture (linear clamp sampler).
    pub curve: &'a Target,
}

/// Draws the develop pass into level 0 of `dst`.
pub fn run(gpu: &Gpu, inputs: &Inputs, uniforms: &Uniforms, dst: &Target) {
    let pass = &gpu.pipelines.develop;
    let arena = UniformArena::new(gpu, "develop uniforms", std::slice::from_ref(uniforms));
    let textures = [
        inputs.image,
        inputs.blur_m,
        inputs.blur_l,
        inputs.blur_s,
        inputs.blur_t,
        inputs.curve,
    ];
    let mut entries: Vec<wgpu::BindGroupEntry> = textures
        .iter()
        .enumerate()
        .map(|(i, t)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(&t.view),
        })
        .collect();
    entries.extend(textures.iter().enumerate().map(|(i, t)| {
        let sampler = if i == 5 {
            &gpu.samplers.linear_clamp
        } else {
            gpu.samplers.for_target(t)
        };
        wgpu::BindGroupEntry {
            binding: 6 + i as u32,
            resource: wgpu::BindingResource::Sampler(sampler),
        }
    }));
    let (buffer, size) = arena.binding();
    entries.push(wgpu::BindGroupEntry {
        binding: 12,
        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer,
            offset: 0,
            size: wgpu::BufferSize::new(size),
        }),
    });
    let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("develop"),
        layout: pass.layout(),
        entries: &entries,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("develop"),
        });
    pass.draw(
        &mut encoder,
        &dst.level_view(0),
        dst.format,
        &bind,
        arena.offsets[0],
    );
    gpu.queue.submit([encoder.finish()]);
}
