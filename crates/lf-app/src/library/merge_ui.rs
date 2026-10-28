//! The library's side of Merge to HDR: the dialog for the selection (two to seven photos),
//! the merge on the `Long` lane ([`crate::merge_job::run`]), and, once written, the merged
//! photo catalogued by rescanning the catalogue folder it lies in.

use super::{Env, Library};
use crate::dialogs::merge::{MergeAction, MergeDialog, Stage, can_merge};
use crate::jobs::{Lane, Msg};
use eframe::egui;
use std::path::PathBuf;

impl Library {
    /// The merge dialog, while open.
    pub fn merge(&self) -> Option<&MergeDialog> {
        self.merge.as_ref()
    }

    /// `Ctrl+H`: the dialog for the selected bracket.
    pub(super) fn open_merge(&mut self) {
        let ids = self.selected_in_order();
        if self.merge.is_none() && can_merge(ids.len()) {
            self.merge = Some(MergeDialog::new(ids));
        }
    }

    pub(super) fn merge_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.merge else {
            return;
        };
        match dialog.show(ctx) {
            MergeAction::None => {}
            MergeAction::Close => {
                if !dialog.running() {
                    self.merge = None;
                }
            }
            MergeAction::Start => self.start_merge(env),
        }
    }

    /// Starts the merge of the dialog's photos.
    pub fn start_merge(&mut self, env: &mut Env) {
        let (Some(dialog), Some(root)) = (&mut self.merge, self.root.clone()) else {
            return;
        };
        dialog.started();
        let ids = dialog.ids.clone();
        let stop = dialog.stop.clone();
        let gpu = self.gpu.clone();
        let reporter = env.jobs.reporter();
        log::info!("merging {} photos to HDR", ids.len());
        env.jobs.spawn(Lane::Long, "merge to HDR", move || {
            let result = crate::merge_job::run(
                gpu,
                &root,
                &ids,
                |done, total| reporter.send(Msg::MergeProgress { done, total }),
                &stop,
            );
            Msg::MergeDone(result)
        });
    }

    pub(super) fn merge_progress(&mut self, done: usize, total: usize) {
        if let Some(dialog) = &mut self.merge {
            dialog.progress(done, total);
        }
    }

    /// The merge ended: said in the dialog, and a merged photo catalogued.
    pub(super) fn merge_done(&mut self, result: Result<PathBuf, String>, env: &mut Env) {
        match &result {
            Ok(path) => {
                log::info!("merged to {}", path.display());
                let folder = self
                    .folders
                    .iter()
                    .find(|f| path.starts_with(&f.path))
                    .cloned();
                if let Some(folder) = folder {
                    self.rescan(&folder, env);
                }
            }
            Err(e) => log::warn!("the merge failed: {e}"),
        }
        if let Some(dialog) = &mut self.merge {
            dialog.stage = Stage::Done(result);
        }
    }
}
