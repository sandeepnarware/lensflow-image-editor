//! This version is older than the studio's minimum: install the latest to go on.

use super::{Action, View, card, logo, update_button};
use eframe::egui;
use lf_account::entitlement::Release;

pub const UPDATE_REQUIRED_MESSAGE: &str =
    "This version of lensflow-ImageEditor is too old for your studio. Update to keep working.";

pub fn show(ui: &mut egui::Ui, latest: Option<&Release>, view: &View) -> Option<Action> {
    let mut action = None;
    ui.vertical_centered(|ui| {
        logo(ui, view);
        card(ui, |ui| {
            ui.label(UPDATE_REQUIRED_MESSAGE);
            if let Some(release) = latest {
                ui.label(format!("Version {} is ready.", release.version));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                action = update_button(ui, view);
                if ui.button("Sign out").clicked() {
                    action = Some(Action::SignOut);
                }
            });
        });
    });
    action
}
