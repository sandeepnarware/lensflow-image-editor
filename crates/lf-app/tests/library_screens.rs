//! The library, driven headless through its accessibility tree: the first-run and
//! network-drive screens, typed folder paths, then a real catalogue with imports, the grid,
//! thumbnails, the loading state and the "mark missing" question.
//! The folder pickers are never clicked (they would open a system dialog); their answers
//! are fed in as the messages the pickers send.

use eframe::egui::{self, Key, accesskit::Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_app::catalog_ext::{FOLDER_MISSING, NETWORK_WARNING};
use lf_app::jobs::{Jobs, Lane, Msg};
use lf_app::library::cull::{Filter, FlagFilter};
use lf_app::library::import_ui::{Chosen, FIRST_RUN, KEEP_ROOT, Purpose, TYPE_PATH};
use lf_app::library::{Env, Library, Mode, Stage};
use lf_app::prefs::Prefs;
use lf_app::shortcuts::Action;
use lf_app::toast::Toasts;
use lf_catalog::cull::{Flag, Label};
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

/// A drag over the grid draws a box and selects every photo it covers; Ctrl+click adds and
/// removes one. (The fit of a box to cells, and the Ctrl+drag rule, are unit-tested in
/// `library::grid`.)
#[test]
fn a_drag_box_selects_what_it_covers_and_ctrl_click_adds_or_removes() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 4);
    import_folder(&mut h, &shoot, 4);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    assert_eq!(
        h.state().lib.selection.current,
        vec![ids[0]],
        "the import selects the first photo"
    );

    // The first two cells share a row; a box from one's corner to the other's covers both,
    // and nothing of the row below.
    let first = h.get_by_label("p0.png").rect();
    let second = h.get_by_label("p1.png").rect();
    assert!(
        (first.center().y - second.center().y).abs() < 0.5,
        "the first two cells are side by side"
    );
    let start = first.min + egui::vec2(10.0, 10.0);
    let end = second.max - egui::vec2(10.0, 10.0);
    h.drag_at(start);
    h.step();
    for i in 1..=4 {
        h.hover_at(start + (end - start) * (i as f32 / 4.0));
        h.step();
    }
    // While the button is down the box is live and the selection already follows it.
    let boxed = h
        .state()
        .lib
        .marquee
        .as_ref()
        .expect("a box is being dragged");
    assert!(boxed.live, "grown past a nudge");
    assert_eq!(h.state().lib.selection.current, vec![ids[0], ids[1]]);
    h.drop_at(end);
    h.step();
    h.step();
    assert!(h.state().lib.marquee.is_none(), "the box is gone");
    assert_eq!(h.state().lib.selection.current, vec![ids[0], ids[1]]);
    assert_eq!(h.state().lib.selection.active(), Some(ids[1]));
    h.get_by_label("4 photos · 2 selected");

    // Ctrl+click a third photo: it is added, and is active.
    let third = h.get_by_label("p2.png").rect().center();
    ctrl_click_at(&mut h, third);
    assert_eq!(
        h.state().lib.selection.current,
        vec![ids[0], ids[1], ids[2]]
    );
    assert_eq!(h.state().lib.selection.active(), Some(ids[2]));
    // Ctrl+click one already selected: it comes out.
    let second_again = h.get_by_label("p1.png").rect().center();
    ctrl_click_at(&mut h, second_again);
    assert_eq!(h.state().lib.selection.current, vec![ids[0], ids[2]]);
    // A plain click replaces the selection with one photo.
    let fourth = h.get_by_label("p3.png").rect().center();
    h.hover_at(fourth);
    h.step();
    h.drag_at(fourth);
    h.step();
    h.drop_at(fourth);
    h.step();
    h.step();
    assert_eq!(h.state().lib.selection.current, vec![ids[3]]);
    assert!(h.state().lib.marquee.is_none(), "a click draws no box");
}

/// Clicks at `pos` with Ctrl held, as a real keyboard would hold it across the press and the
/// release.
fn ctrl_click_at(h: &mut Harness<'static, State>, pos: egui::Pos2) {
    let command = egui::Modifiers::COMMAND;
    h.hover_at(pos);
    h.step();
    h.event(egui::Event::ModifiersChanged(command));
    h.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: command,
    });
    h.step();
    h.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: command,
    });
    h.step();
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
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

    // Its Amount (100 % sits in the middle of 0–200 %): dragged towards half, one more
    // history step, the switch kept and the numbers part-way.
    let full = settings(&h);
    let rect = h
        .get_by_role_and_label(Role::Slider, lf_app::develop::extras::AMOUNT)
        .rect();
    let start = rect.center();
    let dx = -rect.width() / 4.0;
    h.drag_at(start);
    h.step();
    for i in 1..=4 {
        h.hover_at(start + egui::vec2(dx * i as f32 / 4.0, 0.0));
        h.step();
    }
    h.drop_at(start + egui::vec2(dx, 0.0));
    h.step();
    h.step();
    let part = settings(&h);
    assert!(part.bw, "a switch stays on above 0 %");
    assert_ne!(part, full, "the numbers moved");
    assert_eq!(history(&root, id), 3, "the whole drag is one step");

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
    // Done with looks and presets: closed, so the adjust panels below stay in view.
    h.get_by_label(looks::TITLE).click();
    h.run_steps(20);
    h.get_by_label(presets::TITLE).click();
    h.run_steps(20);

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
    h.get_by_label(presets::TITLE).click();
    h.run_steps(20);
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
    // Its edit file follows a quiet second later, off the UI thread.
    let pasted = lf_catalog::sidecar::path_for(&paths[1]);
    step_until(&mut h, "the pasted photo's edit file", |_| pasted.exists());

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
    let written = lf_catalog::sidecar::path_for(&second);
    step_until(&mut h, "the edit file", |_| written.exists());
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

