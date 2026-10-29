//! The web's `developPass`: every adjustment, geometry and effects, straight to the output.

use crate::gpu::Gpu;
use crate::passes::{FullscreenPass, UniformArena, sampler_entry, texture_entry, uniform_entry};
use crate::texture::Target;
use lf_settings::EditSettings;
use lf_settings::geometry::Affine;
use lf_settings::masks::{MAX_MASKS, Mask, Masks, Range, Shape};
use lf_settings::shader_values::{shader_values, wb_gains};

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
    /// Desktop only, the preview's overlays: bit 1 paints clipped highlights red and blocked
    /// shadows blue, bit 2 sharp edges green (focus peaking), bit 4 shows the photo as edges
    /// (visualize spots).
    pub clip_warn: u32,
    /// One output pixel in canvas UV (`1 / target size`): the derivatives `texture()` would
    /// take implicitly (WGSL only; see `develop.wgsl`).
    pub pixel: [f32; 2],
    /// `uHsl`: hue, saturation, luminance per band (`w` unused).
    pub hsl: [[f32; 4]; 8],
    /// `uBandHue` in `x`.
    pub band_hue: [[f32; 4]; 8],
    /// `uBandWidth` in `x`.
    pub band_width: [[f32; 4]; 8],
    /// Desktop only: how many of `masks` are in use (0: the web's pass exactly).
    pub mask_count: u32,
    /// The working photo's width per unit of height (masks work in a square space).
    pub aspect: f32,
    /// Desktop only: 1 when a lens correction is on (0: the web's pass exactly).
    pub lens_on: u32,
    /// Desktop only: how much of the 3D LUT look to mix in, 0..1 (0: none, the web's pass).
    pub lut_amount: f32,
    pub masks: [MaskUniform; MAX_MASKS],
    /// Distortion (signed strength), lens vignetting, red and blue scale (`lf_settings::lens`).
    pub lens: [f32; 4],
    /// Vertical and horizontal keystone, the corner's squared radius, unused.
    pub persp: [f32; 4],
    /// Desktop only: colour calibration's primaries matrix, row by row (`w` unused).
    pub calib: [[f32; 4]; 3],
    /// The shadows tint, 1 when calibration is on (0: the web's pass exactly), unused, unused.
    pub calib_tint: [f32; 4],
    /// Desktop only: film negative conversion's base (linear RGB) and 1 when on (0: none).
    pub neg_base: [f32; 4],
    /// Each channel's largest density (`w` unused).
    pub neg_dmax: [f32; 4],
    /// Desktop only: defringe's purple amount (0..1), purple hue, green amount, green hue.
    pub fringe: [f32; 4],
    /// 1 when defringe is on (0: the web's pass exactly), the hues' reach in degrees, unused.
    pub fringe_on: [f32; 4],
    /// Desktop only: how many Point colour points are in use (`x`; 0: the web's pass exactly).
    pub point_n: [f32; 4],
    /// Each point's colour (hue, saturation, brightness, range) and shifts (hue in degrees,
    /// saturation, luminance, unused).
    pub point_a: [[f32; 4]; 4],
    pub point_b: [[f32; 4]; 4],
    /// Desktop only: lens blur's radius at full blur (a share of the height), focus, half the
    /// sharp band, and 1 when on (0: the web's pass exactly).
    pub blur: [f32; 4],
}

/// One local adjustment as `develop.wgsl`'s `MaskU` takes it (seven 16-byte rows).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaskUniform {
    /// kind (0 linear, 1 radial, 2 brush), invert (0 or 1), the radial's inner radius (1 − feather),
    /// strength (the opacity, 0 when switched off).
    pub kind: [f32; 4],
    /// Linear: start x, y and end x, y. Radial: centre x, y and radii x, y. All in the square
    /// space (x × aspect). Brush: its layer in the brush texture.
    pub shape: [f32; 4],
    /// Radial: cos and sin of the angle.
    pub turn: [f32; 4],
    /// Exposure (stops), contrast, highlights, shadows (÷ 100).
    pub tone: [f32; 4],
    /// Whites, blacks, saturation, clarity (÷ 100).
    pub more: [f32; 4],
    /// The white-balance multiplier where the mask applies fully (normalised as the global
    /// one is), and an unused `w`.
    pub wb: [f32; 4],
    /// The range ([`lf_settings::masks::Range`]): 0 all; 1 brightness low, high, smooth (0..1);
    /// 2 colour hue, width, smooth (radians).
    pub range: [f32; 4],
}

