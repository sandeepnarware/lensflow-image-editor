//! The adjust panels, driven headless through their accessibility tree (no window, no GPU).

use eframe::egui::{self, accesskit::Role};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_app::develop::panels::{self, PanelContext, PanelOutput, PanelState, SECTIONS};
use lf_settings::EditSettings;

struct State {
    panels: PanelState,
    settings: EditSettings,
    raw: bool,
    last: PanelOutput,
    commits: usize,
    profiles: Vec<(String, String)>,
}

fn harness(raw: bool) -> Harness<'static, State> {
    // Frames a 60th of a second apart, so two clicks a frame apart are a double-click.
    Harness::builder()
        .with_size([300.0, 1400.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, s: &mut State| {
                let ctx = PanelContext {
                    as_shot_kelvin: None,
                    raw: s.raw,
                    histogram: None,
                    sample: None,
                    picking_wb: false,
                    luts: &[],
                    profiles: &s.profiles,
                };
                s.last = panels::show(ui, &mut s.panels, &mut s.settings, &ctx);
                if s.last.commit {
                    s.commits += 1;
                }
            },
            State {
                panels: PanelState::default(),
                settings: EditSettings::default_settings(),
                raw,
                last: PanelOutput::default(),
                commits: 0,
                profiles: Vec::new(),
            },
        )
}

fn slider_labels(h: &Harness<'static, State>) -> Vec<String> {
    h.query_all_by_role(Role::Slider)
        .map(|n| n.accesskit_node().label().unwrap_or_default())
        .collect()
}

#[test]
fn light_section_has_six_sliders_with_web_labels() {
    let mut h = harness(false);
    h.run();
    for sec in &SECTIONS {
        assert!(
            h.query_by_label(sec.label).is_some(),
            "section {:?} is listed",
            sec.label
        );
    }
    assert!(
        h.query_by_label(panels::RAW).is_none(),
        "RAW only for RAW photos"
    );
    let labels = slider_labels(&h);
    assert_eq!(
        labels[..6],
        [
            "Exposure",
            "Contrast",
            "Highlights",
            "Shadows",
            "Whites",
            "Blacks"
        ],
        "{labels:?}"
    );
    // Colour is open too; vibrance and saturation leave in black & white.
    assert_eq!(
        labels[6..],
        ["Temperature", "Tint", "Vibrance", "Saturation"]
    );
    h.get_by_label(panels::color::BW).click();
    h.run();
    assert!(h.state().settings.bw);
    assert_eq!(h.state().commits, 1, "a toggle click is one commit");
    assert_eq!(slider_labels(&h)[6..], ["Temperature", "Tint"]);
}

#[test]
fn a_raw_photo_shows_its_profile_and_highlights() {
    let mut h = harness(true);
    h.run();
    assert!(h.query_by_label(panels::RAW).is_some());
    h.get_by_label("Linear").click();
    h.run();
    let develop = lf_settings::DevelopSettings::from_settings(&h.state().settings);
    assert_eq!(develop.profile, lf_settings::Profile::Linear);
    assert_eq!(h.state().commits, 1);
}

#[test]
fn a_raw_photo_picks_a_camera_profile_and_asks_to_import_one() {
    use lf_app::develop::panels::raw;
    use lf_settings::DevelopSettings;
    let mut h = harness(true);
    h.state_mut().profiles = vec![("Test Standard".into(), "Test Cam".into())];
    h.run();
    h.get_by_role_and_label(Role::ComboBox, raw::CAMERA_PROFILE)
        .click();
    h.run();
    h.get_by_label("Test Standard (Test Cam)").click();
    h.run();
    assert_eq!(
        DevelopSettings::dcp_name(&h.state().settings).as_deref(),
        Some("Test Standard")
    );
    assert_eq!(h.state().commits, 1);
    assert!(h.query_by_label(raw::NOT_LOADED).is_none());

    // A profile this catalogue does not have: said, and the photo shows without it.
    h.state_mut().profiles.clear();
    h.run();
    assert!(h.query_by_label(raw::NOT_LOADED).is_some());

    h.get_by_label(raw::IMPORT).click();
    h.step();
    assert!(h.state().last.import_dcp, "Import .dcp… asks the library");
}

#[test]
fn clicking_a_value_types_it_and_double_click_resets() {
    let mut h = harness(false);
    h.run();
    let rect = h.get_by_label("Exposure").rect();
    // The value sits at the row's top right.
    let at = rect.right_top() + egui::vec2(-8.0, 8.0);
    h.drag_at(at);
    h.drop_at(at);
    h.run();
    let field = h.get_by_role(Role::TextInput);
    field.type_text("1.27");
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(
        h.state().settings.exposure,
        1.25,
        "snapped to the 0.05 step"
    );
    assert_eq!(h.state().commits, 1);
    assert!(h.query_by_role(Role::TextInput).is_none());

    // Double-click anywhere on the row: back to neutral, one more commit. (Half a second
    // first, so egui does not count it with the click on the value as a triple click.)
    h.run_steps(30);
    for _ in 0..2 {
        h.get_by_label("Exposure").click();
        h.step();
    }
    h.run();
    assert_eq!(h.state().settings.exposure, 0.0);
    assert_eq!(h.state().commits, 2);
}

