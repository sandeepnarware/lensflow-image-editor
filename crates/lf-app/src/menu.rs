//! The window's one menu bar: File, Edit, View, Photo, Account and Help. Each item is an
//! [`Action`] (the same ones the keyboard map fires) and shows its key from [`MAP`]; the app
//! says which are possible now and does what is clicked.

use crate::session::Session;
use crate::shortcuts::{Action, MAP};
use eframe::egui;
use std::collections::HashSet;

/// One line of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Do(Action),
    Separator,
    /// The studio signed in to (a label).
    Studio,
}

use Item::{Do, Separator, Studio};

/// The menus and their items, in order.
pub const MENUS: &[(&str, &[Item])] = &[
    (
        "File",
        &[
            Do(Action::AddFolder),
            Do(Action::Export),
            Separator,
            Do(Action::Quit),
        ],
    ),
    (
        "Edit",
        &[
            Do(Action::Undo),
            Do(Action::Redo),
            Separator,
            Do(Action::CopySettings),
            Do(Action::PasteSettings),
            Do(Action::ApplyToSelected),
            Do(Action::SelectAll),
            Separator,
            Do(Action::Preferences),
        ],
    ),
    (
        "View",
        &[
            Do(Action::Library),
            Do(Action::Develop),
            Separator,
            Do(Action::BeforeAfter),
            Do(Action::Fit),
            Do(Action::ActualSize),
        ],
    ),
    (
        "Photo",
        &[
            Do(Action::MatchReference),
            Do(Action::AddVersion),
            Do(Action::ResetAll),
        ],
    ),
    (
        "Account",
        &[
            Studio,
            Separator,
            Do(Action::SwitchStudio),
            Do(Action::CheckEntitlement),
            Do(Action::SignOut),
        ],
    ),
    (
        "Help",
        &[
            Do(Action::About),
            Do(Action::ShowLogFolder),
            Do(Action::KeyboardShortcuts),
        ],
    ),
];

/// Shown in the Account menu when no studio is open.
pub const NOT_SIGNED_IN: &str = "Not signed in";

/// What the menus show: the actions possible now, the studio, and the Before/After state.
#[derive(Debug, Clone, Default)]
pub struct MenuState {
    pub enabled: HashSet<Action>,
    pub studio: Option<String>,
    pub before: bool,
}

/// The Account menu's actions possible in `session`: all need a studio open; switching
/// needs a second studio (`can_switch`).
pub fn account_actions(session: &Session, can_switch: bool) -> Vec<Action> {
    if !matches!(session, Session::Active { .. }) {
        return Vec::new();
    }
    let mut actions = vec![Action::CheckEntitlement, Action::SignOut];
    if can_switch {
        actions.push(Action::SwitchStudio);
    }
    actions
}

/// The actions possible on any screen: Quit, Preferences and the Help menu.
pub const ALWAYS: &[Action] = &[
    Action::Quit,
    Action::Preferences,
    Action::About,
    Action::ShowLogFolder,
    Action::KeyboardShortcuts,
];

/// Draws the menu bar; returns the item clicked.
pub fn bar(ui: &mut egui::Ui, state: &MenuState) -> Option<Action> {
    let mut clicked = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for (title, items) in MENUS {
            ui.menu_button(*title, |ui| {
                for item in *items {
                    match item {
                        Separator => {
                            ui.separator();
                        }
                        Studio => {
                            let name = state.studio.as_deref().unwrap_or(NOT_SIGNED_IN);
                            ui.label(egui::RichText::new(name).strong());
                        }
                        Do(action) => {
                            let mut button = if *action == Action::BeforeAfter {
                                egui::Button::selectable(state.before, action.label())
                            } else {
                                egui::Button::new(action.label())
                            };
                            if let Some(shortcut) = MAP.shortcut(*action) {
                                button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
                            }
                            if ui
                                .add_enabled(state.enabled.contains(action), button)
                                .clicked()
                            {
                                clicked = Some(*action);
                                ui.close();
                            }
                        }
                    }
                }
            });
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    fn every_action() -> HashSet<Action> {
        MENUS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .filter_map(|item| match item {
                Do(action) => Some(*action),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_menu_exists_with_its_items() {
        let state = MenuState {
            enabled: every_action(),
            studio: Some("Sharma Studio".into()),
            before: false,
        };
        let mut h = Harness::builder().with_size([900.0, 600.0]).build_ui_state(
            |ui, (state, clicked): &mut (MenuState, Option<Action>)| {
                if let Some(action) = bar(ui, state) {
                    *clicked = Some(action);
                }
            },
            (state, None),
        );
        let expected: &[(&str, &[&str])] = &[
            ("File", &["Add folder…", "Export…", "Quit"]),
            (
                "Edit",
                &[
                    "Undo",
                    "Redo",
                    "Copy settings…",
                    "Paste settings",
                    "Select all",
                    "Preferences…",
                ],
            ),
            (
                "View",
                &["Library", "Develop", "Before/After", "Fit", "100 %"],
            ),
            (
                "Photo",
                &["Match a reference…", "Add version…", "Reset all edits"],
            ),
            (
                "Account",
                &[
                    "Sharma Studio",
                    "Switch studio",
                    "Check entitlement now",
                    "Sign out",
                ],
            ),
            ("Help", &["About", "Show log folder", "Keyboard shortcuts"]),
        ];
        for (menu, items) in expected {
            h.get_by_label(menu).click();
            h.run();
            for item in *items {
                assert!(
                    h.query_by_label_contains(item).is_some(),
                    "{menu} has no item {item:?}"
                );
            }
            h.get_by_label(menu).click();
            h.run();
        }
        // An item does what it says.
        h.get_by_label("Help").click();
        h.run();
        h.get_by_label_contains("Keyboard shortcuts").click();
        h.run();
        assert_eq!(h.state().1, Some(Action::KeyboardShortcuts));
    }

    #[test]
    fn a_disabled_item_does_nothing() {
        let state = MenuState::default();
        let mut h = Harness::builder().with_size([900.0, 600.0]).build_ui_state(
            |ui, (state, clicked): &mut (MenuState, Option<Action>)| {
                if let Some(action) = bar(ui, state) {
                    *clicked = Some(action);
                }
            },
            (state, None),
        );
        h.get_by_label("Account").click();
        h.run();
        h.get_by_label(NOT_SIGNED_IN);
        h.get_by_label("Sign out").click();
        h.run();
        assert_eq!(h.state().1, None);
    }
}
