//! Everything the window can be asked to do from a menu or the keyboard ([`Action`]), and the
//! one keyboard map ([`MAP`]) that binds keys to actions. The app reads the map once per frame
//! in `logic()`; the ribbon's tooltips show the same map's keys; Help → Keyboard shortcuts lists it.
//!
//! The keys follow Lightroom where it has a default. `Ctrl` reads as `Cmd` on macOS
//! ([`Modifiers::COMMAND`]). Keys never fire while a text field (or any widget) has the
//! keyboard, nor while a dialog is open; tool keys act only in Develop.

use eframe::egui::{Event, InputState, Key, KeyboardShortcut, ModifierNames, Modifiers};
use lf_catalog::cull::{Flag, Label};

/// What a menu item or a key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // File
    AddFolder,
    /// Copy photos from a memory card (or any folder) into the studio's, then catalogue them.
    ImportFromCard,
    Export,
    /// Upload the selected photos to a LensFlow gallery (while the plan allows it).
    ExportToGallery,
    /// Lay the selected photos out on pages and print them (through a PDF).
    Print,
    Quit,
    // Edit
    Undo,
    Redo,
    CopySettings,
    PasteSettings,
    ApplyToSelected,
    SelectAll,
    Preferences,
    // View
    Library,
    Develop,
    BeforeAfter,
    /// Show clipped highlights red and blocked shadows blue in the preview.
    Clipping,
    /// Show the photo as a printer and paper profile would print it.
    SoftProof,
    /// Choose the printer and paper profile soft-proofing uses.
    ProofProfile,
    Fit,
    ActualSize,
    /// `Space`: fit ⇄ 100 %.
    ToggleZoom,
    /// Show or hide the Library's Metadata panel (title, caption, keywords).
    Metadata,
    /// Two to four selected photos side by side, zoomed together.
    Compare,
    // AI (on this computer)
    /// Remove noise; a new photo beside each.
    AiDenoise,
    /// Enlarge 2× or 4×; a new photo beside each.
    AiUpscale,
    /// Smooth the skin of the faces found; a new photo beside each.
    AiRetouch,
    /// The subject on a transparent background; a PNG beside each.
    AiCutout,
    /// Paint over things and have them filled from their surroundings; a new photo beside it.
    AiRemoveObjects,
    // Albums
    /// Make an album (with the selected photos).
    NewAlbum,
    /// Make an album that gathers photos by a rule.
    NewSmartAlbum,
    /// Put the selected photos in an album.
    AddToAlbum,
    /// Take the selected photos out of the album shown.
    RemoveFromAlbum,
    // Photo
    MatchReference,
    /// Render the photo to a TIFF beside it and open it in the studio's external editor.
    EditIn,
    /// Blend an exposure bracket (the selected photos) into one photo.
    MergeHdr,
    /// Stitch a row of photos (the selected ones, in the grid's order) into a panorama.
    MergePanorama,
    AddVersion,
    ResetAll,
    /// Exposure and the tone sliders from the photo.
    AutoTone,
    /// Temperature and tint from the photo.
    AutoWhiteBalance,
    /// Pick, reject or unflag the selected photos (the open one in Develop).
    Flag(Flag),
    /// 0 (no stars) to 5 stars.
    Rate(u8),
    /// A colour label, or none; a label already on every photo comes off.
    Label(Option<Label>),
    // Account
    SwitchStudio,
    CheckEntitlement,
    SignOut,
    // Help
    About,
    ShowLogFolder,
    KeyboardShortcuts,
    // Keys only
    CropTool,
    SpotTool,
    WhiteBalanceTool,
    MaskTool,
    Previous,
    Next,
    Up,
    Down,
    LeaveTool,
    ApplyCrop,
    DeleteSpot,
}

