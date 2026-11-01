//! "Match a Lightroom example…"'s dialog, driven headless through its accessibility tree: the
//! choices it waits for, and the result with its verdict and save buttons. The library's side
//! (the action, the pickers, saving into the catalogue) is in `library_screens.rs`.

use eframe::egui;
use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_app::dialogs::lr_example::{self, LrExampleAction, LrExampleDialog, Matched, Pick, Stage};
use lf_app::lightroom::example_job::{self, Previews, Verdict};
use lf_grade::example::Fit;
use lf_settings::lut::Lut;
use std::path::PathBuf;

fn harness(dialog: LrExampleDialog) -> Harness<'static, (LrExampleDialog, Vec<LrExampleAction>)> {
    Harness::builder().with_size([900.0, 900.0]).build_ui_state(
        |ui, (dialog, actions): &mut (LrExampleDialog, Vec<LrExampleAction>)| {
            let action = dialog.show(ui.ctx());
            if action != LrExampleAction::None {
                actions.push(action);
            }
        },
        (dialog, Vec::new()),
    )
}

fn fit(aligned: bool, mean: f64, p95: f64) -> Fit {
    Fit {
        table: Lut::identity(17),
        vignette: -20.0,
        vignette_midpoint: 50.0,
        grain: 10.0,
        grain_size: 25.0,
        delta_e_mean: mean,
        delta_e_p95: p95,
        aligned,
        residual: 0.012,
    }
}

fn picture() -> egui::ColorImage {
    egui::ColorImage::from_rgb([8, 6], &[128; 8 * 6 * 3])
}

fn matched(fit: Fit, preset: Option<&serde_json::Value>) -> Box<Matched> {
    Box::new(Matched::new(
        example_job::matched_preset(preset, "Moody", &fit),
        fit,
        Previews {
            before: picture(),
            lightroom: picture(),
            after: picture(),
            difference: picture(),
        },
    ))
}

#[test]
fn match_waits_for_both_files() {
    let mut h = harness(LrExampleDialog::new(None, Some(PathBuf::from("photo.nef"))));
    // A new dialog is laid out unseen on its first frame.
    h.run_steps(2);
    assert!(h.query_by_label(lr_example::NEW_PRESET).is_some());
    assert!(h.query_by_label_contains("photo.nef").is_some());
    let go = h.get_by_role_and_label(Role::Button, lr_example::MATCH);
    assert!(go.accesskit_node().is_disabled());
    h.get_by_label(lr_example::CHOOSE_EXPORT).click();
    h.step();
    assert_eq!(h.state().1, [LrExampleAction::Choose(Pick::Export)]);
    h.state_mut().0.export = Some(PathBuf::from("export.jpg"));
    h.step();
    h.get_by_role_and_label(Role::Button, lr_example::MATCH)
        .click();
    h.step();
    assert_eq!(h.state().1.last(), Some(&LrExampleAction::Match));
}

#[test]
fn the_result_shows_its_verdict_and_both_save_buttons() {
    let preset = serde_json::json!({"v": 2, "exposure": 0.3});
    let mut dialog = LrExampleDialog::new(Some(("Studio - Moody".into(), preset.clone())), None);
    dialog.stage = Stage::Done(matched(fit(true, 1.6, 3.9), Some(&preset)));
    let mut h = harness(dialog);
    h.run_steps(2);
    assert!(h.query_by_label(Verdict::VeryClose.text()).is_some());
    let shown = lr_example::difference_text(&fit(true, 1.6, 3.9));
    assert_eq!(
        shown,
        "Difference: \u{394}E 1.6 average, 3.9 at most for 95 % of pixels."
    );
    assert!(h.query_by_label(&shown).is_some());
    assert!(
        h.query_by_label_contains("Alignment residual 0.012")
            .is_some()
    );
    for caption in [
        lr_example::BEFORE,
        lr_example::LIGHTROOM,
        lr_example::AFTER,
        lr_example::DIFFERENCE,
    ] {
        assert!(h.query_by_label(caption).is_some(), "{caption}");
    }
    h.get_by_role_and_label(Role::Button, lr_example::SAVE)
        .click();
    h.step();
    h.get_by_role_and_label(Role::Button, lr_example::SAVE_AS_NEW)
        .click();
    h.step();
    assert_eq!(
        h.state().1,
        [LrExampleAction::Save, LrExampleAction::SaveAsNew]
    );
}

#[test]
fn a_poor_match_warns_but_can_still_be_saved_and_a_new_preset_has_no_replace() {
    let mut dialog = LrExampleDialog::new(None, None);
    dialog.stage = Stage::Done(matched(fit(false, 1.0, 2.0), None));
    let mut h = harness(dialog);
    h.run_steps(2);
    assert!(h.query_by_label(Verdict::NotAGoodMatch.text()).is_some());
    assert!(
        h.query_by_role_and_label(Role::Button, lr_example::SAVE)
            .is_none()
    );
    let new = h.get_by_role_and_label(Role::Button, lr_example::SAVE_AS_NEW);
    assert!(!new.accesskit_node().is_disabled());
}

#[test]
fn a_failed_match_says_why() {
    let mut dialog = LrExampleDialog::new(None, None);
    dialog.finished(Err(example_job::NOT_CONVERTED.to_owned()));
    let mut h = harness(dialog);
    h.run_steps(2);
    assert!(h.query_by_label(example_job::NOT_CONVERTED).is_some());
    h.get_by_label(lr_example::CLOSE).click();
    h.step();
    assert_eq!(h.state().1, [LrExampleAction::Close]);
}

#[test]
fn a_preset_over_the_size_cap_cannot_be_saved() {
    let mut big = serde_json::json!({"v": 2});
    big["desktop"] = serde_json::json!({"notes": "x".repeat(70_000)});
    let mut dialog = LrExampleDialog::new(Some(("Big".into(), big.clone())), None);
    dialog.finished(Ok(matched(fit(true, 1.0, 2.0), Some(&big))));
    let mut h = harness(dialog);
    h.run_steps(2);
    for label in [lr_example::SAVE, lr_example::SAVE_AS_NEW] {
        let button = h.get_by_role_and_label(Role::Button, label);
        assert!(button.accesskit_node().is_disabled(), "{label}");
    }
    assert!(h.query_by_label(lr_example::TOO_LARGE).is_some());
}
