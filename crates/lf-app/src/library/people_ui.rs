//! The library's people ([`lf_catalog::people`]): AI → Find people (the scan is an AI tool,
//! [`crate::ai_job`]), the sidebar's People list under the albums (named people by name, then
//! the groups of faces waiting for a name, each with a picture of a face), naming, renaming
//! and merging (naming a person after someone else), "Not this person" for the selected
//! photos, and showing a person's photos in the grid in place of a folder. Lists come from the
//! `Query` lane (the face pictures decoded there too) and changes go through the `Io` lane;
//! after each change the list is read again and the person shown is reloaded.

use super::{Env, Library, Mode, Selection};
use crate::ai_job::AiTool;
use crate::catalog_ext::count;
use crate::dialogs::ai::AiDialog;
use crate::jobs::{Lane, Msg};
use crate::theme;
use crate::toast::Level;
use eframe::egui::{self, Key, RichText};
use lf_catalog::Catalog;
use lf_catalog::people::{Group, PersonEntry};
use std::collections::HashMap;
use std::path::PathBuf;

pub const PEOPLE: &str = "People";
pub const NO_PEOPLE: &str = "No people yet. Choose Find people on the AI tab to look for faces.";
pub const UNNAMED: &str = "Who is this?";
pub const NAME_TITLE: &str = "Name this person";
pub const NAME_HINT: &str = "Typing someone's name again puts these faces with theirs.";
pub const NO_PHOTOS_OF_PERSON: &str = "No photos of this person.";
const FACE: f32 = 28.0;

/// One entry of the People list with its face picture, decoded off the UI thread.
#[derive(Debug, Clone)]
pub struct Listed {
    pub entry: PersonEntry,
    pub picture: Option<egui::ColorImage>,
}

/// A change to the people, done on the `Io` lane.
#[derive(Debug, Clone)]
pub(super) enum Change {
    Name {
        group: Group,
        name: String,
    },
    Forget {
        id: i64,
        name: String,
    },
    NotThem {
        id: i64,
        name: String,
        ids: Vec<i64>,
    },
}

fn photos(n: usize) -> String {
    if n == 1 {
        "1 photo".to_owned()
    } else {
        format!("{} photos", count(n))
    }
}

impl Change {
    /// Does the change; returns what to say and the person to show afterwards.
    fn run(self, catalog: &Catalog) -> Result<(String, Option<Group>), String> {
        let text = |e: lf_catalog::CatalogError| e.to_string();
        match self {
            Change::Name { group, name } => {
                let id = catalog.name_group(group, &name).map_err(text)?;
                let name = name.trim();
                Ok((format!("Named {name}."), Some(Group::Person(id))))
            }
            Change::Forget { id, name } => {
                catalog.delete_person(id).map_err(text)?;
                Ok((
                    format!("Forgot {name}. Their faces are waiting for a name again."),
                    None,
                ))
            }
            Change::NotThem { id, name, ids } => {
                let n = catalog.not_this_person(id, &ids).map_err(text)?;
                Ok((
                    format!("Took {} out of {name}.", photos(n)),
                    Some(Group::Person(id)),
                ))
            }
        }
    }
}

/// The name dialog, while open.
#[derive(Debug, Clone)]
pub(super) struct Naming {
    group: Group,
    /// The name it has now (a rename).
    was: Option<String>,
    name: String,
    focused: bool,
}

/// What the library keeps about people.
#[derive(Default)]
pub struct PeopleState {
    pub(super) list: Vec<Listed>,
    textures: HashMap<Group, egui::TextureHandle>,
    /// The person shown in place of a folder; never with a folder or an album.
    pub(super) current: Option<Group>,
    naming: Option<Naming>,
    confirm_forget: Option<(i64, String)>,
}

/// The list as the catalogue has it, each face picture decoded.
fn read_list(root: &std::path::Path) -> Result<Vec<Listed>, String> {
    let list = Catalog::open(root)
        .and_then(|c| c.people())
        .map_err(|e| e.to_string())?;
    Ok(list
        .into_iter()
        .map(|entry| {
            let picture = image::load_from_memory(&entry.thumb).ok().map(|img| {
                let rgba = img.to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                )
            });
            Listed { entry, picture }
        })
        .collect())
}

