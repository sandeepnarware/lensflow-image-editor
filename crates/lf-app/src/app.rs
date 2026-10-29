//! The `eframe::App`: owns the shared GPU, preferences, jobs, toasts and the session, and
//! draws the current screen. The session's rules live in `session.rs`, the screens in
//! `screens/`, the account calls in `account.rs`; this file wires them together.
//!
//! The window has one menu bar (`menu.rs`) and one status bar (the library's counts and
//! progress, then the studio's standing). Menu items and keys are the same [`Action`]s: the
//! keyboard map (`shortcuts.rs`) is read once per frame in `logic()`, the library does its
//! own actions, and the app does the rest (Quit, Preferences, the Account and Help menus).

use crate::diagnostics::{self, GpuFacts};
use crate::dialogs::preferences::{self, PrefsAction, PrefsView};
use crate::dialogs::{about, keys, quit};
use crate::jobs::{Jobs, Lane, Msg};
use crate::library::{self, Library};
use crate::menu::{self, MenuState};
use crate::prefs::{Dirs, Prefs};
use crate::screens::{self, View};
use crate::session::Session;
use crate::session_flow::{self, SessionFlow};
use crate::shortcuts::{Action, Keys};
use crate::toast::{Level, Toasts};
use crate::{device_reset, gpu, logging, platform, theme};
use eframe::egui::{self, RichText};
use eframe::egui_wgpu::RenderState;
use lf_account::entitlement::Decision;
use lf_pipeline::Gpu;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SOFTWARE_BANNER: &str = "No graphics driver found: previews will be slow. Installing or \
     updating the graphics driver (and not running over Remote Desktop) makes them fast.";
pub const DEVICE_LOST_BANNER: &str =
    "The graphics device was reset. Restart lensflow-ImageEditor to continue.";
/// The library's banner when a graphics error (not a lost device) stopped the thumbnails.
pub const THUMBS_STOPPED_BANNER: &str = "Thumbnails stopped because of a graphics error. They \
     will restart when you change folders.";

/// This build's version, sent to the server and compared with its releases.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The session flow's view of the app around it (a macro so the borrows stay disjoint).
macro_rules! flow_env {
    ($app:expr) => {
        session_flow::Env {
            jobs: &$app.jobs,
            toasts: &mut $app.toasts,
            prefs: &mut $app.prefs,
            dirs: &$app.dirs,
            ctx: &$app.ctx,
        }
    };
}

/// The library's view of the app around it (a macro so the borrows stay disjoint).
macro_rules! library_env {
    ($app:expr) => {
        library::Env {
            jobs: &$app.jobs,
            toasts: &mut $app.toasts,
            prefs: &mut $app.prefs,
            config_dir: &$app.dirs.config,
        }
    };
}

/// The window's own dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialog {
    About,
    Keys,
    Preferences,
}

pub struct App {
    dirs: Dirs,
    /// The shared device, or why the window has none.
    gpu: Result<Arc<Gpu>, String>,
    /// egui's renderer, for the Develop preview's texture (a restarted library needs it too).
    render_state: Option<RenderState>,
    prefs: Prefs,
    jobs: Jobs,
    toasts: Toasts,
    logo: Option<egui::TextureHandle>,
    lost_logged: bool,
    /// Uncaptured graphics errors (the library logs them; a burst shows a toast).
    gpu_errors: gpu::ErrorTap,
    /// The device-reset answer was taken (the dialog's thread watches it), and whether to
    /// start again at the end of `on_exit`.
    exit: device_reset::Exit,
    ctx: egui::Context,
    flow: SessionFlow,
    /// The catalogue, grid and filmstrip (none without a graphics device).
    library: Option<Library>,
    dialog: Option<Dialog>,
    /// The keyboard map's reader before a studio is open (the library has its own).
    keys: Keys,
    /// Asking before closing while an export or a batch runs (the question shown).
    quit_question: Option<&'static str>,
    /// The user chose Quit anyway: the next close goes through.
    quit_confirmed: bool,
}

impl App {
    /// The gate's last answer: AI, preset sync and gallery export read it.
    pub fn gate(&self) -> Option<&Decision> {
        self.flow.gate()
    }

