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
}

fn harness(raw: bool) -> Harness<'static, State> {
    // Frames a 60th of a second apart, so two clicks a frame apart are a double-click.
    Harness::builder()
        .with_size([300.0, 1400.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, s: &mut State| {
                let ctx = PanelContext {
                    raw: s.raw,
                    histogram: None,
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