/// Closing the window while an export runs asks first; quitting stops it after the current
/// photo (here, before the first: the export was still waiting for the lane).
#[test]
fn quitting_during_an_export_asks_and_stops_it() {
    use lf_app::dialogs::{export, quit};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    assert_eq!(h.state().lib.quit_question(), None);

    // Hold the Long lane so the export cannot start yet.
    let (release, held) = mpsc::channel::<()>();
    h.state().jobs.spawn(Lane::Long, "hold", move || {
        let _ = held.recv();
        Msg::Cleared
    });
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.step();
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::E);
    h.step();
    h.step();
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
    assert_eq!(h.state().lib.quit_question(), Some(quit::EXPORT_RUNNING));
    assert_eq!(h.state().lib.running_export_folder(), Some(out.clone()));

    h.state().lib.stop_long_work();
    release.send(()).unwrap();
    step_until_label(&mut h, "Stopped after 0 photos.");
    assert!(!out.join("p0.jpg").exists(), "nothing was exported");
    assert_eq!(h.state().lib.quit_question(), None);
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

#[test]
fn a_folder_being_scanned_cannot_be_removed_and_an_added_folder_is_shown() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let shoot_id = h
        .state()
        .lib
        .current_folder()
        .expect("the first import is shown");

    // A rescan waits on the Long lane: its folder cannot be removed while it is read.
    let (release, held) = mpsc::channel::<()>();
    h.state().jobs.spawn(Lane::Long, "hold", move || {
        let _ = held.recv();
        Msg::Cleared
    });
    h.get_by_label("shoot  (2)").click_secondary();
    h.step();
    h.step();
    h.get_by_role_and_label(Role::Button, "Rescan").click();
    h.step();
    h.step();
    h.get_by_label("shoot  (2)").click_secondary();
    h.step();
    h.step();
    assert!(
        h.get_by_role_and_label(Role::Button, "Remove from catalogue…")
            .accesskit_node()
            .is_disabled(),
        "a folder being scanned stays until its import ends"
    );
    h.key_press(Key::Escape);
    h.step();
    release.send(()).unwrap();
    step_until(&mut h, "the rescan", |s| {
        !s.lib.importing() && !s.lib.loading()
    });

    // "Add folder…" shows the folder it added, though another folder was shown.
    let added = dir.path().join("added");
    pngs(&added, 3);
    assert!(run_action(&mut h, Action::AddFolder));
    h.step();
    let ticket = h.state().lib.ticket();
    h.state_mut().handle(Msg::FolderChosen {
        purpose: Purpose::AddFolder,
        ticket,
        result: Ok(Some(Chosen {
            path: added.clone(),
            network: false,
        })),
    });
    step_until(&mut h, "the added folder's photos", |s| {
        !s.lib.importing() && !s.lib.loading() && s.lib.photos().len() == 3
    });
    let shown = h.state().lib.current_folder();
    assert!(
        shown.is_some() && shown != Some(shoot_id),
        "the added folder is shown"
    );
}

#[test]
fn a_match_queued_behind_an_import_says_it_is_waiting() {
    use lf_app::dialogs::match_reference::{AiGate, MatchStep, WAITING_IMPORT};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.step();
    h.state_mut().lib.open_match();
    h.state_mut().handle(Msg::Match(MatchStep::Picked {
        ticket: 0,
        path: Some(shoot.join("p1.png")),
    }));

    // An import is queued behind other long work; the fit queues behind both.
    let (release, held) = mpsc::channel::<()>();
    h.state().jobs.spawn(Lane::Long, "hold", move || {
        let _ = held.recv();
        Msg::Cleared
    });
    let other = dir.path().join("other");
    pngs(&other, 1);
    let scan = import::scan(&other);
    h.state_mut().handle(Msg::Scanned {
        folder: other.clone(),
        scan,
    });
    let s = h.state_mut();
    s.lib.start_match(None, AiGate::default(), &s.jobs);
    h.step();
    assert_eq!(
        h.state().lib.matching().and_then(|m| m.status()),
        Some(WAITING_IMPORT)
    );
    step_until_label(&mut h, WAITING_IMPORT);
    release.send(()).unwrap();
    step_until(&mut h, "the fit to start", |s| {
        s.lib
            .matching()
            .and_then(|m| m.status())
            .is_none_or(|t| t != WAITING_IMPORT)
    });
    h.state_mut().lib.close_match();
}

/// The catalogue's marks for `ids`, read from disk (what was saved, not what is shown).
fn saved_marks(dir: &Path, ids: &[i64]) -> Vec<(Flag, u8, Option<Label>)> {
    let c = lf_catalog::Catalog::open(&dir.join("catalogue")).unwrap();
    ids.iter()
        .map(|id| {
            let r = c.photo(*id).unwrap().unwrap();
            (r.flag, r.rating, r.label)
        })
        .collect()
}

