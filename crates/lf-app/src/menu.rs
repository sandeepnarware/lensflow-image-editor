//! The window's ribbon, in the manner of Microsoft Office: a row of tabs (File, Home,
//! Library, Develop, View, Account, Help), and under the chosen tab its groups of large
//! buttons, each an icon over its name. Every button is an [`Action`], the same ones the
//! keyboard map fires; its tooltip shows the key from [`MAP`]. The app says which actions are
//! possible now and does what is clicked.
//!
//! The ribbon follows the view: entering Develop shows the Develop tab and going back to the
//! library shows the Library tab, once per change, so the tools for what is on screen are at
//! hand while any tab can still be chosen. It can be folded down to its tabs. The chosen tab
//! and the fold live in egui's memory (they are not preferences).

use crate::session::Session;
use crate::shortcuts::{Action, MAP};
use crate::theme;
use eframe::egui::{self, Color32, Sense};
use lf_catalog::cull::{Flag, Label};
use std::collections::HashSet;

/// One thing in a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Do(Action),
    /// The studio signed in to (a label).
    Studio,
}

use Item::{Do, Studio};

/// A labelled group of buttons on a tab.
#[derive(Debug, Clone, Copy)]
pub struct Group {
    pub title: &'static str,
    pub items: &'static [Item],
}

/// A ribbon tab.
#[derive(Debug, Clone, Copy)]
pub struct Tab {
    pub title: &'static str,
    pub groups: &'static [Group],
}

const fn group(title: &'static str, items: &'static [Item]) -> Group {
    Group { title, items }
}

pub const HOME: &str = "Home";
pub const LIBRARY: &str = "Library";
pub const DEVELOP: &str = "Develop";

/// The tabs, their groups and buttons, in order.
pub const RIBBON: &[Tab] = &[
    Tab {
        title: "File",
        groups: &[
            group(
                "Photos",
                &[
                    Do(Action::AddFolder),
                    Do(Action::ImportFromCard),
                    Do(Action::Export),
                    Do(Action::ExportToGallery),
                    Do(Action::Print),
                ],
            ),
            group("Merge", &[Do(Action::MergeHdr), Do(Action::MergePanorama)]),
            group("App", &[Do(Action::Preferences), Do(Action::Quit)]),
        ],
    },
    Tab {
        title: HOME,
        groups: &[
            group("History", &[Do(Action::Undo), Do(Action::Redo)]),
            group(
                "Settings",
                &[
                    Do(Action::CopySettings),
                    Do(Action::PasteSettings),
                    Do(Action::ApplyToSelected),
                ],
            ),
            group("Selection", &[Do(Action::SelectAll)]),
            group(
                "Albums",
                &[
                    Do(Action::NewAlbum),
                    Do(Action::NewSmartAlbum),
                    Do(Action::AddToAlbum),
                    Do(Action::RemoveFromAlbum),
                ],
            ),
        ],
    },
    Tab {
        title: LIBRARY,
        groups: &[
            group(
                "Flag",
                &[
                    Do(Action::Flag(Flag::Picked)),
                    Do(Action::Flag(Flag::Rejected)),
                    Do(Action::Flag(Flag::Unflagged)),
                ],
            ),
            group(
                "Rating",
                &[
                    Do(Action::Rate(0)),
                    Do(Action::Rate(1)),
                    Do(Action::Rate(2)),
                    Do(Action::Rate(3)),
                    Do(Action::Rate(4)),
                    Do(Action::Rate(5)),
                ],
            ),
            group(
                "Colour label",
                &[
                    Do(Action::Label(Some(Label::Red))),
                    Do(Action::Label(Some(Label::Yellow))),
                    Do(Action::Label(Some(Label::Green))),
                    Do(Action::Label(Some(Label::Blue))),
                    Do(Action::Label(Some(Label::Purple))),
                    Do(Action::Label(None)),
                ],
            ),
            group("View", &[Do(Action::Compare)]),
        ],
    },
    Tab {
        title: DEVELOP,
        groups: &[
            group(
                "Tools",
                &[
                    Do(Action::CropTool),
                    Do(Action::SpotTool),
                    Do(Action::WhiteBalanceTool),
                    Do(Action::MaskTool),
                ],
            ),
            group(
                "Auto",
                &[Do(Action::AutoTone), Do(Action::AutoWhiteBalance)],
            ),
            group(
                "Photo",
                &[
                    Do(Action::MatchReference),
                    Do(Action::EditIn),
                    Do(Action::AddVersion),
                    Do(Action::ResetAll),
                ],
            ),
            group("History", &[Do(Action::Undo), Do(Action::Redo)]),
        ],
    },
    Tab {
        title: "AI",
        groups: &[
            group("Enhance", &[Do(Action::AiDenoise), Do(Action::AiUpscale)]),
            group("Retouch", &[Do(Action::AiRetouch), Do(Action::AiCutout)]),
        ],
    },
    Tab {
        title: "View",
        groups: &[
            group("Mode", &[Do(Action::Library), Do(Action::Develop)]),
            group("Panels", &[Do(Action::Metadata)]),
            group(
                "Preview",
                &[
                    Do(Action::BeforeAfter),
                    Do(Action::Clipping),
                    Do(Action::Fit),
                    Do(Action::ActualSize),
                ],
            ),
        ],
    },
    Tab {
        title: "Account",
        groups: &[
            group("Studio", &[Studio]),
            group(
                "Sign-in",
                &[
                    Do(Action::SwitchStudio),
                    Do(Action::CheckEntitlement),
                    Do(Action::SignOut),
                ],
            ),
        ],
    },
    Tab {
        title: "Help",
        groups: &[group(
            "Support",
            &[
                Do(Action::About),
                Do(Action::ShowLogFolder),
                Do(Action::KeyboardShortcuts),
            ],
        )],
    },
];

