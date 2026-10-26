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
use lf_app::library::import_ui::{Chosen, FIRST_RUN, KEEP_ROOT, Purpose, TYPE_PATH};
use lf_app::library::{Env, Library, Mode, Stage};
use lf_app::prefs::Prefs;
use lf_app::shortcuts::Action;
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
    /// Open answers received.
    opened: usize,
}

impl State {
    fn handle(&mut self, msg: Msg) {
        if matches!(msg, Msg::Opened { .. }) {
            self.opened += 1;
        }
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
        opened: 0,
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
                    // As the app does: the keyboard map first, then the status bar and the
                    // library.
                    let ctx = ui.ctx().clone();
                    s.lib.shortcuts(&ctx, &mut env, |_| false);
                    egui::Panel::bottom("status").show(ui, |ui| {
                        ui.horizontal(|ui| s.lib.status_ui(ui));
                    });
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
    // Letting go commits, unless another connection (a thumbnail or import write on a slow
    // machine) holds the write lock past the 250 ms quick timeout; the edit then stays dirty
    // and the commit is retried on a later frame. So wait for the step instead of assuming.
    step_until(&mut h, "the drag to be committed (not dirty)", |s| {
        s.lib.develop().edit_state().is_some_and(|e| !e.dirty)
    });
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

/// The corpus NEF (`scripts/fetch-raw-samples.sh`). `None` (the test skips) when it has not
/// been fetched, unless `LF_REQUIRE_SAMPLES=1`, which CI sets when the whole corpus is
/// present: then a missing file fails instead of skipping.
fn corpus_nef() -> Option<PathBuf> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/raw-samples/files/nikon-d40.nef");
    if path.exists() {
        return Some(path);
    }
    assert!(
        std::env::var("LF_REQUIRE_SAMPLES").as_deref() != Ok("1"),
        "LF_REQUIRE_SAMPLES=1 but {} is missing",
        path.display()
    );
    eprintln!("RAW corpus not fetched: skipped");
    None
}

/// A RAW's profile change commits and decodes the photo again with it, keeping it on
/// screen meanwhile (needs the RAW corpus; see [`corpus_nef`]).
#[test]
fn a_raw_profile_change_reopens_the_photo_with_it() {
    let Some(corpus) = corpus_nef() else {
        return;
    };
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

/// Opens the corpus NEF in Develop (None when the corpus, see [`corpus_nef`], or a GPU is
/// missing).
fn develop_corpus_raw(dir: &Path) -> Option<(Harness<'static, State>, i64)> {
    let corpus = corpus_nef()?;
    let mut h = opened(dir)?;
    let shoot = dir.join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::copy(&corpus, shoot.join("d40.nef")).unwrap();
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    h.key_press(Key::D);
    step_until(&mut h, "the RAW to open", |s| s.lib.develop().is_ready());
    Some((h, id))
}

/// Linear, then Camera again before the Linear decode answers: the photo ends decoded with
/// the last choice, not the one in flight.
#[test]
fn flipping_the_profile_twice_before_the_open_answers_ends_on_the_last_choice() {
    let dir = tempfile::tempdir().unwrap();
    let Some((mut h, _)) = develop_corpus_raw(dir.path()) else {
        return;
    };
    let profile = |s: &State| s.lib.develop().opened().map(|o| o.develop.profile);
    let edited = |s: &State| {
        lf_settings::DevelopSettings::from_settings(&s.lib.develop().edit_state().unwrap().settings)
            .profile
    };
    let answers = h.state().opened;
    h.get_by_label("Linear").click();
    h.step(); // commits Linear
    h.get_by_label("Camera").click();
    h.step(); // asks for the Linear decode, and commits Camera before it can answer
    assert_eq!(edited(h.state()), lf_settings::Profile::Camera);
    step_until(&mut h, "the Linear decode to answer", |s| {
        s.opened > answers
    });
    step_until(&mut h, "the photo decoded as Camera again", |s| {
        profile(s) == Some(lf_settings::Profile::Camera) && !s.lib.develop().wants_reopen()
    });
    assert_eq!(edited(h.state()), lf_settings::Profile::Camera);
}

fn curve_rect(h: &Harness<'static, State>) -> egui::Rect {
    h.get_by_label_contains("Tone curve: click to add a point")
        .rect()
}

#[test]
fn undo_clears_the_selected_curve_point() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    // Fold Light and Colour so the curve is on screen, then unfold the curve.
    for section in ["Light", "Colour", "Tone curve"] {
        h.get_by_role_and_label(Role::Button, section).click();
        h.step();
        h.step();
    }
    let rect = curve_rect(&h);
    h.get_by_label_contains("Tone curve: click to add a point")
        .click();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().lib.develop().curve_selected(), Some(1));
    assert_eq!(
        h.state()
            .lib
            .develop()
            .edit_state()
            .unwrap()
            .settings
            .curve
            .rgb
            .len(),
        3
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    h.step();
    assert_eq!(
        h.state()
            .lib
            .develop()
            .edit_state()
            .unwrap()
            .settings
            .curve
            .rgb
            .len(),
        2,
        "the point was undone"
    );
    assert_eq!(h.state().lib.develop().curve_selected(), None);
    let _ = rect;
}

/// A gesture still in progress when the photo is changed is saved, even while an import
/// batch holds the catalogue's write lock for longer than the quick timeout.
#[test]
fn changing_photo_mid_gesture_saves_it_past_a_busy_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let first = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(first);
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open", |s| s.lib.develop().is_ready());