impl Action {
    /// The action's full name: the ribbon button's tooltip and accessible name, and what
    /// Help → Keyboard shortcuts calls it.
    pub fn label(self) -> &'static str {
        match self {
            Action::AddFolder => "Add folder…",
            Action::ImportFromCard => "Import from card…",
            Action::Export => "Export…",
            Action::ExportToGallery => "Export to gallery…",
            Action::Print => "Print…",
            Action::Quit => "Quit",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::CopySettings => "Copy settings…",
            Action::PasteSettings => "Paste settings",
            Action::ApplyToSelected => "Apply to selected…",
            Action::SelectAll => "Select all",
            Action::Preferences => "Preferences…",
            Action::Library => "Go to Library",
            Action::Develop => "Go to Develop",
            Action::BeforeAfter => "Before/After",
            Action::Clipping => "Show clipping",
            Action::SoftProof => "Soft proof",
            Action::ProofProfile => "Proof profile…",
            Action::Fit => "Fit",
            Action::ActualSize => "100 %",
            Action::ToggleZoom => "Fit / 100 %",
            Action::Metadata => "Metadata panel",
            Action::Compare => "Compare",
            Action::AiDenoise => "AI Denoise…",
            Action::AiUpscale => "AI Upscale…",
            Action::AiRetouch => "Retouch portrait…",
            Action::AiCutout => "Remove background…",
            Action::AiRemoveObjects => "Remove objects…",
            Action::NewAlbum => "New album…",
            Action::NewSmartAlbum => "New smart album…",
            Action::AddToAlbum => "Add to album…",
            Action::RemoveFromAlbum => "Remove from album",
            Action::MatchReference => "Match a reference…",
            Action::EditIn => "Edit in external editor…",
            Action::MergeHdr => "Merge to HDR…",
            Action::MergePanorama => "Merge to panorama…",
            Action::AddVersion => "Add version…",
            Action::ResetAll => "Reset all edits",
            Action::AutoTone => "Auto tone",
            Action::AutoWhiteBalance => "Auto white balance",
            Action::Flag(Flag::Picked) => "Flag as pick",
            Action::Flag(Flag::Rejected) => "Flag as rejected",
            Action::Flag(Flag::Unflagged) => "Unflag",
            Action::Rate(0) => "No stars",
            Action::Rate(1) => "1 star",
            Action::Rate(2) => "2 stars",
            Action::Rate(3) => "3 stars",
            Action::Rate(4) => "4 stars",
            Action::Rate(_) => "5 stars",
            Action::Label(None) => "No colour label",
            Action::Label(Some(Label::Red)) => "Red label",
            Action::Label(Some(Label::Yellow)) => "Yellow label",
            Action::Label(Some(Label::Green)) => "Green label",
            Action::Label(Some(Label::Blue)) => "Blue label",
            Action::Label(Some(Label::Purple)) => "Purple label",
            Action::SwitchStudio => "Switch studio",
            Action::CheckEntitlement => "Check entitlement now",
            Action::SignOut => "Sign out",
            Action::About => "About",
            Action::ShowLogFolder => "Show log folder",
            Action::KeyboardShortcuts => "Keyboard shortcuts",
            Action::CropTool => "Crop and straighten",
            Action::SpotTool => "Heal and clone spots",
            Action::WhiteBalanceTool => "White balance eyedropper",
            Action::MaskTool => "Masks (local adjustments)",
            Action::Previous => "Previous photo",
            Action::Next => "Next photo",
            Action::Up => "Up a row in the grid",
            Action::Down => "Down a row in the grid",
            Action::LeaveTool => "Leave the tool",
            Action::ApplyCrop => "Apply the crop",
            Action::DeleteSpot => "Remove the selected spot",
        }
    }

    /// A held key fires again (key repeat): stepping through photos and undo or redo do;
    /// a held `Space` or tool key does not flicker.
    pub fn repeats(self) -> bool {
        matches!(
            self,
            Action::Previous
                | Action::Next
                | Action::Up
                | Action::Down
                | Action::Undo
                | Action::Redo
        )
    }

    /// Shown while its key is held (`\`), not fired on a press.
    pub fn held(self) -> bool {
        self == Action::BeforeAfter
    }
}

const fn key(k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::NONE, k)
}

const fn cmd(k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND, k)
}

const fn cmd_shift(k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), k)
}

