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

/// Uncaptured graphics errors: the library's handler logs each one; the app's per-frame tap
/// only counts them, and asks for a toast when three arrive within a minute (once a minute
/// at most).
#[derive(Debug, Default)]
pub struct ErrorTap {
    recent: std::collections::VecDeque<std::time::Instant>,
    warned_at: Option<std::time::Instant>,
}

/// The toast after a burst of graphics errors.
pub const ERROR_BURST: &str =
    "The graphics device reported several errors. If a photo looks wrong, reopen it.";

impl ErrorTap {
    /// How many errors make a burst, and the window they must fall in.
    pub const BURST: usize = 3;
    pub const WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

    /// Counts an error seen at `now`; true when the toast should show.
    pub fn record(&mut self, now: std::time::Instant) -> bool {
        self.recent.push_back(now);
        while self
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) > Self::WINDOW)
        {
            self.recent.pop_front();
        }
        let quiet = self
            .warned_at
            .is_none_or(|t| now.duration_since(t) > Self::WINDOW);
        if self.recent.len() >= Self::BURST && quiet {
            self.warned_at = Some(now);
            self.recent.clear();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn three_errors_in_a_minute_ask_for_one_toast() {
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut tap = ErrorTap::default();
        assert!(!tap.record(at(0)));
        assert!(!tap.record(at(30)));
        assert!(!tap.record(at(100)), "the first fell out of the minute");
        assert!(!tap.record(at(120)));
        assert!(tap.record(at(130)), "three within a minute");
        assert!(!tap.record(at(131)));
        assert!(!tap.record(at(132)));
        assert!(!tap.record(at(140)), "no second toast within the minute");
        assert!(!tap.record(at(200)));
        assert!(!tap.record(at(201)));
        assert!(
            tap.record(at(202)),
            "a minute later, another burst toasts again"
        );
    }
}
