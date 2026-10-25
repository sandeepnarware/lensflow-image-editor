//! Loader and comparator for the golden parity fixtures in `tests/fixtures`.

use anyhow::{Context, Result};
use image::RgbaImage;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `tests/fixtures` folder at the workspace root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Pass thresholds, per channel on a 0..1 scale.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub mean: f64,
    pub max: f64,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub sources: BTreeMap<String, (u32, u32)>,
    pub cases: Vec<String>,
    /// Relative path to sha256 (hex).
    pub files: BTreeMap<String, String>,
    pub thresholds: Thresholds,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThresholds {
    mean_absolute_error: f64,
    max_absolute_error: f64,
}

#[derive(Deserialize)]
struct RawManifest {
    sources: BTreeMap<String, (u32, u32)>,
    cases: Vec<String>,
    files: BTreeMap<String, String>,
    thresholds: RawThresholds,
}

impl Manifest {
    pub fn load() -> Result<Manifest> {
        let path = root().join("manifest.json");
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path:?}"))?;
        let raw: RawManifest = serde_json::from_str(&text).context("parsing manifest.json")?;
        Ok(Manifest {
            sources: raw.sources,
            cases: raw.cases,
            files: raw.files,
            thresholds: Thresholds {
                mean: raw.thresholds.mean_absolute_error,
                max: raw.thresholds.max_absolute_error,
            },
        })
    }
}

pub fn load_png(path: impl AsRef<Path>) -> Result<RgbaImage> {
    let path = path.as_ref();
    let img = image::open(path).with_context(|| format!("decoding {path:?}"))?;
    Ok(img.to_rgba8())
}

/// A case's normalised settings JSON.
pub fn case_json(id: &str) -> Result<serde_json::Value> {
    let path = root().join("cases").join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path:?}"))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {path:?}"))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Comparison {
    pub mean: f64,
    pub max: f64,
    pub size_ok: bool,
}

/// Per-channel absolute error over R, G, B on a 0..1 scale (alpha ignored).
pub fn compare(got: &RgbaImage, want: &RgbaImage) -> Comparison {
    if got.dimensions() != want.dimensions() {
        return Comparison {
            mean: f64::INFINITY,
            max: f64::INFINITY,
            size_ok: false,
        };
    }
    let (mut sum, mut max, mut n) = (0.0f64, 0.0f64, 0u64);
    for (a, b) in got.pixels().zip(want.pixels()) {
        for c in 0..3 {
            let d = (f64::from(a.0[c]) - f64::from(b.0[c])).abs() / 255.0;
            sum += d;
            max = max.max(d);
            n += 1;
        }
    }
    Comparison {
        mean: if n == 0 { 0.0 } else { sum / n as f64 },
        max,
        size_ok: true,
    }
}