    /// Checks the entitlement now (before an AI call or an export); the answer arrives as
    /// `Msg::Gate` and updates [`App::gate`].
    pub fn refresh_now(&mut self, reason: &str) {
        self.flow.refresh_now(&self.jobs, reason);
    }

    /// `lock` is the single-instance lock, held until the end of `on_exit`.
    pub fn new(
        cc: &eframe::CreationContext,
        dirs: Dirs,
        lock: Option<crate::single_instance::Guard>,
    ) -> App {
        theme::apply(&cc.egui_ctx);
        let gpu = gpu::attach(cc.wgpu_render_state.as_ref());
        if let Err(e) = &gpu {
            log::error!("{e}");
        }
        let prefs = Prefs::load(&dirs.config);
        let flow = SessionFlow::new(&prefs, &dirs.config);
        let render_state = cc.wgpu_render_state.clone();
        let library = gpu.as_ref().ok().map(|gpu| {
            let mut library = Library::new(gpu.clone(), dirs.cache.join("frames"), &prefs);
            // The Develop preview registers its canvas with egui's renderer.
            library.set_render_state(render_state.clone());
            library
        });
        let mut app = App {
            library,
            gpu,
            render_state,
            prefs,
            jobs: Jobs::new(cc.egui_ctx.clone()),
            toasts: Toasts::default(),
            logo: load_logo(&cc.egui_ctx),
            lost_logged: false,
            gpu_errors: gpu::ErrorTap::default(),
            exit: device_reset::Exit {
                lock,
                ..Default::default()
            },
            ctx: cc.egui_ctx.clone(),
            flow,
            dirs,
            dialog: None,
            keys: Keys::default(),
            quit_question: None,
            quit_confirmed: false,
        };
        app.flow.resume(&app.jobs);
        app
    }

    fn save_prefs(&self) {
        if let Err(e) = self.prefs.save(&self.dirs.config) {
            log::error!("could not save preferences: {e}");
        }
    }

    fn handle(&mut self, msg: Msg) {
        if let Msg::DeviceReset(choice) = msg {
            self.device_reset(choice);
            return;
        }
        let mut env = library_env!(self);
        let msg = match &mut self.library {
            Some(lib) => match lib.handle(msg, &mut env) {
                Some(msg) => msg,
                None => return,
            },
            None => msg,
        };
        let mut env = flow_env!(self);
        if let Some(msg) = self.flow.handle(msg, &mut env) {
            // The library's own messages reach here only when there is no library.
            log::debug!("message left unhandled: {msg:?}");
        }
    }