/// Shown on the Account tab when no studio is open.
pub const NOT_SIGNED_IN: &str = "Not signed in";
/// The fold button, open and folded.
pub const FOLD: &str = "Hide ribbon";
pub const UNFOLD: &str = "Show ribbon";

/// Every action the ribbon holds (once each).
pub fn actions() -> Vec<Action> {
    let mut out = Vec::new();
    for tab in RIBBON {
        for g in tab.groups {
            for item in g.items {
                if let Do(action) = item
                    && !out.contains(action)
                {
                    out.push(*action);
                }
            }
        }
    }
    out
}

/// A button's icon: a glyph egui's own fonts have (`ribbon_icons_are_in_the_fonts` checks).
pub fn icon(action: Action) -> &'static str {
    match action {
        Action::AddFolder => "📁",
        Action::ImportFromCard => "💾",
        Action::Export => "📤",
        Action::ExportToGallery => "☁",
        Action::Print => "🖶",
        Action::Quit => "🚪",
        Action::Undo => "⟲",
        Action::Redo => "⟳",
        Action::CopySettings => "📋",
        Action::PasteSettings => "📥",
        Action::ApplyToSelected => "🗐",
        Action::SelectAll => "☑",
        Action::Preferences => "⚙",
        Action::Library => "📚",
        Action::Develop => "🎨",
        Action::BeforeAfter => "◑",
        Action::Clipping => "⚠",
        Action::Fit => "⛶",
        Action::ActualSize => "🔍",
        Action::ToggleZoom => "🔍",
        Action::Metadata => "📝",
        Action::Compare => "⚖",
        Action::AiDenoise => "✨",
        Action::AiUpscale => "⛶",
        Action::AiRetouch => "☺",
        Action::AiCutout => "✂",
        Action::NewAlbum => "🗀",
        Action::NewSmartAlbum => "⚙",
        Action::AddToAlbum => "➕",
        Action::RemoveFromAlbum => "➖",
        Action::MatchReference => "🎯",
        Action::EditIn => "✏",
        Action::MergeHdr => "◑",
        Action::MergePanorama => "↔",
        Action::AddVersion => "➕",
        Action::ResetAll => "↺",
        Action::AutoTone => "✨",
        Action::AutoWhiteBalance => "☀",
        Action::Flag(Flag::Picked) => "⚑",
        Action::Flag(Flag::Rejected) => "✖",
        Action::Flag(Flag::Unflagged) => "⚐",
        Action::Rate(0) => "☆",
        Action::Rate(_) => "★",
        Action::Label(Some(_)) => "⏺",
        Action::Label(None) => "○",
        Action::SwitchStudio => "🔀",
        Action::CheckEntitlement => "🔄",
        Action::SignOut => "🔒",
        Action::About => "ℹ",
        Action::ShowLogFolder => "📄",
        Action::KeyboardShortcuts => "⌨",
        Action::CropTool => "✂",
        Action::SpotTool => "✚",
        Action::WhiteBalanceTool => "💧",
        Action::MaskTool => "🎭",
        Action::Previous => "⏴",
        Action::Next => "⏵",
        Action::Up => "⏶",
        Action::Down => "⏷",
        Action::LeaveTool => "⎋",
        Action::ApplyCrop => "✔",
        Action::DeleteSpot => "🗑",
    }
}

