//! Smoke tests over the real-camera CC0 corpus in `tests/raw-samples` (fetched by
//! `scripts/fetch-raw-samples.sh`). Entries whose file is absent are skipped.

use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, Picture, open};
use lf_settings::DevelopSettings;
use std::path::PathBuf;
use std::time::Instant;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/raw-samples")
}

#[test]
fn real_camera_samples_open() {
    let manifest = std::fs::read_to_string(corpus_dir().join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let mut failures = Vec::new();
    for s in manifest["samples"].as_array().unwrap() {
        let name = s["name"].as_str().unwrap();
        let camera = s["camera"].as_str().unwrap();
        let path = corpus_dir().join("files").join(name);
        if !path.exists() {
            println!("skipped: no sample for {name} (run scripts/fetch-raw-samples.sh)");
            continue;
        }
        if let Err(e) = check(name, camera, &path) {
            println!("FAIL {name} ({camera}): {e}");
            failures.push(format!("{name} ({camera}): {e}"));
        }
    }
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

fn check(name: &str, camera: &str, path: &std::path::Path) -> Result<(), String> {
    let settings = DevelopSettings::default();
    let t = Instant::now();
    let o = open(path, Quality::Fast, &settings, 8192).map_err(|e| format!("Fast: {e}"))?;
    let fast = t.elapsed();
    if o.kind != Kind::Raw {
        return Err(format!("kind {:?}, expected Raw", o.kind));
    }
    let Picture::Float(dev) = &o.picture else {
        return Err("picture is not Float".into());
    };
    let f = &dev.frame;
    if f.width.max(f.height) <= 1000 {
        return Err(format!("too small: {}x{}", f.width, f.height));
    }
    let (mut sum, mut n) = (0f64, 0u64);
    for px in f.data.as_chunks::<4>().0 {
        for c in &px[..3] {
            let v = c.to_f32();
            if !v.is_finite() {
                return Err("NaN or infinite value".into());
            }
            sum += v as f64;
            n += 1;
        }
    }
    let mean = sum / n as f64;
    if !(0.05..=0.95).contains(&mean) {
        return Err(format!("mean luminance {mean:.3} outside 0.05..0.95"));
    }
    let mut missing = Vec::new();
    if o.exif.make.is_none() {
        missing.push("make");
    }
    if o.exif.model.is_none() {
        missing.push("model");
    }
    let t = Instant::now();
    open(path, Quality::Best, &settings, 8192).map_err(|e| format!("Best: {e}"))?;
    let best = t.elapsed();
    println!(
        "ok {name} ({camera}): {}x{} (full {}x{}), mean {mean:.3}, make {:?} model {:?}, missing exif: {missing:?}, Fast {fast:.2?}, Best {best:.2?}",
        f.width, f.height, o.full_width, o.full_height, o.exif.make, o.exif.model
    );
    Ok(())
}
