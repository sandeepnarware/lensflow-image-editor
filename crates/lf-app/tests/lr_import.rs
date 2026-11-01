//! "Import Lightroom presets…"'s dialog, driven headless through its accessibility tree: the
//! choices it offers, and the summary with each preset's notes. The library's side (the
//! action, the job, the catalogue) is in `library_screens.rs`.

use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_app::dialogs::lr_import::{self, LrImportAction, LrImportDialog, Stage};
use lf_app::lightroom::import::ImportReport;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

fn harness(dialog: LrImportDialog) -> Harness<'static, (LrImportDialog, Vec<LrImportAction>)> {
    Harness::builder().with_size([800.0, 700.0]).build_ui_state(
        |ui, (dialog, actions): &mut (LrImportDialog, Vec<LrImportAction>)| {
            let action = dialog.show(ui.ctx());
            if action != LrImportAction::None {
                actions.push(action);
            }
        },
        (dialog, Vec::new()),
    )
}

#[test]
fn find_is_off_without_adobes_folders_and_import_waits_for_a_choice() {
    let mut h = harness(LrImportDialog::new(Vec::new()));
    // A new dialog is laid out unseen on its first frame.
    h.run_steps(2);
    let find = h.get_by_role_and_label(Role::Button, lr_import::FIND_ADOBE);
    assert!(find.accesskit_node().is_disabled());
    let import = h.get_by_role_and_label(Role::Button, lr_import::IMPORT);
    assert!(import.accesskit_node().is_disabled());
    h.get_by_label(lr_import::CHOOSE_FILES).click();
    h.step();
    assert_eq!(h.state().1, [LrImportAction::ChooseFiles]);
}

#[test]
fn find_adds_adobes_folders_and_import_takes_them() {
    let adobe = vec![PathBuf::from("adobe-settings")];
    let mut h = harness(LrImportDialog::new(adobe.clone()));
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, lr_import::FIND_ADOBE)
        .click();
    h.step();
    assert!(h.query_by_label("adobe-settings").is_some());
    h.get_by_role_and_label(Role::Button, lr_import::IMPORT)
        .click();
    h.step();
    assert_eq!(h.state().1, [LrImportAction::Import(adobe)]);
}

#[test]
fn the_summary_lists_the_notes_of_each_preset() {
    let mut dialog = LrImportDialog::new(Vec::new());
    dialog.stage = Stage::Done(ImportReport {
        imported: vec![
            (
                "Studio - Moody".into(),
                vec!["Grain roughness is not carried.".into()],
            ),
            ("Plain".into(), Vec::new()),
            (
                "Studio - Warm".into(),
                vec!["Masks are not carried: they belong to one photo.".into()],
            ),
        ],
        failed: vec![(
            "pack.zip / broken.xmp".into(),
            "That file is not readable XML, so it is not a Lightroom preset.".into(),
        )],
        left_out: vec!["Extra".into()],
        ..ImportReport::default()
    });
    let mut h = harness(dialog);
    h.run_steps(2);
    assert!(h.query_by_label(&lr_import::imported_text(3)).is_some());
    // Only the presets with notes are listed under them, each opening to its own.
    assert!(h.query_by_label("Plain").is_none());
    assert!(
        h.query_by_label("Grain roughness is not carried.")
            .is_none()
    );
    h.get_by_label("Studio - Moody").click();
    h.run_steps(10);
    assert!(
        h.query_by_label("Grain roughness is not carried.")
            .is_some()
    );
    assert!(
        h.query_by_label("Masks are not carried: they belong to one photo.")
            .is_none(),
        "another preset's notes stay closed"
    );
    assert!(h.query_by_label_contains("pack.zip / broken.xmp").is_some());
    assert!(h.query_by_label(&lr_import::left_out_text(1)).is_some());
    assert!(
        h.query_by_label_contains("stopped").is_none(),
        "a full run says nothing about stopping"
    );
    assert!(h.query_by_label("Extra").is_some());
    h.get_by_label(lr_import::CLOSE).click();
    h.step();
    assert_eq!(h.state().1, [LrImportAction::Close]);
}

#[test]
fn a_name_the_fonts_cannot_draw_is_shown_with_question_marks() {
    let mut dialog = LrImportDialog::new(Vec::new());
    dialog.stage = Stage::Done(ImportReport {
        imported: vec![(
            "\u{5915}\u{713c}\u{3051}".into(),
            vec!["Grain roughness is not carried.".into()],
        )],
        ..ImportReport::default()
    });
    let mut h = harness(dialog);
    // Drawing it must not panic.
    h.run_steps(2);
    assert!(h.query_by_label("???").is_some());
}

#[test]
fn importing_shows_how_many_files_are_done() {
    let dialog = LrImportDialog::new(Vec::new());
    dialog.progress.total.store(5, Ordering::Relaxed);
    dialog.progress.done.store(2, Ordering::Relaxed);
    let mut dialog = dialog;
    dialog.stage = Stage::Importing;
    let mut h = harness(dialog);
    h.run_steps(2);
    assert!(h.query_by_label(&lr_import::progress_text(2, 5)).is_some());
    h.get_by_label(lr_import::STOP).click();
    h.step();
    assert!(h.state().0.stop.load(Ordering::SeqCst));
}

#[test]
fn a_stopped_import_says_how_many_files_were_not_processed() {
    let mut dialog = LrImportDialog::new(Vec::new());
    dialog.stage = Stage::Done(ImportReport {
        imported: vec![("Plain".into(), Vec::new())],
        stopped: Some(3),
        ..ImportReport::default()
    });
    let mut h = harness(dialog);
    h.run_steps(2);
    assert!(h.query_by_label(&lr_import::stopped_text(3)).is_some());
}

#[test]
fn profiles_are_a_separate_skipped_line_not_a_failure() {
    let mut dialog = LrImportDialog::new(Vec::new());
    dialog.stage = Stage::Done(ImportReport {
        imported: vec![("Plain".into(), Vec::new())],
        skipped_profiles: 2,
        ..ImportReport::default()
    });
    let mut h = harness(dialog);
    h.run_steps(2);
    assert_eq!(
        lr_import::skipped_profiles_text(2),
        "Skipped: 2 profiles (Lightroom profiles are not presets)."
    );
    assert!(
        h.query_by_label(&lr_import::skipped_profiles_text(2))
            .is_some()
    );
    assert!(h.query_by_label_contains(lr_import::FAILED_TITLE).is_none());
}

#[test]
fn each_imported_preset_offers_a_match_to_a_lightroom_example() {
    let mut dialog = LrImportDialog::new(Vec::new());
    dialog.stage = Stage::Done(ImportReport {
        imported: vec![("One".into(), Vec::new()), ("Two".into(), Vec::new())],
        ..ImportReport::default()
    });
    let mut h = harness(dialog);
    h.run_steps(2);
    let buttons: Vec<_> = h.get_all_by_label(lr_import::MATCH_EXAMPLE).collect();
    assert_eq!(buttons.len(), 2);
    buttons[1].click();
    h.step();
    assert_eq!(h.state().1, [LrImportAction::MatchExample("Two".into())]);
}
