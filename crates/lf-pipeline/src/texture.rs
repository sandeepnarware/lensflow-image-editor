//! Render targets, uploads, mip chains and readback.
//!
//! Mirrors the web renderer's `texture()`/`target()`: clamp to edge, linear filtering, and
//! trilinear (`LINEAR_MIPMAP_LINEAR`) for textures with mips. The web's textures are `RGBA8`
//! (linear, not sRGB), as are 8-bit photos' here ([`FORMAT`]); a float photo's chain is half
//! float ([`FLOAT_FORMAT`]) so values above 1.0 survive to the develop pass. Outputs are
//! 8-bit.
//! Row 0 of every texture is the image's top.

use crate::error::PipelineError;
use crate::gpu::Gpu;
use half::f16;
use image::RgbaImage;

/// The 8-bit format: 8-bit photos and their intermediates, outputs, the curve texture.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// A float photo's format: its source with mips and every intermediate (filterable in core
/// WebGPU, no device features needed).
pub const FLOAT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// A half-float RGBA picture (row 0 = top), e.g. a developed RAW: values may exceed 1.0.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatFrame {
    pub width: u32,
    pub height: u32,
    /// `4 * width * height` values, RGBA.
    pub data: Vec<f16>,
}

/// The samplers the passes use.
pub struct Samplers {
    /// WebGL `LINEAR_MIPMAP_LINEAR` / `LINEAR` / `CLAMP_TO_EDGE`: for textures with mips.
    pub linear_clamp: wgpu::Sampler,
    /// WebGL `LINEAR` min filter / `LINEAR` / `CLAMP_TO_EDGE`: bilinear within level 0, for
    /// single-level targets (mipmap filter nearest, lod clamped to 0; not point sampling).
    pub linear_level0: wgpu::Sampler,
}

impl Samplers {
    pub fn new(device: &wgpu::Device) -> Samplers {
        let base = wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 1,
            ..Default::default()
        };
        Samplers {
            // lod_max_clamp stays at the default (32): each view's own level count bounds it.
            linear_clamp: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("linear_clamp"),
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..base.clone()
            }),
            linear_level0: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("linear_level0"),
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                lod_max_clamp: 0.0,
                ..base
            }),
        }
    }

    /// The sampler WebGL would use for this target's texture.
    pub fn for_target(&self, t: &Target) -> &wgpu::Sampler {
        if t.mips {
            &self.linear_clamp
        } else {
            &self.linear_level0
        }
    }
}

/// A texture that passes read from and draw into.
pub struct Target {
    pub texture: wgpu::Texture,
    /// All mip levels.
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub mips: bool,
    pub format: wgpu::TextureFormat,
}

/// Number of levels in a full chain: sizes `max(1, floor(prev / 2))` down to 1×1.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

impl Target {
    /// A `width`×`height` [`FORMAT`] target (with a full mip chain when `mips`). A zero side
    /// is [`PipelineError::EmptyImage`]; a side over the device's limit is `TooLarge`.
    pub fn new(gpu: &Gpu, width: u32, height: u32, mips: bool) -> Result<Target, PipelineError> {
        Target::new_with(gpu, width, height, mips, FORMAT)
    }

    /// [`Target::new`] in `format` (a colour format the passes can render into).
    pub fn new_with(
        gpu: &Gpu,
        width: u32,
        height: u32,
        mips: bool,
        format: wgpu::TextureFormat,
    ) -> Result<Target, PipelineError> {
        Source::upload_size_check(gpu, width, height)?;
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: if mips {
                mip_level_count(width, height)
            } else {
                1
            },
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if pollster::block_on(scope.pop()).is_some() {
            return Err(PipelineError::OutOfMemory);
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Target {
            texture,
            view,
            width,
            height,
            mips,
            format,
        })
    }

    /// A view of one mip level.
    pub fn level_view(&self, level: u32) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        })
    }

    pub fn level_count(&self) -> u32 {
        self.texture.mip_level_count()
    }
}

/// The photo, uploaded as the web does (`texImage2D` RGBA8, no premultiply, no flip).
pub struct Source;