impl MaskUniform {
    /// `layer`: where a brush mask's coverage sits in the brush texture (its place in the list).
    fn new(m: &Mask, layer: usize, aspect: f64) -> MaskUniform {
        let p = |x: f64| (x / 100.0) as f32;
        let l = &m.local;
        let g = wb_gains(l.temperature, l.tint);
        let y = 0.2126 * g[0] + 0.7152 * g[1] + 0.0722 * g[2];
        let (kind, shape, turn, inner) = match m.shape {
            Shape::Brush { .. } | Shape::Image { .. } => (
                2.0,
                [layer as f32, 0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
                0.0,
            ),
            Shape::Linear { x0, y0, x1, y1 } => (
                0.0,
                [x0 * aspect, y0, x1 * aspect, y1].map(|v| v as f32),
                [1.0, 0.0, 0.0, 0.0],
                0.0,
            ),
            Shape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
            } => {
                let (s, c) = angle.to_radians().sin_cos();
                (
                    1.0,
                    [cx * aspect, cy, rx, ry].map(|v| v as f32),
                    [c as f32, s as f32, 0.0, 0.0],
                    (1.0 - feather / 100.0) as f32,
                )
            }
        };
        MaskUniform {
            kind: [
                kind,
                f32::from(u8::from(m.invert)),
                inner,
                m.strength() as f32,
            ],
            shape,
            turn,
            tone: [
                l.exposure as f32,
                p(l.contrast),
                p(l.highlights),
                p(l.shadows),
            ],
            more: [p(l.whites), p(l.blacks), p(l.saturation), p(l.clarity)],
            wb: [(g[0] / y) as f32, (g[1] / y) as f32, (g[2] / y) as f32, 0.0],
            range: match m.range {
                Range::All => [0.0; 4],
                Range::Luminance { low, high, smooth } => [1.0, p(low), p(high), p(smooth)],
                Range::Colour { hue, width, smooth } => [
                    2.0,
                    hue.to_radians() as f32,
                    width.to_radians() as f32,
                    smooth.to_radians() as f32,
                ],
            },
        }
    }
}

