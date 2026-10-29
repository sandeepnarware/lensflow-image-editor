//! The library's albums ([`lf_catalog::albums`]): the sidebar's Albums list under the folders,
//! New album / New smart album / Add to album / Remove from album, and showing an album in
//! the grid in place of a folder. Lists come from the `Query` lane and changes go through the
//! `Io` lane; after each change the list is read again, and the album shown is reloaded.

use super::{Env, Library, Mode, Selection};
use crate::catalog_ext::count;
use crate::dialogs::album::{AlbumAction, AlbumDialog, Purpose};
use crate::jobs::{Lane, Msg};
use crate::theme;
use crate::toast::Level;
use eframe::egui::{self, Key, RichText};
use lf_catalog::Catalog;
use lf_catalog::albums::{Album, Rule};
use std::path::PathBuf;

pub const ALBUMS: &str = "Albums";
pub const NO_ALBUMS: &str = "No albums yet. Select photos and choose New album.";
pub const ADD_TITLE: &str = "Add to album";
pub const NO_PLAIN_ALBUMS: &str = "There is no album to add to yet: make one with New album.";

/// A change to the albums, done on the `Io` lane.
#[derive(Debug, Clone)]
pub(super) enum Change {
    Create {
        name: String,
        rule: Option<Rule>,
        with: Vec<i64>,
    },
    Update {
        id: i64,
        name: String,
        rule: Option<Rule>,
    },
    Delete(i64),
    Add {
        album: i64,
        name: String,
        ids: Vec<i64>,
    },
    Remove {
        album: i64,
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
    /// Does the change; returns what to say and the album to show afterwards.
    fn run(self, catalog: &Catalog) -> Result<(String, Option<i64>), String> {
        let text = |e: lf_catalog::CatalogError| e.to_string();
        match self {
            Change::Create { name, rule, with } => {
                let id = catalog.create_album(&name, rule.as_ref()).map_err(text)?;
                let added = catalog.add_to_album(id, &with).map_err(text)?;
                let said = if added > 0 {
                    format!("Made the album {name} with {}.", photos(added))
                } else {
                    format!("Made the album {name}.")
                };
                Ok((said, Some(id)))
            }
            Change::Update { id, name, rule } => {
                catalog
                    .update_album(id, &name, rule.as_ref())
                    .map_err(text)?;
                Ok((format!("Saved the album {name}."), None))
            }
            Change::Delete(id) => {
                catalog.delete_album(id).map_err(text)?;
                Ok((
                    "Deleted the album. Its photos are untouched.".to_owned(),
                    None,
                ))
            }
            Change::Add { album, name, ids } => {
                let n = catalog.add_to_album(album, &ids).map_err(text)?;
                let already = ids.len() - n;
                let mut said = format!("Added {} to {name}.", photos(n));
                if already > 0 {
                    said.push_str(&format!(" {} already in it.", photos(already)));
                }
                Ok((said, None))
            }
            Change::Remove { album, name, ids } => {
                let n = catalog.remove_from_album(album, &ids).map_err(text)?;
                Ok((format!("Removed {} from {name}.", photos(n)), None))
            }
        }
    }
}

impl Library {
    pub fn albums(&self) -> &[Album] {
        &self.albums
    }

    pub fn current_album(&self) -> Option<i64> {
        self.current_album
    }

    /// The album shown, if it is a plain one (photos can be taken out of it).
    fn plain_album_shown(&self) -> Option<&Album> {
        self.current_album
            .and_then(|id| self.albums.iter().find(|a| a.id == id))
            .filter(|a| !a.is_smart())
    }

    pub(super) fn smart_album_shown(&self) -> bool {
        self.current_album
            .and_then(|id| self.albums.iter().find(|a| a.id == id))
            .is_some_and(Album::is_smart)
    }

    pub(super) fn album_dialog_open(&self) -> bool {
        self.album_dialog.is_some() || self.adding_to_album || self.confirm_delete_album.is_some()
    }

    pub(super) fn can_remove_from_album(&self) -> bool {
        self.plain_album_shown().is_some() && !self.selection.current.is_empty()
    }

    pub(super) fn refresh_albums(&self, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Query, "albums", move || {
            let result = Catalog::open(&root)
                .and_then(|c| c.albums())
                .map_err(|e| e.to_string());
            Msg::Albums { root, result }
        });
    }

