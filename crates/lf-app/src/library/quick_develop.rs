//! Library → Quick Develop (as Lightroom's): the selected photos nudged without opening
//! Develop. Each row is one setting with a small and a large step down and up (exposure in
//! stops, the rest in slider units); "Reset all" puts every setting back. A nudge is added to
//! each photo's own value (kept within the slider's range), on the `Long` lane as a paste is
//! ([`crate::clipboard::Recipe::Nudge`]), one history step per photo.

use super::{Env, Library};
use crate::dialogs::batch;
use crate::jobs::{Lane, Msg};
use eframe::egui;
use lf_settings::EditSettings;

pub const TITLE: &str = "Quick develop";
pub const RESET: &str = "Reset all";

/// A row: the setting's key, its name, the small and large steps, and its range.
pub type Row = (&'static str, &'static str, f64, f64, (f64, f64));

/// The rows.
pub const ROWS: [Row; 11] = [
    ("exposure", "Exposure", 1.0 / 3.0, 1.0, (-5.0, 5.0)),
    ("contrast", "Contrast", 5.0, 20.0, (-100.0, 100.0)),
    ("highlights", "Highlights", 5.0, 20.0, (-100.0, 100.0)),
    ("shadows", "Shadows", 5.0, 20.0, (-100.0, 100.0)),
    ("whites", "Whites", 5.0, 20.0, (-100.0, 100.0)),
    ("blacks", "Blacks", 5.0, 20.0, (-100.0, 100.0)),
    ("temperature", "Temperature", 5.0, 20.0, (-100.0, 100.0)),
    ("tint", "Tint", 5.0, 20.0, (-100.0, 100.0)),
    ("vibrance", "Vibrance", 5.0, 20.0, (-100.0, 100.0)),
    ("saturation", "Saturation", 5.0, 20.0, (-100.0, 100.0)),
    ("clarity", "Clarity", 5.0, 20.0, (-100.0, 100.0)),
];

/// `s` with `key` moved by `delta` (within its row's range); unknown keys change nothing.
pub fn nudge(s: &EditSettings, key: &str, delta: f64) -> EditSettings {
    let Some((_, _, _, _, (lo, hi))) = ROWS.iter().find(|r| r.0 == key) else {
        return s.clone();
    };
    let mut v = s.to_value();
    if let Some(n) = v.get(key).and_then(serde_json::Value::as_f64) {
        let moved = ((n + delta) * 1000.0).round() / 1000.0;
        v[key] = serde_json::json!(moved.clamp(*lo, *hi));
    }
    EditSettings::from_json_str(&v.to_string())
}

/// What the panel asked for this frame.
enum Ask {
    Nudge(&'static str, f64),
    Reset,
}

impl Library {
    pub fn quick_develop_open(&self) -> bool {
        self.quick_develop
    }

    pub(super) fn toggle_quick_develop(&mut self) {
        self.quick_develop = !self.quick_develop;
    }

    /// The panel (a window over the grid) while it is open.
    pub(super) fn quick_develop_ui(&mut self, ctx: &egui::Context, env: &mut Env) {
        if !self.quick_develop {
            return;
        }
        let n = self.selection.current.len();
        let mut open = true;
        let mut ask = None;
        egui::Window::new(TITLE)
            .open(&mut open)
            .resizable(false)
            .default_pos(ctx.content_rect().right_top() + egui::vec2(-330.0, 140.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(match n {
                        0 => "Select photos to change them.".to_owned(),
                        1 => "Changes the selected photo.".to_owned(),
                        n => format!(
                            "Changes the {} selected photos.",
                            crate::catalog_ext::count(n)
                        ),
                    })
                    .color(crate::theme::MUTED),
                );
                ui.add_enabled_ui(n > 0, |ui| {
                    egui::Grid::new("quick-develop")
                        .num_columns(5)
                        .show(ui, |ui| {
                            for (key, name, small, large, _) in ROWS {
                                ui.label(name);
                                for (text, delta) in
                                    [("<<", -large), ("<", -small), (">", small), (">>", large)]
                                {
                                    let b = ui.small_button(text);
                                    let b = b.on_hover_text(format!("{name} {delta:+.2}"));
                                    b.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            true,
                                            format!("{name} {text}"),
                                        )
                                    });
                                    if b.clicked() {
                                        ask = Some(Ask::Nudge(key, delta));
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    if ui.button(RESET).clicked() {
                        ask = Some(Ask::Reset);
                    }
                });
            });
        self.quick_develop = open;
        let Some(ask) = ask else {
            return;
        };
        let recipe = match ask {
            Ask::Nudge(key, delta) => crate::clipboard::Recipe::Nudge(key, delta),
            Ask::Reset => {
                crate::clipboard::Recipe::Paste(EditSettings::default_settings().to_value())
                    .without_geometry()
            }
        };
        let ids = self.selected_in_order();
        let Some(root) = self.root.clone() else {
            return;
        };
        let saver = self.develop.saver();
        env.jobs.spawn(Lane::Long, "quick develop", move || {
            let stop = std::sync::atomic::AtomicBool::new(false);
            let p = match lf_catalog::Catalog::open(&root) {
                Ok(c) => batch::run(&c, &ids, &recipe, &saver, &stop, |_| {}),
                Err(e) => batch::BatchProgress {
                    failed: ids.iter().map(|&id| (id, e.to_string())).collect(),
                    ..batch::BatchProgress::default()
                },
            };
            Msg::Turned {
                root,
                ids,
                failed: p.failed,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nudge_adds_to_the_photos_own_value_within_range() {
        let mut s = EditSettings::default_settings();
        s.exposure = 4.5;
        let up = nudge(&s, "exposure", 1.0);
        assert_eq!(up.exposure, 5.0, "kept within +5");
        let down = nudge(&up, "exposure", -1.0 / 3.0);
        assert!((down.exposure - 4.667).abs() < 1e-9, "{}", down.exposure);
        let c = nudge(&s, "contrast", 20.0);
        assert_eq!(c.contrast, 20.0);
        assert_eq!(nudge(&s, "crop", 1.0), s, "only the rows' settings");
    }
}
