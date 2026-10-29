//! The library's side of File → Print: the dialog for the selection (or the photo open in
//! Develop), the "Save PDF…" picker (its own thread), and the run on the `Long` lane
//! ([`crate::print_job::run`]); printing opens the written PDF in the system's viewer.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library, Mode};
use crate::dialogs::print::{PrintAction, PrintDialog};
use crate::jobs::{Lane, Msg};
use crate::print_job::{self, PrintRun};
use eframe::egui;
use lf_catalog::print::PrintSpec;
use std::path::PathBuf;

impl Library {
    /// The print dialog, while open.
    pub fn print(&self) -> Option<&PrintDialog> {
        self.print.as_ref()
    }

    /// `Ctrl+P`: the dialog for the selection, or the photo open in Develop.
    pub(super) fn open_print(&mut self) {
        if self.print.is_some() {
            return;
        }
        let ids = match self.mode {
            Mode::Develop(id) => vec![id],
            Mode::Library => self.selected_in_order(),
        };
        if !ids.is_empty() {
            self.print = Some(PrintDialog::new(ids));
        }
    }

    /// Where the app keeps its own files (the frame cache's parent).
    fn cache_dir(&self) -> PathBuf {
        self.frames_dir
            .parent()
            .map_or_else(|| self.frames_dir.clone(), std::path::Path::to_path_buf)
    }

    pub(super) fn print_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.print else {
            return;
        };
        match dialog.show(ctx) {
            PrintAction::None => {}
            PrintAction::Close => {
                if !dialog.running() {
                    self.print = None;
                }
            }
            PrintAction::Print(spec) => {
                let dir = self.cache_dir().join("print");
                let _ = std::fs::create_dir_all(&dir);
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let out = dir.join(format!("lensflow-print-{stamp}.pdf"));
                self.start_print(spec, out, true, env);
            }
            PrintAction::Save(spec) => {
                self.print_pending = Some(spec);
                on_own_thread(env.jobs.reporter(), Purpose::PrintPdf, 0, move || {
                    let picked = rfd::FileDialog::new()
                        .set_title("Save the PDF")
                        .add_filter("PDF", &["pdf"])
                        .set_file_name("photos.pdf")
                        .save_file();
                    log::info!("PDF save picker answered: {picked:?}");
                    Ok(picked.map(|path| Chosen {
                        path,
                        network: false,
                    }))
                });
            }
        }
    }

    /// "Save PDF…" chose where (`None`: cancelled).
    pub(super) fn print_save_chosen(&mut self, path: Option<PathBuf>, env: &mut Env) {
        let (Some(spec), Some(mut path)) = (self.print_pending.take(), path) else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("pdf");
        }
        self.start_print(spec, path, false, env);
    }

    /// Starts writing the PDF to `out` (and opening it, when `open`).
    pub fn start_print(&mut self, spec: PrintSpec, out: PathBuf, open: bool, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // The open photo's gesture is recorded first, so the print carries it.
        self.develop.finish_edit(env.toasts);
        let temp = self.cache_dir().join("print-render");
        let Some(dialog) = &mut self.print else {
            return;
        };
        let title = dialog.title.clone();
        let job = PrintRun {
            spec,
            out,
            open,
            title,
        };
        dialog.started(&job);
        let ids = dialog.ids.clone();
        let stop = dialog.stop.clone();
        let gpu = self.gpu.clone();
        let reporter = env.jobs.reporter();
        env.jobs.spawn(Lane::Long, "print", move || {
            let result = print_job::run(
                gpu,
                &root,
                &ids,
                &job,
                &temp,
                |done, total| reporter.send(Msg::PrintProgress { done, total }),
                &stop,
            )
            .and_then(|path| {
                if job.open {
                    crate::platform::open_file(&path).map_err(|e| {
                        format!(
                            "The PDF was saved as {}, but could not be opened ({e}).",
                            path.display()
                        )
                    })?;
                }
                Ok((path, job.open))
            });
            let _ = std::fs::remove_dir_all(&temp);
            Msg::PrintDone(result)
        });
    }

    pub(super) fn print_progress(&mut self, done: usize, total: usize) {
        if let Some(dialog) = &mut self.print {
            dialog.progress(done, total);
        }
    }

    pub(super) fn print_done(&mut self, result: Result<(PathBuf, bool), String>) {
        match &result {
            Ok((path, _)) => log::info!("printed to {}", path.display()),
            Err(e) => log::warn!("printing failed: {e}"),
        }
        if let Some(dialog) = &mut self.print {
            dialog.finished(result);
        }
    }
}
