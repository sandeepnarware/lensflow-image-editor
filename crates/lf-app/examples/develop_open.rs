//! Manual check for the Develop view without a window (signing in needs an account): opens
//! each file given on the command line through the same `open_cached` + `renderer_for` path
//! the `Open` lane uses (twice for RAW: cold, then from the frame cache), then draws it the
//! way the preview does (`Renderer::draw` into a canvas the size of the photo's on-screen
//! rectangle) at fit, at 100 %, panned, and as it was (`\`), and makes the histogram.
//!
//! `cargo run -p lf-app --release --example develop_open -- <photo>...`

use lf_app::develop::open::{max_edge, open_cached};
use lf_app::develop::view::Viewport;
use lf_pipeline::Gpu;
use lf_pipeline::renderer::DrawOptions;
use lf_pipeline::texture::Target;
use lf_raw::cache::FrameCache;
use lf_raw::demosaic::Quality;
use lf_settings::{DevelopSettings, EditSettings};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if files.is_empty() {
        return Err("usage: develop_open <photo>...".into());
    }
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let info = gpu.adapter.get_info();
    log::info!(
        "adapter: {} (software: {}), working edge {} px",
        info.name,
        gpu.is_software(),
        max_edge(&gpu)
    );
    let frames = tempfile_dir()?;
    let cache = FrameCache::open(&frames, 4 << 30).map_err(|e| e.to_string())?;
    let settings = EditSettings::default_settings();
    let develop = DevelopSettings::from_settings(&settings);
    // A 1600×1000 physical-pixel preview panel.
    let panel = (1600.0, 1000.0);
    for path in &files {
        let passes: &[&str] = if opened_is_raw(path) {
            &["cold", "warm"]
        } else {
            &["8-bit, no cache"]
        };
        for (i, &pass) in passes.iter().enumerate() {
            let started = Instant::now();
            let opened = open_cached(path, Quality::Best, &develop, max_edge(&gpu), &cache)
                .map_err(|e| e.to_string())?;
            let mut renderer =
                lf_raw::render::renderer_for(&gpu, &opened).map_err(|e| e.to_string())?;
            log::info!(
                "opened {:?} {}×{} in {} ms ({pass}; working copy {}×{}, reduced: {})",
                opened.kind,
                opened.full_width,
                opened.full_height,
                started.elapsed().as_millis(),
                renderer.width,
                renderer.height,
                opened.reduced
            );
            drop(opened);
            if i + 1 == passes.len() {
                let (ow, oh) = renderer.output_size(&settings);
                let output = (f64::from(ow), f64::from(oh));
                let mut viewport = Viewport::default();
                let mut draw = |name: &str, viewport: &Viewport, before: bool| {
                    let p = viewport.placement(output, panel);
                    let (w, h) = (p.rect.2.round() as u32, p.rect.3.round() as u32);
                    let canvas = Target::new(&gpu, w, h, false).map_err(|e| e.to_string())?;
                    let t = Instant::now();
                    let opts = DrawOptions {
                        before,
                        crop: None,
                        view: p.view,
                        clipping: false,
                        peaking: false,
                        spots_view: false,
                        proof: false,
                    };
                    renderer
                        .draw(&settings, &opts, &canvas)
                        .map_err(|e| e.to_string())?;
                    gpu.device
                        .poll(eframe::wgpu::PollType::wait_indefinitely())
                        .map_err(|e| e.to_string())?;
                    log::info!(
                        "  draw {name}: {w}×{h} canvas, view {:.3},{:.3} {:.3}×{:.3}, {} ms",
                        p.view.x,
                        p.view.y,
                        p.view.w,
                        p.view.h,
                        t.elapsed().as_millis()
                    );
                    Ok::<_, String>(())
                };
                draw("fit", &viewport, false)?;
                viewport.toggle_fit_100(output, panel);
                draw("100 %", &viewport, false)?;
                viewport.pan(-400.0, -250.0, output, panel);
                draw("100 % panned", &viewport, false)?;
                draw("100 % panned, before (\\ held)", &viewport, true)?;
                viewport.zoom_at(2.0, (0.25, 0.75), output, panel);
                draw("200 % at the cursor", &viewport, false)?;
                let t = Instant::now();
                let h = renderer.histogram(&settings).map_err(|e| e.to_string())?;
                log::info!(
                    "  histogram: {} ms ({} px counted)",
                    t.elapsed().as_millis(),
                    h.l.iter().map(|&n| u64::from(n)).sum::<u64>()
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&frames);
    Ok(())
}

fn opened_is_raw(path: &std::path::Path) -> bool {
    lf_raw::open::sniff(path).is_ok_and(|k| k == lf_raw::open::Kind::Raw)
}

fn tempfile_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("lf-develop-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}