#[test]
fn culling_keys_mark_the_selection_and_the_filter_walks_on() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 4);
    import_folder(&mut h, &shoot, 4);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();

    // Three stars on every photo.
    h.state_mut().lib.selection.select_all(&ids);
    h.key_press(Key::Num3);
    step_until(&mut h, "three stars saved", |_| {
        saved_marks(dir.path(), &ids).iter().all(|m| m.1 == 3)
    });
    assert!(h.state().lib.photos().iter().all(|p| p.rating == 3));

    // The first photo alone: rejected, then a red label that a second press takes off.
    h.state_mut().lib.selection.click(ids[0]);
    h.key_press(Key::X);
    h.key_press(Key::Num6);
    step_until(&mut h, "reject and red saved", |_| {
        saved_marks(dir.path(), &ids[..1]) == [(Flag::Rejected, 3, Some(Label::Red))]
    });
    h.get_by_label_contains("Rejected, 3 stars, Red label");
    h.key_press(Key::Num6);
    step_until(&mut h, "the red label off", |_| {
        saved_marks(dir.path(), &ids[..1])[0].2.is_none()
    });
    assert_eq!(
        saved_marks(dir.path(), &ids[1..2]),
        [(Flag::Unflagged, 3, None)]
    );

    // Hide rejected: the first photo goes, and the status bar says so.
    let filter = Filter {
        flag: FlagFilter::NotRejected,
        ..Filter::default()
    };
    h.state_mut().lib.set_filter(filter);
    h.step();
    assert_eq!(h.state().lib.shown(), &ids[1..]);
    assert_eq!(
        h.state().lib.selection.active(),
        Some(ids[1]),
        "the photo that took the hidden one's place is active"
    );
    h.get_by_label("3 of 4 photos · 1 selected");
    // Rejecting the active photo with the filter on moves to the next, as culling wants.
    h.key_press(Key::X);
    h.step();
    assert_eq!(h.state().lib.shown(), &ids[2..]);
    assert_eq!(h.state().lib.selection.active(), Some(ids[2]));
    step_until(&mut h, "the second reject saved", |_| {
        saved_marks(dir.path(), &ids[1..2])[0].0 == Flag::Rejected
    });

    // Picked only: nothing is picked, so the grid says so and offers everything back.
    h.state_mut().lib.set_filter(Filter {
        flag: FlagFilter::Picked,
        ..Filter::default()
    });
    h.step();
    h.get_by_label(lf_app::library::NONE_MATCH);
    h.get_by_label("Show all photos").click();
    h.step();
    h.step();
    assert_eq!(h.state().lib.shown(), ids.as_slice());
    assert!(!h.state().lib.filter().narrows());
}

#[test]
fn the_eyedropper_auto_tone_and_auto_white_balance_in_develop() {
    use lf_app::develop::tools::ToolKind;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    // One warm grey photo (as if lit by tungsten), a little dark.
    let shoot = dir.path().join("shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    image::RgbaImage::from_pixel(300, 200, image::Rgba([120, 96, 70, 255]))
        .save(shoot.join("warm.png"))
        .unwrap();
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open", |s| s.lib.develop().is_ready());
    let root = dir.path().join("catalogue");

    // W opens the eyedropper; a click on the photo makes that grey neutral and closes it.
    h.key_press(Key::W);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), Some(ToolKind::WhiteBalance));
    h.get_by_label(lf_app::develop::tools::WB_HINT);
    // The photo fills the middle of the window (a uniform colour: anywhere on it will do).
    let centre = egui::pos2(600.0, 330.0);
    h.hover_at(centre);
    h.step();
    h.drag_at(centre);
    h.step();
    h.drop_at(centre);
    h.step();
    h.step();
    let picked = settings(&h);
    assert!(
        picked.temperature < 0.0,
        "warm light cooled: {}",
        picked.temperature
    );
    assert_eq!(
        h.state().lib.develop().tool(),
        None,
        "one click, then it closes"
    );
    let after_pick = history(&root, id);

    // Back to as shot, then Ctrl+Shift+U finds the same cast on its own.
    h.state_mut()
        .lib
        .develop_mut()
        .edit(|s| (s.temperature, s.tint) = (0.0, 0.0));
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::U);
    h.step();
    let auto = settings(&h);
    assert!(
        (auto.temperature - picked.temperature).abs() <= 2.0
            && (auto.tint - picked.tint).abs() <= 2.0,
        "auto {:?} vs picked {:?}",
        (auto.temperature, auto.tint),
        (picked.temperature, picked.tint)
    );

    // Ctrl+U: Auto tone brightens the dark photo, one more history step.
    let before = history(&root, id);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::U);
    h.step();
    assert!(settings(&h).exposure > 0.0);
    assert!(history(&root, id) > before);
    assert!(before > after_pick, "each auto change is its own step");
}