// The WGSL layout, checked where it is easiest to get wrong: the first vec3, the WGSL-only
// `pixel`, the three 16-byte-stride arrays and the masks after them.
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<MaskUniform>() == 112);
    assert!(size_of::<Uniforms>() == 624 + 16 + MAX_MASKS * 112 + 32 + 64 + 32 + 32 + 144 + 16);
    assert!(offset_of!(Uniforms, wb) == 128);
    assert!(offset_of!(Uniforms, pixel) == 232);
    assert!(offset_of!(Uniforms, hsl) == 240);
    assert!(offset_of!(Uniforms, band_hue) == 240 + 8 * 16);
    assert!(offset_of!(Uniforms, band_width) == 240 + 2 * 8 * 16);
    assert!(offset_of!(Uniforms, mask_count) == 624);
    assert!(offset_of!(Uniforms, masks) == 640);
    assert!(offset_of!(Uniforms, lens) == 640 + MAX_MASKS * 112);
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
        let masks = Masks::from_settings(s);
        let lens = lf_settings::lens::Lens::from_settings(s);
        let calibration = lf_settings::calibration::Calibration::from_settings(s);
        let calib = calibration.matrix();
        let negative = lf_settings::negative::Negative::from_settings(s).filter(|_| f.develop);
        let defringe = lf_settings::defringe::Defringe::from_settings(s);
        let defringe_on = f.develop && !defringe.is_off();
        let points = if f.develop {
            lf_settings::point_colour::Points::from_settings(s).active()
        } else {
            Vec::new()
        };
        let mut point_a = [[0.0f32; 4]; 4];
        let mut point_b = [[0.0f32; 4]; 4];
        for (i, p) in points.iter().take(4).enumerate() {
            [point_a[i], point_b[i]] = p.uniform();
        }
        let aspect = f64::from(f.img_size[0]) / f64::from(f.img_size[1].max(1));
        let mut mask_uniforms = [MaskUniform::default(); MAX_MASKS];
        for (layer, (slot, mask)) in mask_uniforms.iter_mut().zip(&masks.0).enumerate() {
            *slot = MaskUniform::new(mask, layer, aspect);
        }
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
            clip_warn: 0,
            pixel: [1.0 / f.target[0] as f32, 1.0 / f.target[1] as f32],
            hsl,
            band_hue,
            band_width,
            mask_count: masks.0.len() as u32,
            aspect: aspect as f32,
            lens_on: u32::from(f.develop && !lens.is_none()),
            lut_amount: lf_settings::lut::LutChoice::from_settings(s)
                .filter(|_| f.develop)
                .map_or(0.0, |c| (c.amount / 100.0) as f32),
            masks: mask_uniforms,
            lens: [
                (-lens.distortion / 100.0 * lf_settings::lens::DISTORTION) as f32,
                (lens.vignetting / 100.0 * lf_settings::lens::VIGNETTING) as f32,
                (lens.ca_red / 100.0 * lf_settings::lens::CA) as f32,
                (lens.ca_blue / 100.0 * lf_settings::lens::CA) as f32,
            ],
            persp: [
                (lens.vertical / 100.0 * lf_settings::lens::KEYSTONE) as f32,
                (lens.horizontal / 100.0 * lf_settings::lens::KEYSTONE) as f32,
                ((aspect * aspect + 1.0) / 4.0) as f32,
                0.0,
            ],
            calib: calib.map(|r| [r[0], r[1], r[2], 0.0]),
            calib_tint: [
                calibration.tint(),
                f32::from(u8::from(f.develop && !calibration.is_neutral())),
                0.0,
                0.0,
            ],
            neg_base: negative.map_or([0.0; 4], |n| {
                [n.base[0] as f32, n.base[1] as f32, n.base[2] as f32, 1.0]
            }),
            neg_dmax: negative.map_or([1.0; 4], |n| {
                [n.dmax[0] as f32, n.dmax[1] as f32, n.dmax[2] as f32, 1.0]
            }),
            blur: lf_settings::lens_blur::LensBlur::from_settings(s)
                .filter(|_| f.develop)
                .map_or([0.0; 4], |b| b.uniform()),
            point_n: [points.len().min(4) as f32, 0.0, 0.0, 0.0],
            point_a,
            point_b,
            fringe: defringe.uniform(),
            fringe_on: [
                f32::from(u8::from(defringe_on)),
                lf_settings::defringe::WIDTH as f32,
                0.0,
                0.0,
            ],
        }
    }
}

/// The develop pipeline: six textures (bindings 0–5), a sampler for each (6–11), the
/// uniforms (12), and (desktop only) the brush masks' coverage and its sampler (13, 14) and the
/// 3D LUT and its sampler (15, 16).
pub(crate) fn pipeline(device: &wgpu::Device, source: &str) -> FullscreenPass {
    let mut entries: Vec<_> = (0..6).map(texture_entry).collect();
    entries.extend((6..12).map(sampler_entry));
    entries.push(uniform_entry(12));
    entries.push(texture_entry(13));
    entries.push(sampler_entry(14));
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 15,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    });
    entries.push(sampler_entry(16));
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
    /// Desktop only: the brush masks' coverage, one layer per mask stacked top to bottom
    /// ([`crate::brush`]); a 1×1 stand-in when no mask is a brush.
    pub brush: &'a Target,
    /// Desktop only: the 3D LUT (a stand-in when the photo has none).
    pub lut: &'a wgpu::TextureView,
}

/// A one-entry arena for the develop uniforms, to pass to [`run`] draw after draw.
pub fn arena(gpu: &Gpu) -> UniformArena {
    let zero: Uniforms = bytemuck::Zeroable::zeroed();
    UniformArena::new(gpu, "develop uniforms", &[zero])
}

/// Draws the develop pass into level 0 of `dst`, its uniforms written into `arena` (reused,
/// see [`arena`]).
pub fn run(
    gpu: &Gpu,
    inputs: &Inputs,
    uniforms: &Uniforms,
    arena: &mut UniformArena,
    dst: &Target,
) {
    let pass = &gpu.pipelines.develop;
    arena.write(gpu, "develop uniforms", std::slice::from_ref(uniforms));
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
    entries.push(wgpu::BindGroupEntry {
        binding: 13,
        resource: wgpu::BindingResource::TextureView(&inputs.brush.view),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 14,
        resource: wgpu::BindingResource::Sampler(&gpu.samplers.linear_level0),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 15,
        resource: wgpu::BindingResource::TextureView(inputs.lut),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 16,
        resource: wgpu::BindingResource::Sampler(&gpu.samplers.linear_level0),
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
