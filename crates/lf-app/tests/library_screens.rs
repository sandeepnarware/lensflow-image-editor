//! The library, driven headless through its accessibility tree: the first-run and
//! network-drive screens, typed folder paths, then a real catalogue with imports, the grid,
//! thumbnails, the loading state and the "mark missing" question.
//! The folder pickers are never clicked (they would open a system dialog); their answers
//! are fed in as the messages the pickers send.

use eframe::egui::{self, Key, accesskit::Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use lf_app::catalog_ext::{FOLDER_MISSING, NETWORK_WARNING};
use lf_app::jobs::{Jobs, Lane, Msg};
use lf_app::library::import_ui::{Chosen, FIRST_RUN, Purpose, TYPE_PATH};
use lf_app::library::{Env, Library, Mode, Stage};
use lf_app::prefs::Prefs;
use lf_app::toast::Toasts;
use lf_catalog::import;
use lf_pipeline::test_support::try_shared_gpu;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
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
                    s.toasts.show(ui.ctx());
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

/// Steps the harness until `label` is on screen (or fails after 60 s).
fn step_until_label(h: &mut Harness<'static, State>, label: &str) {
    let end = Instant::now() + Duration::from_secs(60);
    while h.query_by_label(label).is_none() {
        assert!(Instant::now() < end, "timed out waiting for {label:?}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn pngs(folder: &Path, n: u8) {
    std::fs::create_dir_all(folder).unwrap();
    for i in 0..n {
        image::RgbaImage::from_pixel(300, 200, image::Rgba([40 * i, 90, 160, 255]))
            .save(folder.join(format!("p{i}.png")))
            .unwrap();
    }
}

/// A harness with an open catalogue at `<dir>/catalogue`.
fn opened(dir: &Path) -> Option<Harness<'static, State>> {
    let root = dir.join("catalogue");
    std::fs::create_dir(&root).unwrap();
    let mut h = harness(dir, Some(root))?;
    step_until(&mut h, "the catalogue to open", |s| {
        s.lib.stage == Stage::Ready
    });
    Some(h)
}

fn import_folder(h: &mut Harness<'static, State>, folder: &Path, photos: usize) {
    let scan = import::scan(folder);
    h.state_mut().handle(Msg::Scanned {
        folder: folder.to_path_buf(),
        scan,
    });
    step_until(h, "the import and the photos", |s| {
        !s.lib.importing() && !s.lib.loading() && s.lib.photos().len() == photos
    });
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
    h.get_by_label(TYPE_PATH);
    h.get_by_role_and_label(Role::Button, "Use this folder");
}

#[test]
fn a_typed_catalogue_path_is_checked_then_used() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = harness(dir.path(), None) else {
        return;
    };
    h.step();
    let field = h.get_by_role_and_label(Role::TextInput, TYPE_PATH);
    field.focus();
    field.type_text(&dir.path().join("nope").to_string_lossy());
    h.step();
    h.get_by_role_and_label(Role::Button, "Use this folder")
        .click();
    step_until_label(&mut h, FOLDER_MISSING);
    assert_eq!(h.state().lib.stage, Stage::NoRoot);

    let root = dir.path().join("typed");
    std::fs::create_dir(&root).unwrap();
    h.state_mut().lib.set_typed_path(&root.to_string_lossy());
    h.step();
    h.get_by_role_and_label(Role::Button, "Use this folder")
        .click();
    step_until(&mut h, "the catalogue to open", |s| {
        s.lib.stage == Stage::Ready
    });
    assert_eq!(
        h.state().prefs.catalogue_root.as_deref(),
        Some(root.as_path())
    );
}

#[test]
fn a_network_root_is_questioned_and_can_be_used_anyway() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = harness(dir.path(), None) else {
        return;
    };
    let root = dir.path().join("share");
    std::fs::create_dir(&root).unwrap();
    let ticket = h.state().lib.ticket();
    h.state_mut().handle(Msg::FolderChosen {
        purpose: Purpose::Root,
        ticket,
        result: Ok(Some(Chosen {
            path: root.clone(),
            network: true,
        })),
    });
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
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    h.get_by_label("Add a folder of photos to begin.");
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    std::fs::write(shoot.join("broken.png"), b"not a png").unwrap();
    import_folder(&mut h, &shoot, 4);
    step_until_label(&mut h, "shoot  (4)");
    h.get_by_label("4 photos · 1 selected");
    step_until(&mut h, "the thumbnails", |s| {
        let (made, failed, pending) = s.lib.thumbnail_counts();
        made == 3 && failed == 1 && pending == 0
    });
}