#[test]
fn masks_are_added_moved_adjusted_and_removed_in_develop() {
    use lf_app::develop::tools::{ToolKind, masks as mask_tool};
    use lf_settings::masks::{Masks, Shape};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    let masks = |h: &Harness<'static, State>| Masks::from_settings(&settings(h));

    // M opens the tool; "Add radial" puts one in the middle, as one history step.
    h.key_press(Key::M);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), Some(ToolKind::Masks));
    let before = history(&root, id);
    h.get_by_label(mask_tool::ADD_RADIAL).click();
    h.step();
    h.step();
    assert_eq!(masks(&h).0.len(), 1);
    let added = history(&root, id);
    assert!(added > before, "the add is saved");

    // Its centre is the photo's centre: dragging it moves the mask, one step on release.
    let centre = egui::pos2(600.0, 330.0);
    let to = centre + egui::vec2(60.0, 0.0);
    h.hover_at(centre);
    h.step();
    h.drag_at(centre);
    h.step();
    for i in 1..=4 {
        h.hover_at(centre + (to - centre) * (i as f32 / 4.0));
        h.step();
    }
    h.drop_at(to);
    h.step();
    h.step();
    let Shape::Radial { cx, cy, .. } = masks(&h).0[0].shape else {
        panic!("a radial")
    };
    assert!(
        cx > 0.55 && (cy - 0.5).abs() < 0.02,
        "moved right: {cx}, {cy}"
    );
    assert_eq!(history(&root, id), added + 1, "the drag is one step");

    // Its own exposure (the panel's slider sets it the same way), as one more step.
    h.get_by_label_contains("1. Radial gradient");
    h.state_mut().lib.develop_mut().edit(|s| {
        let mut m = Masks::from_settings(s);
        m.0[0].local.exposure = 1.25;
        m.write_into(s);
    });
    {
        let s = h.state_mut();
        s.lib.develop_mut().commit(&mut s.toasts);
    }
    h.step();
    assert_eq!(masks(&h).0[0].local.exposure, 1.25);

    // Delete removes the selected mask; undo brings it back.
    h.key_press(Key::Delete);
    h.step();
    assert!(masks(&h).0.is_empty());
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    h.step();
    assert_eq!(masks(&h).0.len(), 1);
    // Esc leaves the tool; the mask stays.
    h.key_press(Key::Escape);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), None);
    assert_eq!(masks(&h).0.len(), 1);
}

#[test]
fn export_to_a_gallery_lists_galleries_uploads_and_says_how_it_went() {
    use httpmock::prelude::*;
    use lf_app::dialogs::gallery_export::{self, Stage as GalleryStage};
    use lf_app::gallery_job::GalleryLink;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 1);
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);

    // Without the gate's client, the action is off.
    assert!(!h.state().lib.enabled(Action::ExportToGallery));

    let server = MockServer::start();
    server.mock(|w, t| {
        w.method(GET).path("/api/galleries");
        t.status(200).json_body(serde_json::json!({"data": [
            {"id": 5, "title": "Ana & Raj", "event_date": "2026-11-02", "status": "published"}
        ]}));
    });
    server.mock(|w, t| {
        w.method(GET).path("/api/galleries/5/sections");
        t.status(200).json_body(serde_json::json!({"data": []}));
    });
    server.mock(|w, t| {
        w.method(POST).path("/api/galleries/5/uploads");
        t.status(201).json_body(serde_json::json!({"data": {
            "id": "8a1f0b0e-1111-4c4c-9999-123456789abc", "chunk_size": 7340032,
            "chunks_total": 1, "status": "pending", "direct": null
        }}));
    });
    let chunk = server.mock(|w, t| {
        w.method(PUT)
            .path("/api/galleries/5/uploads/8a1f0b0e-1111-4c4c-9999-123456789abc/chunks/0");
        t.status(200).json_body(serde_json::json!({"data": {}}));
    });
    server.mock(|w, t| {
        w.method(POST)
            .path("/api/galleries/5/uploads/8a1f0b0e-1111-4c4c-9999-123456789abc/complete");
        t.status(201)
            .json_body(serde_json::json!({"data": {"upload": {}, "photo": {"id": 900}}}));
    });
    let url = reqwest::Url::parse(&server.url("/api")).unwrap();
    let client = lf_account::Client::new(lf_account::Instance::custom(url).unwrap(), "1.0.0")
        .with_token("t")
        .with_studio("frames");
    h.state_mut()
        .lib
        .set_gallery_link(Some(GalleryLink { client }));
    assert!(h.state().lib.enabled(Action::ExportToGallery));

    // Open it as the ribbon would.
    {
        let ctx = h.ctx.clone();
        let s = h.state_mut();
        let mut env = Env {
            jobs: &s.jobs,
            toasts: &mut s.toasts,
            prefs: &mut s.prefs,
            config_dir: &s.config,
        };
        assert!(s.lib.run(Action::ExportToGallery, &ctx, &mut env));
    }
    step_until_label(&mut h, "Ana & Raj  ·  2026-11-02");
    h.get_by_label("Ana & Raj  ·  2026-11-02").click();
    h.step();
    h.step();
    step_until(&mut h, "the sections", |_| true);
    h.get_by_label(gallery_export::UPLOAD).click();
    h.step();
    step_until(&mut h, "the upload to finish", |s| {
        matches!(
            s.lib.gallery().map(|d| &d.stage),
            Some(GalleryStage::Done(_))
        )
    });
    chunk.assert();
    h.get_by_label("1 photo added to the gallery.");
    h.get_by_label(gallery_export::DONE).click();
    h.step();
    assert!(h.state().lib.gallery().is_none());
}