/// A keyboard map: each entry binds one key to one action (an action may have two keys).
pub struct Map(pub &'static [(Action, KeyboardShortcut)]);

/// The window's keyboard map.
pub const MAP: Map = Map(&[
    (Action::Library, key(Key::G)),
    (Action::Develop, key(Key::D)),
    (Action::Export, cmd_shift(Key::E)),
    (Action::EditIn, cmd(Key::E)),
    (Action::Print, cmd(Key::P)),
    (Action::MergeHdr, cmd(Key::H)),
    (Action::MergePanorama, cmd(Key::M)),
    (Action::NewAlbum, cmd(Key::N)),
    (Action::Compare, key(Key::C)),
    (Action::AddToAlbum, cmd(Key::B)),
    (Action::CropTool, key(Key::R)),
    (Action::SpotTool, key(Key::Q)),
    (Action::WhiteBalanceTool, key(Key::W)),
    (Action::MaskTool, key(Key::M)),
    (Action::AutoTone, cmd(Key::U)),
    (Action::AutoWhiteBalance, cmd_shift(Key::U)),
    (Action::BeforeAfter, key(Key::Backslash)),
    (Action::Clipping, key(Key::J)),
    (Action::SoftProof, key(Key::S)),
    (Action::Undo, cmd(Key::Z)),
    (Action::Redo, cmd_shift(Key::Z)),
    (Action::Redo, cmd(Key::Y)),
    (Action::CopySettings, cmd_shift(Key::C)),
    (Action::PasteSettings, cmd_shift(Key::V)),
    (Action::SelectAll, cmd(Key::A)),
    (Action::Previous, key(Key::ArrowLeft)),
    (Action::Next, key(Key::ArrowRight)),
    (Action::Up, key(Key::ArrowUp)),
    (Action::Down, key(Key::ArrowDown)),
    (Action::ToggleZoom, key(Key::Space)),
    (Action::LeaveTool, key(Key::Escape)),
    (Action::ApplyCrop, key(Key::Enter)),
    (Action::DeleteSpot, key(Key::Delete)),
    (Action::Flag(Flag::Picked), key(Key::P)),
    (Action::Flag(Flag::Rejected), key(Key::X)),
    (Action::Flag(Flag::Unflagged), key(Key::U)),
    (Action::Rate(0), key(Key::Num0)),
    (Action::Rate(1), key(Key::Num1)),
    (Action::Rate(2), key(Key::Num2)),
    (Action::Rate(3), key(Key::Num3)),
    (Action::Rate(4), key(Key::Num4)),
    (Action::Rate(5), key(Key::Num5)),
    (Action::Label(Some(Label::Red)), key(Key::Num6)),
    (Action::Label(Some(Label::Yellow)), key(Key::Num7)),
    (Action::Label(Some(Label::Green)), key(Key::Num8)),
    (Action::Label(Some(Label::Blue)), key(Key::Num9)),
    (Action::About, key(Key::F1)),
]);

impl Map {
    /// Every entry, in the order Help lists them.
    pub fn entries(&self) -> &'static [(Action, KeyboardShortcut)] {
        self.0
    }

    /// The first key bound to `action` (the one a menu shows).
    pub fn shortcut(&self, action: Action) -> Option<KeyboardShortcut> {
        self.0.iter().find(|(a, _)| *a == action).map(|(_, s)| *s)
    }

    /// The action a press of `key` with `modifiers` means: the most specific entry whose
    /// modifiers match (so `Ctrl+Shift+Z` is redo, never undo).
    pub fn best(&self, key: Key, modifiers: Modifiers) -> Option<Action> {
        self.0
            .iter()
            .filter(|(_, s)| s.logical_key == key && modifiers.matches_logically(s.modifiers))
            .max_by_key(|(_, s)| modifier_count(s.modifiers))
            .map(|(a, _)| *a)
    }

    /// Takes this frame's presses that mean an enabled action and returns those actions in
    /// order (each once). A press whose action is disabled is left for the widgets (the
    /// curve editor's `Delete`, say).
    pub fn fired(&self, input: &mut InputState, enabled: impl Fn(Action) -> bool) -> Vec<Action> {
        let mut fired = Vec::new();
        input.events.retain(|event| {
            let Event::Key {
                key,
                modifiers,
                pressed: true,
                repeat,
                ..
            } = event
            else {
                return true;
            };
            match self.best(*key, *modifiers) {
                Some(action) if !action.held() && enabled(action) => {
                    // A repeat of a key that does not repeat is swallowed.
                    if !(*repeat && !action.repeats()) && !fired.contains(&action) {
                        fired.push(action);
                    }
                    false
                }
                _ => true,
            }
        });
        fired
    }

    /// The key of a held action (`\` for Before/After) is down now.
    pub fn is_held(&self, input: &InputState, action: Action) -> bool {
        self.0.iter().any(|(a, s)| {
            *a == action
                && input.key_down(s.logical_key)
                && input.modifiers.matches_logically(s.modifiers)
        })
    }

    /// How Help shows an entry's key on this computer.
    pub fn describe(shortcut: &KeyboardShortcut, mac: bool) -> String {
        let names = if mac {
            ModifierNames::SYMBOLS
        } else {
            ModifierNames::NAMES
        };
        shortcut.format(&names, mac)
    }
}