    // Drag exposure without letting go: the edit is live but not committed.
    let start = h.get_by_label("Exposure").rect().center();
    h.drag_at(start);
    h.step();
    for i in 1..=3 {
        h.hover_at(start + egui::vec2(20.0 * i as f32, 0.0));
        h.step();
    }
    let edit = h.state().lib.develop().edit_state().unwrap();
    assert!(edit.dirty && edit.settings.exposure > 0.0);
    let dragged = edit.settings.exposure;

    let db = dir.path().join("catalogue/.lensflow/catalog.sqlite");
    let (locked_tx, locked) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let other = rusqlite::Connection::open(db).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        other.execute_batch("ROLLBACK").unwrap();
    });
    locked.recv().unwrap();
    h.key_press(Key::ArrowRight);
    h.step();
    holder.join().unwrap();
    h.drop_at(start);
    h.step();

    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    assert_eq!(
        lf_catalog::edits::get_settings(&catalog, first)
            .unwrap()
            .expect("the first photo's edit was saved")
            .exposure,
        dragged
    );
    // An error toast stays until closed, so a failed save would still be listed.
    let errors: Vec<&str> = h
        .state()
        .toasts
        .texts()
        .filter(|t| t.contains("could not be saved"))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

fn settings(h: &Harness<'static, State>) -> lf_settings::EditSettings {
    h.state()
        .lib
        .develop()
        .edit_state()
        .unwrap()
        .settings
        .clone()
}

#[test]
fn the_crop_tool_commits_once_on_enter_and_esc_puts_it_back() {
    use lf_app::develop::tools::ToolKind;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    let steps = |c: &lf_catalog::Catalog| lf_catalog::edits::history_len(c, id).unwrap();
    let entry = settings(&h);

    // R opens the crop tool; an aspect is part of the session, a turn commits at once.
    h.key_press(Key::R);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), Some(ToolKind::Crop));
    h.get_by_label("1:1").click();
    h.step();
    assert_eq!(settings(&h).aspect, "1:1");
    assert_eq!(steps(&catalog), 0, "the aspect waits for Enter");
    h.get_by_label("Rotate right").click();
    h.step();
    assert_eq!(settings(&h).rotate, lf_settings::Rotate::R90);
    assert_eq!(steps(&catalog), 2, "the photo before, and the turn");

    // Esc puts back what the tool was entered with (one more step, as the turn was saved).
    h.key_press(Key::Escape);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), None);
    assert_eq!(settings(&h), entry);
    assert_eq!(steps(&catalog), 3);

    // A whole session is one step on Enter.
    h.key_press(Key::R);
    h.step();
    h.get_by_label("4:5").click();
    h.step();
    h.get_by_label("Switch portrait / landscape").click();
    h.step();
    assert_eq!(settings(&h).aspect, "5:4");
    assert_eq!(steps(&catalog), 3);
    h.key_press(Key::Enter);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), None);
    assert_eq!(steps(&catalog), 4);
    assert_eq!(
        lf_catalog::edits::get_settings(&catalog, id)
            .unwrap()
            .unwrap()
            .aspect,
        "5:4"
    );

    // Q opens the spot tool, Esc leaves it with nothing to save.
    h.key_press(Key::Q);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), Some(ToolKind::Spots));
    h.get_by_label("0 of 48 spots");
    h.key_press(Key::Escape);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), None);
    assert_eq!(steps(&catalog), 4);
}

