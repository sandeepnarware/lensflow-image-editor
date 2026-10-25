//! Fullscreen passes ported from the web editor's shaders.

pub mod copy;
pub mod gaussian;

use crate::gpu::Gpu;
use crate::texture::FORMAT;

const FULLSCREEN: &str = include_str!("../shaders/fullscreen.wgsl");
const COPY: &str = concat!(
    include_str!("../shaders/fullscreen.wgsl"),
    include_str!("../shaders/copy.wgsl")
);
const GAUSSIAN: &str = concat!(
    include_str!("../shaders/fullscreen.wgsl"),
    include_str!("../shaders/gaussian.wgsl")
);

/// Every WGSL module the pipeline builds (each fragment file is prefixed with
/// `fullscreen.wgsl`), by name.
pub const SHADERS: &[(&str, &str)] = &[
    ("fullscreen.wgsl", FULLSCREEN),
    ("copy.wgsl", COPY),
    ("gaussian.wgsl", GAUSSIAN),
];

/// One pipeline drawing the fullscreen triangle `[-1,-1, 3,-1, -1,3]` into an `RGBA8`
/// target, with a texture (binding 0), a sampler (binding 1) and optional uniforms
/// (binding 2, a slice of a shared buffer chosen by a dynamic offset).
pub struct FullscreenPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: bool,
}

impl FullscreenPass {
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        source: &str,
        entry: &str,
        uniforms: bool,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        if uniforms {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        FullscreenPass {
            pipeline,
            layout,
            uniforms,
        }
    }

    /// The bind group for one draw.
    pub fn bind(
        &self,
        gpu: &Gpu,
        texture: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniforms: Option<(&wgpu::Buffer, u64)>,
    ) -> wgpu::BindGroup {
        assert_eq!(self.uniforms, uniforms.is_some(), "uniform buffer mismatch");
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(texture),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ];
        if let Some((buffer, size)) = uniforms {
            entries.push(wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size),
                }),
            });
        }
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &entries,
        })
    }

    /// Records one draw covering `target` (the whole view; the viewport is the view's size).
    /// `offset` picks the uniforms' slice (0 for passes without uniforms).
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        bind: &wgpu::BindGroup,
        offset: u32,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        if self.uniforms {
            pass.set_bind_group(0, bind, &[offset]);
        } else {
            pass.set_bind_group(0, bind, &[]);
        }
        pass.draw(0..3, 0..1);
    }
}

/// Byte offsets of `count` uniform entries of `entry_size` bytes packed into one buffer,
/// each slot rounded up to a multiple of `alignment` (`min_uniform_buffer_offset_alignment`).
pub fn uniform_slots(entry_size: u64, alignment: u64, count: usize) -> Vec<u64> {
    let slot = entry_size.max(1).div_ceil(alignment) * alignment;
    (0..count as u64).map(|i| i * slot).collect()
}

/// One uniform buffer holding a value per draw, each at an aligned slot, for passes that
/// bind their uniforms with a dynamic offset (see [`FullscreenPass::draw`]).
pub struct UniformArena {
    pub buffer: wgpu::Buffer,
    /// Size of one entry, the binding size.
    pub entry_size: u64,
    /// The dynamic offset of each entry, in order.
    pub offsets: Vec<u32>,
}

impl UniformArena {
    pub fn new<T: bytemuck::Pod>(gpu: &Gpu, label: &str, entries: &[T]) -> UniformArena {
        assert!(
            !entries.is_empty(),
            "a uniform arena needs at least one entry"
        );
        let entry_size = std::mem::size_of::<T>() as u64;
        let alignment = gpu.limits.min_uniform_buffer_offset_alignment as u64;
        let slots = uniform_slots(entry_size, alignment, entries.len());
        let size = slots[slots.len() - 1] + entry_size;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for (entry, &offset) in entries.iter().zip(&slots) {
            gpu.queue
                .write_buffer(&buffer, offset, bytemuck::bytes_of(entry));
        }
        UniformArena {
            buffer,
            entry_size,
            offsets: slots
                .into_iter()
                .map(|o| u32::try_from(o).expect("uniform offset fits in u32"))
                .collect(),
        }
    }

    /// The `(buffer, binding size)` pair [`FullscreenPass::bind`] takes.
    pub fn binding(&self) -> (&wgpu::Buffer, u64) {
        (&self.buffer, self.entry_size)
    }
}

/// The pipelines, built once per device.
pub(crate) struct Pipelines {
    pub copy: FullscreenPass,
    pub mip: FullscreenPass,
    pub gaussian: FullscreenPass,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Pipelines {
        Pipelines {
            copy: FullscreenPass::new(device, "copy", COPY, "fs_copy", false),
            mip: FullscreenPass::new(device, "mip", COPY, "fs_mip", false),
            gaussian: FullscreenPass::new(device, "gaussian", GAUSSIAN, "fs_gaussian", true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::uniform_slots;

    #[test]
    fn uniform_slots_round_each_entry_up_to_the_alignment() {
        assert_eq!(uniform_slots(16, 256, 2), vec![0, 256]);
        assert_eq!(uniform_slots(1500, 256, 2), vec![0, 1536]);
        assert_eq!(uniform_slots(256, 256, 3), vec![0, 256, 512]);
    }
}