/// The panels over a real edit state and catalogue: each gesture's commit is recorded.
struct Recorded {
    panels: PanelState,
    edit: lf_app::develop::edit_state::EditState,
    catalog: lf_catalog::Catalog,
    photo: i64,
    _dir: tempfile::TempDir,
}

fn recorded() -> Harness<'static, Recorded> {
    let dir = tempfile::tempdir().unwrap();
    let catalog = lf_catalog::Catalog::open(dir.path()).unwrap();
    let folder = catalog.add_folder("shoot").unwrap();
    let photo = catalog
        .add_photo(
            folder,
            &lf_catalog::NewPhoto {
                path: "shoot/a.jpg".into(),
                kind: "jpeg".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let state = Recorded {
        panels: PanelState::default(),
        edit: lf_app::develop::edit_state::EditState::new(photo, EditSettings::default_settings()),
        catalog,
        photo,
        _dir: dir,
    };
    let mut h = Harness::builder()
        .with_size([300.0, 1400.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, s: &mut Recorded| {
                let ctx = PanelContext {
                    as_shot_kelvin: None,
                    raw: false,
                    histogram: None,
                    sample: None,
                    picking_wb: false,
                    luts: &[],
                    profiles: &[],
                };
                let mut out = PanelOutput::default();
                let panels = &mut s.panels;
                s.edit.set(|e| out = panels::show(ui, panels, e, &ctx));
                if out.commit {
                    s.edit.commit(&s.catalog).unwrap();
                }
            },
            state,
        );
    h.run();
    h.get_by_label("Tone curve").click();
    h.run();
    // Half a second, so egui does not count the next clicks with this one.
    h.run_steps(30);
    h
}

fn steps(h: &Harness<'static, Recorded>) -> i64 {
    lf_catalog::edits::history_len(&h.state().catalog, h.state().photo).unwrap()
}

fn curve<'a>(h: &'a Harness<'static, Recorded>) -> egui_kittest::Node<'a> {
    h.get_by_label_contains("Tone curve: click to add a point")
}

#[test]
fn a_double_click_on_empty_curve_space_records_nothing() {
    let mut h = recorded();
    for _ in 0..2 {
        curve(&h).click();
        h.step();
    }
    h.run_steps(40);
    assert_eq!(
        h.state().edit.settings.curve.rgb,
        lf_settings::identity_curve()
    );
    assert_eq!(
        steps(&h),
        0,
        "no history step for an add taken back at once"
    );
}

#[test]
fn dragging_an_inner_point_off_the_side_removes_it() {
    let mut h = recorded();
    let rect = curve(&h).rect();
    curve(&h).click();
    h.run_steps(40);
    assert_eq!(
        h.state().edit.settings.curve.rgb.len(),
        3,
        "a point was added"
    );
    assert_eq!(h.state().panels.curve.selected(), Some(1));
    let added = steps(&h);
    assert_eq!(added, 2, "the photo before, and the added point");

    let at = rect.center();
    h.drag_at(at);
    h.step();
    for x in [at.x - 40.0, at.x - 120.0, rect.left() - 60.0] {
        h.hover_at(egui::pos2(x, at.y));
        h.step();
    }
    h.drop_at(egui::pos2(rect.left() - 60.0, at.y));
    h.run_steps(5);
    assert_eq!(
        h.state().edit.settings.curve.rgb,
        lf_settings::identity_curve(),
        "dragged out past the left edge"
    );
    assert_eq!(steps(&h), added + 1);
    assert_eq!(h.state().panels.curve.selected(), None);
}

/// The crop panel alone over a 3:2 photo.
fn crop_panel(settings: EditSettings) -> Harness<'static, EditSettings> {
    Harness::builder()
        .with_size([300.0, 600.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, s: &mut EditSettings| {
                lf_app::develop::panels::crop::show(ui, s, 6000, 4000);
            },
            settings,
        )
}

#[test]
fn clicking_the_selected_aspect_fits_the_frame_to_it_again() {
    // Saved as 1:1 but with a frame that is not square (as a web edit can be).
    let mut s = EditSettings::default_settings();
    s.aspect = "1:1".into();
    let mut h = crop_panel(s);
    h.run();
    h.get_by_label("1:1").click();
    h.run();
    let c = h.state().crop;
    assert!(
        (c.w * 6000.0 / (c.h * 4000.0) - 1.0).abs() < 1e-6,
        "re-fitted to a square: {c:?}"
    );
}

/// The auto buttons and the eyedropper, with a sample of a dark, warm photo.
#[test]
fn auto_tone_auto_white_balance_and_the_eyedropper_button() {
    use lf_app::develop::auto::Sample;
    use lf_app::develop::panels::{color, light};
    struct S {
        panels: PanelState,
        settings: EditSettings,
        sample: Sample,
        last: PanelOutput,
        commits: usize,
        picked: bool,
    }
    // Dark, and warmer than neutral: a ramp of greys with red up and blue down.
    let rgb = (0..64 * 48)
        .map(|i| {
            let v = 0.05 + 0.3 * (i % 64) as f32 / 64.0;
            [v * 1.15, v, v * 0.8]
        })
        .collect();
    let sample = Sample {
        width: 64,
        height: 48,
        rgb,
    };
    let mut h = Harness::builder()
        .with_size([300.0, 1400.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, s: &mut S| {
                let ctx = PanelContext {
                    as_shot_kelvin: None,
                    raw: false,
                    histogram: None,
                    sample: Some(&s.sample),
                    picking_wb: false,
                    luts: &[],
                    profiles: &[],
                };
                s.last = panels::show(ui, &mut s.panels, &mut s.settings, &ctx);
                s.picked |= s.last.pick_wb;
                if s.last.commit {
                    s.commits += 1;
                }
            },
            S {
                panels: PanelState::default(),
                settings: EditSettings::default_settings(),
                sample,
                last: PanelOutput::default(),
                commits: 0,
                picked: false,
            },
        );
    h.run();

    h.get_by_label(light::AUTO_TONE).click();
    h.run();
    assert!(
        h.state().settings.exposure > 0.0,
        "a dark photo is brightened"
    );
    assert_eq!(h.state().commits, 1, "one history step");

    // White balance: the list shows "As shot", Auto cools the warm photo.
    h.get_by_role_and_label(Role::ComboBox, color::WHITE_BALANCE)
        .click();
    h.run();
    h.get_by_label(color::AUTO_WB).click();
    h.run();
    assert!(h.state().settings.temperature < 0.0, "warm made cooler");
    assert_eq!(h.state().commits, 2);
    h.get_by_role_and_label(Role::ComboBox, color::WHITE_BALANCE)
        .click();
    h.run();
    h.get_by_label(color::AS_SHOT).click();
    h.run();
    assert_eq!(
        (h.state().settings.temperature, h.state().settings.tint),
        (0.0, 0.0)
    );
    assert_eq!(h.state().commits, 3);

    // A preset: Shade warms a photo shot in daylight, and the list then says "Shade".
    h.get_by_role_and_label(Role::ComboBox, color::WHITE_BALANCE)
        .click();
    h.run();
    h.get_by_label("Shade").click();
    h.run();
    assert!(h.state().settings.temperature > 0.0, "shade warms");
    assert_eq!(
        color::wb_name(&h.state().settings, color::ASSUMED_KELVIN),
        "Shade"
    );
    assert_eq!(h.state().commits, 4);

    // The eyedropper's button asks the Develop view to open its tool.
    h.get_by_label(color::PICK_WB).click();
    h.run();
    assert!(h.state().picked, "the eyedropper asks for its tool");
}

#[test]
fn the_lens_section_writes_desktop_lens_and_its_reset_clears_it() {
    use lf_app::develop::panels::lens;
    use lf_settings::lens::Lens;
    let mut h = harness(false);
    h.run();
    h.get_by_label(lens::TITLE).click();
    h.run();
    for label in lens::LABELS {
        assert!(h.query_by_label(label).is_some(), "{label} is shown");
    }
    // Set through the settings as a slider gesture would, then Reset puts none back.
    let mut l = Lens::from_settings(&h.state().settings);
    l.vertical = 30.0;
    l.write_into(&mut h.state_mut().settings);
    h.run();
    assert_eq!(Lens::from_settings(&h.state().settings).vertical, 30.0);
    let commits = h.state().commits;
    h.get_by_label(&format!("Reset {}", lens::TITLE)).click();
    h.run();
    assert!(Lens::from_settings(&h.state().settings).is_none());
    assert!(h.state().settings.desktop.is_none());
    assert_eq!(h.state().commits, commits + 1, "one step");
}

#[test]
fn the_calibration_section_writes_desktop_calibration_and_its_reset_clears_it() {
    use lf_app::develop::panels::calibration;
    use lf_settings::calibration::Calibration;
    let mut h = harness(false);
    h.run();
    h.get_by_label(calibration::TITLE).click();
    h.run();
    let labels = slider_labels(&h);
    for l in calibration::LABELS {
        assert!(labels.iter().any(|s| s == l), "{l} missing from {labels:?}");
    }
    let rect = h.get_by_label("Red primary hue").rect();
    let at = rect.center();
    h.drag_at(at);
    h.run();
    h.hover_at(at + egui::vec2(40.0, 0.0));
    h.run();
    h.drop_at(at + egui::vec2(40.0, 0.0));
    h.run();
    let c = Calibration::from_settings(&h.state().settings);
    assert!(c.red_hue > 0.0, "{c:?}");
    assert_eq!(h.state().commits, 1);
    h.state_mut().settings = {
        let mut s = h.state().settings.clone();
        calibration::reset(&mut s);
        s
    };
    assert!(h.state().settings.desktop.is_none());
}