fn history(root: &Path, id: i64) -> i64 {
    let catalog = lf_catalog::Catalog::open(root).unwrap();
    lf_catalog::edits::history_len(&catalog, id).unwrap()
}

/// Clicks the text field of an open dialog and types `text` into it.
fn type_into_field(h: &mut Harness<'static, State>, text: &str) {
    h.get_by_role(Role::TextInput).click();
    h.step();
    h.get_by_role(Role::TextInput).type_text(text);
    h.step();
}

#[test]
fn looks_presets_and_versions_from_the_side_panel() {
    use lf_app::develop::panels::{looks, presets, versions};
    use lf_app::dialogs::save_preset;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    h.state_mut().lib.set_studio(Some(7));
    h.step();

    // A look: one click, one history step.
    h.get_by_label(looks::TITLE).click();
    // The section opens over a few frames.
    h.run_steps(20);
    h.get_by_label("B&W classic").click();
    h.step();
    assert!(settings(&h).bw, "the look applied");
    assert_eq!(history(&root, id), 2, "the photo before, and the look");

    // A preset: saved here for studio 7, listed, and a sync asked for.
    h.get_by_label(presets::TITLE).click();
    // The section opens over a few frames.
    h.run_steps(20);
    assert!(h.query_by_label(presets::EMPTY).is_some());
    h.get_by_label(presets::SAVE).click();
    // A new dialog is laid out unseen on its first frame.
    h.step();
    h.step();
    assert!(
        h.query_by_label(save_preset::TITLE).is_some(),
        "the dialog opened"
    );
    type_into_field(&mut h, "Mono");
    h.get_by_label(save_preset::SAVE).click();
    h.step();
    h.step();
    assert!(
        h.query_by_label(save_preset::TITLE).is_none(),
        "the dialog closed"
    );
    let catalog = lf_catalog::Catalog::open(&root).unwrap();
    let rows = lf_catalog::presets::list(&catalog, 7).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].settings["bw"], serde_json::json!(true));
    assert!(lf_catalog::presets::list(&catalog, 8).unwrap().is_empty());
    assert!(
        h.state_mut().lib.take_presets_changed(),
        "a sync is asked for"
    );
    assert!(!h.state_mut().lib.take_presets_changed());

    // The same name again is refused.
    h.get_by_label(presets::SAVE).click();
    // A new dialog is laid out unseen on its first frame.
    h.step();
    h.step();
    type_into_field(&mut h, "Mono");
    h.get_by_label(save_preset::SAVE).click();
    h.step();
    h.step();
    assert!(h.query_by_label(save_preset::TAKEN).is_some());
    h.get_by_label(save_preset::CANCEL).click();
    h.step();

    // Applying it puts black & white back after a change.
    h.get_by_label("Natural").click();
    h.step();
    assert!(!settings(&h).bw);
    h.get_by_label("Mono").click();
    h.step();
    assert!(settings(&h).bw, "the preset applied");
    let steps = history(&root, id);

    // Versions: added (tagged current), its preview made once, used, deleted.
    h.get_by_label(versions::TITLE).click();
    // The section opens over a few frames.
    h.run_steps(20);
    h.get_by_label(versions::ADD).click();
    // A new dialog is laid out unseen on its first frame.
    h.step();
    h.step();
    h.get_by_label(versions::SAVE).click();
    h.step();
    h.step();
    assert!(h.query_by_label("Version 1").is_some());
    assert!(h.query_by_label(versions::CURRENT).is_some());
    for _ in 0..5 {
        h.step();
    }
    assert_eq!(h.state().lib.develop().version_previews(), 1);
    drag_slider(&mut h, "Exposure", 60.0);
    assert!(
        h.query_by_label(versions::CURRENT).is_none(),
        "the edit moved on"
    );
    h.get_by_label("Use Version 1").click();
    // A new dialog is laid out unseen on its first frame.
    h.step();
    h.step();
    assert!(h.query_by_label(versions::USE_QUESTION).is_some());
    h.get_by_label(versions::USE_CONFIRM).click();
    h.step();
    h.step();
    assert_eq!(exposure(&h), 0.0, "the version's settings are back");
    assert!(h.query_by_label(versions::CURRENT).is_some());
    assert_eq!(
        history(&root, id),
        steps + 2,
        "the drag, then the version, each one step"
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    h.step();
    assert!(
        exposure(&h) > 0.0,
        "the edit before the version is in history"
    );
    h.get_by_label("Delete Version 1").click();
    // A new dialog is laid out unseen on its first frame.
    h.step();
    h.step();
    h.get_by_label(versions::DELETE_CONFIRM).click();
    h.step();
    h.step();
    assert!(h.query_by_label("Version 1").is_none());

    // Deleting the preset asks first and leaves a tombstone for the sync.
    h.get_by_label(&presets::delete_label("Mono")).click();
    // A new dialog is laid out unseen on its first frame.
    h.step();
    h.step();
    h.get_by_label(presets::DELETE_CONFIRM).click();
    h.step();
    h.step();
    assert!(lf_catalog::presets::list(&catalog, 7).unwrap().is_empty());
    assert_eq!(
        lf_catalog::presets::tombstones(&catalog, 7).unwrap().len(),
        1
    );
    assert!(h.state_mut().lib.take_presets_changed());
}

