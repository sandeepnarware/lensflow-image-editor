//! Edit → Preferences…: the catalogue folder (shown, with "Change…", which restarts the
//! library on another one), the frame cache's size, how carefully RAW photos are decoded for
//! Develop, and where the log is. Each change is saved at once.

use crate::prefs::{DevelopQuality, Prefs};
use crate::theme;
use eframe::egui::{self, RichText};
use std::path::Path;

pub const TITLE: &str = "Preferences";
pub const CATALOGUE: &str = "Catalogue folder";
pub const CHANGE: &str = "Change…";
pub const NO_ROOT: &str = "None chosen yet.";
pub const CHANGE_NOTE: &str = "Changing it closes this catalogue and opens another. Your photos and edits stay where they are.";
pub const CACHE: &str = "Frame cache size (GB)";
pub const CACHE_NOTE: &str = "Decoded RAW photos are kept here so they open again quickly. A new size applies the next time the catalogue opens.";
pub const QUALITY: &str = "Develop quality";
pub const BEST: &str = "Best";
pub const FAST: &str = "Fast";
pub const QUALITY_NOTE: &str =
    "Fast opens RAW photos sooner, with a little less detail. Photos opened from now on use it.";
pub const LOGS: &str = "Log folder";
pub const SHOW_LOGS: &str = "Show";

/// The smallest and largest frame cache offered, in GB.
pub const CACHE_GB: std::ops::RangeInclusive<u64> = 1..=64;

/// What the dialog shows besides the preferences.
pub struct PrefsView<'a> {
    pub root: Option<&'a Path>,
    /// Why "Change…" is off now, if it is.
    pub change_blocked: Option<&'static str>,
    pub log_dir: &'a Path,
}

/// What the user did in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsAction {
    Close,
    ChangeRoot,
    ShowLogs,
    /// A preference changed (save it).
    Changed,
}

/// The frame cache's size in whole GB (at least 1).
pub fn cache_gb(bytes: u64) -> u64 {
    (bytes >> 30).max(1)
}

/// Draws the dialog over `prefs`.
pub fn show(ctx: &egui::Context, prefs: &mut Prefs, view: &PrefsView) -> Option<PrefsAction> {
    let mut action = None;
    let _ = egui::Modal::new(egui::Id::unique("preferences")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.heading(TITLE);
        ui.add_space(12.0);

        ui.label(RichText::new(CATALOGUE).strong());
        ui.horizontal(|ui| {
            let shown = view
                .root
                .map_or_else(|| NO_ROOT.to_owned(), |p| p.display().to_string());
            ui.label(RichText::new(shown).color(theme::MUTED));
            let change = ui.add_enabled(view.change_blocked.is_none(), egui::Button::new(CHANGE));
            let change = match view.change_blocked {
                Some(reason) => change.on_disabled_hover_text(reason),
                None => change,
            };
            if change.clicked() {
                action = Some(PrefsAction::ChangeRoot);
            }
        });
        ui.label(RichText::new(CHANGE_NOTE).small());
        ui.add_space(12.0);

        let mut gb = cache_gb(prefs.frame_cache_bytes);
        ui.horizontal(|ui| {
            let label = ui.label(RichText::new(CACHE).strong());
            ui.add(egui::DragValue::new(&mut gb).range(CACHE_GB))
                .labelled_by(label.id);
        });
        if gb != cache_gb(prefs.frame_cache_bytes) {
            prefs.frame_cache_bytes = gb << 30;
            action = Some(PrefsAction::Changed);
        }
        ui.label(RichText::new(CACHE_NOTE).small());
        ui.add_space(12.0);

        ui.label(RichText::new(QUALITY).strong());
        ui.horizontal(|ui| {
            for (value, label) in [(DevelopQuality::Best, BEST), (DevelopQuality::Fast, FAST)] {
                if ui.radio(prefs.develop_quality == value, label).clicked()
                    && prefs.develop_quality != value
                {
                    prefs.develop_quality = value;
                    action = Some(PrefsAction::Changed);
                }
            }
        });
        ui.label(RichText::new(QUALITY_NOTE).small());
        ui.add_space(12.0);

        ui.label(RichText::new(LOGS).strong());
        ui.horizontal(|ui| {
            ui.label(RichText::new(view.log_dir.display().to_string()).color(theme::MUTED));
            if ui.button(SHOW_LOGS).clicked() {
                action = Some(PrefsAction::ShowLogs);
            }
        });
        ui.add_space(16.0);
        if ui.button("Close").clicked() {
            action = Some(PrefsAction::Close);
        }
    });
    if action.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(PrefsAction::Close);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;
    use std::path::PathBuf;

    struct State {
        prefs: Prefs,
        blocked: Option<&'static str>,
        actions: Vec<PrefsAction>,
    }

    fn harness(blocked: Option<&'static str>) -> Harness<'static, State> {
        let state = State {
            prefs: Prefs {
                catalogue_root: Some(PathBuf::from("photos-catalogue")),
                ..Prefs::default()
            },
            blocked,
            actions: Vec::new(),
        };
        Harness::builder().with_size([700.0, 700.0]).build_ui_state(
            |ui, s: &mut State| {
                let ctx = ui.ctx().clone();
                let root = s.prefs.catalogue_root.clone();
                let view = PrefsView {
                    root: root.as_deref(),
                    change_blocked: s.blocked,
                    log_dir: Path::new("logs-here"),
                };
                if let Some(a) = show(&ctx, &mut s.prefs, &view) {
                    s.actions.push(a);
                }
            },
            state,
        )
    }

    #[test]
    fn preferences_show_the_catalogue_and_change_quality() {
        let mut h = harness(None);
        h.run();
        h.get_by_label("photos-catalogue");
        h.get_by_label("logs-here");
        h.get_by_label(FAST).click();
        h.run();
        assert_eq!(h.state().prefs.develop_quality, DevelopQuality::Fast);
        assert_eq!(h.state().actions, [PrefsAction::Changed]);
        h.get_by_label(CHANGE).click();
        h.run();
        assert_eq!(h.state().actions.last(), Some(&PrefsAction::ChangeRoot));
        assert_eq!(cache_gb(4 << 30), 4);
        assert_eq!(cache_gb(0), 1);
    }

    #[test]
    fn change_is_off_while_an_export_runs() {
        let mut h = harness(Some(
            "Wait for the export to finish, then change the catalogue folder.",
        ));
        h.run();
        h.get_by_label(CHANGE).click();
        h.run();
        assert!(h.state().actions.is_empty());
    }
}
