//! Help → About (`F1`): the logo, name and version, what the app is built with, the licences
//! of everything it bundles (the repository's NOTICE file, compiled in) and the website.

use crate::app::VERSION;
use crate::theme;
use eframe::egui::{self, RichText};

pub const NAME: &str = "lensflow-ImageEditor";
pub const BUILT_WITH: &str = "Built with Rust, egui and wgpu.";
pub const WEBSITE: &str = "https://lensflow.in";
/// The repository's NOTICE file: the app's licence and every bundled component's.
pub const NOTICE: &str = include_str!("../../../../NOTICE");

/// "Version 0.1.0".
pub fn version_text() -> String {
    format!("Version {VERSION}")
}

/// Draws the dialog; true when it was closed (Close or Esc).
pub fn show(ctx: &egui::Context, logo: Option<egui::TextureId>) -> bool {
    let mut close = false;
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
                close = true;
            }
        });
    });
    close || ctx.input(|i| i.key_pressed(egui::Key::Escape))
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
        let mut h = Harness::builder().with_size([800.0, 700.0]).build_ui_state(
            |ui, closed: &mut bool| *closed |= show(ui.ctx(), None),
            false,
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
        h.get_by_label("Close").click();
        h.run();
        assert!(*h.state());
    }
}