    /// Shows the album `id` in the grid (in place of a folder) and loads its photos.
    pub(super) fn show_album(&mut self, id: i64, env: &mut Env) {
        self.compare = None;
        self.map = None;
        if self.current_album != Some(id) || self.people.current.is_some() {
            self.selection = Selection::default();
            self.photos.clear();
            self.rows.clear();
            self.order.clear();
            self.index.clear();
            self.thumbs.reset();
            self.reveal_active = true;
        }
        self.current_folder = None;
        self.people.current = None;
        self.current_album = Some(id);
        self.load_album(env);
    }

    pub(super) fn load_album(&self, env: &mut Env) {
        let (Some(album_id), Some(root)) = (self.current_album, self.root.clone()) else {
            return;
        };
        env.jobs.spawn(Lane::Query, "album photos", move || {
            let result = Catalog::open(&root)
                .and_then(|c| c.album_photos(album_id))
                .map_err(|e| e.to_string());
            Msg::AlbumPhotos {
                root,
                album_id,
                result,
            }
        });
    }

    pub(super) fn albums_loaded(&mut self, root: PathBuf, result: Result<Vec<Album>, String>) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok(albums) => {
                self.albums = albums;
                // An album deleted elsewhere: nothing to show.
                if let Some(id) = self.current_album
                    && !self.albums.iter().any(|a| a.id == id)
                {
                    self.current_album = None;
                    self.photos.clear();
                    self.rows.clear();
                    self.arrange();
                }
            }
            Err(e) => log::error!("could not list the albums: {e}"),
        }
    }

    pub(super) fn album_photos_loaded(
        &mut self,
        root: PathBuf,
        album_id: i64,
        result: Result<Vec<lf_catalog::PhotoRow>, String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) || self.current_album != Some(album_id) {
            return;
        }
        match result {
            Ok(rows) => self.rows_loaded(rows),
            Err(e) => env.toasts.push(
                Level::Error,
                format!("Could not list the album's photos. {e}"),
            ),
        }
    }

    pub(super) fn change_albums(&mut self, change: Change, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Io, "album", move || {
            let result = Catalog::open(&root)
                .map_err(|e| e.to_string())
                .and_then(|c| change.run(&c));
            Msg::AlbumChanged { root, result }
        });
    }

    pub(super) fn album_changed(
        &mut self,
        root: PathBuf,
        result: Result<(String, Option<i64>), String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok((said, show)) => {
                self.album_dialog = None;
                env.toasts.push(Level::Info, said);
                if let Some(id) = show {
                    self.mode = Mode::Library;
                    self.show_album(id, env);
                } else if self.current_album.is_some() {
                    self.load_album(env);
                }
            }
            Err(e) => match &mut self.album_dialog {
                Some(dialog) => {
                    dialog.saving = false;
                    dialog.error = Some(e);
                }
                None => env.toasts.push(Level::Error, e),
            },
        }
        self.refresh_albums(env);
    }

    pub(super) fn open_album_dialog(&mut self, purpose: Purpose) {
        let selected = if self.mode == Mode::Library {
            self.selected_in_order()
        } else {
            Vec::new()
        };
        self.album_dialog = Some(AlbumDialog::new(purpose, selected));
    }

    pub(super) fn remove_from_album(&mut self, env: &mut Env) {
        let Some(album) = self.plain_album_shown().cloned() else {
            return;
        };
        let ids = self.selected_in_order();
        if ids.is_empty() {
            return;
        }
        // Gone from the grid at once; the catalogue follows on the `Io` lane.
        self.photos.retain(|p| !ids.contains(&p.id));
        self.rows = self
            .photos
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id, i))
            .collect();
        self.arrange();
        self.change_albums(
            Change::Remove {
                album: album.id,
                name: album.name,
                ids,
            },
            env,
        );
    }

    /// The Albums list, under the folders.
    pub(super) fn albums_sidebar(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ALBUMS).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("New…", |ui| {
                    if ui.button("Album…").clicked() {
                        self.open_album_dialog(Purpose::New);
                        ui.close();
                    }
                    if ui.button("Smart album…").clicked() {
                        self.open_album_dialog(Purpose::NewSmart);
                        ui.close();
                    }
                });
            });
        });
        ui.separator();
        if self.albums.is_empty() {
            ui.label(RichText::new(NO_ALBUMS).color(theme::MUTED).small());
            return;
        }
        let selected_ids = self.selected_in_order();
        let mut chosen = None;
        let mut change = None;
        for album in &self.albums {
            let shown = self.current_album == Some(album.id);
            let label = if album.is_smart() {
                format!("⚙ {}", album.name)
            } else {
                format!("{}  ({})", album.name, count(album.count as usize))
            };
            let response = ui.selectable_label(shown, label);
            if response.clicked() {
                chosen = Some(album.id);
            }
            response.context_menu(|ui| {
                if !album.is_smart() && !selected_ids.is_empty() {
                    let n = selected_ids.len();
                    if ui
                        .button(format!("Add the selected {}", photos(n)))
                        .clicked()
                    {
                        change = Some(Change::Add {
                            album: album.id,
                            name: album.name.clone(),
                            ids: selected_ids.clone(),
                        });
                        ui.close();
                    }
                }
                let edit = if album.is_smart() {
                    "Edit…"
                } else {
                    "Rename…"
                };
                if ui.button(edit).clicked() {
                    self.album_dialog =
                        Some(AlbumDialog::edit(album.id, &album.name, album.rule.clone()));
                    ui.close();
                }
                if ui.button("Delete…").clicked() {
                    self.confirm_delete_album = Some(album.clone());
                    ui.close();
                }
            });
        }
        if let Some(id) = chosen {
            self.mode = Mode::Library;
            self.show_album(id, env);
        }
        if let Some(change) = change {
            self.change_albums(change, env);
        }
    }

    /// The album dialogs: new or edit, the "Add to album" picker, and the delete question.
    pub(super) fn album_dialogs(&mut self, ctx: &egui::Context, env: &mut Env) {
        if let Some(dialog) = &mut self.album_dialog {
            match dialog.show(ctx) {
                AlbumAction::None => {}
                AlbumAction::Close => self.album_dialog = None,
                AlbumAction::Save {
                    purpose,
                    name,
                    rule,
                    with,
                } => {
                    let change = match purpose {
                        Purpose::Edit(id) => Change::Update { id, name, rule },
                        Purpose::New | Purpose::NewSmart => Change::Create { name, rule, with },
                    };
                    self.change_albums(change, env);
                }
            }
        }
        if self.adding_to_album {
            self.add_to_album_dialog(ctx, env);
        }
        if let Some(album) = self.confirm_delete_album.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("confirm-delete-album")).show(ctx, |ui| {
                ui.set_max_width(400.0);
                ui.heading(format!("Delete the album “{}”?", album.name));
                ui.add_space(8.0);
                ui.label("The photos stay in the catalogue and on disk; only the album goes.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete album").clicked() {
                        self.change_albums(Change::Delete(album.id), env);
                        close = true;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm_delete_album = None;
            }
        }
    }

    /// `Ctrl+B`: puts the selected photos in a plain album picked from a list.
    fn add_to_album_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let ids = match self.mode {
            Mode::Develop(id) => vec![id],
            Mode::Library => self.selected_in_order(),
        };
        let mut close = false;
        let mut change = None;
        egui::Modal::new(egui::Id::unique("add-to-album")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading(format!("{ADD_TITLE}: {}", photos(ids.len())));
            ui.add_space(8.0);
            let plain: Vec<&Album> = self.albums.iter().filter(|a| !a.is_smart()).collect();
            if plain.is_empty() {
                ui.label(RichText::new(NO_PLAIN_ALBUMS).color(theme::MUTED));
            }
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    for album in plain {
                        if ui
                            .button(format!("{}  ({})", album.name, count(album.count as usize)))
                            .clicked()
                        {
                            change = Some(Change::Add {
                                album: album.id,
                                name: album.name.clone(),
                                ids: ids.clone(),
                            });
                            close = true;
                        }
                    }
                });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("New album…").clicked() {
                    self.album_dialog = Some(AlbumDialog::new(Purpose::New, ids.clone()));
                    close = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
            });
        });
        if let Some(change) = change {
            self.change_albums(change, env);
        }
        if close {
            self.adding_to_album = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_say_what_they_did() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/a").unwrap();
        let p = c
            .add_photo(
                f,
                &lf_catalog::NewPhoto {
                    path: "D:/a/1.jpg".into(),
                    kind: "jpeg".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let (said, show) = Change::Create {
            name: "Picks".into(),
            rule: None,
            with: vec![p],
        }
        .run(&c)
        .unwrap();
        assert_eq!(said, "Made the album Picks with 1 photo.");
        let id = show.unwrap();
        let (said, _) = Change::Add {
            album: id,
            name: "Picks".into(),
            ids: vec![p],
        }
        .run(&c)
        .unwrap();
        assert_eq!(said, "Added 0 photos to Picks. 1 photo already in it.");
        let refused = Change::Create {
            name: "picks".into(),
            rule: None,
            with: vec![],
        }
        .run(&c)
        .unwrap_err();
        assert_eq!(refused, "You already have an album called picks.");
    }
}