impl Library {
    pub fn people(&self) -> &[Listed] {
        &self.people.list
    }

    pub fn current_person(&self) -> Option<Group> {
        self.people.current
    }

    pub(super) fn people_dialog_open(&self) -> bool {
        self.people.naming.is_some() || self.people.confirm_forget.is_some()
    }

    /// The person shown, when they are named: (id, name).
    fn named_shown(&self) -> Option<(i64, String)> {
        let Some(Group::Person(id)) = self.people.current else {
            return None;
        };
        self.people
            .list
            .iter()
            .find(|l| l.entry.group == Group::Person(id))
            .and_then(|l| l.entry.name.clone())
            .map(|name| (id, name))
    }

    /// AI → Find people: the selection when several photos are selected, else every photo
    /// the grid shows.
    pub(super) fn open_find_people(&mut self) {
        let ids = if self.selection.current.len() > 1 {
            self.selected_in_order()
        } else {
            self.order.clone()
        };
        if ids.is_empty() || self.ai.is_some() {
            return;
        }
        let tool = AiTool::FindPeople;
        let download = self.ai_engine().to_download(&tool.models());
        self.ai = Some(AiDialog::new(ids, tool, download));
    }

    /// The finished scan's "Show people": the first group waiting for a name (else the first
    /// person).
    pub(super) fn show_people(&mut self, env: &mut Env) {
        let first = self
            .people
            .list
            .iter()
            .find(|l| l.entry.name.is_none())
            .or_else(|| self.people.list.first())
            .map(|l| l.entry.group);
        if let Some(group) = first {
            self.mode = Mode::Library;
            self.show_person(group, env);
        }
    }