#[test]
fn copy_then_paste_onto_one_photo_and_then_on_many() {
    use lf_app::dialogs::{batch, group_picker};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    import_folder(&mut h, &shoot, 3);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    let paths: Vec<PathBuf> = h
        .state()
        .lib
        .photos()
        .iter()
        .map(|p| p.path.clone())
        .collect();
    let root = dir.path().join("catalogue");

    // Edit the first photo in Develop, then copy its edits there.
    h.state_mut().lib.selection.click(ids[0]);
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open", |s| s.lib.develop().is_ready());
    drag_slider(&mut h, "Exposure", 60.0);
    let dragged = exposure(&h);
    let both = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    h.key_press_modifiers(both, Key::C);
    h.step();
    assert!(h.query_by_label(group_picker::COPY_TITLE).is_some());
    h.get_by_label(group_picker::COPY).click();
    h.step();
    let slot = h.state().lib.clipboard().slot().cloned().unwrap();
    assert_eq!(slot["exposure"].as_f64(), Some(dragged));
    assert!(slot.get("crop").is_none(), "crop is not copied by default");

    // Back in the library: one other photo is pasted at once.
    h.key_press(Key::G);
    h.step();
    h.state_mut().lib.selection.click(ids[1]);
    h.key_press_modifiers(both, Key::V);
    h.step();
    assert!(h.state().lib.batch().is_none(), "no dialog for one photo");
    let catalog = lf_catalog::Catalog::open(&root).unwrap();
    let exposure_of = |id| {
        lf_catalog::edits::get_settings(&catalog, id)
            .unwrap()
            .map(|s| s.exposure)
    };
    assert_eq!(exposure_of(ids[1]), Some(dragged));
    assert!(lf_catalog::sidecar::path_for(&paths[1]).exists());

    // Every photo: "Apply to 3 photos".
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.step();
    h.key_press_modifiers(both, Key::V);
    h.step();
    assert!(h.query_by_label(&batch::title(3)).is_some());
    h.get_by_label(&batch::apply_label(3)).click();
    h.step();
    step_until(&mut h, "the batch to finish", |s| {
        s.lib
            .batch()
            .and_then(|b| b.progress())
            .is_some_and(|p| p.finished)
    });
    assert!(h.query_by_label("3 of 3 edited.").is_some());
    for (id, path) in ids.iter().zip(&paths) {
        assert_eq!(exposure_of(*id), Some(dragged));
        let side = lf_catalog::sidecar::read(&lf_catalog::sidecar::path_for(path))
            .unwrap()
            .unwrap();
        assert_eq!(side.settings.exposure, dragged);
    }
    h.get_by_role_and_label(Role::Button, batch::DONE).click();
    h.step();
    assert!(h.state().lib.batch().is_none());
}

