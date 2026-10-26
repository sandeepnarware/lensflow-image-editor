//! Email and password, then the two-factor code when the account asks for one.

use super::{Action, View, card, logo};
use crate::session::Session;
use crate::theme;
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, RichText, TextEdit};

pub const CODE_LABEL: &str = "Code from your authenticator app";

/// The sign-in form; `session` must be `SigningIn` (anything else draws nothing).
pub fn show(ui: &mut egui::Ui, session: &mut Session, view: &View) -> Option<Action> {
    let Session::SigningIn {
        email,
        password,
        code,
        two_factor,
        busy,
        error,
    } = session
    else {
        return None;
    };
    let mut action = None;
    let toggle = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::D);
    if !*busy && ui.input_mut(|i| i.consume_shortcut(&toggle)) {
        action = Some(Action::ToggleInstance);
    }
    ui.vertical_centered(|ui| {
        logo(ui, view);
        card(ui, |ui| {
            ui.heading("Sign in to LensFlow");
            ui.add_space(12.0);
            ui.add_enabled_ui(!*busy, |ui| {
                if *two_factor {
                    field(ui, CODE_LABEL, code, false);
                } else {
                    field(ui, "Email", email, false);
                    field(ui, "Password", password, true);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    if ui.button("Sign in").clicked() || enter {
                        action = Some(Action::SignIn);
                    }
                    if *two_factor && ui.button("Use a different account").clicked() {
                        action = Some(Action::StartOver);
                    }
                    if *busy {
                        ui.spinner();
                    }
                });
            });
            if let Some(error) = error {
                ui.add_space(8.0);
                ui.label(RichText::new(error.as_str()).color(theme::DANGER));
            }
        });
    });
    if *busy && action == Some(Action::SignIn) {
        return None;
    }
    action
}

fn field(ui: &mut egui::Ui, label: &str, text: &mut String, password: bool) {
    let label = ui.label(label);
    ui.add(
        TextEdit::singleline(text)
            .password(password)
            .desired_width(f32::INFINITY),
    )
    .labelled_by(label.id);
    ui.add_space(6.0);
}
