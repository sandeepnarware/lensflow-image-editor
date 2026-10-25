//! Smoke tests over the real-camera CC0 corpus in `tests/raw-samples` (fetched by
//! `scripts/fetch-raw-samples.sh`). Entries whose file is absent are skipped.

use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, Picture, open};
use lf_settings::DevelopSettings;
use std::path::PathBuf;
use std::time::Instant;

/// The two tests run one at a time: the summary test times header-only reads, which the
/// smoke test's parallel full decodes would otherwise starve of CPU (on a 4-core CI
/// runner the DNG summary went from 0.5 s to over 2 s).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/raw-samples")
}

#[test]
fn real_camera_samples_open() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let manifest = std::fs::read_to_string(corpus_dir().join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let mut failures = Vec::new();
    let (mut opened, mut skipped) = (0, 0);
    for s in manifest["samples"].as_array().unwrap() {
        let name = s["name"].as_str().unwrap();
        let camera = s["camera"].as_str().unwrap();
        let path = corpus_dir().join("files").join(name);
        if !path.exists() {
            println!("skipped: no sample for {name} (run scripts/fetch-raw-samples.sh)");
            skipped += 1;
            continue;
        }
        match check(name, camera, &path) {
            Ok(()) => opened += 1,
            Err(e) => {
                println!("FAIL {name} ({camera}): {e}");
                failures.push(format!("{name} ({camera}): {e}"));
            }
        }
    }
    let total = opened + skipped + failures.len();
    println!("RAW samples: {opened} opened, {skipped} skipped of {total}");
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
    assert!(
        skipped == 0 || std::env::var("LF_REQUIRE_SAMPLES").as_deref() != Ok("1"),
        "LF_REQUIRE_SAMPLES=1 but {skipped} of {total} samples are missing"
    );
}

#[test]
fn summary_gives_raw_dimensions_without_decoding() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let manifest = std::fs::read_to_string(corpus_dir().join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let mut checked = 0;
    for s in manifest["samples"].as_array().unwrap() {
        let name = s["name"].as_str().unwrap();
        let path = corpus_dir().join("files").join(name);
        if !path.exists() {
            println!("skipped: no sample for {name}");
            continue;
        }
        let t = Instant::now();
        let (kind, _exif, size) = lf_raw::open::summary(&path).unwrap();
        let took = t.elapsed();
        println!("summary {name}: {size:?} in {took:.2?}");
        assert_eq!(kind, Kind::Raw);
        assert!(took.as_secs() < 2, "{name}: summary took {took:?}");
        let o = open(&path, Quality::Fast, &DevelopSettings::default(), 512).unwrap();
        assert_eq!(size, Some((o.full_width, o.full_height)), "{name}");
        checked += 1;
    }
    println!("{checked} samples checked");
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
    let (mut sum, mut n) = ([0f64; 3], 0u64);
    for px in f.data.as_chunks::<4>().0 {
        for (c, v) in px[..3].iter().enumerate() {
            let v = v.to_f32();
            if !v.is_finite() {
                return Err("NaN or infinite value".into());
            }
            sum[c] += v as f64;
        }
        n += 1;
    }
    let rgb = sum.map(|s| s / n as f64);
    let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    if !(0.05..=0.95).contains(&mean) {
        return Err(format!("mean luminance {mean:.3} outside 0.05..0.95"));
    }
    // Grey world, loosely: no channel's mean is more than 2.5x off the mean luminance.
    // A wrong black level or matrix (the yellow-green Canon sRAW failure) breaks it;
    // ordinary scenes do not.
    let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    if rgb.iter().any(|&c| c > luma * 2.5 || c < luma / 2.5) {
        return Err(format!(
            "colour cast: channel means {rgb:.3?} vs luminance {luma:.3}"
        ));
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
    let t = Instant::now();
    let p = open(path, Quality::Preview, &settings, 8192).map_err(|e| format!("Preview: {e}"))?;
    let preview = t.elapsed();
    let Picture::Float(pd) = &p.picture else {
        return Err("preview is not Float".into());
    };
    let long = pd.frame.width.max(pd.frame.height);
    let full_long = o.full_width.max(o.full_height);
    if !p.reduced || (p.full_width, p.full_height) != (o.full_width, o.full_height) {
        return Err("preview does not report the full frame and reduced".into());
    }
    if long > full_long / 2 || long < full_long / 3 - 1 {
        return Err(format!("preview long edge {long} for a full {full_long}"));
    }
    println!(
        "ok {name} ({camera}): {}x{} (full {}x{}), mean {mean:.3}, rgb {rgb:.3?}, make {:?} model {:?}, missing exif: {missing:?}, Fast {fast:.2?}, Best {best:.2?}, Preview {preview:.2?} ({}x{})",
        f.width,
        f.height,
        o.full_width,
        o.full_height,
        o.exif.make,
        o.exif.model,
        pd.frame.width,
        pd.frame.height
    );
    Ok(())
}