/// A button's short name on the ribbon (the menu's longer label is its tooltip).
pub fn short_label(action: Action) -> &'static str {
    match action {
        Action::AddFolder => "Add folder",
        Action::ImportFromCard => "From card",
        Action::Export => "Export",
        Action::ExportToGallery => "To gallery",
        Action::CopySettings => "Copy settings",
        Action::ApplyToSelected => "Apply to selected",
        Action::Preferences => "Preferences",
        Action::MatchReference => "Match reference",
        Action::EditIn => "Edit in…",
        Action::MergeHdr => "Merge HDR",
        Action::MergePanorama => "Panorama",
        Action::AddVersion => "Add version",
        Action::ResetAll => "Reset all",
        Action::Flag(Flag::Picked) => "Pick",
        Action::Flag(Flag::Rejected) => "Reject",
        Action::Rate(0) => "No stars",
        Action::Rate(1) => "1",
        Action::Rate(2) => "2",
        Action::Rate(3) => "3",
        Action::Rate(4) => "4",
        Action::Rate(_) => "5",
        Action::Label(Some(label)) => label.name(),
        Action::Label(None) => "None",
        Action::CropTool => "Crop",
        Action::SpotTool => "Spots",
        Action::WhiteBalanceTool => "White balance",
        Action::MaskTool => "Masks",
        Action::AutoWhiteBalance => "Auto WB",
        Action::CheckEntitlement => "Check now",
        Action::ShowLogFolder => "Log folder",
        Action::KeyboardShortcuts => "Shortcuts",
        Action::Metadata => "Metadata",
        Action::Clipping => "Clipping",
        Action::AiDenoise => "Denoise",
        Action::AiUpscale => "Upscale",
        Action::AiRetouch => "Retouch",
        Action::AiCutout => "Background",
        Action::NewAlbum => "New album",
        Action::NewSmartAlbum => "Smart album",
        Action::AddToAlbum => "Add to album",
        Action::RemoveFromAlbum => "Remove",
        other => other.label(),
    }
}

/// An icon's colour: a label's own colour, else the ink.
fn icon_colour(action: Action) -> Color32 {
    match action {
        Action::Label(Some(label)) => crate::library::cull_ui::label_colour(label),
        Action::Flag(Flag::Picked) => theme::SUCCESS,
        Action::Flag(Flag::Rejected) => theme::DANGER,
        Action::Rate(n) if n > 0 => theme::WARNING,
        _ => theme::INK,
    }
}

/// What the ribbon shows: the actions possible now, the studio, the Before/After state, and
/// the tab the view would like in front (Develop while developing, Library in the library).
#[derive(Debug, Clone, Default)]
pub struct MenuState {
    pub enabled: HashSet<Action>,
    pub studio: Option<String>,
    pub before: bool,
    pub clipping: bool,
    pub context: Option<&'static str>,
}

/// The Account tab's actions possible in `session`: all need a studio open; switching needs
/// a second studio (`can_switch`).
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

/// The actions possible on any screen: Quit, Preferences and the Help tab.
pub const ALWAYS: &[Action] = &[
    Action::Quit,
    Action::Preferences,
    Action::About,
    Action::ShowLogFolder,
    Action::KeyboardShortcuts,
];

/// The ribbon's own state between frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Memory {
    tab: &'static str,
    folded: bool,
    /// The view's wish last frame (a change brings its tab to the front).
    context: Option<&'static str>,
}

impl Default for Memory {
    fn default() -> Memory {
        Memory {
            tab: HOME,
            folded: false,
            context: None,
        }
    }
}

/// A button's size: wide enough for its name on up to two lines.
const BUTTON_W: f32 = 64.0;
const BUTTON_H: f32 = 58.0;
const ICON_SIZE: f32 = 22.0;

/// Draws the ribbon; returns the button clicked.
pub fn bar(ui: &mut egui::Ui, state: &MenuState) -> Option<Action> {
    let id = egui::Id::unique("ribbon");
    let mut mem: Memory = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    if state.context != mem.context {
        if let Some(tab) = state.context {
            mem.tab = tab;
        }
        mem.context = state.context;
    }
    let mut clicked = None;
    ui.horizontal(|ui| {
        for tab in RIBBON {
            let chosen = mem.tab == tab.title;
            let text = egui::RichText::new(tab.title).strong();
            let response = ui.add(egui::Button::selectable(chosen, text));
            if response.clicked() {
                if chosen && mem.folded {
                    mem.folded = false;
                }
                mem.tab = tab.title;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (glyph, name) = if mem.folded {
                ("⏷", UNFOLD)
            } else {
                ("⏶", FOLD)
            };
            let button = ui
                .add(egui::Button::new(glyph).frame(false))
                .on_hover_text(name);
            button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
            if button.clicked() {
                mem.folded = !mem.folded;
            }
        });
    });
    if !mem.folded
        && let Some(tab) = RIBBON.iter().find(|t| t.title == mem.tab)
    {
        ui.add_space(2.0);
        egui::ScrollArea::horizontal()
            .id_salt("ribbon-groups")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, g) in tab.groups.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        group_ui(ui, g, state, &mut clicked);
                    }
                });
            });
        ui.add_space(2.0);
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, mem));
    clicked
}

