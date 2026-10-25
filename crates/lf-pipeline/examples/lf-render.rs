//! `lf-render <in.png> <settings.json> <out.png>`: renders a photo with edit settings.

use lf_pipeline::{Gpu, Renderer};
use lf_settings::normalise;

fn run(args: &[String]) -> Result<(), String> {
    let [input, settings, output] = args else {
        return Err("usage: lf-render <in.png> <settings.json> <out.png>".into());
    };
    let image = image::open(input)
        .map_err(|e| format!("Could not open {input}: {e}"))?
        .to_rgba8();
    let json =
        std::fs::read_to_string(settings).map_err(|e| format!("Could not read {settings}: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("Bad settings in {settings}: {e}"))?;
    let s = normalise(&json);
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let mut renderer =
        Renderer::new(&gpu, &image, image.width(), image.height()).map_err(|e| e.to_string())?;
    let out = renderer.render(&s).map_err(|e| e.to_string())?;
    out.save(output)
        .map_err(|e| format!("Could not write {output}: {e}"))?;
    println!("output {}x{}", out.width(), out.height());
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
