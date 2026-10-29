//! The library's side of the AI tools that make new photos ([`crate::ai_job`]): the dialog for
//! the selection (the open photo in Develop), the run on the `Long` lane, and, once written,
//! the new photos catalogued by rescanning the catalogue folders they lie in. The tools need
//! the studio in good standing (`local_ai`, from the entitlement gate).

use super::{Env, Library, Mode};
use crate::ai_job::{self, AiTool, Report, Step};
use crate::dialogs::ai::{AiAction, AiDialog, Stage};
use crate::dialogs::remove::{RemoveAction, RemoveDialog};
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
    pub(super) fn ai_targets(&self) -> Vec<i64> {
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
        if matches!(tool, AiTool::Mask { .. } | AiTool::ObjectMask { .. }) && download == 0 {
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
            AiAction::ChooseSky => {
                super::import_ui::on_own_thread(
                    env.jobs.reporter(),
                    super::import_ui::Purpose::SkyFile,
                    0,
                    move || {
                        let picked = rfd::FileDialog::new()
                            .set_title("Choose a photo of a sky")
                            .add_filter("Pictures", &["jpg", "jpeg", "png", "tif", "tiff", "webp"])
                            .pick_file();
                        Ok(picked.map(|path| super::import_ui::Chosen {
                            path,
                            network: false,
                        }))
                    },
                );
            }
            AiAction::ShowPeople => {
                self.ai = None;
                self.show_people(env);
            }
            AiAction::ShowIssues => {
                self.ai = None;
                self.set_filter(super::cull::Filter {
                    review: super::cull::ReviewFilter::Issues,
                    ..self.filter()
                });
            }
            AiAction::RejectIssues => {
                let issues = self.cull_issues();
                self.ai = None;
                let n = issues.len();
                self.set_mark(
                    issues,
                    lf_catalog::cull::Mark::Flag(lf_catalog::cull::Flag::Rejected),
                    env,
                );
                env.toasts.push(
                    crate::toast::Level::Info,
                    if n == 1 {
                        "Rejected 1 photo.".to_owned()
                    } else {
                        format!("Rejected {} photos.", crate::catalog_ext::count(n))
                    },
                );
            }
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
        let painted = dialog.mask.clone();
        let sky = dialog.sky.clone();
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
                painted.as_deref(),
                sky.as_deref(),
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

    /// AI → Assisted culling: the selection when several photos are selected, else every
    /// photo the grid shows.
    pub(super) fn open_cull(&mut self) {
        let ids = if self.selection.current.len() > 1 {
            self.selected_in_order()
        } else {
            self.order.clone()
        };
        if ids.is_empty() || self.ai.is_some() {
            return;
        }
        let tool = AiTool::Cull;
        let download = self.ai_engine().to_download(&tool.models());
        self.ai = Some(AiDialog::new(ids, tool, download));
    }

    /// AI → Find keywords: the selection when several photos are selected, else every photo
    /// the grid shows.
    pub(super) fn open_find_keywords(&mut self) {
        let ids = if self.selection.current.len() > 1 {
            self.selected_in_order()
        } else {
            self.order.clone()
        };
        if ids.is_empty() || self.ai.is_some() {
            return;
        }
        let tool = AiTool::FindKeywords;
        let download = self.ai_engine().to_download(&tool.models());
        self.ai = Some(AiDialog::new(ids, tool, download));
    }

    /// The photos the finished culling run says look wrong.
    fn cull_issues(&self) -> Vec<i64> {
        match self.ai.as_ref().map(|d| &d.stage) {
            Some(Stage::Done(Ok(Report {
                culled: Some(c), ..
            }))) => c.issues.clone(),
            _ => Vec::new(),
        }
    }

    /// The run ended: said in the dialog, and the new photos catalogued.
    pub(super) fn ai_done(&mut self, result: Result<Report, String>, env: &mut Env) {
        // New scores: the rows are read again so the grid shows them.
        if matches!(
            &result,
            Ok(Report {
                culled: Some(_),
                ..
            })
        ) {
            self.reload_photos(env);
        }
        // New suggestions: the rows (which carry them for search) are read again.
        if let Ok(Report {
            keywords: Some(_), ..
        }) = &result
        {
            self.reload_photos(env);
        }
        // New faces: the People list (and the rows' names) are read again.
        if let Ok(Report {
            people: Some(_), ..
        }) = &result
        {
            self.refresh_people(env);
            self.reload_photos(env);
        }
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

impl Library {
    /// AI → Remove objects: the painting dialog for the one photo (the open one in Develop),
    /// its picture rendered on the `Open` lane.
    pub(super) fn open_remove(&mut self, env: &mut Env) {
        let ids = self.ai_targets();
        let (Some(&id), Some(root)) = (ids.first(), self.root.clone()) else {
            return;
        };
        let Some(path) = self.photo(id).map(|p| p.path.clone()) else {
            return;
        };
        // The open photo's gesture is recorded first, so the picture shows it.
        self.develop.finish_edit(env.toasts);
        let gpu = self.gpu.clone();
        let cache = self.frames.clone();
        let quality = crate::develop::open::quality(env.prefs.develop_quality);
        env.jobs.spawn(Lane::Open, "remove preview", move || {
            let result =
                crate::library::compare::render(&gpu, &root, id, &path, cache.as_deref(), quality)
                    .map(|p| Box::new(crate::library::compare::Pixels(p)));
            Msg::RemoveRendered { id, result }
        });
        self.remove = Some(RemoveDialog::new(id));
    }

    pub(super) fn wires_selected(&mut self, id: i64, result: Result<lf_ai::Gray, String>) {
        if let Some(dialog) = self.remove.as_mut().filter(|d| d.id == id) {
            dialog.wires_found(result);
        }
    }

    pub(super) fn people_selected(&mut self, id: i64, result: Result<lf_ai::Gray, String>) {
        if let Some(dialog) = self.remove.as_mut().filter(|d| d.id == id) {
            dialog.people_found(result);
        }
    }

    pub(super) fn remove_rendered(
        &mut self,
        id: i64,
        result: Result<Box<crate::library::compare::Pixels>, String>,
    ) {
        if let Some(dialog) = self.remove.as_mut().filter(|d| d.id == id) {
            dialog.rendered(result.map(|p| p.0));
        }
    }

    pub(super) fn remove_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.remove else {
            return;
        };
        match dialog.show(ctx) {
            RemoveAction::None => {}
            RemoveAction::Close => self.remove = None,
            RemoveAction::SelectPeople(picture) => {
                let id = dialog.id;
                let engine = self.ai_engine();
                env.jobs.spawn(Lane::Long, "select people", move || {
                    let target = lf_ai::segment::Target::People;
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    let result = engine
                        .prepare(target.models(), &mut |_, _| {}, &stop)
                        .and_then(|()| {
                            lf_ai::segment::mask(&engine, target, &picture, picture.w, picture.h)
                        })
                        .map_err(|e| e.to_string());
                    Msg::PeopleSelected { id, result }
                });
            }
            RemoveAction::SelectWires(picture) => {
                let id = dialog.id;
                let engine = self.ai_engine();
                env.jobs.spawn(Lane::Long, "select wires", move || {
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    let result = engine
                        .prepare(&lf_ai::segment::SKY_MODELS, &mut |_, _| {}, &stop)
                        .and_then(|()| lf_ai::wires::find(&engine, &picture))
                        .map_err(|e| e.to_string());
                    Msg::WiresSelected { id, result }
                });
            }
            RemoveAction::Remove(mask) => {
                let id = dialog.id;
                self.remove = None;
                let tool = AiTool::Remove;
                let download = self.ai_engine().to_download(&tool.models());
                let mut ai = AiDialog::new(vec![id], tool, download);
                ai.mask = Some(std::sync::Arc::new(mask));
                self.ai = Some(ai);
                self.start_ai_now = download == 0;
            }
        }
    }
}