    /// The device-reset dialog (or the banner's button) answered. The answer is marked
    /// taken first, so the dialog's thread never ends the process while this window runs;
    /// a restart happens at the end of `on_exit`, after the jobs stop and the sidecars are
    /// written.
    fn device_reset(&mut self, choice: device_reset::Choice) {
        self.exit.answer(choice);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// What the device-reset dialog's thread does if this window never takes its answer.
    fn force_exit(&self) -> device_reset::ForceExit {
        device_reset::ForceExit {
            stop_import: self.library.as_ref().map(Library::import_stop_flag),
            saver: self.library.as_ref().map(Library::sidecar_saver),
        }
    }

    fn view(&self) -> View {
        self.flow
            .view(self.logo.as_ref().map(|logo| logo.id()), &self.prefs)
    }

    fn active(&self) -> bool {
        matches!(self.flow.session, Session::Active { .. })
    }

    /// The library, while a studio is open (its menus and keys work only then).
    fn active_library(&self) -> Option<&Library> {
        self.library.as_ref().filter(|_| self.active())
    }

    /// Whether `action` is possible now.
    fn enabled(&self, action: Action) -> bool {
        menu::ALWAYS.contains(&action)
            || menu::account_actions(&self.flow.session, self.view().can_switch).contains(&action)
            || self.active_library().is_some_and(|lib| lib.enabled(action))
    }

    fn menu_state(&self) -> MenuState {
        let enabled = menu::actions()
            .into_iter()
            .filter(|action| self.enabled(*action))
            .collect();
        MenuState {
            enabled,
            studio: match &self.flow.session {
                Session::Active { studio, .. } => Some(studio.name.clone()),
                _ => None,
            },
            before: self
                .active_library()
                .is_some_and(|lib| lib.develop().before()),
            clipping: self
                .active_library()
                .is_some_and(|lib| lib.develop().clipping()),
            peaking: self
                .active_library()
                .is_some_and(|lib| lib.develop().peaking()),
            spots_view: self
                .active_library()
                .is_some_and(|lib| lib.develop().spots_view()),
            proofing: self
                .active_library()
                .is_some_and(|lib| lib.develop().proofing()),
            reference: self
                .active_library()
                .is_some_and(|lib| lib.reference_shown()),
            // The ribbon brings the view's own tab forward when the view changes.
            context: self
                .active_library()
                .filter(|lib| lib.stage == crate::library::Stage::Ready)
                .map(|lib| match lib.mode {
                    crate::library::Mode::Develop(_) => menu::DEVELOP,
                    crate::library::Mode::Library => menu::LIBRARY,
                }),
        }
    }

    /// Does a menu item's or a key's action.
    fn run(&mut self, action: Action) {
        match action {
            Action::Quit => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Preferences => self.dialog = Some(Dialog::Preferences),
            Action::About => self.dialog = Some(Dialog::About),
            Action::KeyboardShortcuts => self.dialog = Some(Dialog::Keys),
            Action::ShowLogFolder => self.show_logs(),
            Action::SwitchStudio => self.session_action(screens::Action::SwitchStudio),
            Action::SignOut => self.session_action(screens::Action::SignOut),
            Action::CheckEntitlement => {
                self.flow
                    .refresh_now(&self.jobs, "a check asked for in the Account menu");
            }
            other => {
                if !self.active() {
                    return;
                }
                let ctx = self.ctx.clone();
                let mut env = library_env!(self);
                if let Some(lib) = &mut self.library {
                    lib.run(other, &ctx, &mut env);
                }
            }
        }
    }

    fn session_action(&mut self, action: screens::Action) {
        let mut env = flow_env!(self);
        self.flow.act(action, &mut env);
    }

    fn show_logs(&mut self) {
        let file = logging::dir(&self.dirs.data).join("app.log");
        if let Err(e) = platform::show_in_folder(&file) {
            self.toasts.push(
                Level::Error,
                format!("The log folder could not be shown. {e}"),
            );
        }
    }

    /// The keyboard map: the library's keys while a studio is open, and `F1` anywhere.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let dialog = self.dialog.is_some();
        let app_key = |a: Action| a == Action::About && !dialog;
        let active = self.active();
        let fired = match &mut self.library {
            Some(lib) if active => {
                let mut env = library_env!(self);
                lib.shortcuts(ctx, &mut env, app_key)
            }
            _ => self.keys.read(ctx, false, app_key),
        };
        for action in fired {
            self.run(action);
        }
    }

    /// Preferences → Change…: the library starts again on the first-run screen (it can go
    /// back to this catalogue). The open photo's gesture is committed, an import stops after
    /// its batch, the waiting edit files are written, and the old thumbnailer and catalogue
    /// connections are dropped.
    fn change_catalogue(&mut self) {
        let (Ok(gpu), Some(lib)) = (&self.gpu, &mut self.library) else {
            return;
        };
        let Some((keep, generation)) = lib.prepare_restart(&mut self.toasts) else {
            return;
        };
        let mut fresh = Library::new(gpu.clone(), self.dirs.cache.join("frames"), &self.prefs);
        fresh.set_render_state(self.render_state.clone());
        fresh.choose_another_root(keep, generation);
        self.library = Some(fresh);
        self.dialog = None;
        log::info!("the library restarted to choose another catalogue folder");
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        match self.dialog {
            None => {}
            Some(Dialog::About) => {
                let log_dir = logging::dir(&self.dirs.data);
                let gpu = self.gpu.as_deref().map(GpuFacts::of);
                let diagnostics = diagnostics::text(
                    gpu.as_ref().map_err(|e| e.as_str()),
                    &self.dirs,
                    &self.prefs,
                );
                let view = about::AboutView {
                    logo: self.logo.as_ref().map(|l| l.id()),
                    log_dir: &log_dir,
                    diagnostics: &diagnostics,
                };
                match about::show(ctx, &view) {
                    None => {}
                    Some(about::AboutAction::Close) => self.dialog = None,
                    Some(about::AboutAction::ShowLogs) => self.show_logs(),
                    Some(about::AboutAction::Copied) => {
                        self.toasts.push(Level::Success, about::COPIED);
                    }
                }
            }
            Some(Dialog::Keys) => {
                if keys::show(ctx) {
                    self.dialog = None;
                }
            }
            Some(Dialog::Preferences) => {
                let root = self
                    .library
                    .as_ref()
                    .and_then(|lib| lib.root().or(lib.kept_root()))
                    .map(Path::to_path_buf)
                    .or_else(|| self.prefs.catalogue_root.clone());
                let blocked = match &self.library {
                    None => Some("The catalogue needs the graphics device."),
                    Some(lib) => lib.restart_blocked(),
                };
                let log_dir = logging::dir(&self.dirs.data);
                let view = PrefsView {
                    root: root.as_deref(),
                    change_blocked: blocked,
                    log_dir: &log_dir,
                };
                match preferences::show(ctx, &mut self.prefs, &view) {
                    None => {}
                    Some(PrefsAction::Close) => self.dialog = None,
                    Some(PrefsAction::Changed) => self.save_prefs(),
                    Some(PrefsAction::ShowLogs) => self.show_logs(),
                    Some(PrefsAction::ChangeRoot) => self.change_catalogue(),
                }
            }
        }
    }