impl Source {
    /// Refuses empty images and sizes the device cannot hold, without allocating anything.
    pub fn upload_size_check(gpu: &Gpu, width: u32, height: u32) -> Result<(), PipelineError> {
        if width == 0 || height == 0 {
            return Err(PipelineError::EmptyImage);
        }
        let limit = gpu.max_edge();
        if width > limit || height > limit {
            return Err(PipelineError::TooLarge {
                width,
                height,
                limit,
            });
        }
        Ok(())
    }

    /// Uploads `image` (row 0 = top) into a new target, building its mip chain when `mips`.
    pub fn upload(gpu: &Gpu, image: &RgbaImage, mips: bool) -> Result<Target, PipelineError> {
        let (width, height) = image.dimensions();
        let target = Target::new(gpu, width, height, mips)?;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        generate_mips(gpu, &target);
        Ok(target)
    }

    /// Uploads `frame` (row 0 = top) into a new [`FLOAT_FORMAT`] target, building its mip
    /// chain when `mips`. A frame whose data is not `4 * width * height` values is refused.
    pub fn upload_f16(gpu: &Gpu, frame: &FloatFrame, mips: bool) -> Result<Target, PipelineError> {
        let (width, height) = (frame.width, frame.height);
        Source::upload_size_check(gpu, width, height)?;
        let len = 4 * width as usize * height as usize;
        if frame.data.len() != len {
            return Err(PipelineError::BadFrame {
                width,
                height,
                expected: len,
                got: frame.data.len(),
            });
        }
        let target = Target::new_with(gpu, width, height, mips, FLOAT_FORMAT)?;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&frame.data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        generate_mips(gpu, &target);
        Ok(target)
    }
}

/// Rebuilds levels 1.. from level 0 (no-op without mips). Each level is one render pass
/// that samples the previous level with the linear clamp sampler at its own texel centres
/// `((x + 0.5) / w, (y + 0.5) / h)`: a 2×2 box average for even sizes, and our model of
/// SwiftShader's `generateMipmap` for odd ones.
pub fn generate_mips(gpu: &Gpu, t: &Target) {
    let levels = t.level_count();
    if !t.mips || levels < 2 {
        return;
    }
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mips"),
        });
    let pass = &gpu.pipelines.mip;
    for level in 1..levels {
        let src = t.level_view(level - 1);
        let dst = t.level_view(level);
        let bind = pass.bind(gpu, &src, &gpu.samplers.linear_clamp, None);
        pass.draw(&mut encoder, &dst, t.format, &bind, 0);
    }
    gpu.queue.submit([encoder.finish()]);
}

/// Level 0 of `t` as an image (row 0 = top).
pub fn read_back(gpu: &Gpu, t: &Target) -> Result<RgbaImage, PipelineError> {
    read_back_level(gpu, t, 0)
}

/// One mip level of an 8-bit target (`Rgba8Unorm`, or `Bgra8Unorm` returned swizzled to RGBA)
/// as an image. A float target is refused (see [`read_back_f16`]).
pub fn read_back_level(gpu: &Gpu, t: &Target, level: u32) -> Result<RgbaImage, PipelineError> {
    let bgra = match t.format {
        wgpu::TextureFormat::Rgba8Unorm => false,
        wgpu::TextureFormat::Bgra8Unorm => true,
        f => {
            return Err(PipelineError::Readback(format!(
                "read_back takes an 8-bit texture, not {f:?}"
            )));
        }
    };
    let (width, height, mut pixels) = read_back_raw(gpu, t, level, 4, None)?;
    if bgra {
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
    }
    RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| PipelineError::Readback("the copied buffer was short".into()))
}

/// Level 0 of a [`FLOAT_FORMAT`] target as a frame (row 0 = top). Any other format is refused
/// (see [`read_back`]).
pub fn read_back_f16(gpu: &Gpu, t: &Target) -> Result<FloatFrame, PipelineError> {
    if t.format != FLOAT_FORMAT {
        return Err(PipelineError::Readback(format!(
            "read_back_f16 takes an Rgba16Float texture, not {:?}",
            t.format
        )));
    }
    let (width, height, bytes) = read_back_raw(gpu, t, 0, 8, None)?;
    let data = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| f16::from_le_bytes(b))
        .collect();
    Ok(FloatFrame {
        width,
        height,
        data,
    })
}

/// The largest staging buffer one readback band uses, whatever the device allows.
pub const MAX_BAND_BYTES: u64 = 256 << 20;