#[test]
fn a_folder_still_loading_never_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    // Hold the query lane so the folder's photos cannot arrive yet.
    let (release, held) = mpsc::channel::<()>();
    h.state().jobs.spawn(Lane::Query, "hold", move || {
        let _ = held.recv();
        Msg::Cleared
    });
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    let scan = import::scan(&shoot);
    h.state_mut().handle(Msg::Scanned {
        folder: shoot.clone(),
        scan,
    });
    step_until(&mut h, "the import", |s| !s.lib.importing());
    for _ in 0..5 {
        h.step();
        h.get_by_label("Loading photos…");
        assert!(h.query_by_label("No photos in this folder.").is_none());
        assert!(
            h.query_by_label("Add a folder of photos to begin.")
                .is_none()
        );
    }
    release.send(()).unwrap();
    step_until(&mut h, "the photos", |s| s.lib.photos().len() == 2);
    assert!(h.query_by_label("Loading photos…").is_none());
}

#[test]
fn an_emptied_folder_asks_before_marking_its_photos_missing() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);

    for i in 0..2 {
        std::fs::remove_file(shoot.join(format!("p{i}.png"))).unwrap();
    }
    let scan = import::scan(&shoot);
    assert!(scan.root_readable && scan.candidates.is_empty());
    h.state_mut().handle(Msg::Scanned {
        folder: shoot.clone(),
        scan,
    });
    let question = "None of the 2 photos are in this folder any more. Mark them missing?";
    step_until_label(&mut h, question);
    assert!(h.state().lib.photos().iter().all(|p| !p.missing));
    h.get_by_role_and_label(Role::Button, "Keep them");
    h.get_by_role_and_label(Role::Button, "Mark missing")
        .click();
    step_until(&mut h, "the photos marked missing", |s| {
        s.lib.photos().len() == 2 && s.lib.photos().iter().all(|p| p.missing)
    });
    assert!(h.query_by_label(question).is_none());
}

#[test]
fn develop_never_moves_onto_a_missing_photo() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    // The second photo goes away; a rescan that still finds the first marks it missing.
    std::fs::remove_file(shoot.join("p1.png")).unwrap();
    import_folder(&mut h, &shoot, 2);
    step_until(&mut h, "the missing flag", |s| {
        s.lib.photos().iter().filter(|p| p.missing).count() == 1
    });
    let first = h.state().lib.photos()[0].id;
    assert!(!h.state().lib.photos()[0].missing);
    h.state_mut().lib.selection.click(first);
    h.key_press(Key::D);
    h.step();
    assert_eq!(h.state().lib.mode, Mode::Develop(first));
    h.key_press(Key::ArrowRight);
    h.step();
    h.step();
    assert_eq!(
        h.state().lib.mode,
        Mode::Develop(first),
        "the missing photo is not opened"
    );
}