/// The window's keys may fire: no widget has the keyboard (a text field being typed in, a
/// slider stepped with the arrows) and no dialog is open.
pub fn keys_free(ctx: &eframe::egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input() && !crate::dialogs::modal_open(ctx)
}

/// Reads [`MAP`] once per frame. `Esc` that just took the keyboard back from a field (egui
/// lets go of the focus on `Esc` before anything reads the press) does not also leave the
/// tool: it only cancelled the typing.
#[derive(Debug, Default, Clone, Copy)]
pub struct Keys {
    was_free: bool,
}

impl Keys {
    /// The actions this frame's presses fire; nothing while a widget has the keyboard, a
    /// dialog is open or `blocked` (one of the caller's own dialogs).
    pub fn read(
        &mut self,
        ctx: &eframe::egui::Context,
        blocked: bool,
        enabled: impl Fn(Action) -> bool,
    ) -> Vec<Action> {
        let free = keys_free(ctx) && !blocked;
        let just_freed = free && !self.was_free;
        self.was_free = free;
        if !free {
            return Vec::new();
        }
        ctx.input_mut(|i| MAP.fired(i, |a| !(just_freed && a == Action::LeaveTool) && enabled(a)))
    }
}

fn modifier_count(m: Modifiers) -> u8 {
    u8::from(m.alt) + u8::from(m.shift) + u8::from(m.ctrl || m.command || m.mac_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The plan's keyboard map (Global Constraints), entry by entry.
    fn plan() -> Vec<(Action, KeyboardShortcut)> {
        vec![
            (Action::Library, key(Key::G)),
            (Action::Develop, key(Key::D)),
            (Action::Export, cmd_shift(Key::E)),
            (Action::EditIn, cmd(Key::E)),
            (Action::Print, cmd(Key::P)),
            (Action::MergeHdr, cmd(Key::H)),
            (Action::MergePanorama, cmd(Key::M)),
            (Action::NewAlbum, cmd(Key::N)),
            (Action::Compare, key(Key::C)),
            (Action::AddToAlbum, cmd(Key::B)),
            (Action::CropTool, key(Key::R)),
            (Action::SpotTool, key(Key::Q)),
            (Action::WhiteBalanceTool, key(Key::W)),
            (Action::MaskTool, key(Key::M)),
            (Action::AutoTone, cmd(Key::U)),
            (Action::AutoWhiteBalance, cmd_shift(Key::U)),
            (Action::BeforeAfter, key(Key::Backslash)),
            (Action::Clipping, key(Key::J)),
            (Action::SoftProof, key(Key::S)),
            (Action::Undo, cmd(Key::Z)),
            (Action::Redo, cmd_shift(Key::Z)),
            (Action::Redo, cmd(Key::Y)),
            (Action::CopySettings, cmd_shift(Key::C)),
            (Action::PasteSettings, cmd_shift(Key::V)),
            (Action::SelectAll, cmd(Key::A)),
            (Action::Previous, key(Key::ArrowLeft)),
            (Action::Next, key(Key::ArrowRight)),
            (Action::ToggleZoom, key(Key::Space)),
            (Action::LeaveTool, key(Key::Escape)),
            (Action::DeleteSpot, key(Key::Delete)),
            (Action::Flag(Flag::Picked), key(Key::P)),
            (Action::Flag(Flag::Rejected), key(Key::X)),
            (Action::Flag(Flag::Unflagged), key(Key::U)),
            (Action::Rate(0), key(Key::Num0)),
            (Action::Rate(5), key(Key::Num5)),
            (Action::Label(Some(Label::Red)), key(Key::Num6)),
            (Action::Label(Some(Label::Blue)), key(Key::Num9)),
            (Action::About, key(Key::F1)),
        ]
    }

    /// What the keyboard sends for `Ctrl`/`Cmd` on each platform.
    fn pressed(m: Modifiers, mac: bool) -> Modifiers {
        let mut m = m;
        if m.command {
            if mac {
                m.mac_cmd = true;
            } else {
                m.ctrl = true;
            }
        }
        m
    }

    #[test]
    fn every_planned_key_is_bound_exactly_once_on_every_platform() {
        for (action, shortcut) in plan() {
            let n = MAP
                .entries()
                .iter()
                .filter(|(a, s)| *a == action && *s == shortcut)
                .count();
            assert_eq!(n, 1, "{action:?} on {shortcut:?} is bound {n} times");
        }
        for mac in [false, true] {
            let mut seen = HashSet::new();
            for (action, shortcut) in MAP.entries() {
                let text = Map::describe(shortcut, mac);
                assert!(
                    seen.insert(text.clone()),
                    "{text} is bound twice (mac: {mac})"
                );
                // A press of exactly this key means exactly this action, never a less
                // specific one (Ctrl+Shift+Z is redo, not undo).
                let modifiers = pressed(shortcut.modifiers, mac);
                assert_eq!(
                    MAP.best(shortcut.logical_key, modifiers),
                    Some(*action),
                    "{text} (mac: {mac})"
                );
            }
        }
    }

    #[test]
    fn keys_never_fire_while_a_text_field_has_focus() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;
        let mut h = Harness::new_ui_state(
            |ui, (keys, text, fired): &mut (Keys, String, Vec<Action>)| {
                fired.extend(keys.read(ui.ctx(), false, |_| true));
                let label = ui.label("Name");
                ui.text_edit_singleline(text).labelled_by(label.id);
            },
            (Keys::default(), String::new(), Vec::new()),
        );
        h.run();
        h.get_by_label("Name").click();
        h.run();
        h.key_press(Key::G);
        h.run();
        assert!(
            h.state().2.is_empty(),
            "G typed into the field fired nothing"
        );
        // Esc leaves the field and nothing else (not the crop tool, say).
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().2.is_empty(), "Esc only cancelled the typing");
        h.key_press(Key::G);
        h.run();
        h.key_press(Key::Escape);
        h.run();
        assert_eq!(h.state().2, [Action::Library, Action::LeaveTool]);
    }

    #[test]
    fn a_held_key_repeats_only_steps_and_undo() {
        let mut input = InputState::default();
        let repeat = |key, modifiers| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers,
        };
        input.events = vec![
            repeat(Key::Space, Modifiers::NONE),
            repeat(Key::R, Modifiers::NONE),
            repeat(Key::Escape, Modifiers::NONE),
            repeat(Key::ArrowRight, Modifiers::NONE),
            repeat(Key::Z, pressed(Modifiers::COMMAND, false)),
        ];
        let fired = MAP.fired(&mut input, |_| true);
        assert_eq!(fired, vec![Action::Next, Action::Undo]);
        assert!(
            input.events.is_empty(),
            "the repeats are swallowed, not passed on"
        );
    }

    #[test]
    fn a_disabled_action_leaves_its_press_for_the_widgets() {
        let mut input = InputState::default();
        let press = |key, modifiers| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        input.events = vec![
            press(
                Key::Z,
                pressed(Modifiers::COMMAND | Modifiers::SHIFT, false),
            ),
            press(Key::Delete, Modifiers::NONE),
            press(Key::G, Modifiers::NONE),
        ];
        // Redo is not possible: Ctrl+Shift+Z must not fall through to undo.
        let fired = MAP.fired(&mut input, |a| a != Action::Redo && a != Action::DeleteSpot);
        assert_eq!(fired, vec![Action::Library]);
        assert_eq!(input.events.len(), 2, "the redo and delete presses stay");
    }
}
