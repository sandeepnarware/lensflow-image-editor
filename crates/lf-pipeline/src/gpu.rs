//! The headless device every pass runs on.

use crate::error::PipelineError;
use crate::passes::Pipelines;
use crate::texture::Samplers;

/// How to pick the adapter.
#[derive(Debug, Clone, Copy)]
pub struct GpuOptions {
    /// Accept a software (fallback) adapter when no hardware one is found.
    pub allow_fallback: bool,
    /// The backends to try; `None` means `WGPU_BACKEND` if set, else Vulkan, Metal and DirectX 12.
    pub backend: Option<wgpu::Backends>,
}

impl GpuOptions {
    /// Options from the environment: `LF_ALLOW_FALLBACK_ADAPTER=0` forbids the fallback
    /// adapter (it is allowed otherwise); `WGPU_BACKEND` picks the backends.
    pub fn from_env() -> GpuOptions {
        let allow_fallback = std::env::var("LF_ALLOW_FALLBACK_ADAPTER")
            .map(|v| v.trim() != "0")
            .unwrap_or(true);
        GpuOptions {
            allow_fallback,
            backend: None,
        }
    }
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub limits: wgpu::Limits,
    pub samplers: Samplers,
    pub(crate) pipelines: Pipelines,
}

impl Gpu {
    /// A device chosen by [`GpuOptions::from_env`].
    pub fn new() -> Result<Gpu, PipelineError> {
        Gpu::new_with(GpuOptions::from_env())
    }

    /// Tries a high-performance hardware adapter first, then (when allowed) the fallback adapter.
    pub fn new_with(options: GpuOptions) -> Result<Gpu, PipelineError> {
        let backends = options
            .backend
            .unwrap_or_else(|| wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY));
        if backends.is_empty() {
            return Err(PipelineError::NoAdapter);
        }
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends;
        desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
        let instance = wgpu::Instance::new(desc);

        let request = |force_fallback_adapter| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter,
                compatible_surface: None,
                apply_limit_buckets: false,
            }))
            .ok()
        };
        let is_software = |a: &wgpu::Adapter| a.get_info().device_type == wgpu::DeviceType::Cpu;
        let adapter = match request(false) {
            Some(a) if options.allow_fallback || !is_software(&a) => Some(a),
            _ if options.allow_fallback => request(true),
            _ => None,
        }
        .ok_or(PipelineError::NoAdapter)?;

        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lensflow"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .map_err(|e| PipelineError::DeviceRequest(e.to_string()))?;
        let limits = device.limits();
        let samplers = Samplers::new(&device);
        let pipelines = Pipelines::new(&device);
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
            limits,
            samplers,
            pipelines,
        })
    }

    /// The longest texture edge this device accepts.
    pub fn max_edge(&self) -> u32 {
        self.limits.max_texture_dimension_2d
    }
}