#[test]
fn develop_opens_a_photo_shows_its_facts_and_steps_over_missing_ones() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    import_folder(&mut h, &shoot, 3);
    std::fs::remove_file(shoot.join("p1.png")).unwrap();
    import_folder(&mut h, &shoot, 3);
    step_until(&mut h, "the missing flag", |s| {
        s.lib.photos().iter().filter(|p| p.missing).count() == 1
    });
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    let missing = h.state().lib.photos()[1].missing;
    assert!(missing, "p1 is the missing one");
    h.state_mut().lib.selection.click(ids[0]);
    h.key_press(Key::D);
    step_until(&mut h, "the first photo to open", |s| {
        s.lib.develop().is_ready()
    });
    assert_eq!(h.state().lib.develop().photo_id(), Some(ids[0]));
    h.get_by_label("p0.png");
    h.get_by_label("300 × 200");
    h.get_by_label("PNG");
    h.get_by_label(lf_app::develop::BEFORE);
    // Headless: no egui renderer to share a texture with.
    h.get_by_label(lf_app::develop::NO_PREVIEW);

    // → steps over the missing photo to the third.
    h.key_press(Key::ArrowRight);
    step_until(&mut h, "the third photo to open", |s| {
        s.lib.develop().is_ready() && s.lib.develop().photo_id() == Some(ids[2])
    });
    assert_eq!(h.state().lib.mode, Mode::Develop(ids[2]));
    h.get_by_label("p2.png");
    // ← steps back over it too.
    h.key_press(Key::ArrowLeft);
    step_until(&mut h, "the first photo again", |s| {
        s.lib.develop().is_ready() && s.lib.develop().photo_id() == Some(ids[0])
    });

    // G leaves Develop and frees the renderer.
    h.key_press(Key::G);
    h.step();
    assert_eq!(h.state().lib.mode, Mode::Library);
    assert!(h.state().lib.develop().photo_id().is_none());
    assert!(!h.state().lib.develop().is_ready());
}

#[test]
fn a_photo_that_cannot_be_opened_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::write(shoot.join("broken.jpg"), b"\xff\xd8\xff not really a jpeg").unwrap();
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    h.key_press(Key::D);
    step_until_label(&mut h, lf_app::develop::CANT_OPEN);
    assert!(!h.state().lib.develop().is_ready());
    assert_eq!(h.state().lib.develop().photo_id(), Some(id));
    h.step();
    assert!(
        h.query_by_label_contains("This file can't be opened: ")
            .is_some(),
        "the toast names the problem"
    );
}

/// Opens the only photo of a fresh one-photo folder in Develop; its path.
fn develop_one(h: &mut Harness<'static, State>, dir: &Path) -> PathBuf {
    let shoot = dir.join("shoot");
    pngs(&shoot, 1);
    import_folder(h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    h.key_press(Key::D);
    step_until(h, "the photo to open", |s| s.lib.develop().is_ready());
    h.state().lib.photos()[0].path.clone()
}

/// Drags the slider labelled `label` by `dx` points, a few frames at a time.
fn drag_slider(h: &mut Harness<'static, State>, label: &str, dx: f32) {
    let rect = h.get_by_label(label).rect();
    let start = rect.center();
    h.drag_at(start);
    h.step();
    for i in 1..=4 {
        h.hover_at(start + egui::vec2(dx * i as f32 / 4.0, 0.0));
        h.step();
    }
    h.drop_at(start + egui::vec2(dx, 0.0));
    h.step();
    h.step();
}

fn exposure(h: &Harness<'static, State>) -> f64 {
    h.state()
        .lib
        .develop()
        .edit_state()
        .unwrap()
        .settings
        .exposure
}

#[test]
fn a_drag_is_one_history_step_undone_with_ctrl_z_and_saved_beside_the_photo() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let photo = develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    assert!(!h.state().lib.develop().can_undo());

    drag_slider(&mut h, "Exposure", 60.0);
    let dragged = exposure(&h);
    assert!(dragged > 0.0, "the drag moved exposure");
    assert!(!h.state().lib.develop().edit_state().unwrap().dirty);
    let catalog = lf_catalog::Catalog::open(&root).unwrap();
    assert_eq!(
        lf_catalog::edits::history_len(&catalog, id).unwrap(),
        2,
        "the photo before, and one step for the whole drag"
    );
    assert!(h.state().lib.develop().can_undo());

    // The sidecar is written a quiet second later, on the Io lane.
    let sidecar = lf_catalog::sidecar::path_for(&photo);
    step_until(&mut h, "the sidecar", |_| sidecar.exists());
    let written = lf_catalog::sidecar::read(&sidecar).unwrap().unwrap();
    assert_eq!(written.settings.exposure, dragged);

    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    h.step();
    assert_eq!(exposure(&h), 0.0, "Ctrl+Z undid the drag");
    assert!(h.state().lib.develop().can_redo());
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    h.step();
    assert_eq!(exposure(&h), dragged, "Ctrl+Y redid it");
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z);
    h.step();
    assert_eq!(exposure(&h), dragged, "nothing more to redo");
    assert_eq!(
        lf_catalog::edits::get_settings(&catalog, id)
            .unwrap()
            .unwrap()
            .exposure,
        dragged
    );

    // Reset puts the section back as one more step.
    h.get_by_label("Reset Light").click();
    h.step();
    assert_eq!(exposure(&h), 0.0);
    assert_eq!(lf_catalog::edits::history_len(&catalog, id).unwrap(), 3);
}