/// Bytes per texel of the formats readback takes.
fn texel_size(format: wgpu::TextureFormat) -> Result<u32, PipelineError> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm => Ok(4),
        FLOAT_FORMAT => Ok(8),
        f => Err(PipelineError::Readback(format!("cannot read back {f:?}"))),
    }
}

/// Rows per readback band for a level `padded` bytes per (aligned) row: as many as fit in
/// `min(max_buffer_size, MAX_BAND_BYTES)`, at least one.
fn band_rows(gpu: &Gpu, padded: u64, height: u32) -> u32 {
    let cap = gpu.limits.max_buffer_size.min(MAX_BAND_BYTES);
    let rows = (cap / padded).clamp(1, u64::from(height));
    rows as u32
}

/// The raw bytes of one mip level of `t` (top row first, tightly packed), copied in bands of
/// at most `band_rows` rows. Exposed so tests can force small bands; use [`read_back`] or
/// [`read_back_f16`].
#[doc(hidden)]
pub fn read_back_with_band_rows(
    gpu: &Gpu,
    t: &Target,
    level: u32,
    band_rows: u32,
) -> Result<(u32, u32, Vec<u8>), PipelineError> {
    let texel = texel_size(t.format)?;
    read_back_raw(gpu, t, level, texel, Some(band_rows))
}

/// The bytes of one mip level of `t` (`texel` bytes per texel), top row first. Copies in
/// horizontal bands (`band_rows`, or as many rows as [`band_rows`] allows) through one
/// staging buffer whose rows are padded to `COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes), so no
/// buffer exceeds the device's `max_buffer_size`; strips the padding. Sizes are `u64`.
fn read_back_raw(
    gpu: &Gpu,
    t: &Target,
    level: u32,
    texel: u32,
    band: Option<u32>,
) -> Result<(u32, u32, Vec<u8>), PipelineError> {
    if level >= t.level_count() {
        return Err(PipelineError::Readback(format!(
            "mip level {level} does not exist (the texture has {})",
            t.level_count()
        )));
    }
    let width = (t.width >> level).max(1);
    let height = (t.height >> level).max(1);
    let row = u64::from(texel) * u64::from(width);
    let align = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let padded = row.div_ceil(align) * align;
    let padded_u32 = u32::try_from(padded)
        .map_err(|_| PipelineError::Readback(format!("a {width}-texel row is too wide")))?;
    let rows = band
        .unwrap_or_else(|| band_rows(gpu, padded, height))
        .clamp(1, height);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read_back"),
        size: padded * u64::from(rows),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let total = usize::try_from(row * u64::from(height))
        .map_err(|_| PipelineError::Readback("the image does not fit in memory".into()))?;
    let mut pixels = Vec::with_capacity(total);
    let result = (|| {
        let mut y = 0;
        while y < height {
            let h = rows.min(height - y);
            copy_band(gpu, t, level, &buffer, padded_u32, y, width, h)?;
            {
                let len = padded * u64::from(h);
                let data = buffer
                    .get_mapped_range(..len)
                    .map_err(|e| PipelineError::Readback(e.to_string()))?;
                for r in data.chunks_exact(padded as usize) {
                    pixels.extend_from_slice(&r[..row as usize]);
                }
            }
            buffer.unmap();
            y += h;
        }
        Ok(())
    })();
    buffer.destroy();
    result.map(|()| (width, height, pixels))
}

/// Copies rows `y..y + h` of level `level` into the start of `buffer` and maps it.
#[allow(clippy::too_many_arguments)]
fn copy_band(
    gpu: &Gpu,
    t: &Target,
    level: u32,
    buffer: &wgpu::Buffer,
    padded: u32,
    y: u32,
    width: u32,
    h: u32,
) -> Result<(), PipelineError> {
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("read_back"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &t.texture,
            mip_level: level,
            origin: wgpu::Origin3d { x: 0, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.map_async(
        wgpu::MapMode::Read,
        ..u64::from(padded) * u64::from(h),
        move |r| {
            let _ = sender.send(r);
        },
    );
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| PipelineError::Readback(e.to_string()))?;
    receiver
        .recv()
        .map_err(|e| PipelineError::Readback(e.to_string()))?
        .map_err(|e| PipelineError::Readback(e.to_string()))
}
