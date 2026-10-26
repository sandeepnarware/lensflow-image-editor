//! The library, driven headless through its accessibility tree: the first-run and
//! network-drive screens, then a real catalogue with an import, the grid and thumbnails.
//! The folder pickers are never clicked (they would open a system dialog); their answers
//! are fed in as the messages the pickers send.

use eframe::egui::{self, accesskit::Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use lf_app::catalog_ext::NETWORK_WARNING;
use lf_app::jobs::{Jobs, Msg};
use lf_app::library::import_ui::FIRST_RUN;
use lf_app::library::{Env, Library, Stage};
use lf_app::prefs::Prefs;
use lf_app::toast::Toasts;
use lf_catalog::import;
use lf_pipeline::test_support::try_shared_gpu;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct State {
    lib: Library,
    jobs: Jobs,
    toasts: Toasts,
    prefs: Prefs,
    config: PathBuf,
}

impl State {
    fn handle(&mut self, msg: Msg) {
        let mut env = Env {
            jobs: &self.jobs,
            toasts: &mut self.toasts,
            prefs: &mut self.prefs,
            config_dir: &self.config,
        };
        if let Some(Msg::JobFailed { name, message }) = self.lib.handle(msg, &mut env) {
            panic!("job {name} failed: {message}");
        }
    }
}

fn harness(dir: &Path, root: Option<PathBuf>) -> Option<Harness<'static, State>> {
    let gpu = try_shared_gpu()?;
    let prefs = Prefs {
        catalogue_root: root,
        frame_cache_bytes: 64 << 20,
        ..Prefs::default()
    };
    let state = State {
        lib: Library::new(gpu, dir.join("frames"), &prefs),
        jobs: Jobs::new(egui::Context::default()),
        toasts: Toasts::default(),
        prefs,
        config: dir.join("config"),
    };
    Some(
        Harness::builder()
            .with_size([1280.0, 800.0])
            .build_ui_state(
                |ui, s: &mut State| {
                    for msg in s.jobs.drain() {
                        s.handle(msg);
                    }
                    let mut env = Env {
                        jobs: &s.jobs,
                        toasts: &mut s.toasts,
                        prefs: &mut s.prefs,
                        config_dir: &s.config,
                    };
                    s.lib.ui(ui, &mut env);
                },
                state,
            ),
    )
}

/// Steps the harness until `done` holds (or fails after 60 s).
fn step_until(h: &mut Harness<'static, State>, what: &str, done: impl Fn(&State) -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !done(h.state()) {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

#[test]
fn first_run_asks_for_a_catalogue_folder() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = harness(dir.path(), None) else {
        return;
    };
    h.step();
    assert_eq!(h.state().lib.stage, Stage::NoRoot);
    h.get_by_label(FIRST_RUN);
    h.get_by_role_and_label(Role::Button, "Choose a folder…");
}

#[test]
fn a_network_root_is_questioned_and_can_be_used_anyway() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = harness(dir.path(), None) else {
        return;
    };
    let root = dir.path().join("share");
    std::fs::create_dir(&root).unwrap();
    h.state_mut()
        .handle(Msg::RootPicked(Some((root.clone(), true))));
    h.step();
    h.get_by_label(NETWORK_WARNING);
    h.get_by_role_and_label(Role::Button, "Choose another");
    h.get_by_role_and_label(Role::Button, "Use it anyway")
        .click();
    h.step();
    assert_eq!(
        h.state().prefs.catalogue_root.as_deref(),
        Some(root.as_path())
    );
    assert_eq!(
        Prefs::load(&h.state().config).catalogue_root.as_deref(),
        Some(root.as_path()),
        "remembered"
    );
    step_until(&mut h, "the catalogue to open", |s| {
        s.lib.stage == Stage::Ready
    });
    assert!(root.join(".lensflow/catalog.sqlite").exists());
}

#[test]
fn import_fills_the_grid_status_bar_and_thumbnails() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("catalogue");
    std::fs::create_dir(&root).unwrap();
    let Some(mut h) = harness(dir.path(), Some(root.clone())) else {
        return;
    };
    step_until(&mut h, "the catalogue to open", |s| {
        s.lib.stage == Stage::Ready
    });
    h.get_by_label("Add a folder of photos to begin.");

    let shoot = dir.path().join("shoot");
    std::fs::create_dir(&shoot).unwrap();
    for i in 0..3 {
        image::RgbaImage::from_pixel(300, 200, image::Rgba([40 * i, 90, 160, 255]))
            .save(shoot.join(format!("p{i}.png")))
            .unwrap();
    }
    std::fs::write(shoot.join("broken.png"), b"not a png").unwrap();
    let scan = import::scan(&shoot);
    h.state_mut().handle(Msg::Scanned {
        folder: shoot.clone(),
        scan,
    });
    step_until(&mut h, "the import and the photos", |s| {
        !s.lib.importing() && s.lib.photos().len() == 4
    });
    h.get_by_label("4 photos · 1 selected");
    h.get_by_label("shoot  (4)");
    step_until(&mut h, "the thumbnails", |s| {
        let (made, failed, pending) = s.lib.thumbnail_counts();
        made == 3 && failed == 1 && pending == 0
    });
}