#[test]
fn a_brush_mask_is_painted_by_dragging_as_one_step() {
    use lf_app::develop::tools::{ToolKind, masks as mask_tool};
    use lf_settings::masks::{Masks, Shape};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    let masks = |h: &Harness<'static, State>| Masks::from_settings(&settings(h));

    h.key_press(Key::M);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), Some(ToolKind::Masks));
    h.get_by_label(mask_tool::ADD_BRUSH).click();
    h.step();
    h.step();
    let added = history(&root, id);
    let Shape::Brush { strokes } = &masks(&h).0[0].shape else {
        panic!("a brush")
    };
    assert!(strokes.is_empty());

    // A drag across the middle lays one stroke with several points, saved on release.
    let from = egui::pos2(520.0, 330.0);
    let to = egui::pos2(680.0, 330.0);
    h.hover_at(from);
    h.step();
    h.drag_at(from);
    h.step();
    for i in 1..=8 {
        h.hover_at(from + (to - from) * (i as f32 / 8.0));
        h.step();
    }
    h.drop_at(to);
    h.step();
    h.step();
    let Shape::Brush { strokes } = &masks(&h).0[0].shape else {
        panic!("a brush")
    };
    assert_eq!(strokes.len(), 1);
    assert!(strokes[0].points.len() >= 3, "{:?}", strokes[0].points);
    let (first, last) = (strokes[0].points[0], *strokes[0].points.last().unwrap());
    assert!(
        last[0] > first[0] + 0.1,
        "it runs left to right: {first:?} {last:?}"
    );
    assert!((first[1] - last[1]).abs() < 0.02);
    assert_eq!(history(&root, id), added + 1, "the stroke is one step");
    h.key_press(Key::Escape);
    h.step();
    assert_eq!(h.state().lib.develop().tool(), None);
}

fn saved_meta(dir: &Path, id: i64) -> lf_catalog::meta::Meta {
    let c = lf_catalog::Catalog::open(&dir.join("catalogue")).unwrap();
    c.photo(id).unwrap().unwrap().meta
}

#[test]
fn the_metadata_panel_describes_the_selection_and_search_finds_it() {
    use lf_app::library::meta_ui;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    import_folder(&mut h, &shoot, 3);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();

    // The ribbon's View → Metadata shows the panel; nothing is selected yet.
    assert!(run_action(&mut h, Action::Metadata));
    h.state_mut().lib.selection = Default::default();
    h.step();
    h.get_by_label(meta_ui::NOTHING_SELECTED);

    // Two photos: a title typed and the field left saves on both.
    h.state_mut().lib.selection.select_all(&ids[..2]);
    h.step();
    h.get_by_label("2 photos selected");
    h.get_by_role_and_label(Role::TextInput, "Title").click();
    h.step();
    h.get_by_role_and_label(Role::TextInput, "Title")
        .type_text("Haldi");
    h.step();
    h.key_press(Key::Tab);
    step_until(&mut h, "the title saved", |_| {
        saved_meta(dir.path(), ids[1]).title.as_deref() == Some("Haldi")
    });
    assert_eq!(saved_meta(dir.path(), ids[2]).title, None);

    // Keywords typed with commas, then Enter.
    h.get_by_label(meta_ui::ADD_KEYWORDS).click();
    h.step();
    h.get_by_label(meta_ui::ADD_KEYWORDS)
        .type_text("Jaipur, family");
    h.step();
    h.key_press(Key::Enter);
    step_until(&mut h, "the keywords saved", |_| {
        saved_meta(dir.path(), ids[0]).keywords == ["family", "Jaipur"]
    });
    h.get_by_label("Jaipur ×");

    // The third photo alone has none; the folder's list adds one with a click.
    h.state_mut().lib.selection.click(ids[2]);
    h.step();
    h.get_by_label("Jaipur 2").click();
    step_until(&mut h, "the folder keyword added", |_| {
        saved_meta(dir.path(), ids[2]).keywords == ["Jaipur"]
    });

    // Search: every word must match somewhere (title, keyword, file name).
    let search = |h: &mut Harness<'static, State>, text: &str| {
        h.state_mut().lib.set_filter(Filter {
            text: text.into(),
            ..Filter::default()
        });
        h.step();
        h.state().lib.shown().to_vec()
    };
    assert_eq!(search(&mut h, "jaipur"), ids);
    assert_eq!(search(&mut h, "haldi FAMILY"), ids[..2]);
    assert_eq!(search(&mut h, "p2 jaipur"), ids[2..]);
    assert!(search(&mut h, "mountains").is_empty());
    h.get_by_label("0 of 3 photos");
}

