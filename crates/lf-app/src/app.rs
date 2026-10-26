//! The `eframe::App`: owns the shared GPU, preferences, jobs, toasts and the session, and
//! draws the current screen. The session's rules live in `session.rs`, the screens in
//! `screens/`, the account calls in `account.rs`; this file wires them together.

use crate::jobs::{Jobs, Msg};
use crate::library::{self, Library};
use crate::prefs::{Dirs, Prefs};
use crate::screens::{self, View};
use crate::session::Session;
use crate::session_flow::{self, SessionFlow};
use crate::toast::Toasts;
use crate::{device_reset, gpu, theme};
use eframe::egui::{self, RichText};
use lf_account::entitlement::Decision;
use lf_pipeline::Gpu;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SOFTWARE_BANNER: &str = "No graphics driver found: previews will be slow.";
pub const DEVICE_LOST_BANNER: &str = "The graphics device was reset. Reopen the photo to continue.";

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

pub struct App {
    dirs: Dirs,
    /// The shared device, or why the window has none.
    gpu: Result<Arc<Gpu>, String>,
    prefs: Prefs,
    jobs: Jobs,
    toasts: Toasts,
    logo: Option<egui::TextureHandle>,
    lost_logged: bool,
    /// The device-reset answer was taken (the dialog's thread watches it), and whether to
    /// start again at the end of `on_exit`.
    exit: device_reset::Exit,
    ctx: egui::Context,
    flow: SessionFlow,
    /// The catalogue, grid and filmstrip (none without a graphics device).
    library: Option<Library>,
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

    pub fn new(cc: &eframe::CreationContext, dirs: Dirs) -> App {
        theme::apply(&cc.egui_ctx);
        let gpu = gpu::attach(cc.wgpu_render_state.as_ref());
        if let Err(e) = &gpu {
            log::error!("{e}");
        }
        let prefs = Prefs::load(&dirs.config);
        let flow = SessionFlow::new(&prefs, &dirs.config);
        let library = gpu.as_ref().ok().map(|gpu| {
            let mut library = Library::new(gpu.clone(), dirs.cache.join("frames"), &prefs);
            // The Develop preview registers its canvas with egui's renderer.
            library.set_render_state(cc.wgpu_render_state.clone());
            library
        });
        let mut app = App {
            library,
            gpu,
            prefs,
            jobs: Jobs::new(cc.egui_ctx.clone()),
            toasts: Toasts::default(),
            logo: load_logo(&cc.egui_ctx),
            lost_logged: false,
            exit: device_reset::Exit::default(),
            ctx: cc.egui_ctx.clone(),
            flow,
            dirs,
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
        let mut env = library::Env {
            jobs: &self.jobs,
            toasts: &mut self.toasts,
            prefs: &mut self.prefs,
            config_dir: &self.dirs.config,
        };
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
        let action = if matches!(self.flow.session, Session::Active { .. }) {
            let top = screens::active_top(ui, &self.flow.session, &view);
            let status = screens::status_bar(ui, &self.flow.session, &view);
            match &mut self.library {
                Some(lib) => {
                    let mut env = library::Env {
                        jobs: &self.jobs,
                        toasts: &mut self.toasts,
                        prefs: &mut self.prefs,
                        config_dir: &self.dirs.config,
                    };
                    lib.ui(ui, &mut env);
                }
                None => {
                    egui::CentralPanel::default_margins()
                        .show(ui, |ui| self.signed_in_placeholder(ui));
                }
            }
            top.or(status)
        } else {
            egui::CentralPanel::default_margins()
                .show(ui, |ui| screens::show(ui, &mut self.flow.session, &view))
                .inner
        };
        if let Some(action) = action {
            let mut env = flow_env!(self);
            self.flow.act(action, &mut env);
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
        self.flow.tick(Instant::now(), &self.jobs);
        if matches!(self.flow.session, Session::Active { .. }) {
            // The status bar's "Last checked" and the next scheduled check.
            ctx.request_repaint_after(Duration::from_secs(60));
        }
        if let Ok(gpu) = &self.gpu {
            if let Some(error) = gpu.take_error() {
                log::error!("graphics error: {error}");
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
                if gpu.is_lost() {
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
                } else if gpu.is_software() {
                    banner(ui, "software", SOFTWARE_BANNER, theme::WARNING);
                }
                self.session_ui(ui);
            }
        }
        self.toasts.show(ui.ctx());
    }

    /// Review Focus 4: a gesture in progress is committed, a running import stops after its
    /// batch (each batch commits), the lanes finish their running jobs, then the waiting
    /// sidecars are written here, synchronously. A restart asked for starts last.
    fn on_exit(&mut self) {
        if let Some(lib) = &mut self.library {
            lib.finish_edits(&mut self.toasts);
            lib.stop_import();
        }
        self.jobs.shutdown(Duration::from_secs(10));
        if let Some(lib) = &self.library
            && let Err(e) = lib.sidecar_saver().flush()
        {
            log::error!("an edit file could not be written before quitting: {e}");
        }
        self.save_prefs();
        self.exit.finish();
    }

    /// Only the window's size and position persist (in `window.ron`), not egui's memory.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}
