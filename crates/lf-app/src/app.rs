//! The `eframe::App`: owns the shared GPU, preferences, jobs and toasts, and draws the
//! current screen.

use crate::jobs::{Jobs, Msg};
use crate::prefs::{Dirs, Prefs};
use crate::toast::{Level, Toasts};
use crate::{gpu, theme};
use eframe::egui::{self, RichText};
use lf_pipeline::Gpu;
use std::sync::Arc;

pub const SOFTWARE_BANNER: &str = "No graphics driver found: previews will be slow.";
pub const DEVICE_LOST_BANNER: &str = "The graphics device was reset. Reopen the photo to continue.";

pub struct App {
    dirs: Dirs,
    /// The shared device, or why the window has none.
    gpu: Result<Arc<Gpu>, String>,
    prefs: Prefs,
    jobs: Jobs,
    toasts: Toasts,
    logo: Option<egui::TextureHandle>,
    lost_logged: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, dirs: Dirs) -> App {
        theme::apply(&cc.egui_ctx);
        let gpu = gpu::attach(cc.wgpu_render_state.as_ref());
        if let Err(e) = &gpu {
            log::error!("{e}");
        }
        let prefs = Prefs::load(&dirs.config);
        App {
            gpu,
            prefs,
            jobs: Jobs::new(cc.egui_ctx.clone()),
            toasts: Toasts::default(),
            logo: load_logo(&cc.egui_ctx),
            lost_logged: false,
            dirs,
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::JobFailed { name, message } => {
                self.toasts.push(
                    Level::Error,
                    format!("Something went wrong ({name}). {message}"),
                );
            }
            #[cfg(test)]
            Msg::Probe(_) => {}
        }
    }

    fn placeholder(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            if let Some(logo) = &self.logo {
                ui.image(egui::load::SizedTexture::new(logo.id(), [96.0, 96.0]));
                ui.add_space(16.0);
            }
            ui.heading("Sign in to begin.");
        });
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
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for msg in self.jobs.drain() {
            self.handle(msg);
        }
        if let Ok(gpu) = &self.gpu {
            if let Some(error) = gpu.take_error() {
                log::error!("graphics error: {error}");
            }
            if gpu.is_lost() && !self.lost_logged {
                log::error!("the graphics device was lost");
                self.lost_logged = true;
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
                    banner(ui, "device-lost", DEVICE_LOST_BANNER, theme::DANGER);
                } else if gpu.is_software() {
                    banner(ui, "software", SOFTWARE_BANNER, theme::WARNING);
                }
                egui::CentralPanel::default_margins().show(ui, |ui| self.placeholder(ui));
            }
        }
        self.toasts.show(ui.ctx());
    }

    fn on_exit(&mut self) {
        if let Err(e) = self.prefs.save(&self.dirs.config) {
            log::error!("could not save preferences: {e}");
        }
    }

    /// Only the window's size and position persist (in `window.ron`), not egui's memory.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}