#[test]
fn importing_from_a_card_copies_renames_backs_up_and_catalogues() {
    use lf_app::dialogs::ingest;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let card = dir.path().join("card").join("DCIM");
    pngs(&card, 3);
    let photos = dir.path().join("Photos");
    let backup = dir.path().join("Backup");

    assert!(run_action(&mut h, Action::ImportFromCard));
    h.step();
    h.step();
    h.get_by_role_and_label(Role::TextInput, ingest::CARD_PATH)
        .click();
    h.step();
    h.get_by_role_and_label(Role::TextInput, ingest::CARD_PATH)
        .type_text(&card.to_string_lossy());
    h.step();
    h.get_by_label(ingest::LOOK).click();
    step_until(&mut h, "the card planned", |s| {
        s.lib
            .ingest()
            .is_some_and(|d| d.stage == ingest::Stage::Form)
    });
    {
        let d = h.state_mut().lib.ingest_mut().unwrap();
        d.destination = photos.to_string_lossy().into_owned();
        d.subfolder = "Sharma wedding".into();
        d.rename = true;
        d.pattern = "sharma_{seq}".into();
        d.backup_on = true;
        d.backup = backup.to_string_lossy().into_owned();
    }
    h.step();
    // The dialog grew (rename and backup rows) and centres itself again on the next frame.
    h.step();
    h.step();
    h.get_by_label("Import 3 photos").click();
    step_until(&mut h, "the copy and its catalogue import", |s| {
        s.lib
            .ingest()
            .is_some_and(|d| matches!(d.stage, ingest::Stage::Done(_)))
            && !s.lib.importing()
            && s.lib.folder_list().len() == 1
    });
    let shoot = photos.join("Sharma wedding");
    for i in 1..=3 {
        assert!(shoot.join(format!("sharma_{i}.png")).exists());
        assert!(
            backup
                .join("Sharma wedding")
                .join(format!("sharma_{i}.png"))
                .exists()
        );
    }
    assert!(card.join("p0.png").exists(), "the card is only read");
    assert_eq!(h.state().lib.folder_list()[0].path, shoot);
    h.get_by_label("3 photos copied.");
    h.get_by_label(ingest::CLOSE).click();
    h.step();
    assert!(h.state().lib.ingest().is_none());
    // Remembered for next time.
    assert_eq!(
        h.state()
            .prefs
            .last_ingest
            .as_ref()
            .unwrap()
            .backup
            .as_deref(),
        Some(backup.as_path())
    );

    // Photos the catalogue has (same name and size; PNGs have no capture time) are
    // suspected duplicates.
    assert!(run_action(&mut h, Action::ImportFromCard));
    h.step();
    h.state_mut().lib.ingest_mut().unwrap().card = shoot.to_string_lossy().into_owned();
    h.step();
    h.step();
    h.get_by_label(ingest::LOOK).click();
    step_until(&mut h, "the card planned again", |s| {
        s.lib
            .ingest()
            .is_some_and(|d| d.stage == ingest::Stage::Form)
    });
    h.get_by_label("3 already in the catalogue (same name and size).");
}

#[test]
fn an_imported_lut_goes_on_the_open_photo_and_stays_with_the_catalogue() {
    use lf_settings::lut::{Lut, LutChoice};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let mut text = String::from("TITLE \"Cool\"\nLUT_3D_SIZE 3\n");
    for p in Lut::identity(3).data {
        text.push_str(&format!("{} {} {}\n", p[0], p[1], p[2] * 0.5 + 0.5));
    }
    let file = dir.path().join("Cool blue.cube");
    std::fs::write(&file, text).unwrap();
    {
        let s = h.state_mut();
        let mut env = Env {
            jobs: &s.jobs,
            toasts: &mut s.toasts,
            prefs: &mut s.prefs,
            config_dir: &s.config,
        };
        s.lib.import_lut(file, &mut env);
    }
    step_until(&mut h, "the LUT on the photo", |s| {
        s.lib
            .develop()
            .edit_state()
            .and_then(|e| LutChoice::from_settings(&e.settings))
            .is_some_and(|c| c.name == "Cool blue" && c.amount == 100.0)
    });
    assert!(
        dir.path()
            .join("catalogue/.lensflow/luts/Cool blue.cube")
            .exists()
    );
}

#[test]
fn edit_in_makes_a_tiff_copy_beside_the_photo_and_catalogues_it() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 1);
    import_folder(&mut h, &shoot, 1);
    let id = h.state().lib.photos()[0].id;
    h.state_mut().lib.selection.click(id);
    // A stand-in "editor" that exists but is not an application: the copy is still made
    // and catalogued, and the toast says the editor could not start.
    let editor = dir.path().join("not-an-editor.txt");
    std::fs::write(&editor, b"x").unwrap();
    h.state_mut().prefs.external_editor = Some(editor);
    assert!(run_action(&mut h, Action::EditIn));
    step_until(&mut h, "the copy catalogued", |s| {
        !s.lib.importing() && s.lib.photos().len() == 2
    });
    assert!(shoot.join("p0-Edit.tif").exists());
    assert!(shoot.join("p0.png").exists(), "the original stays");
}

#[test]
fn print_writes_a_pdf_of_the_selection() {
    use lf_app::dialogs::print;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    import_folder(&mut h, &shoot, 3);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    h.state_mut().lib.selection.select_all(&ids);
    assert!(run_action(&mut h, Action::Print));
    h.step();
    h.step();
    h.get_by_label("Print 3 photos");
    h.get_by_role_and_label(Role::ComboBox, print::LAYOUT);
    // "Save PDF…" with the picker's answer given directly.
    let spec = h.state().lib.print().unwrap().spec();
    let out = dir.path().join("proofs.pdf");
    {
        let s = h.state_mut();
        let mut env = Env {
            jobs: &s.jobs,
            toasts: &mut s.toasts,
            prefs: &mut s.prefs,
            config_dir: &s.config,
        };
        s.lib.start_print(spec, out.clone(), false, &mut env);
    }
    step_until(&mut h, "the PDF written", |s| {
        s.lib
            .print()
            .is_some_and(|d| matches!(d.stage, print::Stage::Done(..)))
    });
    let doc = std::fs::read(&out).unwrap();
    assert!(doc.starts_with(b"%PDF"));
    assert_eq!(
        String::from_utf8_lossy(&doc)
            .matches("/Type /Page ")
            .count(),
        3,
        "one photo a page"
    );
    h.get_by_label(print::CLOSE).click();
    h.step();
    assert!(h.state().lib.print().is_none());
}