#[test]
fn paste_works_in_the_library_before_any_photo_is_opened() {
    use lf_app::dialogs::group_picker;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    let second = h.state().lib.photos()[1].path.clone();
    let both = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    h.state_mut().lib.selection.click(ids[0]);
    h.key_press_modifiers(both, Key::C);
    h.step();
    h.step();
    h.get_by_label(group_picker::COPY).click();
    h.step();
    assert!(h.state().lib.clipboard().slot().is_some());
    h.state_mut().lib.selection.click(ids[1]);
    h.key_press_modifiers(both, Key::V);
    h.step();
    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    assert!(
        lf_catalog::edits::get_settings(&catalog, ids[1])
            .unwrap()
            .is_some(),
        "pasted: {:?}",
        h.state().toasts.texts().collect::<Vec<_>>()
    );
    assert!(lf_catalog::sidecar::path_for(&second).exists());
}

#[test]
fn export_to_disk_from_the_shortcut_remembers_the_form_and_refuses_existing_files() {
    use lf_app::dialogs::export;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let both = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;

    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.step();
    h.key_press_modifiers(both, Key::E);
    h.step();
    h.step();
    assert!(h.query_by_label(&export::title(2)).is_some());
    // Nothing chosen yet: Export says what is missing.
    h.get_by_role_and_label(Role::Button, export::EXPORT)
        .click();
    h.step();
    h.step();
    assert!(h.query_by_label(export::NO_FOLDER).is_some());

    // A typed folder is checked off the UI thread, then used.
    h.state_mut()
        .lib
        .export_mut()
        .unwrap()
        .set_typed(&out.to_string_lossy());
    h.step();
    h.get_by_role_and_label(Role::Button, export::USE_FOLDER)
        .click();
    step_until(&mut h, "the typed folder", |s| {
        s.lib.export_folder().as_deref() == Some(out.as_path())
    });
    h.get_by_role_and_label(Role::Button, export::EXPORT)
        .click();
    h.step();
    step_until_label(&mut h, "Exported 2 photos.");
    // The dialog's new contents are laid out unseen first.
    h.step();
    assert!(out.join("p0.jpg").exists() && out.join("p1.jpg").exists());
    assert!(h.query_by_label(export::SHOW_FOLDER).is_some());
    let remembered = h.state().prefs.last_export.clone().unwrap();
    assert_eq!(
        remembered["destination"]["Folder"],
        out.to_string_lossy().as_ref()
    );
    h.get_by_role_and_label(Role::Button, export::DONE).click();
    h.step();
    assert!(h.state().lib.export().is_none());

    // Again: the folder is remembered, and the files there now stop the run up front.
    h.key_press_modifiers(both, Key::E);
    h.step();
    h.step();
    assert_eq!(h.state().lib.export_folder(), Some(out.clone()));
    h.get_by_role_and_label(Role::Button, export::EXPORT)
        .click();
    h.step();
    step_until_label(
        &mut h,
        "p0.jpg already exists. Turn on 'Replace existing files' or choose another folder.",
    );
    assert!(
        h.query_by_label(&export::title(2)).is_some(),
        "the form stays"
    );
}

