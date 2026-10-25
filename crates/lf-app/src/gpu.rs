//! The one shared graphics device: egui's own, wrapped once as an `lf_pipeline::Gpu`.

use eframe::egui_wgpu::{RenderState, WgpuConfiguration, WgpuSetup};
use eframe::wgpu;
use lf_pipeline::Gpu;
use std::sync::Arc;

/// Wraps eframe's device as the app's `Gpu`. Called exactly once, from `App::new`, with
/// `CreationContext::wgpu_render_state`; this installs the `Gpu`'s error handler and
/// device-lost callback on egui's device.
pub fn attach(render_state: Option<&RenderState>) -> Result<Arc<Gpu>, String> {
    let rs = render_state.ok_or_else(|| {
        "The window opened without a graphics device, so photos cannot be shown.".to_owned()
    })?;
    let gpu = Gpu::from_device(rs.adapter.clone(), rs.device.clone(), rs.queue.clone());
    let info = gpu.adapter.get_info();
    log::info!(
        "graphics adapter: {} ({:?}, driver {} {}), software: {}, max texture size: {} px",
        info.name,
        info.backend,
        info.driver,
        info.driver_info,
        gpu.is_software(),
        gpu.max_edge()
    );
    Ok(gpu)
}

/// Makes eframe open its device with the adapter's own limits, as `Gpu::new` does: egui's
/// default (`Limits::default()`: 8192 px textures, 256 MiB buffers) would refuse 45 to
/// 100 MP photos.
pub fn request_full_limits(options: &mut WgpuConfiguration) {
    if let WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        let egui_descriptor = setup.device_descriptor.clone();
        setup.device_descriptor = Arc::new(move |adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..egui_descriptor(adapter)
        });
    }
}
