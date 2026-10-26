// Release builds on Windows open no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use lf_app::app::App;
use lf_app::prefs::Dirs;
use lf_app::{cli, gpu, logging, single_instance, startup, theme};
use rfd::MessageLevel;

fn main() -> eframe::Result {
    // `--version` and `--help` answer before any window or graphics work.
    match cli::parse(std::env::args_os().skip(1)) {
        cli::Cli::Run => {}
        other => std::process::exit(cli::answer(&other)),
    }
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

    // One app at a time: a second launch says so and exits.
    let lock =
        match single_instance::acquire_waiting(&dirs.config, single_instance::wait_for_launch()) {
            Ok(guard) => Some(guard),
            Err(single_instance::Refused::AlreadyRunning) => {
                log::info!("another lensflow-ImageEditor is open; this launch exits");
                startup::tell(MessageLevel::Info, single_instance::ALREADY_OPEN);
                return Ok(());
            }
            Err(single_instance::Refused::Io(e)) => {
                log::warn!(
                    "could not take the instance lock, so a second launch is not stopped: {e}"
                );
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
        Box::new(move |cc| Ok(Box::new(App::new(cc, dirs, lock)))),
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
