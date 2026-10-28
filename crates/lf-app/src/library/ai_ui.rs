//! The library's side of the AI tools that make new photos ([`crate::ai_job`]): the dialog for
//! the selection (the open photo in Develop), the run on the `Long` lane, and, once written,
//! the new photos catalogued by rescanning the catalogue folders they lie in. The tools need
//! the studio in good standing (`local_ai`, from the entitlement gate).

use super::{Env, Library, Mode};
use crate::ai_job::{self, AiTool, Report, Step};
use crate::dialogs::ai::{AiAction, AiDialog, Stage};
use crate::jobs::{Lane, Msg};
use eframe::egui;
use std::sync::atomic::Ordering;

impl Library {
    /// The app passes the gate's answer each frame.
    pub fn set_local_ai(&mut self, allowed: bool) {
        self.local_ai = allowed;
        self.develop.set_local_ai(allowed);
    }

    pub fn ai(&self) -> Option<&AiDialog> {
        self.ai.as_ref()
    }

    /// The AI engine over the cache's AI folder.
    pub(super) fn ai_engine(&self) -> lf_ai::Engine {
        lf_ai::Engine::new(ai_job::ai_dir(&self.frames_dir))
    }

    /// Whether an AI tool may start now on the photos it would take.
    pub(super) fn ai_ready(&self) -> bool {
        let develop = matches!(self.mode, Mode::Develop(_));
        self.local_ai
            && self.ai.is_none()
            && (develop || !self.selection.current.is_empty())
            && self.can_add()
    }

    /// The photos an AI tool works on: the open one in Develop, else the selection.
    fn ai_targets(&self) -> Vec<i64> {
        match self.mode {
            Mode::Develop(id) => vec![id],
            Mode::Library => self.selected_in_order(),
        }
    }

    pub(super) fn open_ai(&mut self, tool: AiTool) {
        let ids = self.ai_targets();
        if ids.is_empty() || self.ai.is_some() {
            return;
        }
        let download = self.ai_engine().to_download(&tool.models());
        self.ai = Some(AiDialog::new(ids, tool, download));
        // A mask with nothing to download starts at once.
        if matches!(tool, AiTool::Mask { .. }) && download == 0 {
            self.start_ai_now = true;
        }
    }

    pub(super) fn ai_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        if std::mem::take(&mut self.start_ai_now)
            && let Some(tool) = self.ai.as_ref().map(|d| d.tool)
        {
            self.start_ai(tool, env);
        }
        let Some(dialog) = &mut self.ai else {
            return;
        };
        match dialog.show(ctx) {
            AiAction::None => {}
            AiAction::Close => {
                if !dialog.running() {
                    self.ai = None;
                }
            }
            AiAction::Start(tool) => self.start_ai(tool, env),
        }
    }

    fn start_ai(&mut self, tool: AiTool, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // The open photo's gesture is recorded first, so the new photo carries it.
        self.develop.finish_edit(env.toasts);
        let Some(dialog) = &mut self.ai else {
            return;
        };
        dialog.tool = tool;
        dialog.started();
        let ids = dialog.ids.clone();
        let stop = dialog.stop.clone();
        let gpu = self.gpu.clone();
        let engine = self.ai_engine();
        let reporter = env.jobs.reporter();
        log::info!("{} on {} photos", tool.name(), ids.len());
        env.jobs.spawn(Lane::Long, "ai", move || {
            let result = ai_job::run(
                gpu,
                &engine,
                &root,
                &ids,
                tool,
                |step| reporter.send(Msg::AiProgress(step)),
                &stop,
            );
            Msg::AiDone(result)
        });
    }

    pub(super) fn ai_progress(&mut self, step: Step) {
        if let Some(dialog) = &mut self.ai {
            dialog.progress(step);
        }
    }

    /// The run ended: said in the dialog, and the new photos catalogued.
    pub(super) fn ai_done(&mut self, result: Result<Report, String>, env: &mut Env) {
        if let Ok(report) = &result {
            let mut folders: Vec<_> = report
                .written
                .iter()
                .filter_map(|p| {
                    self.folders
                        .iter()
                        .find(|f| p.starts_with(&f.path))
                        .cloned()
                })
                .collect();
            folders.dedup_by_key(|f| f.id);
            for folder in folders {
                self.rescan(&folder, env);
            }
        }
        // AI masks go to the open photo; the dialog closes when that was all it did.
        let mut only_masks = false;
        if let Ok(report) = &result {
            for (id, shape) in &report.masks {
                if self.mode == Mode::Develop(*id) {
                    self.develop.add_ai_mask(shape.clone(), env.toasts);
                } else {
                    env.toasts.push(
                        crate::toast::Level::Info,
                        "The mask was not added: its photo is no longer open.",
                    );
                }
            }
            only_masks =
                !report.masks.is_empty() && report.written.is_empty() && report.failed.is_empty();
        }
        if only_masks {
            self.ai = None;
        } else if let Some(dialog) = &mut self.ai {
            dialog.stage = Stage::Done(result);
        }
    }

    /// The app is quitting or the catalogue changing: a run stops after its current tile.
    pub(super) fn stop_ai(&self) {
        if let Some(ai) = &self.ai {
            ai.stop.store(true, Ordering::SeqCst);
        }
    }
}