/// Preferences → Change…: the app swaps in a fresh library asking for another folder. It can
/// go back to the catalogue it left; a folder list from the old catalogue arriving late, and
/// work for it, are ignored.
#[test]
fn changing_the_catalogue_restarts_the_library_and_can_keep_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let old_root = h.state().lib.root().unwrap().to_path_buf();
    let generation = h.state().lib.develop().generation();

    let gpu = try_shared_gpu().unwrap();
    let mut fresh = Library::new(gpu, dir.path().join("frames"), &h.state().prefs);
    fresh.choose_another_root(Some(old_root.clone()), generation);
    h.state_mut().lib = fresh;
    h.step();
    assert_eq!(h.state().lib.stage, Stage::NoRoot);
    h.get_by_label(FIRST_RUN);
    h.get_by_label(&old_root.display().to_string());
    // Late answers for the old catalogue change nothing here.
    h.state_mut().handle(Msg::Folders {
        root: dir.path().join("elsewhere"),
        result: Ok(Vec::new()),
    });
    h.state_mut().handle(Msg::ImportDone(Err("stopped".into())));
    assert!(h.state().toasts.texts().all(|t| !t.contains("stopped")));

    h.get_by_role_and_label(Role::Button, KEEP_ROOT).click();
    step_until(&mut h, "the catalogue to open again", |s| {
        s.lib.stage == Stage::Ready
    });
    step_until(&mut h, "the photos", |s| s.lib.photos().len() == 2);
    assert_eq!(h.state().lib.root(), Some(old_root.as_path()));
    assert!(h.state().lib.kept_root().is_none());
    assert!(h.state().lib.develop().generation() >= generation);
}

/// Runs a menu action as the app does (through `Library::run`).
fn run_action(h: &mut Harness<'static, State>, action: Action) -> bool {
    let s = h.state_mut();
    let mut env = Env {
        jobs: &s.jobs,
        toasts: &mut s.toasts,
        prefs: &mut s.prefs,
        config_dir: &s.config,
    };
    s.lib.run(action, &egui::Context::default(), &mut env)
}

#[test]
fn reset_all_edits_is_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    drag_slider(&mut h, "Exposure", 60.0);
    step_until(&mut h, "the drag to be committed", |s| {
        s.lib.develop().edit_state().is_some_and(|e| !e.dirty)
    });
    let dragged = exposure(&h);
    assert!(dragged > 0.0);
    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    let before = lf_catalog::edits::history_len(&catalog, id).unwrap();

    assert!(run_action(&mut h, Action::ResetAll));
    h.step();
    assert_eq!(exposure(&h), 0.0);
    assert_eq!(
        lf_catalog::edits::history_len(&catalog, id).unwrap(),
        before + 1,
        "one step"
    );
    assert!(run_action(&mut h, Action::Undo));
    h.step();
    assert_eq!(exposure(&h), dragged, "one undo brings the edit back");
}

#[test]
fn a_photo_list_from_another_catalogue_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let folder_id = h.state().lib.photos()[0].folder_id;
    h.state_mut().handle(Msg::Photos {
        root: dir.path().join("elsewhere"),
        folder_id,
        result: Ok(Vec::new()),
    });
    h.step();
    assert_eq!(
        h.state().lib.photos().len(),
        2,
        "the old catalogue's list is dropped"
    );
}

