//! The library's side of "Export N photos": opening the dialog for the selection, the
//! destination picker (its own thread, as the import picker) and typed path, starting
//! [`export_job::run`] on the `Long` lane, and its progress in the dialog and the status bar.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library};
use crate::dialogs::export::{ExportAction, ExportDialog, ExportForm, Stage};
use crate::export_job::{self, ExportOutcome};
use crate::jobs::{Lane, Msg};
use crate::platform;
use crate::toast::Level;
use eframe::egui;
use std::path::PathBuf;

impl Library {
    /// The export dialog, while open or while its export runs.
    pub fn export(&self) -> Option<&ExportDialog> {
        self.export.as_ref()
    }

    /// The export dialog, to change (tests use it).
    pub fn export_mut(&mut self) -> Option<&mut ExportDialog> {
        self.export.as_mut()
    }

    /// `Ctrl+Shift+E`: the export dialog for the selection, filled in as last time; a hidden
    /// dialog whose export still runs is shown again.
    pub(super) fn open_export(&mut self, env: &mut Env) {
        if let Some(dialog) = &mut self.export {
            dialog.visible = true;
            return;
        }
        let ids = self.selected_in_order();
        if ids.is_empty() {
            return;
        }
        let form = ExportForm::from_prefs(env.prefs.last_export.as_ref());
        self.export = Some(ExportDialog::new(ids, form));
    }

    pub(super) fn export_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.export else {
            return;
        };
        if !dialog.visible {
            return;
        }
        match dialog.show(ctx) {
            ExportAction::None => {}
            ExportAction::Close => {
                if dialog.running() {
                    dialog.visible = false;
                } else {
                    self.export = None;
                }
            }
            ExportAction::Pick(ticket) => {
                on_own_thread(env.jobs.reporter(), Purpose::ExportTo, ticket, move || {
                    let picked = rfd::FileDialog::new()
                        .set_title("Export to this folder")
                        .pick_folder();
                    log::info!("export folder picker answered: {picked:?}");
                    match picked {
                        Some(path) => {
                            export_job::check_writable(&path)?;
                            Ok(Some(Chosen {
                                path,
                                network: false,
                            }))
                        }
                        None => Ok(None),
                    }
                });
            }
            ExportAction::PickLogo(ticket) => {
                on_own_thread(env.jobs.reporter(), Purpose::Watermark, ticket, move || {
                    let picked = rfd::FileDialog::new()
                        .set_title("Choose a logo for the watermark")
                        .add_filter("Pictures", &["png", "jpg", "jpeg", "tif", "tiff", "webp"])
                        .pick_file();
                    log::info!("watermark picker answered: {picked:?}");
                    Ok(picked.map(|path| Chosen {
                        path,
                        network: false,
                    }))
                });
            }
            ExportAction::CheckTyped(ticket, path) => {
                on_own_thread(env.jobs.reporter(), Purpose::ExportTo, ticket, move || {
                    export_job::check_writable(&path)?;
                    Ok(Some(Chosen {
                        path,
                        network: false,
                    }))
                });
            }
            ExportAction::ShowFolder(file) => {
                if let Err(e) = platform::show_in_folder(&file) {
                    log::error!("could not show {}: {e}", file.display());
                    env.toasts
                        .push(Level::Error, format!("Could not open the folder. {e}"));
                }
            }
            ExportAction::Start(spec) => {
                let Some(root) = self.root.clone() else {
                    return;
                };
                match serde_json::to_value(&spec) {
                    Ok(value) => {
                        env.prefs.last_export = Some(value);
                        env.save_prefs();
                    }
                    Err(e) => log::error!("could not remember the export settings: {e}"),
                }
                // The open photo's gesture is recorded first, so the export carries it.
                self.develop.finish_edit(env.toasts);
                let Some(dialog) = &mut self.export else {
                    return;
                };
                dialog.started();
                let ids = dialog.ids.clone();
                let stop = dialog.stop.clone();
                let gpu = self.gpu.clone();
                let reporter = env.jobs.reporter();
                log::info!("exporting {} photos with {spec:?}", ids.len());
                env.jobs.spawn(Lane::Long, "export", move || {
                    let result = export_job::run(
                        gpu,
                        &root,
                        &ids,
                        &spec,
                        |done, total| reporter.send(Msg::ExportProgress { done, total }),
                        &stop,
                    );
                    Msg::ExportDone(result)
                });
            }
        }
    }

    pub(super) fn export_progress(&mut self, done: usize, total: usize) {
        if let Some(dialog) = &mut self.export {
            dialog.progress(done, total);
        }
    }

    /// The export ended: shown in the dialog, or (closed while running) as a toast.
    pub(super) fn export_done(&mut self, result: Result<ExportOutcome, String>, env: &mut Env) {
        match &result {
            Ok(o) => log::info!(
                "export ended: {} exported, {} failed, stopped: {}, device lost: {}",
                o.exported.len(),
                o.failed.len(),
                o.cancelled,
                o.device_lost
            ),
            Err(e) => log::warn!("export refused or failed: {e}"),
        }
        // A lost device may end the app before this is read (the restart question comes
        // first): the log keeps how far the export got.
        if let Ok(o) = &result
            && o.device_lost
        {
            log::warn!("{}", export_job::done_text(o));
        }
        let Some(dialog) = &mut self.export else {
            return;
        };
        if dialog.visible {
            dialog.finished(result);
            return;
        }
        self.export = None;
        match result {
            Ok(outcome) => {
                let level =
                    if outcome.failed.is_empty() && !outcome.cancelled && !outcome.device_lost {
                        Level::Success
                    } else {
                        Level::Warning
                    };
                env.toasts.push(level, export_job::done_text(&outcome));
            }
            // One sentence of context, unless the message already says the export stopped.
            Err(e) if e.contains("export stopped") => env.toasts.push(Level::Error, e),
            Err(e) => env
                .toasts
                .push(Level::Error, format!("The export stopped. {e}")),
        }
    }

    /// "Exporting 12 of 40" for the status bar, while an export runs.
    pub(super) fn export_status(&self) -> Option<(usize, usize)> {
        match self.export.as_ref().map(|d| &d.stage) {
            Some(Stage::Running { done, total }) => Some((*done, *total)),
            _ => None,
        }
    }

    /// The export dialog's destination, if one is chosen (tests read it).
    pub fn export_folder(&self) -> Option<PathBuf> {
        self.export.as_ref().and_then(|d| d.form.folder.clone())
    }
}