/// One group: its buttons in a row, its title under them.
fn group_ui(ui: &mut egui::Ui, g: &Group, state: &MenuState, clicked: &mut Option<Action>) {
    ui.vertical(|ui| {
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for item in g.items {
                match item {
                    Studio => {
                        let name = state.studio.as_deref().unwrap_or(NOT_SIGNED_IN);
                        ui.add_sized(
                            [140.0, BUTTON_H],
                            egui::Label::new(egui::RichText::new(name).strong()).wrap(),
                        );
                    }
                    Do(action) => {
                        let enabled = state.enabled.contains(action);
                        let on = (*action == Action::BeforeAfter && state.before)
                            || (*action == Action::Clipping && state.clipping);
                        if button(ui, *action, enabled, on).clicked() {
                            *clicked = Some(*action);
                        }
                    }
                }
            }
        });
        // Centred under the buttons, in their own width (`vertical_centered` would take all
        // the width the ribbon's scroll area offers, which is unbounded).
        let width = row.response.rect.width();
        ui.allocate_ui_with_layout(
            egui::vec2(width, 14.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.set_width(width);
                ui.label(egui::RichText::new(g.title).small().color(theme::MUTED));
            },
        );
    });
}

/// A large ribbon button: the icon over the name. Named for screen readers (and the tests)
/// by its menu label; disabled buttons are drawn faded and do nothing.
fn button(ui: &mut egui::Ui, action: Action, enabled: bool, on: bool) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(BUTTON_W, BUTTON_H), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, action.label()));
    let tip = match MAP.shortcut(action) {
        Some(key) => format!("{} ({})", action.label(), ui.ctx().format_shortcut(&key)),
        None => action.label().to_owned(),
    };
    let response = response.on_hover_text(tip);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if on {
            painter.rect_filled(rect, 4.0, theme::BRAND.gamma_multiply(0.35));
        } else if enabled && response.hovered() {
            painter.rect_filled(rect, 4.0, theme::SURFACE_2);
        }
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.35) };
        painter.text(
            egui::pos2(rect.center().x, rect.top() + 4.0),
            egui::Align2::CENTER_TOP,
            icon(action),
            egui::FontId::proportional(ICON_SIZE),
            fade(icon_colour(action)),
        );
        let name = painter.layout(
            short_label(action).to_owned(),
            egui::FontId::proportional(11.0),
            fade(theme::INK),
            BUTTON_W - 4.0,
        );
        let at = egui::pos2(
            rect.center().x - name.size().x / 2.0,
            rect.top() + ICON_SIZE + 10.0,
        );
        painter.galley(at, name, fade(theme::INK));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    fn every_action() -> HashSet<Action> {
        actions().into_iter().collect()
    }

    fn harness(state: MenuState) -> Harness<'static, (MenuState, Option<Action>)> {
        Harness::builder()
            .with_size([1280.0, 300.0])
            .build_ui_state(
                |ui, (state, clicked): &mut (MenuState, Option<Action>)| {
                    if let Some(action) = bar(ui, state) {
                        *clicked = Some(action);
                    }
                },
                (state, None),
            )
    }

    #[test]
    fn each_tab_shows_its_buttons() {
        let mut h = harness(MenuState {
            enabled: every_action(),
            studio: Some("Sharma Studio".into()),
            ..MenuState::default()
        });
        h.run();
        for tab in RIBBON {
            h.get_by_label(tab.title).click();
            h.run();
            for g in tab.groups {
                for item in g.items {
                    let label = match item {
                        Do(action) => action.label(),
                        Studio => "Sharma Studio",
                    };
                    assert!(
                        h.query_by_label(label).is_some(),
                        "{} has no {label:?}",
                        tab.title
                    );
                }
            }
        }
        // A button does what it says.
        h.get_by_label("Help").click();
        h.run();
        h.get_by_label("Keyboard shortcuts").click();
        h.run();
        assert_eq!(h.state().1, Some(Action::KeyboardShortcuts));
        h.get_by_label(LIBRARY).click();
        h.run();
        // The widest tab fits a 1280-point window: groups are as wide as their buttons.
        {
            let last = h.get_by_label("Compare").rect();
            assert!(last.max.x <= 1280.0, "the Library tab overflows: {last:?}");
        }
        h.get_by_label("3 stars").click();
        h.run();
        assert_eq!(h.state().1, Some(Action::Rate(3)));
    }

    #[test]
    fn a_disabled_button_does_nothing() {
        let mut h = harness(MenuState::default());
        h.run();
        h.get_by_label("Account").click();
        h.run();
        h.get_by_label(NOT_SIGNED_IN);
        h.get_by_label("Sign out").click();
        h.run();
        assert_eq!(h.state().1, None);
    }

    #[test]
    fn the_ribbon_follows_the_view_and_folds() {
        let mut h = harness(MenuState {
            enabled: every_action(),
            ..MenuState::default()
        });
        h.run();
        assert!(h.query_by_label("Undo").is_some(), "Home first");
        h.state_mut().0.context = Some(DEVELOP);
        h.run();
        assert!(
            h.query_by_label("Auto tone").is_some(),
            "Develop came forward"
        );
        // The user picks another tab: it stays while the view does not change.
        h.get_by_label("View").click();
        h.run();
        h.run();
        assert!(h.query_by_label("Fit").is_some());
        assert!(h.query_by_label("Auto tone").is_none());
        h.state_mut().0.context = Some(LIBRARY);
        h.run();
        assert!(h.query_by_label("Flag as pick").is_some());
        // Folded: only the tabs.
        h.get_by_label(FOLD).click();
        h.run();
        assert!(h.query_by_label("Flag as pick").is_none());
        h.get_by_label(UNFOLD).click();
        h.run();
        assert!(h.query_by_label("Flag as pick").is_some());
    }

    #[test]
    fn every_action_a_menu_offered_is_on_the_ribbon() {
        let all = every_action();
        for action in [
            Action::AddFolder,
            Action::ImportFromCard,
            Action::Metadata,
            Action::EditIn,
            Action::Print,
            Action::MergeHdr,
            Action::MergePanorama,
            Action::AiDenoise,
            Action::AiUpscale,
            Action::AiRetouch,
            Action::AiCutout,
            Action::Compare,
            Action::NewAlbum,
            Action::NewSmartAlbum,
            Action::AddToAlbum,
            Action::RemoveFromAlbum,
            Action::Export,
            Action::Quit,
            Action::Undo,
            Action::Redo,
            Action::CopySettings,
            Action::PasteSettings,
            Action::ApplyToSelected,
            Action::SelectAll,
            Action::Preferences,
            Action::Library,
            Action::Develop,
            Action::BeforeAfter,
            Action::Clipping,
            Action::Fit,
            Action::ActualSize,
            Action::MatchReference,
            Action::AddVersion,
            Action::ResetAll,
            Action::AutoTone,
            Action::AutoWhiteBalance,
            Action::SwitchStudio,
            Action::CheckEntitlement,
            Action::SignOut,
            Action::About,
            Action::ShowLogFolder,
            Action::KeyboardShortcuts,
            Action::Metadata,
            Action::CropTool,
            Action::SpotTool,
            Action::WhiteBalanceTool,
            Action::Flag(Flag::Picked),
            Action::Flag(Flag::Rejected),
            Action::Flag(Flag::Unflagged),
            Action::Label(None),
        ]
        .into_iter()
        .chain((0..=5).map(Action::Rate))
        .chain(Label::ALL.into_iter().map(|l| Action::Label(Some(l))))
        {
            assert!(all.contains(&action), "{action:?} is not on the ribbon");
        }
    }

    #[test]
    fn ribbon_icons_are_in_the_fonts() {
        let ctx = egui::Context::default();
        // Fonts are loaded on the first frame.
        let mut frame = ctx.run_ui(egui::RawInput::default(), |_| {});
        frame.textures_delta.clear();
        let font = egui::FontId::proportional(ICON_SIZE);
        let mut missing = Vec::new();
        for action in actions() {
            let glyphs = icon(action);
            let ok = ctx.fonts_mut(|f| glyphs.chars().all(|c| f.has_glyph(&font, c)));
            if !ok {
                missing.push((action, glyphs));
            }
        }
        assert!(missing.is_empty(), "no glyph for {missing:?}");
    }
}