/// Preferences → Change…: the open gesture is committed, an import is told to stop, and the
/// waiting edit file is written before the library is replaced; while "Match a reference"
/// is open, a toast says why not and nothing happens.
#[test]
fn winding_down_for_another_catalogue_commits_stops_then_writes() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let photo = develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;

    h.state_mut().lib.open_match();
    let s = h.state_mut();
    assert!(s.lib.prepare_restart(&mut s.toasts).is_none());
    assert!(
        s.toasts.texts().any(|t| t.contains("Match a reference")),
        "the toast says why"
    );
    assert!(
        !s.lib
            .import_stop_flag()
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    h.state_mut().lib.close_match();

    h.state_mut().lib.develop_mut().edit(|s| s.exposure = 1.5);
    let sidecar = lf_catalog::sidecar::path_for(&photo);
    assert!(!sidecar.exists());
    let s = h.state_mut();
    let (keep, generation) = s.lib.prepare_restart(&mut s.toasts).expect("not blocked");
    assert_eq!(
        keep.as_deref(),
        Some(dir.path().join("catalogue").as_path())
    );
    assert!(generation >= 1);
    assert!(
        s.lib
            .import_stop_flag()
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    let written = lf_catalog::sidecar::read(&sidecar).unwrap().unwrap();
    assert_eq!(
        written.settings.exposure, 1.5,
        "committed, then written at once"
    );
    let catalog = lf_catalog::Catalog::open(&dir.path().join("catalogue")).unwrap();
    assert_eq!(
        lf_catalog::edits::get_settings(&catalog, id)
            .unwrap()
            .unwrap()
            .exposure,
        1.5
    );
}

#[test]
fn an_edit_in_develop_remakes_the_thumbnail_and_marks_the_photo_edited() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    let quality = lf_app::catalog_ext::THUMB_QUALITY;
    step_until(&mut h, "the first thumbnail", |s| {
        let (made, _, pending) = s.lib.thumbnail_counts();
        made == 1 && pending == 0
    });
    assert!(!h.state().lib.photos()[0].edited);

    drag_slider(&mut h, "Exposure", 60.0);
    step_until(&mut h, "the drag to be committed", |s| {
        s.lib.develop().edit_state().is_some_and(|e| !e.dirty)
    });
    let catalog = lf_catalog::Catalog::open(&root).unwrap();
    assert!(
        catalog.fresh_thumbnail(id, quality).unwrap().is_none(),
        "the stored thumbnail is stale after the edit"
    );

    // G leaves the photo: its thumbnail is asked for again at once and its row is re-read.
    h.key_press(Key::G);
    h.step();
    assert_eq!(h.state().lib.mode, Mode::Library);
    step_until(&mut h, "a fresh thumbnail and the edited flag", |s| {
        let (made, _, pending) = s.lib.thumbnail_counts();
        s.lib.photos()[0].edited
            && made == 1
            && pending == 0
            && catalog.fresh_thumbnail(id, quality).unwrap().is_some()
    });
}

#[test]
fn edits_in_develop_remake_the_thumbnail_a_quiet_second_later() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    let quality = lf_app::catalog_ext::THUMB_QUALITY;
    drag_slider(&mut h, "Exposure", 60.0);
    step_until(&mut h, "the drag to be committed", |s| {
        s.lib.develop().edit_state().is_some_and(|e| !e.dirty)
    });
    // Still in Develop: the filmstrip's thumbnail follows once the edits go quiet.
    let catalog = lf_catalog::Catalog::open(&root).unwrap();
    step_until(
        &mut h,
        "the filmstrip's thumbnail and the edited flag",
        |s| s.lib.photos()[0].edited && catalog.fresh_thumbnail(id, quality).unwrap().is_some(),
    );
    assert_eq!(h.state().lib.mode, Mode::Develop(id));
}

#[test]
fn an_import_finishing_leaves_develop_and_another_folder_alone() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    // Nothing shown yet: the first import shows its folder.
    develop_one(&mut h, dir.path());
    let first_folder = h.state().lib.photos()[0].folder_id;
    let open = h.state().lib.photos()[0].id;
    assert_eq!(h.state().lib.mode, Mode::Develop(open));

    // Another folder's import ends while a photo is open in Develop.
    let other = dir.path().join("other");
    pngs(&other, 2);
    let scan = import::scan(&other);
    h.state_mut().handle(Msg::Scanned {
        folder: other.clone(),
        scan,
    });
    step_until(&mut h, "the import", |s| !s.lib.importing());
    step_until_label(&mut h, "other  (2)");
    assert_eq!(h.state().lib.mode, Mode::Develop(open), "still in Develop");
    assert_eq!(h.state().lib.current_folder(), Some(first_folder));
    assert!(h.state().lib.develop().is_ready());

    // In the library on the first folder, a rescan of the other one keeps the first shown.
    h.key_press(Key::G);
    h.step();
    let scan = import::scan(&other);
    h.state_mut().handle(Msg::Scanned {
        folder: other.clone(),
        scan,
    });
    step_until(&mut h, "the rescan", |s| {
        !s.lib.importing() && !s.lib.loading()
    });
    assert_eq!(h.state().lib.current_folder(), Some(first_folder));
    assert_eq!(h.state().lib.photos().len(), 1);
}
