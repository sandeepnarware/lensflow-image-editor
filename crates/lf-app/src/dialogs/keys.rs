//! Help → Keyboard shortcuts: the keyboard map in force, as this computer writes its keys, and
//! where the keys are changed: "Change…" beside an action waits for the next key pressed
//! (with its modifiers; Esc cancels), which then belongs to that action alone
//! ([`crate::shortcuts::KeyOverride`]); "Reset all" goes back to the default keys.

use crate::shortcuts::{Action, current};
use eframe::egui::{self, KeyboardShortcut, RichText};

pub const TITLE: &str = "Keyboard shortcuts";
/// Under the list.
pub const NOTE: &str = "Keys do nothing while you type in a field or a dialog is open. The tool keys work in Develop. Hold \\ to see the photo as it was.";
pub const CHANGE: &str = "Change…";
pub const PRESS: &str = "Press a key (Esc cancels)…";
pub const RESET: &str = "Reset all";

/// What the dialog asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum KeysAction {
    None,
    Close,
    /// `Action` now has this key.
    Set(Action, KeyboardShortcut),
    /// Back to the default keys.
    Reset,
}

/// The key pressed this frame, if any (not a modifier on its own, not Esc).
fn pressed(ctx: &egui::Context) -> Option<Result<KeyboardShortcut, ()>> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => Some(if *key == egui::Key::Escape {
                Err(())
            } else {
                Ok(KeyboardShortcut::new(*modifiers, *key))
            }),
            _ => None,
        })
    })
}

/// Draws the list; `capture` is the action waiting for its new key.
pub fn show(ctx: &egui::Context, capture: &mut Option<Action>) -> KeysAction {
    let mut action = KeysAction::None;
    if let Some(waiting) = *capture
        && let Some(key) = pressed(ctx)
    {
        *capture = None;
        if let Ok(shortcut) = key {
            return KeysAction::Set(waiting, shortcut);
        }
        return KeysAction::None;
    }
    let map = current();
    let _ = egui::Modal::new(egui::Id::unique("keyboard-shortcuts")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.heading(TITLE);
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .max_height(560.0)
            .show(ui, |ui| {
                egui::Grid::new("shortcut-list")
                    .num_columns(3)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (a, shortcut) in map.entries() {
                            if *capture == Some(*a) {
                                ui.label(RichText::new(PRESS).italics());
                            } else {
                                ui.label(RichText::new(ctx.format_shortcut(shortcut)).monospace());
                            }
                            ui.label(a.label());
                            let change = ui.small_button(CHANGE);
                            let name = format!("Change {}", a.label());
                            change.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                            });
                            if change.clicked() {
                                *capture = Some(*a);
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(8.0);
        ui.label(RichText::new(NOTE).small());
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Close").clicked() {
                action = KeysAction::Close;
            }
            if ui.button(RESET).clicked() {
                action = KeysAction::Reset;
            }
        });
    });
    if action == KeysAction::None
        && capture.is_none()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        action = KeysAction::Close;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::MAP;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    #[test]
    fn the_list_shows_every_entry() {
        let mut h = Harness::builder().with_size([600.0, 900.0]).build_ui_state(
            |ui, (closed, capture): &mut (bool, Option<Action>)| {
                *closed |= show(ui.ctx(), capture) == KeysAction::Close;
            },
            (false, None),
        );
        h.run();
        h.get_by_label(Action::Export.label());
        h.get_by_label("Ctrl+Shift+E");
        assert!(MAP.entries().len() > 20);
    }

    #[test]
    fn change_waits_for_the_next_key() {
        let mut h = Harness::builder().with_size([600.0, 900.0]).build_ui_state(
            |ui, (got, capture): &mut (Option<KeysAction>, Option<Action>)| {
                let a = show(ui.ctx(), capture);
                if a != KeysAction::None {
                    *got = Some(a);
                }
            },
            (None, None),
        );
        h.run();
        // A Change… button starts the wait (the harness's click is not used to pick the row:
        // the modal is still settling).
        h.get_all_by_label_contains("Change ")
            .next()
            .unwrap()
            .click();
        h.run();
        assert!(h.state().1.is_some());
        h.state_mut().1 = Some(Action::Export);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
        h.run();
        assert_eq!(
            h.state().0,
            Some(KeysAction::Set(
                Action::Export,
                KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K)
            ))
        );
    }
}
