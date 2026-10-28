//! The library's stacks ([`lf_catalog::stacks`]): Stack (`Ctrl+G`) gathers the selection
//! under the active photo, Unstack (`Ctrl+Shift+G`) takes apart the stacks of the selection,
//! Expand / collapse (`Shift+S`) opens or closes the active photo's stack, and Auto-stack…
//! finds bursts and duplicates on the `Long` lane. Each change is saved on the `Io` lane and
//! the folder's rows are read again.

use super::{Env, Library, Mode};
use crate::dialogs::auto_stack::{AutoStackAction, AutoStackDialog};
use crate::jobs::{Lane, Msg};
use crate::toast::Level;
use eframe::egui;
use lf_catalog::Catalog;
use std::sync::atomic::Ordering;

/// What a stack change does in the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Stack { top: i64 },
    Unstack,
    Collapse { stack: i64, collapsed: bool },
}

impl Library {
    /// Whether `Stack` may run: two or more photos selected in the grid.
    pub(super) fn can_stack(&self) -> bool {
        self.mode == Mode::Library && self.selection.current.len() > 1
    }

    /// Whether `Unstack` may run: something selected is in a stack.
    pub(super) fn can_unstack(&self) -> bool {
        self.mode == Mode::Library
            && self
                .selection
                .current
                .iter()
                .any(|id| self.photo(*id).is_some_and(|p| p.stack.is_some()))
    }

    /// The active photo's stack, for Expand / collapse.
    pub(super) fn active_stack(&self) -> Option<lf_catalog::stacks::Stack> {
        if self.mode != Mode::Library {
            return None;
        }
        self.selection
            .active()
            .and_then(|id| self.photo(id))
            .and_then(|p| p.stack)
    }

    pub(super) fn stack_selected(&mut self, env: &mut Env) {
        let Some(top) = self.selection.active() else {
            return;
        };
        let ids = self.selected_in_order();
        self.change_stacks(ids, Change::Stack { top }, env);
    }

    pub(super) fn unstack_selected(&mut self, env: &mut Env) {
        let ids = self.selected_in_order();
        self.change_stacks(ids, Change::Unstack, env);
    }

    pub(super) fn toggle_stack(&mut self, env: &mut Env) {
        let Some(stack) = self.active_stack() else {
            return;
        };
        // On screen at once; the catalogue follows.
        for row in &mut self.photos {
            if let Some(s) = row.stack.as_mut().filter(|s| s.id == stack.id) {
                s.collapsed = !stack.collapsed;
            }
        }
        self.arrange();
        self.change_stacks(
            Vec::new(),
            Change::Collapse {
                stack: stack.id,
                collapsed: !stack.collapsed,
            },
            env,
        );
    }

    fn change_stacks(&mut self, ids: Vec<i64>, change: Change, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Io, "stacks", move || {
            let result = Catalog::open(&root)
                .and_then(|c| match change {
                    Change::Stack { top } => c.stack(&ids, top).map(|_| None),
                    Change::Unstack => c.dissolve_stacks_of(&ids).map(|_| None),
                    Change::Collapse { stack, collapsed } => {
                        c.set_collapsed(stack, collapsed).map(|_| None)
                    }
                })
                .map_err(|e| e.to_string());
            Msg::Stacked {
                root,
                auto: false,
                result,
            }
        });
    }

    /// A stack change was saved (or not): the rows are read again; a message is a toast.
    pub(super) fn stacked(
        &mut self,
        root: std::path::PathBuf,
        result: Result<Option<String>, String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok(Some(text)) => env.toasts.push(Level::Info, text),
            Ok(None) => {}
            Err(e) => env
                .toasts
                .push(Level::Error, format!("The stack could not be saved. {e}")),
        }
        self.reload_photos(env);
    }

    /// Home → Auto-stack…: the selection when several photos are selected, else every photo
    /// of the folder (shown or not), in capture order.
    pub(super) fn open_auto_stack(&mut self) {
        let ids: Vec<i64> = if self.selection.current.len() > 1 {
            self.selected_in_order()
        } else {
            self.photos.iter().map(|p| p.id).collect()
        };
        if !ids.is_empty() {
            self.auto_stack = Some(AutoStackDialog::new(ids));
        }
    }

    pub(super) fn auto_stack_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.auto_stack else {
            return;
        };
        match dialog.show(ctx) {
            AutoStackAction::None => {}
            AutoStackAction::Close => self.auto_stack = None,
            AutoStackAction::Start(how) => {
                let Some(root) = self.root.clone() else {
                    return;
                };
                dialog.running = Some((0, 0));
                dialog.stop.store(false, Ordering::SeqCst);
                let ids = dialog.ids.clone();
                let stop = dialog.stop.clone();
                let gpu = self.gpu.clone();
                let reporter = env.jobs.reporter();
                env.jobs.spawn(Lane::Long, "auto-stack", move || {
                    let result = crate::stack_job::run(
                        gpu,
                        &root,
                        &ids,
                        how,
                        |done, total| reporter.send(Msg::StackProgress { done, total }),
                        &stop,
                    )
                    .map(|done| Some(done.text()));
                    Msg::Stacked {
                        root,
                        auto: true,
                        result,
                    }
                });
            }
        }
    }

    pub(super) fn stack_progress(&mut self, done: usize, total: usize) {
        if let Some(d) = self.auto_stack.as_mut().filter(|d| d.running.is_some()) {
            d.running = Some((done, total));
        }
    }

    /// The auto-stack run ended (its dialog closes).
    pub(super) fn auto_stack_done(&mut self) {
        if self
            .auto_stack
            .as_ref()
            .is_some_and(|d| d.running.is_some())
        {
            self.auto_stack = None;
        }
    }

    /// Quitting or changing the catalogue: a run stops after its current photo.
    pub(super) fn stop_auto_stack(&self) {
        if let Some(d) = &self.auto_stack {
            d.stop.store(true, Ordering::SeqCst);
        }
    }
}
