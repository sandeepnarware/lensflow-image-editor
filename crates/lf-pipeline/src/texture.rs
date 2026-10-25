//! Render targets, uploads, mip chains and readback.
//!
//! Mirrors the web renderer's `texture()`/`target()`: `RGBA8` (linear, not sRGB), clamp to
//! edge, linear filtering, and trilinear (`LINEAR_MIPMAP_LINEAR`) for textures with mips.
//! Row 0 of every texture is the image's top.

use crate::error::PipelineError;
use crate::gpu::Gpu;
use image::RgbaImage;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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
}

/// Number of levels in a full chain: sizes `max(1, floor(prev / 2))` down to 1×1.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

impl Target {
    pub fn new(gpu: &Gpu, width: u32, height: u32, mips: bool) -> Result<Target, PipelineError> {
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
            format: FORMAT,
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
    /// Refuses sizes the device cannot hold, without allocating anything.
    pub fn upload_size_check(gpu: &Gpu, width: u32, height: u32) -> Result<(), PipelineError> {
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
        pass.draw(&mut encoder, &dst, &bind, 0);
    }
    gpu.queue.submit([encoder.finish()]);
}

/// Level 0 of `t` as an image (row 0 = top).
pub fn read_back(gpu: &Gpu, t: &Target) -> Result<RgbaImage, PipelineError> {
    read_back_level(gpu, t, 0)
}

/// One mip level of `t` as an image. Copies through a buffer whose rows are padded to
/// `COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes), then strips the padding.
pub fn read_back_level(gpu: &Gpu, t: &Target, level: u32) -> Result<RgbaImage, PipelineError> {
    if level >= t.level_count() {
        return Err(PipelineError::Readback(format!(
            "mip level {level} does not exist (the texture has {})",
            t.level_count()
        )));
    }
    let width = (t.width >> level).max(1);
    let height = (t.height >> level).max(1);
    let row = 4 * width;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = row.div_ceil(align) * align;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read_back"),
        size: padded as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("read_back"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &t.texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = sender.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| PipelineError::Readback(e.to_string()))?;
    receiver
        .recv()
        .map_err(|e| PipelineError::Readback(e.to_string()))?
        .map_err(|e| PipelineError::Readback(e.to_string()))?;

    let mut pixels = Vec::with_capacity((row * height) as usize);
    {
        let data = buffer
            .get_mapped_range(..)
            .map_err(|e| PipelineError::Readback(e.to_string()))?;
        for r in data.chunks_exact(padded as usize) {
            pixels.extend_from_slice(&r[..row as usize]);
        }
    }
    buffer.unmap();
    buffer.destroy();
    RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| PipelineError::Readback("the copied buffer was short".into()))
}