#[test]
fn a_bracket_merges_to_hdr_and_the_result_is_catalogued_beside_it() {
    use lf_app::dialogs::merge;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    import_folder(&mut h, &shoot, 3);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    // One photo cannot be merged.
    h.state_mut().lib.selection.click(ids[0]);
    assert!(!h.state().lib.enabled(Action::MergeHdr));
    h.state_mut().lib.selection.select_all(&ids);
    assert!(run_action(&mut h, Action::MergeHdr));
    h.step();
    h.step();
    h.get_by_label("Merge 3 photos to HDR");
    h.get_by_label(merge::MERGE).click();
    step_until(&mut h, "the merge and its catalogue import", |s| {
        s.lib
            .merge()
            .is_some_and(|d| matches!(d.stage, merge::Stage::Done(Ok(_))))
            && !s.lib.importing()
            && s.lib.photos().len() == 4
    });
    assert!(shoot.join("p0-HDR.tif").exists());
    h.get_by_label(merge::CLOSE).click();
    h.step();
    assert!(h.state().lib.merge().is_none());
}

#[test]
fn a_row_of_photos_merges_to_a_panorama() {
    use lf_app::dialogs::merge;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    // A wide textured scene cut into three frames overlapping by 150 px.
    let world = image::RgbaImage::from_fn(900, 260, |x, y| {
        let mut n = (x / 12).wrapping_mul(374_761_393) ^ (y / 12).wrapping_mul(668_265_263);
        n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
        let v = (n >> 24) as u8;
        image::Rgba([v, v / 2 + 60, 255 - v, 255])
    });
    let shoot = dir.path().join("pano");
    std::fs::create_dir_all(&shoot).unwrap();
    for (i, x0) in [0u32, 250, 500].into_iter().enumerate() {
        image::imageops::crop_imm(&world, x0, 0, 400, 260)
            .to_image()
            .save(shoot.join(format!("f{i}.png")))
            .unwrap();
    }
    import_folder(&mut h, &shoot, 3);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    h.state_mut().lib.selection.select_all(&ids);
    assert!(run_action(&mut h, Action::MergePanorama));
    h.step();
    h.step();
    h.get_by_label("Merge 3 photos to a panorama");
    h.get_by_label(merge::MERGE).click();
    step_until(&mut h, "the panorama and its catalogue import", |s| {
        s.lib
            .merge()
            .is_some_and(|d| matches!(d.stage, merge::Stage::Done(Ok(_))))
            && !s.lib.importing()
            && s.lib.photos().len() == 4
    });
    let (w, _) = image::image_dimensions(shoot.join("f0-Pano.tif")).unwrap();
    assert!((880..=905).contains(&w), "{w}");
}

/// Select object waits for a click on the photo, which asks for an AI mask of what is there
/// (the AI dialog opens with the point, in the photo's 0..1 space); nothing else changes.
#[test]
fn select_object_asks_for_a_mask_where_the_photo_is_clicked() {
    use lf_app::ai_job::AiTool;
    use lf_app::develop::tools::masks as mask_tool;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    h.state_mut().lib.set_local_ai(true);
    h.key_press(Key::M);
    h.step();
    h.step();
    let before = settings(&h);
    h.get_by_label(mask_tool::SELECT_OBJECT).click();
    h.step();
    assert!(h.query_by_label(mask_tool::PICK_OBJECT).is_some());
    let centre = egui::pos2(600.0, 330.0);
    h.hover_at(centre);
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: centre,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }
    h.step();
    let tool = h.state().lib.ai().map(|d| d.tool);
    let Some(AiTool::ObjectMask { u, v }) = tool else {
        panic!("no object mask asked for: {tool:?}");
    };
    assert!(
        (u - 0.5).abs() < 0.1 && (v - 0.5).abs() < 0.1,
        "the middle: {u}, {v}"
    );
    assert!(h.query_by_label(mask_tool::PICK_OBJECT).is_none());
    assert_eq!(settings(&h), before, "a click to pick changes nothing");
}

/// Auto-sync: with three photos selected and the box ticked, a drag on the open photo's
/// Exposure is made on the other two as well (on the `Long` lane), each keeping the rest of
/// its own edit; with the box clear, nothing follows.
#[test]
fn auto_sync_sends_a_change_to_the_other_selected_photos() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 3);
    import_folder(&mut h, &shoot, 3);
    let root = dir.path().join("catalogue");
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    // The third photo has its own contrast, which must survive the sync.
    {
        let catalog = lf_catalog::Catalog::open(&root).unwrap();
        let mut s = lf_settings::EditSettings::default_settings();
        s.contrast = 25.0;
        lf_catalog::edits::set_settings(&catalog, ids[2], &s).unwrap();
    }
    h.state_mut().lib.selection.select(&ids);
    h.state_mut().lib.selection.set_active(ids[0]);
    h.key_press(Key::D);
    step_until(&mut h, "the photo to open", |s| s.lib.develop().is_ready());
    h.step();
    let stored = |id: i64| {
        let catalog = lf_catalog::Catalog::open(&root).unwrap();
        lf_catalog::edits::get_settings(&catalog, id)
            .unwrap()
            .unwrap_or_else(lf_settings::EditSettings::default_settings)
    };

    // Off: the others stay as they were.
    drag_slider(&mut h, "Exposure", 40.0);
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(stored(ids[1]).exposure, 0.0, "auto-sync is off");

    h.get_by_label("Auto-sync the 2 other selected photos")
        .click();
    h.step();
    assert!(h.state().lib.develop().auto_sync());
    drag_slider(&mut h, "Exposure", 40.0);
    let open = settings(&h).exposure;
    step_until(&mut h, "the sync to reach the others", |_| {
        stored(ids[1]).exposure == open && stored(ids[2]).exposure == open
    });
    assert_eq!(stored(ids[2]).contrast, 25.0, "its own contrast stays");
}

