//! The window's one menu bar: File, Edit, View, Photo, Account and Help. Each item is an
//! [`Action`] (the same ones the keyboard map fires) and shows its key from [`MAP`]; the app
//! says which are possible now and does what is clicked.

use crate::session::Session;
use crate::shortcuts::{Action, MAP};
use eframe::egui;
use lf_catalog::cull::{Flag, Label};
use std::collections::HashSet;

/// One line of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Do(Action),
    Separator,
    /// The studio signed in to (a label).
    Studio,
    /// A submenu.
    Sub(&'static str, &'static [Item]),
}

use Item::{Do, Separator, Studio, Sub};

/// Photo → Set flag.
const FLAGS: &[Item] = &[
    Do(Action::Flag(Flag::Picked)),
    Do(Action::Flag(Flag::Rejected)),
    Do(Action::Flag(Flag::Unflagged)),
];

/// Photo → Set rating.
const RATINGS: &[Item] = &[
    Do(Action::Rate(0)),
    Do(Action::Rate(1)),
    Do(Action::Rate(2)),
    Do(Action::Rate(3)),
    Do(Action::Rate(4)),
    Do(Action::Rate(5)),
];

/// Photo → Set colour label.
const LABELS: &[Item] = &[
    Do(Action::Label(Some(Label::Red))),
    Do(Action::Label(Some(Label::Yellow))),
    Do(Action::Label(Some(Label::Green))),
    Do(Action::Label(Some(Label::Blue))),
    Do(Action::Label(Some(Label::Purple))),
    Separator,
    Do(Action::Label(None)),
];

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
            Separator,
            Do(Action::AutoTone),
            Do(Action::AutoWhiteBalance),
            Separator,
            Sub("Set flag", FLAGS),
            Sub("Set rating", RATINGS),
            Sub("Set colour label", LABELS),
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

/// Every action the menus hold, submenus included.
pub fn actions() -> Vec<Action> {
    fn walk(items: &[Item], out: &mut Vec<Action>) {
        for item in items {
            match item {
                Do(action) => out.push(*action),
                Sub(_, items) => walk(items, out),
                Separator | Studio => {}
            }
        }
    }
    let mut out = Vec::new();
    for (_, items) in MENUS {
        walk(items, &mut out);
    }
    out
}

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
            ui.menu_button(*title, |ui| items_ui(ui, items, state, &mut clicked));
        }
    });
    clicked
}

/// One menu's (or submenu's) lines.
fn items_ui(ui: &mut egui::Ui, items: &[Item], state: &MenuState, clicked: &mut Option<Action>) {
    for item in items {
        match item {
            Separator => {
                ui.separator();
            }
            Studio => {
                let name = state.studio.as_deref().unwrap_or(NOT_SIGNED_IN);
                ui.label(egui::RichText::new(name).strong());
            }
            Sub(title, items) => {
                ui.menu_button(*title, |ui| items_ui(ui, items, state, clicked));
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
                    *clicked = Some(*action);
                    ui.close();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    fn every_action() -> HashSet<Action> {
        actions().into_iter().collect()
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
                &[
                    "Match a reference…",
                    "Add version…",
                    "Reset all edits",
                    "Set flag",
                    "Set rating",
                    "Set colour label",
                ],
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
        // A submenu's item too.
        h.get_by_label("Photo").click();
        h.run();
        h.get_by_label("Set rating").hover();
        h.run();
        h.get_by_label_contains("3 stars").click();
        h.run();
        assert_eq!(h.state().1, Some(Action::Rate(3)));
    }

    #[test]
    fn every_culling_action_is_in_a_menu() {
        let all = every_action();
        for action in [
            Action::Flag(Flag::Picked),
            Action::Flag(Flag::Rejected),
            Action::Flag(Flag::Unflagged),
            Action::Label(None),
        ]
        .into_iter()
        .chain((0..=5).map(Action::Rate))
        .chain(Label::ALL.into_iter().map(|l| Action::Label(Some(l))))
        {
            assert!(all.contains(&action), "{action:?} is in no menu");
        }
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
