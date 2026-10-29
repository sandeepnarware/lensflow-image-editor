//! Library → Web gallery…: the selected photos made into a website in a folder
//! ([`lf_catalog::web_gallery`]): exported at [`EDGE`] pixels as JPEG into `images/` (the
//! export pipeline, with each photo's edits), then thumbnails and `index.html`. Runs on the
//! `Long` lane; the page opens in the browser when it is done.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library};
use crate::dialogs::export::{ExportForm, FormatKind, SizeKind};
use crate::jobs::{Lane, Msg};
use crate::theme;
use crate::toast::Level;
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub const TITLE: &str = "Web gallery";
pub const TEXT: &str = "The selected photos as a website in a folder: a grid of thumbnails, each photo large on a click. Upload the folder anywhere, or open its index.html.";
pub const PAGE_TITLE: &str = "Page title";
pub const FOLDER: &str = "Folder";
pub const MAKE: &str = "Make gallery";
pub const MAKING: &str = "Making the gallery…";
/// The photos' long edge on the site.
pub const EDGE: u32 = 2048;

pub struct WebGalleryDialog {
    pub ids: Vec<i64>,
    pub title: String,
    pub folder: Option<PathBuf>,
    pub ticket: u64,
    pub running: bool,
}

impl Library {
    pub(super) fn open_web_gallery(&mut self) {
        let ids = self.selected_in_order();
        if !ids.is_empty() {
            self.web_gallery = Some(WebGalleryDialog {
                ids,
                title: String::new(),
                folder: None,
                ticket: 0,
                running: false,
            });
        }
    }

    pub(super) fn web_gallery_folder_chosen(&mut self, ticket: u64, path: Option<PathBuf>) {
        if let (Some(d), Some(path)) = (&mut self.web_gallery, path)
            && d.ticket == ticket
        {
            d.folder = Some(path);
        }
    }

    pub(super) fn web_gallery_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(d) = &mut self.web_gallery else {
            return;
        };
        let (mut choose, mut make, mut close) = (false, false, false);
        let _ = egui::Modal::new(egui::Id::unique("web-gallery")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(TITLE);
            ui.label(RichText::new(TEXT).color(theme::MUTED));
            ui.add_space(8.0);
            if d.running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(MAKING);
                });
                return;
            }
            egui::Grid::new("web-gallery-grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    let label = ui.label(PAGE_TITLE);
                    ui.text_edit_singleline(&mut d.title).labelled_by(label.id);
                    ui.end_row();
                    ui.label(FOLDER);
                    ui.horizontal(|ui| {
                        let shown = d
                            .folder
                            .as_ref()
                            .map_or("None chosen".to_owned(), |f| f.display().to_string());
                        ui.label(shown);
                        choose = ui.button("Choose…").clicked();
                    });
                    ui.end_row();
                });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                make = ui
                    .add_enabled(d.folder.is_some(), egui::Button::new(MAKE))
                    .clicked();
                close =
                    ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if close && !d.running {
            self.web_gallery = None;
            return;
        }
        if choose {
            d.ticket += 1;
            on_own_thread(
                env.jobs.reporter(),
                Purpose::WebGalleryFolder,
                d.ticket,
                || {
                    let picked = rfd::FileDialog::new()
                        .set_title("Choose a folder for the gallery")
                        .pick_folder();
                    Ok(picked.map(|path| Chosen {
                        path,
                        network: false,
                    }))
                },
            );
        }
        if !make {
            return;
        }
        let (Some(folder), Some(root)) = (d.folder.clone(), self.root.clone()) else {
            return;
        };
        d.running = true;
        let ids = d.ids.clone();
        let title = if d.title.trim().is_empty() {
            folder
                .file_name()
                .map_or("Gallery".to_owned(), |n| n.to_string_lossy().into_owned())
        } else {
            d.title.trim().to_owned()
        };
        // Each photo's caption: its title, else its file name.
        let captions: std::collections::HashMap<i64, String> = ids
            .iter()
            .filter_map(|id| self.photo(*id))
            .map(|p| {
                let name = crate::develop::open::file_name(&p.path);
                (p.id, p.meta.title.clone().unwrap_or(name))
            })
            .collect();
        self.develop.finish_edit(env.toasts);
        let gpu = self.gpu.clone();
        env.jobs.spawn(Lane::Long, "web gallery", move || {
            let result = (|| {
                let images = folder.join("images");
                std::fs::create_dir_all(&images)
                    .map_err(|e| format!("The gallery's folder could not be made. {e}"))?;
                let form = ExportForm {
                    format: FormatKind::Jpeg,
                    quality: 85,
                    size: SizeKind::LongEdge,
                    pixels: EDGE,
                    folder: Some(images),
                    overwrite: true,
                    ..ExportForm::default()
                };
                let spec = form.spec().map_err(str::to_owned)?;
                let stop = AtomicBool::new(false);
                let done = crate::export_job::run(gpu, &root, &ids, &spec, |_, _| {}, &stop)?;
                let entries: Vec<lf_catalog::web_gallery::Entry> = done
                    .exported
                    .iter()
                    .map(|e| lf_catalog::web_gallery::Entry {
                        image: e.path.clone(),
                        caption: captions.get(&e.photo_id).cloned().unwrap_or_default(),
                    })
                    .collect();
                lf_catalog::web_gallery::write(&folder, &title, &entries)
                    .map_err(|e| format!("The gallery could not be written. {e}"))
            })();
            Msg::WebGalleryDone(result)
        });
    }

    pub(super) fn web_gallery_done(&mut self, result: Result<PathBuf, String>, env: &mut Env) {
        self.web_gallery = None;
        match result {
            Ok(index) => {
                env.toasts.push(
                    Level::Success,
                    format!("The gallery is ready: {}", index.display()),
                );
                if let Err(e) = crate::platform::open_file(&index) {
                    log::warn!("could not open {}: {e}", index.display());
                }
            }
            Err(e) => env.toasts.push(Level::Error, e),
        }
    }
}
