//! `lf-render <in> <settings.json> <out.png> [--develop camera|linear]
//! [--highlights clip|reconstruct] [--quality fast|best|preview]`: renders a photo (RAW, JPEG,
//! PNG, TIFF or WebP) with edit settings. The develop flags override `desktop.develop`
//! in the settings file.

use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, open};
use lf_raw::render::renderer_for;
use lf_settings::{DevelopSettings, Highlights, Profile, normalise};
use std::path::Path;

const USAGE: &str = "usage: lf-render <in> <settings.json> <out.png> [--develop camera|linear] \
[--highlights clip|reconstruct] [--quality fast|best|preview]";

fn run(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let (mut profile, mut highlights, mut quality) = (None, None, Quality::Best);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with("--") {
            let v = it
                .next()
                .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))?;
            match (a.as_str(), v.as_str()) {
                ("--develop", "camera") => profile = Some(Profile::Camera),
                ("--develop", "linear") => profile = Some(Profile::Linear),
                ("--highlights", "clip") => highlights = Some(Highlights::Clip),
                ("--highlights", "reconstruct") => highlights = Some(Highlights::Reconstruct),
                ("--quality", "fast") => quality = Quality::Fast,
                ("--quality", "best") => quality = Quality::Best,
                ("--quality", "preview") => quality = Quality::Preview,
                _ => return Err(format!("Unknown option {a} {v}\n{USAGE}")),
            }
        } else {
            positional.push(a.as_str());
        }
    }
    let [input, settings, output] = positional[..] else {
        return Err(USAGE.into());
    };
    let json =
        std::fs::read_to_string(settings).map_err(|e| format!("Could not read {settings}: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("Bad settings in {settings}: {e}"))?;
    let s = normalise(&json);
    let mut develop = DevelopSettings::from_settings(&s);
    develop.profile = profile.unwrap_or(develop.profile);
    develop.highlights = highlights.unwrap_or(develop.highlights);

    // The device first: its largest texture is the cap `open` develops to.
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let opened =
        open(Path::new(input), quality, &develop, gpu.max_edge()).map_err(|e| e.to_string())?;
    let mut renderer = renderer_for(&gpu, &opened).map_err(|e| e.to_string())?;
    let out = renderer.render(&s).map_err(|e| e.to_string())?;
    out.save(output)
        .map_err(|e| format!("Could not write {output}: {e}"))?;
    let kind = match opened.kind {
        Kind::Raw => "raw",
        Kind::Jpeg => "jpeg",
        Kind::Png => "png",
        Kind::Tiff => "tiff",
        Kind::WebP => "webp",
        Kind::Heic | Kind::Unknown => "other",
    };
    println!("kind {kind}");
    println!("output {}x{}", out.width(), out.height());
    let mut sum = [0u64; 3];
    for p in out.pixels() {
        for (acc, v) in sum.iter_mut().zip(p.0) {
            *acc += u64::from(v);
        }
    }
    let n = u64::from(out.width()) * u64::from(out.height());
    let mean = sum.map(|v| v as f64 / n.max(1) as f64);
    println!("mean rgb {:.1} {:.1} {:.1}", mean[0], mean[1], mean[2]);
    if opened.reduced {
        println!("reduced from {}x{}", opened.full_width, opened.full_height);
    }
    println!("adapter {}", gpu.adapter.get_info().name);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(message) = run(&args) {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
