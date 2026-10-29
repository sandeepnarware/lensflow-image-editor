//! Auto import's polling (see [`lf_catalog::auto_import`]): while it is on
//! (`prefs.auto_import`), the watched folder is looked at on the `Io` lane every [`EVERY`];
//! photos that have stopped changing are moved into the destination, which is then scanned
//! (`Library::rescan`), and a toast says how many came in.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library};
use crate::dialogs::auto_import::{AutoImportAction, AutoImportDialog};
use crate::jobs::{Lane, Msg};
use crate::prefs::AutoImportPrefs;
use crate::toast::Level;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the watched folder is looked at.
pub const EVERY: Duration = Duration::from_secs(3);

/// "3 photos auto-imported."
pub fn imported(n: usize) -> String {
    if n == 1 {
        "1 photo auto-imported.".to_owned()
    } else {
        format!("{} photos auto-imported.", crate::catalog_ext::count(n))
    }
}

/// The polling's state.
#[derive(Default)]
pub struct AutoImport {
    /// The files as last seen (a file is ready once it stops changing).
    seen: Arc<Mutex<lf_catalog::auto_import::Seen>>,
    last: Option<Instant>,
    busy: bool,
    /// The last failure, said once.
    failed: Option<String>,
}

impl Library {
    pub fn auto_import_on(&self, env: &Env) -> bool {
        env.prefs.auto_import.is_some()
    }

    pub(super) fn open_auto_import(&mut self, env: &Env) {
        let p = env.prefs.auto_import.clone();
        self.auto_import_dialog = Some(AutoImportDialog::new(
            p.as_ref().map(|p| p.watch.clone()),
            p.map(|p| p.folder),
        ));
    }

    pub(super) fn auto_import_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let folders: Vec<PathBuf> = self.folders.iter().map(|f| f.path.clone()).collect();
        let running = env.prefs.auto_import.is_some();
        let Some(dialog) = &mut self.auto_import_dialog else {
            return;
        };
        match dialog.show(ctx, &folders, running) {
            AutoImportAction::None => {}
            AutoImportAction::Close => self.auto_import_dialog = None,
            AutoImportAction::Choose => {
                dialog.ticket += 1;
                on_own_thread(
                    env.jobs.reporter(),
                    Purpose::AutoImportWatch,
                    dialog.ticket,
                    move || {
                        let picked = rfd::FileDialog::new()
                            .set_title("Choose the folder to watch")
                            .pick_folder();
                        Ok(picked.map(|path| Chosen {
                            path,
                            network: false,
                        }))
                    },
                );
            }
            AutoImportAction::Start { watch, folder } => {
                env.prefs.auto_import = Some(AutoImportPrefs { watch, folder });
                env.save_prefs();
                self.auto_import = super::auto_import_ui::AutoImport::default();
            }
            AutoImportAction::Stop => {
                env.prefs.auto_import = None;
                env.save_prefs();
            }
        }
    }

    /// The watched folder was chosen (`ticket`: the dialog's request).
    pub(super) fn auto_import_watch_chosen(&mut self, ticket: u64, path: Option<PathBuf>) {
        if let (Some(dialog), Some(path)) = (&mut self.auto_import_dialog, path)
            && dialog.ticket == ticket
        {
            dialog.watch = Some(path);
        }
    }

    /// Every frame: a look at the watched folder when one is due.
    pub(super) fn follow_auto_import(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(p) = env.prefs.auto_import.clone() else {
            return;
        };
        let Some(root) = self.root.clone() else {
            return;
        };
        let now = Instant::now();
        ctx.request_repaint_after(EVERY);
        if self.auto_import.busy || self.auto_import.last.is_some_and(|t| now - t < EVERY) {
            return;
        }
        self.auto_import.last = Some(now);
        self.auto_import.busy = true;
        let seen = self.auto_import.seen.clone();
        env.jobs.spawn(Lane::Io, "auto import", move || {
            let result = (|| {
                let mut seen = seen.lock().map_err(|_| "busy".to_owned())?;
                let ready = lf_catalog::auto_import::ready(&p.watch, &mut seen)
                    .map_err(|e| format!("The watched folder could not be read. {e}"))?;
                let mut moved = 0;
                for file in ready {
                    match lf_catalog::auto_import::move_into(&file, &p.folder) {
                        Ok(_) => moved += 1,
                        Err(e) => log::warn!("auto import of {} failed: {e}", file.display()),
                    }
                }
                Ok(moved)
            })();
            Msg::AutoImported {
                root,
                folder: p.folder,
                result,
            }
        });
    }

    pub(super) fn auto_imported(
        &mut self,
        root: PathBuf,
        folder: PathBuf,
        result: Result<usize, String>,
        env: &mut Env,
    ) {
        self.auto_import.busy = false;
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok(0) => self.auto_import.failed = None,
            Ok(n) => {
                self.auto_import.failed = None;
                env.toasts.push(Level::Success, imported(n));
                if let Some(f) = self.folders.iter().find(|f| f.path == folder).cloned() {
                    self.rescan(&f, env);
                }
            }
            Err(e) => {
                if self.auto_import.failed.as_deref() != Some(e.as_str()) {
                    env.toasts.push(Level::Warning, e.clone());
                }
                self.auto_import.failed = Some(e);
            }
        }
    }
}
