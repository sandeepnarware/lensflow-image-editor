//! "Import Lightroom presets…"'s dialog, driven headless through its accessibility tree: the
//! choices it offers, and the summary with each preset's notes. The library's side (the
//! action, the job, the catalogue) is in `library_screens.rs`.

use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_app::dialogs::lr_import::{self, LrImportAction, LrImportDialog, Stage};
use lf_app::lightroom::import::ImportReport;
use std::path::PathBuf;

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
