//! Choosing one of the user's studios (name and role).

use super::{Action, View, card, logo};
use crate::session::NO_STUDIO_MESSAGE;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_account::auth::User;

pub fn show(ui: &mut egui::Ui, user: &User, view: &View) -> Option<Action> {
    let mut action = None;
    ui.vertical_centered(|ui| {
        logo(ui, view);
        card(ui, |ui| {
            ui.heading("Choose a studio");
            ui.add_space(12.0);
            if user.studios.is_empty() {
                ui.label(NO_STUDIO_MESSAGE);
            }
            for studio in &user.studios {
                ui.horizontal(|ui| {
                    if ui.button(&studio.name).clicked() {
                        action = Some(Action::ChooseStudio(studio.slug.clone()));
                    }
                    if let Some(role) = &studio.role {
                        ui.label(RichText::new(role).color(theme::MUTED));
                    }
                });
            }
            ui.add_space(12.0);
            if ui.button("Sign out").clicked() {
                action = Some(Action::SignOut);
            }
        });
    });
    action
}
