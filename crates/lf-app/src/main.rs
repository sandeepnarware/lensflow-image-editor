// Release builds on Windows open no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use lf_app::app::App;
use lf_app::prefs::Dirs;
use lf_app::{gpu, logging, startup, theme};
use rfd::MessageLevel;

fn main() -> eframe::Result {
    let dirs = Dirs::locate();
    let log = match logging::init(&dirs.data) {
        Ok(path) => Some(path),
        Err(e) => {
            eprintln!("{e}");
            if startup::NO_CONSOLE {
                startup::tell(MessageLevel::Warning, &startup::log_failed_text(&e));
            }
            None
        }
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("lensflow-ImageEditor")
        .with_app_id("lensflow-image-editor")
        .with_inner_size([1400.0, 900.0])
        .with_min_inner_size([1024.0, 640.0]);
    match eframe::icon_data::from_png_bytes(theme::LOGO_PNG) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => log::warn!("could not decode the window icon: {e}"),
    }
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        persistence_path: Some(dirs.config.join("window.ron")),
        ..Default::default()
    };
    gpu::request_full_limits(&mut options.wgpu_options);
    let result = eframe::run_native(
        "lensflow-ImageEditor",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, dirs)))),
    );
    if let Err(e) = &result {
        log::error!("the window could not open: {e}");
        startup::tell(
            MessageLevel::Error,
            &startup::failed_text(startup::reason(e), log.as_deref()),
        );
    }
    result
}
