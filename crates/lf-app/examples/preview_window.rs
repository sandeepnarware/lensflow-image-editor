//! Manual check of the shared-texture preview inside a real eframe window (signing in needs
//! an account, so the app's own Develop view cannot be reached without one): opens a photo
//! on egui's device, draws it with `Renderer::draw` into the `Preview` canvas registered with
//! egui-wgpu, shows it, holds "before" on the third frame, takes a screenshot of the window
//! and closes.
//!
//! `cargo run -p lf-app --example preview_window -- <photo> <screenshot.png>`

use eframe::egui;
use lf_app::develop::open::max_edge;
use lf_app::develop::preview::Preview;
use lf_app::develop::view::Viewport;
use lf_pipeline::renderer::DrawOptions;
use lf_pipeline::{Gpu, Renderer};
use lf_raw::demosaic::Quality;
use lf_settings::{DevelopSettings, EditSettings};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

struct Check {
    gpu: Arc<Gpu>,
    renderer: Renderer,
    settings: EditSettings,
    preview: Preview,
    frame: u32,
    out: PathBuf,
}

impl eframe::App for Check {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            let ppp = ui.ctx().pixels_per_point();
            let panel = (
                f64::from(rect.width() * ppp),
                f64::from(rect.height() * ppp),
            );
            let (ow, oh) = self.renderer.output_size(&self.settings);
            let output = (f64::from(ow), f64::from(oh));
            let p = Viewport::default().placement(output, panel);
            let size = (p.rect.2.round() as u32, p.rect.3.round() as u32);
            let (canvas, texture, fresh) = self
                .preview
                .ensure(&self.gpu, size.0, size.1)
                .expect("canvas");
            let before = self.frame == 3;
            let t = Instant::now();
            let opts = DrawOptions {
                before,
                crop: None,
                view: p.view,
                clipping: false,
            };
            self.renderer
                .draw(&self.settings, &opts, canvas)
                .expect("draw");
            log::info!(
                "frame {}: drew {}×{} (fresh canvas: {fresh}, before: {before}) in {} ms",
                self.frame,
                size.0,
                size.1,
                t.elapsed().as_millis()
            );
            let shown = egui::Rect::from_min_size(
                rect.min + egui::vec2(p.rect.0 as f32 / ppp, p.rect.1 as f32 / ppp),
                egui::vec2(p.rect.2 as f32 / ppp, p.rect.3 as f32 / ppp),
            );
            ui.painter().image(
                texture,
                shown,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        });
        self.frame += 1;
        let ctx = ui.ctx().clone();
        if self.frame == 2 {
            let out = self.out.clone();
            let closer = ctx.clone();
            ctx.request_screenshot(move |image| {
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                let saved =
                    image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, rgba)
                        .map(|img| img.save(&out));
                log::info!("screenshot {}: {saved:?}", out.display());
                closer.request_repaint();
            });
        }
        if self.frame > 8 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(photo), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: preview_window <photo> <screenshot.png>");
        std::process::exit(2);
    };
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 700.0]),
        ..Default::default()
    };
    lf_app::gpu::request_full_limits(&mut options.wgpu_options);
    eframe::run_native(
        "preview check",
        options,
        Box::new(move |cc| {
            let gpu = lf_app::gpu::attach(cc.wgpu_render_state.as_ref())?;
            let settings = EditSettings::default_settings();
            let develop = DevelopSettings::from_settings(&settings);
            let t = Instant::now();
            let opened = lf_raw::open::open(&photo, Quality::Best, &develop, max_edge(&gpu))?;
            let renderer = lf_raw::render::renderer_for(&gpu, &opened)?;
            log::info!(
                "opened {:?} {}×{} in {} ms",
                opened.kind,
                opened.full_width,
                opened.full_height,
                t.elapsed().as_millis()
            );
            let render_state = cc.wgpu_render_state.clone().ok_or("no render state")?;
            Ok(Box::new(Check {
                gpu,
                renderer,
                settings,
                preview: Preview::new(render_state),
                frame: 0,
                out,
            }))
        }),
    )
}