    /// The window was asked to close while an export or a batch runs: ask first (a restart
    /// after a device reset never asks).
    fn check_close(&mut self, ctx: &egui::Context) {
        let asked = ctx.input(|i| i.viewport().close_requested());
        let resetting = self.exit.answered.load(std::sync::atomic::Ordering::SeqCst);
        if !asked || self.quit_confirmed || resetting {
            return;
        }
        if let Some(question) = self.library.as_ref().and_then(Library::quit_question) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_question = Some(question);
        }
    }

    /// The question asked by [`App::check_close`]: Quit stops the export or batch after the
    /// current photo and closes; Keep working leaves everything running.
    fn quit_dialog(&mut self, ctx: &egui::Context) {
        let Some(question) = self.quit_question else {
            return;
        };
        match quit::show(ctx, question) {
            None => {}
            Some(false) => self.quit_question = None,
            Some(true) => {
                self.quit_question = None;
                self.quit_confirmed = true;
                if let Some(lib) = &self.library {
                    lib.stop_long_work();
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The signed-in screen until the library arrives.
    fn signed_in_placeholder(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            if let Some(logo) = &self.logo {
                ui.image(egui::load::SizedTexture::new(logo.id(), [96.0, 96.0]));
                ui.add_space(16.0);
            }
            if let Session::Active { studio, .. } = &self.flow.session {
                ui.heading(format!("Signed in to {}.", studio.name));
            }
        });
    }

    /// The session's screen, or the chrome around the signed-in view.
    fn session_ui(&mut self, ui: &mut egui::Ui) {
        let view = self.view();
        let action = if self.active() {
            screens::read_only_banner(ui, &self.flow.session);
            let library = self.library.as_ref();
            let status = screens::status_bar(ui, &self.flow.session, &view, |ui| {
                library.is_some_and(|lib| lib.status_ui(ui))
            });
            match &mut self.library {
                Some(lib) => {
                    let mut env = library_env!(self);
                    lib.ui(ui, &mut env);
                }
                None => {
                    egui::CentralPanel::default_margins()
                        .show(ui, |ui| self.signed_in_placeholder(ui));
                }
            }
            status
        } else {
            egui::CentralPanel::default_margins()
                .show(ui, |ui| screens::show(ui, &mut self.flow.session, &view))
                .inner
        };
        if let Some(action) = action {
            self.session_action(action);
        }
    }
}

fn banner(ui: &mut egui::Ui, id: &str, text: &str, colour: egui::Color32) {
    egui::Panel::top(id).show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(text).color(colour));
        ui.add_space(4.0);
    });
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = match image::load_from_memory(theme::LOGO_PNG) {
        Ok(image) => image.to_rgba8(),
        Err(e) => {
            log::warn!("could not decode the logo: {e}");
            return None;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture("logo", pixels, egui::TextureOptions::LINEAR))
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for msg in self.jobs.drain() {
            self.handle(msg);
        }
        self.check_close(ctx);
        self.flow.tick(Instant::now(), &self.jobs);
        self.shortcuts(ctx);
        if let Some(lib) = &mut self.library {
            lib.set_studio(self.flow.presets_key());
            // Match a reference: its cost line reads the gate; a fit asked for gets the AI
            // link, whose fresh check before each AI request goes through the session.
            lib.set_match_gate(self.flow.gate());
            lib.set_local_ai(
                self.flow
                    .gate()
                    .is_some_and(lf_account::entitlement::Decision::local_ai_allowed),
            );
            lib.set_gallery_link(self.flow.gallery_link());
            if let Some(shown) = lib.take_match_run() {
                lib.start_match(self.flow.ai_link(&self.jobs), shown, &self.jobs);
            }
            if lib.take_presets_changed() {
                let root = lib.root().map(std::path::Path::to_path_buf);
                self.flow.sync_presets(&self.jobs, root.as_deref());
            }
        }
        if self.active() {
            // The status bar's "Last checked" and the next scheduled check.
            ctx.request_repaint_after(Duration::from_secs(60));
        }
        if let Ok(gpu) = &self.gpu {
            // The library's handler has logged it; a burst is worth a word on screen.
            if gpu.take_error().is_some() && self.gpu_errors.record(Instant::now()) {
                self.toasts.push(Level::Warning, gpu::ERROR_BURST);
            }
            if gpu.is_lost() && !self.lost_logged {
                // egui's own drawing may fail from here on, so the banner may never show:
                // ask through a native message box as well.
                log::error!("the graphics device was lost");
                self.lost_logged = true;
                device_reset::ask(
                    self.jobs.reporter(),
                    self.exit.answered.clone(),
                    self.force_exit(),
                );
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match &self.gpu {
            Err(message) => {
                egui::CentralPanel::default_margins().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        ui.heading("lensflow-ImageEditor cannot start.");
                        ui.label(message.as_str());
                        ui.add_space(12.0);
                        if ui.button("Quit").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            }
            Ok(gpu) => {
                let lost = gpu.is_lost();
                let software = gpu.is_software();
                let state = self.menu_state();
                let clicked = egui::Panel::top("ribbon")
                    .show(ui, |ui| menu::bar(ui, &state))
                    .inner;
                if let Some(action) = clicked {
                    self.run(action);
                }
                if lost {
                    let restart = egui::Panel::top("device-lost")
                        .show(ui, |ui| {
                            ui.add_space(4.0);
                            let clicked = ui
                                .horizontal(|ui| {
                                    ui.label(
                                        RichText::new(DEVICE_LOST_BANNER).color(theme::DANGER),
                                    );
                                    ui.button(device_reset::RESTART).clicked()
                                })
                                .inner;
                            ui.add_space(4.0);
                            clicked
                        })
                        .inner;
                    if restart {
                        self.device_reset(device_reset::Choice::Restart);
                    }
                } else if software {
                    banner(ui, "software", SOFTWARE_BANNER, theme::WARNING);
                }
                self.session_ui(ui);
                self.dialogs(ui.ctx());
                self.quit_dialog(ui.ctx());
            }
        }
        self.toasts.show(ui.ctx());
    }

    /// Review Focus 4: a gesture in progress is committed, a running import stops after its
    /// batch (each batch commits), an export or a batch after the current photo, the lanes
    /// finish their running jobs, then the waiting sidecars are written here, synchronously.
    /// An export still writing at the deadline has its unfinished files removed. Last, the
    /// single-instance lock is let go and a verified installer, or a restart asked for,
    /// starts (`Exit::finish`).
    fn on_exit(&mut self) {
        let mut exporting_to = None;
        if let Some(lib) = &mut self.library {
            lib.finish_edits(&mut self.toasts);
            lib.stop_long_work();
            exporting_to = lib.running_export_folder();
        }
        let abandoned = self.jobs.shutdown(Duration::from_secs(10));
        if let Some(folder) = exporting_to
            && abandoned.contains(&Lane::Long)
        {
            crate::export_job::remove_unfinished(&folder);
        }
        if let Some(lib) = &self.library
            && let Err(e) = lib.sidecar_saver().flush()
        {
            log::error!("an edit file could not be written before quitting: {e}");
        }
        self.save_prefs();
        if let Some(installer) = self.flow.take_installer() {
            self.exit.installer = Some(installer);
        }
        self.exit.finish();
    }

    /// Only the window's size and position persist (in `window.ron`), not egui's memory.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}
