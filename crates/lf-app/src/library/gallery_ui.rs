//! The library's side of "Export to a gallery": the dialog for the selection, its lists on
//! the `Account` lane, the upload on the `Long` lane ([`crate::gallery_job::run`]) and its
//! progress, in the dialog or (closed while running) a toast at the end.

use super::{Env, Library};
use crate::dialogs::gallery_export::{GalleryAction, GalleryDialog};
use crate::gallery_job::{self, GalleryLink, GalleryOutcome};
use crate::jobs::{Lane, Msg};
use crate::toast::Level;
use eframe::egui;
use lf_account::galleries;

impl Library {
    /// The app hands over the studio's client while the gate allows gallery export, and
    /// `None` otherwise (signed out, read-only, locked).
    pub fn set_gallery_link(&mut self, link: Option<GalleryLink>) {
        self.gallery_link = link;
    }

    /// Gallery export is possible now (the gate allows it and the app gave a client).
    pub fn gallery_allowed(&self) -> bool {
        self.gallery_link.is_some()
    }

    /// The gallery dialog, while open or while its upload runs.
    pub fn gallery(&self) -> Option<&GalleryDialog> {
        self.gallery.as_ref()
    }

    /// Opens the dialog for the selection (or shows a hidden one whose upload still runs).
    pub(super) fn open_gallery(&mut self, env: &mut Env) {
        if let Some(dialog) = &mut self.gallery {
            dialog.visible = true;
            return;
        }
        let ids = self.selected_in_order();
        if ids.is_empty() {
            return;
        }
        let (dialog, first) = GalleryDialog::new(ids);
        self.gallery = Some(dialog);
        self.gallery_action(first, env);
    }

    pub(super) fn gallery_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.gallery else {
            return;
        };
        if !dialog.visible {
            return;
        }
        let action = dialog.show(ctx);
        self.gallery_action(action, env);
    }

    fn gallery_action(&mut self, action: GalleryAction, env: &mut Env) {
        match action {
            GalleryAction::None => {}
            GalleryAction::Close => {
                if let Some(dialog) = &mut self.gallery {
                    if dialog.running() {
                        dialog.visible = false;
                    } else {
                        self.gallery = None;
                    }
                }
            }
            GalleryAction::Search(ticket, search) => {
                let Some(link) = self.gallery_link.clone() else {
                    return;
                };
                env.jobs.spawn(Lane::Account, "gallery list", move || {
                    let search = Some(search.as_str()).filter(|s| !s.trim().is_empty());
                    let result = galleries::list(&link.client, search).map_err(|e| e.to_string());
                    Msg::Galleries { ticket, result }
                });
            }
            GalleryAction::Sections(gallery) => {
                let Some(link) = self.gallery_link.clone() else {
                    return;
                };
                env.jobs.spawn(Lane::Account, "gallery sections", move || {
                    let result =
                        galleries::sections(&link.client, gallery).map_err(|e| e.to_string());
                    Msg::GallerySections { gallery, result }
                });
            }
            GalleryAction::Start(run) => {
                let (Some(root), Some(link)) = (self.root.clone(), self.gallery_link.clone())
                else {
                    env.toasts.push(Level::Warning, NOT_ALLOWED);
                    return;
                };
                // The open photo's gesture is recorded first, so the upload carries it.
                self.develop.finish_edit(env.toasts);
                let Some(dialog) = &mut self.gallery else {
                    return;
                };
                dialog.started();
                let ids = dialog.ids.clone();
                let stop = dialog.stop.clone();
                let gpu = self.gpu.clone();
                let reporter = env.jobs.reporter();
                let cache = self
                    .frames_dir
                    .parent()
                    .map_or_else(|| self.frames_dir.clone(), std::path::Path::to_path_buf);
                log::info!("uploading {} photos to gallery {}", ids.len(), run.gallery);
                env.jobs.spawn(Lane::Long, "gallery upload", move || {
                    let result = gallery_job::temp_folder(&cache).and_then(|temp| {
                        let mut upload = gallery_job::uploader(link, &run);
                        let out = gallery_job::run(
                            gpu,
                            &root,
                            &ids,
                            &run,
                            &temp,
                            &mut upload,
                            |done, total| reporter.send(Msg::GalleryProgress { done, total }),
                            &stop,
                        );
                        let _ = std::fs::remove_dir_all(&temp);
                        out
                    });
                    Msg::GalleryDone(result)
                });
            }
        }
    }

    pub(super) fn galleries_loaded(
        &mut self,
        ticket: u64,
        result: Result<Vec<galleries::Gallery>, String>,
    ) {
        if let Some(dialog) = &mut self.gallery {
            dialog.galleries_loaded(ticket, result);
        }
    }

    pub(super) fn gallery_sections_loaded(
        &mut self,
        gallery: i64,
        result: Result<Vec<galleries::Section>, String>,
    ) {
        if let Some(dialog) = &mut self.gallery {
            dialog.sections_loaded(gallery, result);
        }
    }

    pub(super) fn gallery_progress(&mut self, done: usize, total: usize) {
        if let Some(dialog) = &mut self.gallery {
            dialog.progress(done, total);
        }
    }

    /// The upload ended: shown in the dialog, or (closed while running) as a toast.
    pub(super) fn gallery_done(&mut self, result: Result<GalleryOutcome, String>, env: &mut Env) {
        match &result {
            Ok(o) => log::info!("gallery upload ended: {}", gallery_job::done_text(o)),
            Err(e) => log::warn!("gallery upload refused or failed: {e}"),
        }
        let Some(dialog) = &mut self.gallery else {
            return;
        };
        if dialog.visible {
            dialog.finished(result);
            return;
        }
        self.gallery = None;
        match result {
            Ok(o) => {
                let level = if o.failed.is_empty() && !o.cancelled {
                    Level::Success
                } else {
                    Level::Warning
                };
                env.toasts.push(level, gallery_job::done_text(&o));
            }
            Err(e) => env.toasts.push(
                Level::Error,
                format!("The upload to the gallery stopped. {e}"),
            ),
        }
    }

    /// "Uploading 12 of 40" for the status bar, while an upload runs.
    pub(super) fn gallery_status(&self) -> Option<(usize, usize)> {
        match self.gallery.as_ref().map(|d| &d.stage) {
            Some(crate::dialogs::gallery_export::Stage::Running { done, total }) => {
                Some((*done, *total))
            }
            _ => None,
        }
    }
}

/// Export to a gallery was asked for while the plan does not allow it (or signed out).
pub const NOT_ALLOWED: &str =
    "Export to a gallery needs an active LensFlow plan. Export to disk still works.";
