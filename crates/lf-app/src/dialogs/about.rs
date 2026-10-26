//! Help → About (`F1`): the logo, name and version, what the app is built with, the log
//! folder (with "Show log folder"), "Copy diagnostics" for a support request, the licences
//! of everything it bundles (the repository's NOTICE file, compiled in) and the website.

use crate::app::VERSION;
use crate::theme;
use eframe::egui::{self, RichText};
use std::path::Path;

pub const NAME: &str = "lensflow-ImageEditor";
pub const BUILT_WITH: &str = "Built with Rust, egui and wgpu.";
pub const WEBSITE: &str = "https://lensflow.in";
/// The repository's NOTICE file: the app's licence and every bundled component's.
pub const NOTICE: &str = include_str!("../../../../NOTICE");

pub const SHOW_LOGS: &str = "Show log folder";
pub const COPY_DIAGNOSTICS: &str = "Copy diagnostics";
/// The toast after "Copy diagnostics".
pub const COPIED: &str = "Diagnostics copied. Paste them into your message to support.";

/// What the dialog shows besides its fixed text.
pub struct AboutView<'a> {
    pub logo: Option<egui::TextureId>,
    pub log_dir: &'a Path,
    /// [`crate::diagnostics::text`], copied by "Copy diagnostics".
    pub diagnostics: &'a str,
}

/// What a click asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AboutAction {
    Close,
    ShowLogs,
    /// The diagnostics were put on the clipboard.
    Copied,
}

/// "Version 0.1.0".
pub fn version_text() -> String {
    format!("Version {VERSION}")
}

/// Draws the dialog; what was clicked (Close or Esc closes it).
pub fn show(ctx: &egui::Context, view: &AboutView) -> Option<AboutAction> {
    let logo = view.logo;
    let mut action = None;
    let _ = egui::Modal::new(egui::Id::unique("about")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.vertical_centered(|ui| {
            if let Some(logo) = logo {
                ui.image(egui::load::SizedTexture::new(logo, [72.0, 72.0]));
                ui.add_space(8.0);
            }
            ui.heading(NAME);
            ui.label(RichText::new(version_text()).color(theme::MUTED));
            ui.add_space(4.0);
            ui.label(BUILT_WITH);
            ui.hyperlink(WEBSITE);
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("Log folder: {}", view.log_dir.display()))
                    .color(theme::MUTED)
                    .small(),
            );
            ui.horizontal(|ui| {
                if ui.button(SHOW_LOGS).clicked() {
                    action = Some(AboutAction::ShowLogs);
                }
                if ui.button(COPY_DIAGNOSTICS).clicked() {
                    ui.ctx().copy_text(view.diagnostics.to_owned());
                    action = Some(AboutAction::Copied);
                }
            });
        });
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("notice")
            .max_height(280.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.label(RichText::new(NOTICE).monospace().small());
            });
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if ui.button("Close").clicked() {
                action = Some(AboutAction::Close);
            }
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(AboutAction::Close);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    #[test]
    fn about_shows_name_version_credits_notice_and_website() {
        assert!(
            NOTICE.contains("Apache License"),
            "the NOTICE file is compiled in"
        );
        let log_dir = Path::new("/data/logs");
        let mut h = Harness::builder().with_size([800.0, 760.0]).build_ui_state(
            |ui, last: &mut Option<AboutAction>| {
                let view = AboutView {
                    logo: None,
                    log_dir,
                    diagnostics: "lensflow-ImageEditor 0.1.0 (test)",
                };
                if let Some(action) = show(ui.ctx(), &view) {
                    *last = Some(action);
                }
            },
            None,
        );
        h.run();
        h.get_by_label(NAME);
        h.get_by_label(&version_text());
        h.get_by_label(BUILT_WITH);
        h.get_by_label(NOTICE);
        // The link asks the platform to open the website (eframe's `links` feature carries
        // the request to the browser; without it egui-winit only logs it).
        h.get_by_label(WEBSITE).click();
        h.step();
        let opened = h
            .output()
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::OpenUrl(open) if open.url == WEBSITE));
        assert!(opened, "clicking the link opens {WEBSITE}");
        assert!(
            include_str!("../../Cargo.toml").contains("\"links\""),
            "eframe is built with its links feature"
        );
        h.get_by_label(&format!("Log folder: {}", log_dir.display()));
        h.get_by_label(SHOW_LOGS).click();
        h.step();
        assert_eq!(*h.state(), Some(AboutAction::ShowLogs));
        h.get_by_label(COPY_DIAGNOSTICS).click();
        h.step();
        let copied = h.output().platform_output.commands.iter().any(|c| {
            matches!(c, egui::OutputCommand::CopyText(t) if t == "lensflow-ImageEditor 0.1.0 (test)")
        });
        assert!(copied, "the diagnostics go to the clipboard");
        assert_eq!(*h.state(), Some(AboutAction::Copied));
        h.get_by_label("Close").click();
        h.run();
        assert_eq!(*h.state(), Some(AboutAction::Close));
    }
}
