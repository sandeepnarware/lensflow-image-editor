//! Review Focus 3: losing the graphics device while a photo is open in Develop drops the
//! renderer and says so, never panics, and opens nothing more on that device. This destroys
//! its device, so it opens its own in a test binary of its own (the other tests share one).

use eframe::egui::{self, Key};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use lf_app::app::DEVICE_LOST_BANNER;
use lf_app::jobs::{Jobs, Msg};
use lf_app::library::{Env, Library, Mode, Stage};
use lf_app::prefs::Prefs;
use lf_app::toast::Toasts;
use lf_catalog::import;
use lf_pipeline::{Gpu, PipelineError};
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct State {
    lib: Library,
    jobs: Jobs,
    toasts: Toasts,
    prefs: Prefs,
    config: PathBuf,
}

impl State {
    fn env(&mut self) -> (&mut Library, Env<'_>) {
        (
            &mut self.lib,
            Env {
                jobs: &self.jobs,
                toasts: &mut self.toasts,
                prefs: &mut self.prefs,
                config_dir: &self.config,
            },
        )
    }
}

fn step_until(h: &mut Harness<'static, State>, what: &str, done: impl Fn(&State) -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !done(h.state()) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

/// Runs `body` on its own thread and fails after `limit` instead of hanging the whole run:
/// on a stall it dumps every thread's stack (`gdb`, when installed) and panics with a plain
/// message. For GPU tests that destroy a device, where a wait on the dead device could hang.
fn with_timeout(limit: Duration, body: impl FnOnce() + Send + 'static) {
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        body();
        let _ = done.send(());
    });
    match finished.recv_timeout(limit) {
        Ok(()) => worker.join().unwrap(),
        // The sender is gone without a message: the body panicked; show its message.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            std::panic::resume_unwind(worker.join().unwrap_err())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let pid = std::process::id().to_string();
            let _ = std::process::Command::new("gdb")
                .args(["-p", &pid, "-batch", "-ex", "thread apply all bt 30"])
                .status();
            panic!(
                "the device-lost test stalled for {limit:?}: a wait on the lost device never returned"
            );
        }
    }
}

#[test]
fn losing_the_device_drops_the_renderer_and_never_panics() {
    with_timeout(Duration::from_secs(120), device_is_lost_in_develop);
}

fn device_is_lost_in_develop() {
    let gpu = match Gpu::new() {
        Ok(gpu) => gpu,
        Err(PipelineError::NoAdapter) => {
            eprintln!("no GPU adapter: skipped");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("catalogue");
    std::fs::create_dir(&root).unwrap();
    let shoot = dir.path().join("shoot");
    std::fs::create_dir(&shoot).unwrap();
    for i in 0..2u8 {
        image::RgbaImage::from_pixel(120, 80, image::Rgba([60 * i, 90, 160, 255]))
            .save(shoot.join(format!("p{i}.png")))
            .unwrap();
    }
    let prefs = Prefs {
        catalogue_root: Some(root),
        frame_cache_bytes: 64 << 20,
        ..Prefs::default()
    };
    let state = State {
        lib: Library::new(gpu.clone(), dir.path().join("frames"), &prefs),
        jobs: Jobs::new(egui::Context::default()),
        toasts: Toasts::default(),
        prefs,
        config: dir.path().join("config"),
    };
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(
            |ui, s: &mut State| {
                for msg in s.jobs.drain() {
                    let (lib, mut env) = s.env();
                    lib.handle(msg, &mut env);
                }
                let (lib, mut env) = s.env();
                lib.ui(ui, &mut env);
            },
            state,
        );
    step_until(&mut h, "the catalogue to open", |s| {
        s.lib.stage == Stage::Ready
    });
    h.state_mut().handle_scan(&shoot);
    step_until(&mut h, "the photos", |s| {
        !s.lib.importing() && !s.lib.loading() && s.lib.photos().len() == 2
    });
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    h.state_mut().lib.selection.click(ids[0]);
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open", |s| s.lib.develop().is_ready());

    // wgpu loses a destroyed device (calling the callback) at the first poll that finds its
    // queue empty; in the window, eframe polls every frame.
    let _ = gpu.device.poll(eframe::wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    });
    gpu.device.destroy();
    let _ = gpu.device.poll(eframe::wgpu::PollType::Poll);
    step_until(&mut h, "the device-lost state", |s| {
        s.lib.develop().is_lost()
    });
    assert!(gpu.is_lost());
    assert!(
        h.state().lib.develop().opened().is_none(),
        "renderer dropped"
    );
    h.get_by_label(DEVICE_LOST_BANNER);

    // Stepping on, leaving and coming back: nothing is opened on the lost device, nothing
    // panics, and the view keeps saying why.
    h.key_press(Key::ArrowRight);
    h.step();
    h.key_press(Key::G);
    h.step();
    assert_eq!(h.state().lib.mode, Mode::Library);
    h.key_press(Key::D);
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().lib.develop().is_lost());
    assert!(h.state().lib.develop().opened().is_none());
    h.get_by_label(DEVICE_LOST_BANNER);
}

trait Scan {
    fn handle_scan(&mut self, folder: &std::path::Path);
}

impl Scan for State {
    fn handle_scan(&mut self, folder: &std::path::Path) {
        let scan = import::scan(folder);
        let (lib, mut env) = self.env();
        lib.handle(
            Msg::Scanned {
                folder: folder.to_path_buf(),
                scan,
            },
            &mut env,
        );
    }
}
