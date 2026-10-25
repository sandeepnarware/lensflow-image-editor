//! Device sharing, uncaptured errors and device loss. These change device-wide state (the
//! error handler, destroying the device), so they open their own devices, one test at a
//! time, in this binary of their own instead of using the shared test device.

use std::sync::{Arc, Mutex, MutexGuard};

use image::{Rgba, RgbaImage};
use lf_pipeline::error::PipelineError;
use lf_pipeline::passes::texture_entry;
use lf_pipeline::{Gpu, Renderer};
use lf_settings::normalise;
use serde_json::json;

/// One device at a time in this process (WARP crashes on parallel devices).
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn photo() -> RgbaImage {
    RgbaImage::from_fn(48, 32, |x, y| {
        Rgba([(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255])
    })
}

#[test]
fn gpu_from_device_shares_handles() {
    let _guard = one_at_a_time();
    let own = Gpu::new().unwrap();
    let shared = Gpu::from_device(own.adapter.clone(), own.device.clone(), own.queue.clone());
    assert!(shared.device == own.device, "from_device made a new device");
    assert!(shared.instance.is_none());
    assert_eq!(shared.max_edge(), own.max_edge());
    assert_eq!(shared.is_software(), own.is_software());
    assert!(!shared.is_lost());

    let img = photo();
    let mut r = Renderer::new(shared.clone(), &img, 48, 32).unwrap();
    let got = r.render(&normalise(&json!({}))).unwrap();
    assert!(
        got.as_raw() == img.as_raw(),
        "identity through a shared device"
    );
}

#[test]
fn device_lost_is_reported_not_panicked() {
    let _guard = one_at_a_time();
    let gpu = Gpu::new().unwrap();
    assert!(!gpu.is_lost());
    let img = photo();
    let s = normalise(&json!({ "exposure": 0.5 }));
    let mut r = Renderer::new(gpu.clone(), &img, 48, 32).unwrap();
    assert!(!r.is_lost());
    r.render(&s).unwrap();

    // A validation error outside any error scope: wgpu's default handler would panic.
    bad_bind_group(&gpu);
    match r.render(&s) {
        Err(PipelineError::Gpu(msg)) => assert!(!msg.is_empty()),
        other => panic!(
            "expected PipelineError::Gpu, got {:?}",
            other.map(|i| i.dimensions())
        ),
    }
    // Taken and cleared: the next render is clean.
    r.render(&s).unwrap();

    // The same error under an error scope is the scope's, not the renderer's.
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    bad_bind_group(&gpu);
    assert!(pollster::block_on(scope.pop()).is_some());
    r.render(&s).unwrap();
}

#[test]
fn a_destroyed_device_is_lost() {
    let _guard = one_at_a_time();
    let gpu = Gpu::new().unwrap();
    let img = photo();
    let s = normalise(&json!({}));
    let mut r = Renderer::new(gpu.clone(), &img, 48, 32).unwrap();
    // wgpu loses a destroyed device (calling the callback) at the first poll that finds
    // its queue empty.
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    gpu.device.destroy();
    let _ = gpu.device.poll(wgpu::PollType::Poll);
    assert!(gpu.is_lost() && r.is_lost());
    assert!(matches!(r.render(&s), Err(PipelineError::DeviceLost)));
    assert!(matches!(r.pixels(&s, 20), Err(PipelineError::DeviceLost)));
    assert!(matches!(r.histogram(&s), Err(PipelineError::DeviceLost)));
}

/// A bind group whose texture binding is given a sampler.
fn bad_bind_group(gpu: &Arc<Gpu>) {
    let layout = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deliberately mismatched"),
            entries: &[texture_entry(0)],
        });
    let _ = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("deliberately mismatched"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Sampler(&gpu.samplers.linear_clamp),
        }],
    });
}