    pub(super) fn refresh_people(&self, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Query, "people", move || {
            let result = read_list(&root);
            Msg::People { root, result }
        });
    }

    /// Shows `group`'s photos in the grid (in place of a folder) and loads them.
    pub(super) fn show_person(&mut self, group: Group, env: &mut Env) {
        self.compare = None;
        self.map = None;
        if self.people.current != Some(group) {
            self.selection = Selection::default();
            self.photos.clear();
            self.rows.clear();
            self.order.clear();
            self.index.clear();
            self.thumbs.reset();
            self.reveal_active = true;
        }
        self.current_folder = None;
        self.current_album = None;
        self.people.current = Some(group);
        self.load_person(env);
    }

    pub(super) fn load_person(&self, env: &mut Env) {
        let (Some(group), Some(root)) = (self.people.current, self.root.clone()) else {
            return;
        };
        env.jobs.spawn(Lane::Query, "person photos", move || {
            let result = Catalog::open(&root)
                .and_then(|c| c.group_photos(group))
                .map_err(|e| e.to_string());
            Msg::PersonPhotos {
                root,
                group,
                result,
            }
        });
    }

    pub(super) fn people_loaded(&mut self, root: PathBuf, result: Result<Vec<Listed>, String>) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok(list) => {
                self.people.list = list;
                // Pictures are made again from the new list (a group's face may have changed).
                self.people.textures.clear();
                // A person forgotten or a group gone: nothing to show.
                if let Some(group) = self.people.current
                    && !self.people.list.iter().any(|l| l.entry.group == group)
                {
                    self.people.current = None;
                    self.photos.clear();
                    self.rows.clear();
                    self.arrange();
                }
            }
            Err(e) => log::error!("could not list the people: {e}"),
        }
    }

    pub(super) fn person_photos_loaded(
        &mut self,
        root: PathBuf,
        group: Group,
        result: Result<Vec<lf_catalog::PhotoRow>, String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) || self.people.current != Some(group) {
            return;
        }
        match result {
            Ok(rows) => self.rows_loaded(rows),
            Err(e) => env.toasts.push(
                Level::Error,
                format!("Could not list this person's photos. {e}"),
            ),
        }
    }

    pub(super) fn change_people(&mut self, change: Change, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Io, "people", move || {
            let result = Catalog::open(&root)
                .map_err(|e| e.to_string())
                .and_then(|c| change.run(&c));
            Msg::PeopleChanged { root, result }
        });
    }

    pub(super) fn people_changed(
        &mut self,
        root: PathBuf,
        result: Result<(String, Option<Group>), String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok((said, show)) => {
                env.toasts.push(Level::Info, said);
                match show {
                    Some(group) => {
                        self.mode = Mode::Library;
                        self.show_person(group, env);
                    }
                    None if self.people.current.is_some() => self.load_person(env),
                    None => {}
                }
            }
            Err(e) => env.toasts.push(Level::Error, e),
        }
        self.refresh_people(env);
        // Names are searched: the rows carry them.
        if self.people.current.is_none() {
            self.reload_photos(env);
        }
    }

    /// The People list, under the albums.
    pub(super) fn people_sidebar(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(PEOPLE).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = self.enabled(crate::shortcuts::Action::FindPeople);
                if ui
                    .add_enabled(can, egui::Button::new("Find…"))
                    .on_hover_text(crate::shortcuts::Action::FindPeople.label())
                    .clicked()
                {
                    self.open_find_people();
                }
            });
        });
        ui.separator();
        if self.people.list.is_empty() {
            ui.label(RichText::new(NO_PEOPLE).color(theme::MUTED).small());
            return;
        }
        let selected_ids = self.selected_in_order();
        let shown = self.named_shown();
        let mut chosen = None;
        let mut change = None;
        let mut naming = None;
        let mut forget = None;
        for listed in &self.people.list {
            let entry = &listed.entry;
            let texture = match (
                self.people.textures.get(&entry.group),
                listed.picture.as_ref(),
            ) {
                (Some(t), _) => Some(t.clone()),
                (None, Some(picture)) => {
                    let t = ui.ctx().load_texture(
                        format!("face-{:?}", entry.group),
                        picture.clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    self.people.textures.insert(entry.group, t.clone());
                    Some(t)
                }
                (None, None) => None,
            };
            ui.horizontal(|ui| {
                match &texture {
                    Some(t) => {
                        ui.add(
                            egui::Image::from_texture(egui::load::SizedTexture::from_handle(t))
                                .fit_to_exact_size(egui::vec2(FACE, FACE))
                                .corner_radius(FACE / 2.0),
                        );
                    }
                    None => {
                        ui.add_space(FACE + ui.spacing().item_spacing.x);
                    }
                }
                let label = match &entry.name {
                    Some(name) => format!("{name}  ({})", count(entry.photos as usize)),
                    None => format!("{UNNAMED}  ({})", count(entry.photos as usize)),
                };
                let is_shown = self.people.current == Some(entry.group);
                let text = if entry.name.is_some() {
                    RichText::new(label)
                } else {
                    RichText::new(label).italics()
                };
                let response = ui.selectable_label(is_shown, text);
                if response.clicked() {
                    chosen = Some(entry.group);
                }
                response.context_menu(|ui| {
                    let ask = if entry.name.is_some() {
                        "Rename…"
                    } else {
                        "Name…"
                    };
                    if ui.button(ask).clicked() {
                        naming = Some((entry.group, entry.name.clone()));
                        ui.close();
                    }
                    if let (Group::Person(id), Some(name)) = (entry.group, &entry.name) {
                        if shown.as_ref().is_some_and(|(s, _)| *s == id) && !selected_ids.is_empty()
                        {
                            let n = selected_ids.len();
                            if ui
                                .button(format!("Not {name}: the selected {}", photos(n)))
                                .clicked()
                            {
                                change = Some(Change::NotThem {
                                    id,
                                    name: name.clone(),
                                    ids: selected_ids.clone(),
                                });
                                ui.close();
                            }
                        }
                        if ui.button(format!("Forget {name}…")).clicked() {
                            forget = Some((id, name.clone()));
                            ui.close();
                        }
                    }
                });
                if entry.name.is_none() && ui.small_button("Name…").clicked() {
                    naming = Some((entry.group, None));
                }
            });
        }
        if let Some(group) = chosen {
            self.mode = Mode::Library;
            self.show_person(group, env);
        }
        if let Some((group, was)) = naming {
            self.people.naming = Some(Naming {
                group,
                name: was.clone().unwrap_or_default(),
                was,
                focused: false,
            });
        }
        if forget.is_some() {
            self.people.confirm_forget = forget;
        }
        if let Some(change) = change {
            self.change_people(change, env);
        }
    }

    /// The name dialog and the forget question.
    pub(super) fn people_dialogs(&mut self, ctx: &egui::Context, env: &mut Env) {
        if let Some(naming) = &mut self.people.naming {
            let mut close = false;
            let mut save = None;
            egui::Modal::new(egui::Id::unique("name-person")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading(match &naming.was {
                    Some(name) => format!("Rename {name}"),
                    None => NAME_TITLE.to_owned(),
                });
                ui.add_space(8.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut naming.name)
                        .hint_text("Name")
                        .desired_width(f32::INFINITY),
                );
                if !naming.focused {
                    edit.request_focus();
                    naming.focused = true;
                }
                ui.label(RichText::new(NAME_HINT).color(theme::MUTED).small());
                ui.add_space(12.0);
                let ready = !naming.name.trim().is_empty();
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.horizontal(|ui| {
                    if ui.add_enabled(ready, egui::Button::new("Save")).clicked()
                        || (ready && enter)
                    {
                        save = Some(Change::Name {
                            group: naming.group,
                            name: naming.name.clone(),
                        });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                });
            });
            if let Some(change) = save {
                self.change_people(change, env);
            }
            if close {
                self.people.naming = None;
            }
        }
        if let Some((id, name)) = self.people.confirm_forget.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("forget-person")).show(ctx, |ui| {
                ui.set_max_width(400.0);
                ui.heading(format!("Forget {name}?"));
                ui.add_space(8.0);
                ui.label(
                    "Their name goes; their faces stay and wait for a name again. No photo is changed.",
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Forget").clicked() {
                        self.change_people(Change::Forget { id, name: name.clone() }, env);
                        close = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                });
            });
            if close {
                self.people.confirm_forget = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::people::NewFace;

    fn unit(axis: usize) -> Vec<f32> {
        let mut v = vec![0.0; 4];
        v[axis] = 1.0;
        v
    }

    #[test]
    fn changes_say_what_they_did_and_the_list_decodes_faces() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/a").unwrap();
        // A real JPEG face picture, so the list decodes it.
        let mut jpeg = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 150, 120]))
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let ids: Vec<i64> = (0..2)
            .map(|i| {
                let p = c
                    .add_photo(
                        f,
                        &lf_catalog::NewPhoto {
                            path: format!("D:/a/{i}.jpg"),
                            kind: "jpeg".into(),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                c.set_faces(
                    p,
                    &[NewFace {
                        rect: [0.0, 0.0, 0.5, 0.5],
                        description: unit(0),
                        thumb: jpeg.clone(),
                    }],
                )
                .unwrap();
                p
            })
            .collect();
        c.regroup_faces().unwrap();
        let list = read_list(dir.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].picture.as_ref().map(|p| p.size),
            Some([8, 8]),
            "the face picture decodes"
        );
        let (said, show) = Change::Name {
            group: list[0].entry.group,
            name: " Anna ".into(),
        }
        .run(&c)
        .unwrap();
        assert_eq!(said, "Named Anna.");
        let Some(Group::Person(anna)) = show else {
            panic!("shows the person: {show:?}");
        };
        let (said, _) = Change::NotThem {
            id: anna,
            name: "Anna".into(),
            ids: vec![ids[1]],
        }
        .run(&c)
        .unwrap();
        assert_eq!(said, "Took 1 photo out of Anna.");
        let (said, show) = Change::Forget {
            id: anna,
            name: "Anna".into(),
        }
        .run(&c)
        .unwrap();
        assert_eq!(
            said,
            "Forgot Anna. Their faces are waiting for a name again."
        );
        assert_eq!(show, None);
    }
}
