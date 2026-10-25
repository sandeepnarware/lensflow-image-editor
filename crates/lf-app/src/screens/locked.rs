//! Why the app cannot be used right now (paused, suspended, not enabled, no permission,
//! offline, refused), with a way to check again, switch studio or sign out.

use super::{Action, View, card, logo};
use crate::theme;
use eframe::egui::{self, RichText};

pub fn show(ui: &mut egui::Ui, message: &str, studio: Option<&str>, view: &View) -> Option<Action> {
    let mut action = None;
    ui.vertical_centered(|ui| {
        logo(ui, view);
        card(ui, |ui| {
            if let Some(studio) = studio {
                ui.label(RichText::new(studio).strong());
                ui.add_space(4.0);
            }
            ui.label(message);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Try again").clicked() {
                    action = Some(Action::Retry);
                }
                if view.can_switch && ui.button("Switch studio").clicked() {
                    action = Some(Action::SwitchStudio);
                }
                if ui
                    .button(RichText::new("Sign out").color(theme::MUTED))
                    .clicked()
                {
                    action = Some(Action::SignOut);
                }
            });
        });
    });
    action
}
