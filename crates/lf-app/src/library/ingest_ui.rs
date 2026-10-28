//! The library's side of "Import from a card": the dialog, its folder pickers (each on its
//! own thread, as the other pickers), the scan and plan and then the copy on the `Long` lane
//! ([`lf_catalog::ingest`]), and, once copied, the new folder catalogued: rescanned with the
//! catalogue folder it lies in, or added as a folder of its own.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library};
use crate::dialogs::ingest::{IngestAction, IngestDialog, Pick, done_text};
use crate::jobs::{Lane, Msg};
use crate::toast::Level;
use eframe::egui;
use lf_catalog::Catalog;
use lf_catalog::ingest::{self, IngestOutcome, Planned};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// The copies are made but another import holds the catalogue: added by hand later.
pub const ADD_LATER: &str =
    "The photos were copied. Add their folder once the import running now ends:";

impl Library {
    /// The "Import from a card" dialog, while open or while its copy runs.
    pub fn ingest(&self) -> Option<&IngestDialog> {
        self.ingest.as_ref()
    }

    /// The dialog, to change (tests use it).
    pub fn ingest_mut(&mut self) -> Option<&mut IngestDialog> {
        self.ingest.as_mut()
    }

    /// Opens the dialog, filled in as last time (or shows one whose copy still runs).
    pub(super) fn open_ingest(&mut self, env: &mut Env) {
        if let Some(dialog) = &mut self.ingest {
            dialog.visible = true;
            return;
        }
        self.ingest = Some(IngestDialog::new(
            env.prefs.last_ingest.as_ref(),
            self.root.clone(),
        ));
    }

    pub(super) fn ingest_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.ingest else {
            return;
        };
        if !dialog.visible {
            return;
        }
        let action = dialog.show(ctx);
        self.ingest_action(action, env);
    }

    /// A folder picker of the dialog answered.
    pub(super) fn ingest_picked(
        &mut self,
        pick: Pick,
        ticket: u64,
        path: Option<PathBuf>,
        env: &mut Env,
    ) {
        if let Some(dialog) = &mut self.ingest {
            let action = dialog.picked(pick, ticket, path);
            self.ingest_action(action, env);
        }
    }

    pub(super) fn ingest_action(&mut self, action: IngestAction, env: &mut Env) {
        match action {
            IngestAction::None => {}
            IngestAction::Close => {
                if let Some(dialog) = &mut self.ingest {
                    if dialog.running() {
                        dialog.visible = false;
                    } else {
                        self.ingest = None;
                    }
                }
            }
            IngestAction::Pick(pick, ticket) => {
                let title = match pick {
                    Pick::Card => "Choose the card or folder to import from",
                    Pick::Destination => "Copy the photos into this folder",
                    Pick::Backup => "Make a backup copy in this folder",
                };
                on_own_thread(
                    env.jobs.reporter(),
                    Purpose::Ingest(pick),
                    ticket,
                    move || {
                        let picked = rfd::FileDialog::new().set_title(title).pick_folder();
                        log::info!("import-from-card picker ({pick:?}) answered: {picked:?}");
                        Ok(picked.map(|path| Chosen {
                            path,
                            network: false,
                        }))
                    },
                );
            }
            IngestAction::Scan(card) => {
                let Some(root) = self.root.clone() else {
                    return;
                };
                env.jobs.spawn(Lane::Long, "card scan", move || {
                    let scan = lf_catalog::import::scan(&card);
                    let result = if scan.root_readable {
                        Catalog::open(&root)
                            .and_then(|c| ingest::plan(&c, &scan.candidates))
                            .map_err(|e| e.to_string())
                    } else {
                        Err(format!("{} could not be read.", card.display()))
                    };
                    Msg::IngestPlanned(result)
                });
            }
            IngestAction::Start(run) => {
                let Some(dialog) = &mut self.ingest else {
                    return;
                };
                env.prefs.last_ingest = Some(dialog.prefs());
                dialog.started();
                let stop = dialog.stop.clone();
                let reporter = env.jobs.reporter();
                log::info!(
                    "importing {} photos from a card into {}",
                    run.plan.len(),
                    run.spec.destination.display()
                );
                env.jobs.spawn(Lane::Long, "card import", move || {
                    let outcome =
                        ingest::run(&run.spec, &run.plan, &run.names, &mut |done, total| {
                            reporter.send(Msg::IngestProgress { done, total });
                            if stop.load(Ordering::SeqCst) {
                                ControlFlow::Break(())
                            } else {
                                ControlFlow::Continue(())
                            }
                        });
                    Msg::IngestDone(Ok((run.spec.destination, outcome)))
                });
            }
        }
    }

    pub(super) fn ingest_planned(&mut self, result: Result<Vec<Planned>, String>) {
        if let Some(dialog) = &mut self.ingest {
            dialog.planned(result);
        }
    }

    pub(super) fn ingest_progress(&mut self, done: usize, total: usize) {
        if let Some(dialog) = &mut self.ingest {
            dialog.progress(done, total);
        }
    }

    /// The copy ended: said in the dialog (or a toast, when it was closed), and the copies
    /// catalogued.
    pub(super) fn ingest_done(
        &mut self,
        result: Result<(PathBuf, IngestOutcome), String>,
        env: &mut Env,
    ) {
        let (folder, outcome) = match result {
            Ok(done) => done,
            Err(e) => {
                if let Some(dialog) = &mut self.ingest {
                    dialog.finished(Err(e));
                }
                return;
            }
        };
        log::info!("card import ended: {}", done_text(&outcome));
        let copied = outcome.copied;
        match &mut self.ingest {
            Some(dialog) if dialog.visible => dialog.finished(Ok(outcome)),
            _ => {
                let level = if outcome.failed.is_empty() && outcome.backup_failed == 0 {
                    Level::Success
                } else {
                    Level::Warning
                };
                env.toasts.push(level, done_text(&outcome));
                self.ingest = None;
            }
        }
        if copied > 0 {
            self.catalogue_copies(folder, env);
        }
    }

    /// Catalogues the folder the copies went into: with the catalogue folder it lies in, or
    /// as a folder of its own.
    fn catalogue_copies(&mut self, folder: PathBuf, env: &mut Env) {
        if !self.can_add() {
            env.toasts
                .push(Level::Info, format!("{ADD_LATER} {}", folder.display()));
            return;
        }
        let parent = self
            .folders
            .iter()
            .find(|f| folder.starts_with(&f.path))
            .cloned();
        match parent {
            Some(f) => self.rescan(&f, env),
            None => self.scan_folder(folder, true, env),
        }
    }
}