#[test]
fn a_damaged_edit_file_is_never_overwritten_and_the_warning_shows_once() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 1);
    let sidecar = lf_catalog::sidecar::path_for(&shoot.join("p0.png"));
    std::fs::write(&sidecar, b"{ not json").unwrap();
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open", |s| s.lib.develop().is_ready());
    let warning = lf_app::develop::damaged_text("p0.png");
    h.get_by_label(&warning);
    h.get_by_role_and_label(Role::Button, lf_app::develop::SHOW_FILE);

    drag_slider(&mut h, "Exposure", 60.0);
    assert!(exposure(&h) > 0.0);
    // Well past the quiet second: nothing was written over the damaged file.
    let waited = Instant::now();
    while waited.elapsed() < Duration::from_millis(1500) {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read(&sidecar).unwrap(), b"{ not json");
    assert!(!h.state().lib.sidecar_saver().has_pending());
    // The edit is in the catalogue.
    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    assert!(
        lf_catalog::edits::get_settings(&catalog, id)
            .unwrap()
            .unwrap()
            .exposure
            > 0.0
    );

    // Back to the library and in again: the warning was shown once this session.
    h.key_press(Key::G);
    h.step();
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open again", |s| {
        s.lib.develop().is_ready()
    });
    assert!(h.query_by_label(&warning).is_none());
}

/// A RAW's profile change commits and decodes the photo again with it, keeping it on
/// screen meanwhile (skipped when the RAW corpus has not been fetched, as in CI).
#[test]
fn a_raw_profile_change_reopens_the_photo_with_it() {
    let corpus =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/raw-samples/files/nikon-d40.nef");
    if !corpus.exists() {
        eprintln!("RAW corpus not fetched: skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::copy(&corpus, shoot.join("d40.nef")).unwrap();
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    h.key_press(Key::D);
    step_until(&mut h, "the RAW to open", |s| s.lib.develop().is_ready());
    let profile = |s: &State| s.lib.develop().opened().map(|o| o.develop.profile);
    assert_eq!(profile(h.state()), Some(lf_settings::Profile::Camera));
    h.get_by_label(lf_app::develop::panels::RAW);
    h.get_by_label("Linear").click();
    h.step();
    assert!(
        h.state().lib.develop().is_ready(),
        "still shown while it reopens"
    );
    step_until(&mut h, "the RAW to reopen as linear", |s| {
        profile(s) == Some(lf_settings::Profile::Linear)
    });
    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    let stored = lf_catalog::edits::get_settings(&catalog, id)
        .unwrap()
        .unwrap();
    assert_eq!(
        lf_settings::DevelopSettings::from_settings(&stored).profile,
        lf_settings::Profile::Linear
    );
}