/// "Rename…": the selected photos' files take names from the pattern (the preview shows
/// them first), their sidecars go with them, and the grid shows the new names; a name taken
/// by a file outside the batch refuses everything and says why.
#[test]
fn rename_gives_the_selected_photos_new_names() {
    use lf_app::dialogs::rename;
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    let shoot = dir.path().join("shoot");
    pngs(&shoot, 2);
    import_folder(&mut h, &shoot, 2);
    let ids: Vec<i64> = h.state().lib.photos().iter().map(|p| p.id).collect();
    let names = |h: &Harness<'static, State>| -> Vec<String> {
        let mut v: Vec<String> = h
            .state()
            .lib
            .photos()
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    std::fs::write(shoot.join("Taken.png"), b"not catalogued").unwrap();

    h.state_mut().lib.selection.select(&ids);
    {
        let ctx = h.ctx.clone();
        let s = h.state_mut();
        let mut env = Env {
            jobs: &s.jobs,
            toasts: &mut s.toasts,
            prefs: &mut s.prefs,
            config_dir: &s.config,
        };
        assert!(
            s.lib
                .run(lf_app::shortcuts::Action::RenamePhotos, &ctx, &mut env)
        );
    }
    h.step();
    h.step();
    assert!(h.query_by_label("Rename 2 photos").is_some());
    let field = h.get_by_role_and_label(Role::TextInput, rename::PATTERN);
    field.click();
    h.step();
    h.key_press(Key::End);
    for _ in 0..16 {
        h.key_press(Key::Backspace);
    }
    h.get_by_role_and_label(Role::TextInput, rename::PATTERN)
        .type_text("Taken");
    h.step();
    h.get_by_label(rename::RENAME).click();
    // The refusal (a long line, wrapped: matched by part).
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h
        .query_by_label_contains("is already taken in its folder")
        .is_none()
    {
        assert!(std::time::Instant::now() < end, "the refusal was not shown");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    h.get_by_role_and_label(Role::TextInput, rename::PATTERN)
        .click();
    h.step();
    h.key_press(Key::End);
    for _ in 0..16 {
        h.key_press(Key::Backspace);
    }
    h.get_by_role_and_label(Role::TextInput, rename::PATTERN)
        .type_text("Party_{seq}");
    h.step();
    assert!(
        h.query_by_label_contains("->  Party_1.png").is_some(),
        "previewed"
    );
    h.get_by_label(rename::RENAME).click();
    step_until(&mut h, "the new names in the grid", |s| {
        s.lib
            .photos()
            .iter()
            .any(|p| p.path.ends_with("Party_2.png"))
    });
    assert_eq!(names(&h), vec!["Party_1.png", "Party_2.png"]);
    assert!(shoot.join("Party_1.png").exists() && shoot.join("Taken.png").exists());
}

/// Guided Upright: two guides drawn along edges that lean towards each other set the
/// vertical perspective, one history step; one guide alone changes nothing.
#[test]
fn guided_upright_straightens_to_two_drawn_guides() {
    use lf_app::develop::tools::{self, ToolKind};
    let dir = tempfile::tempdir().unwrap();
    let Some(mut h) = opened(dir.path()) else {
        return;
    };
    develop_one(&mut h, dir.path());
    let id = h.state().lib.photos()[0].id;
    let root = dir.path().join("catalogue");
    h.get_by_label(tools::GUIDED).click();
    h.step();
    assert_eq!(h.state().lib.develop().tool(), Some(ToolKind::Guided));
    let draw = |h: &mut Harness<'static, State>, from: egui::Pos2, to: egui::Pos2| {
        h.hover_at(from);
        h.step();
        h.drag_at(from);
        h.step();
        for i in 1..=4 {
            h.hover_at(from + (to - from) * (i as f32 / 4.0));
            h.step();
        }
        h.drop_at(to);
        h.step();
        h.step();
    };
    let before = history(&root, id);
    // Edges that close in towards the top, as a building shot from below.
    draw(&mut h, egui::pos2(520.0, 220.0), egui::pos2(480.0, 440.0));
    assert_eq!(history(&root, id), before, "one guide is not enough");
    draw(&mut h, egui::pos2(680.0, 220.0), egui::pos2(720.0, 440.0));
    let lens = lf_settings::lens::Lens::from_settings(&settings(&h));
    assert!(
        lens.vertical.abs() > 5.0,
        "the verticals were straightened: {lens:?}"
    );
    assert_eq!(lens.horizontal, 0.0);
    assert_eq!(before, 0);
    assert_eq!(history(&root, id), 2, "the photo before, and the correction");
    assert!(h.query_by_label("2 guides.").is_some());
}
