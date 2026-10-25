//! The headless device every pass runs on.
//!
//! A [`Gpu`] is shared (`Arc<Gpu>`) between renderers and, in the app, with the UI's own
//! wgpu device ([`Gpu::from_device`]). Errors wgpu does not hand to an error scope are
//! recorded here instead of panicking (wgpu's default), and a lost device sets a flag; the
//! renderer turns both into [`PipelineError::Gpu`] and [`PipelineError::DeviceLost`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    /// The instance [`Gpu::new`] created; `None` for [`Gpu::from_device`] (the owner keeps it).
    pub instance: Option<wgpu::Instance>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub limits: wgpu::Limits,
    pub samplers: Samplers,
    pub(crate) pipelines: Pipelines,
    /// The last uncaptured error's message, taken by [`Gpu::check`].
    last_error: Arc<Mutex<Option<String>>>,
    /// Set by the device-lost callback.
    lost: Arc<AtomicBool>,
}

impl Gpu {
    /// A device chosen by [`GpuOptions::from_env`].
    pub fn new() -> Result<Arc<Gpu>, PipelineError> {
        Gpu::new_with(GpuOptions::from_env())
    }

    /// Tries a high-performance hardware adapter first, then (when allowed) the fallback adapter.
    pub fn new_with(options: GpuOptions) -> Result<Arc<Gpu>, PipelineError> {
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
        Ok(Gpu::with_instance(Some(instance), adapter, device, queue))
    }

    /// Wraps a device someone else opened (e.g. egui-wgpu's), so renders share it. Installs
    /// this `Gpu`'s uncaptured-error handler and device-lost callback on `device`, replacing
    /// any set before (wgpu keeps one of each per device). The device needs no features;
    /// its limits bound the photo size ([`Gpu::max_edge`]) and readback bands.
    pub fn from_device(
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Arc<Gpu> {
        Gpu::with_instance(None, adapter, device, queue)
    }

    fn with_instance(
        instance: Option<wgpu::Instance>,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Arc<Gpu> {
        let last_error = Arc::new(Mutex::new(None));
        let lost = Arc::new(AtomicBool::new(false));
        {
            let last_error = last_error.clone();
            device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
                let mut slot = last_error.lock().unwrap_or_else(|p| p.into_inner());
                *slot = Some(e.to_string());
            }));
        }
        {
            let lost = lost.clone();
            device.set_device_lost_callback(move |_reason, _message| {
                lost.store(true, Ordering::SeqCst);
            });
        }
        let limits = device.limits();
        let samplers = Samplers::new(&device);
        let pipelines = Pipelines::new(&device);
        Arc::new(Gpu {
            instance,
            adapter,
            device,
            queue,
            limits,
            samplers,
            pipelines,
            last_error,
            lost,
        })
    }

    /// The adapter is a software rasteriser (WARP, lavapipe, SwiftShader).
    pub fn is_software(&self) -> bool {
        self.adapter.get_info().device_type == wgpu::DeviceType::Cpu
    }

    /// The device was lost (driver reset, removal, or destroyed); nothing more will render.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// The last uncaptured error recorded since the previous call, cleared by this call.
    pub fn take_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }

    /// [`PipelineError::DeviceLost`] when the device is lost, else [`PipelineError::Gpu`]
    /// with the last uncaptured error since the previous check (take-and-clear).
    pub fn check(&self) -> Result<(), PipelineError> {
        if self.is_lost() {
            return Err(PipelineError::DeviceLost);
        }
        match self.take_error() {
            Some(msg) => Err(PipelineError::Gpu(msg)),
            None => Ok(()),
        }
    }

    /// The longest texture edge this device accepts.
    pub fn max_edge(&self) -> u32 {
        self.limits.max_texture_dimension_2d
    }
}
