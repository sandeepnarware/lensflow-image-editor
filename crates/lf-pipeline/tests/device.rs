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

    // A validation error outside any renderer call (another thread's, or the UI's) is not
    // the renderer's: it is logged and kept for `take_error`, and the render is clean.
    bad_bind_group(&gpu);
    r.render(&s).unwrap();
    assert!(gpu.take_error().is_some_and(|m| !m.is_empty()));
    assert!(gpu.take_error().is_none(), "taken and cleared");

    // The same error under the caller's own error scope is the scope's.
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    bad_bind_group(&gpu);
    assert!(pollster::block_on(scope.pop()).is_some());
    r.render(&s).unwrap();
    assert!(gpu.take_error().is_none());
}

#[test]
fn an_error_inside_a_call_is_that_calls_and_resets_the_caches() {
    let _guard = one_at_a_time();
    let gpu = Gpu::new().unwrap();
    let img = photo();
    let s = normalise(&json!({ "sharpen": 40, "texture": 20 }));
    let mut r = Renderer::new(gpu.clone(), &img, 48, 32).unwrap();
    let good = r.render(&s).unwrap();
    assert!(r.has_cached("sharp") && r.has_cached("texture"));

    r.inject_validation_error();
    match r.render(&s) {
        Err(PipelineError::Gpu(msg)) => assert!(!msg.is_empty()),
        other => panic!(
            "expected PipelineError::Gpu, got {:?}",
            other.map(|i| i.dimensions())
        ),
    }
    assert!(
        !r.has_cached("sharp") && !r.has_cached("texture") && !r.has_cached("output"),
        "caches dropped after a device error"
    );
    assert!(
        gpu.take_error().is_none(),
        "captured by the call, not uncaptured"
    );
    // The next call on the same renderer succeeds with fresh caches.
    assert!(r.render(&s).unwrap().as_raw() == good.as_raw());
    for call in 0..3 {
        r.inject_validation_error();
        let failed = match call {
            0 => r.pixels(&s, 20).is_err(),
            1 => r.histogram(&s).is_err(),
            _ => r.render_f16(&s).is_err(),
        };
        assert!(failed, "call {call} did not report the error");
        r.pixels(&s, 20).unwrap();
    }
}

#[test]
fn an_error_on_one_thread_does_not_fail_another_threads_render() {
    let _guard = one_at_a_time();
    let gpu = Gpu::new().unwrap();
    let s = normalise(&json!({ "exposure": 0.3 }));
    let mut a = Renderer::new(gpu.clone(), &photo(), 48, 32).unwrap();
    let mut b = Renderer::new(gpu.clone(), &photo(), 48, 32).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (ba, bb) = (barrier.clone(), barrier);
    let sa = s.clone();
    let ta = std::thread::spawn(move || {
        ba.wait();
        (0..20).all(|_| {
            a.inject_validation_error();
            a.render(&sa).is_err()
        })
    });
    let tb = std::thread::spawn(move || {
        bb.wait();
        (0..20).all(|_| b.render(&s).is_ok())
    });
    assert!(
        ta.join().unwrap(),
        "every injected error reached its own call"
    );
    assert!(tb.join().unwrap(), "the other thread never saw them");
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
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    gpu.device.destroy();
    let _ = gpu.device.poll(wgpu::PollType::Poll);
    assert!(gpu.is_lost() && r.is_lost());
    assert!(matches!(r.render(&s), Err(PipelineError::DeviceLost)));
    assert!(matches!(r.pixels(&s, 20), Err(PipelineError::DeviceLost)));
    assert!(matches!(r.histogram(&s), Err(PipelineError::DeviceLost)));
}

#[test]
fn waiting_for_an_answer_that_never_comes_times_out() {
    let _guard = one_at_a_time();
    let gpu = Gpu::new().unwrap();
    let (_keep, rx) = std::sync::mpsc::channel::<()>();
    let t = std::time::Instant::now();
    let r = gpu.wait_for(&rx, std::time::Duration::from_millis(300));
    assert!(matches!(r, Err(PipelineError::Timeout(_))), "{r:?}");
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn waiting_on_a_destroyed_device_reports_it_lost() {
    let _guard = one_at_a_time();
    let gpu = Gpu::new().unwrap();
    let (_keep, rx) = std::sync::mpsc::channel::<()>();
    gpu.device.destroy();
    let _ = gpu.device.poll(wgpu::PollType::Poll);
    let t = std::time::Instant::now();
    let r = gpu.wait_for(&rx, std::time::Duration::from_secs(20));
    assert!(matches!(r, Err(PipelineError::DeviceLost)), "{r:?}");
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
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
