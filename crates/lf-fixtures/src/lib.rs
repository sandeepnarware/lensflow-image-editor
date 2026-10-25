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

/// The parity gate's 99.9th-percentile bound: 4/255. Not in `manifest.json` (a verbatim copy
/// of the web repo's fixtures); see the README's parity rule.
pub const P999_THRESHOLD: f64 = 4.0 / 255.0;

/// The parity gate's sanity bound on the largest single error: 16/255, **inclusive**
/// (`max <= 16/255`, unlike the strict mean and p999 bounds). It is there to catch gross
/// breakage. A 16/255 spike on a few dozen pixels (e.g. saturation +100 on a software
/// rasteriser) is rounding amplification, and the mean and p999 already bound the picture.
pub const MAX_SANITY_THRESHOLD: f64 = 16.0 / 255.0;

/// Pass thresholds, per channel on a 0..1 scale.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Mean absolute error bound (from the manifest).
    pub mean: f64,
    /// The manifest's maximum absolute error (4/255). The gate uses `p999` and `max_sanity`
    /// instead; kept for reference.
    pub max: f64,
    /// 99.9th-percentile bound ([`P999_THRESHOLD`]).
    pub p999: f64,
    /// Sanity bound on the maximum, inclusive ([`MAX_SANITY_THRESHOLD`]).
    pub max_sanity: f64,
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
                p999: P999_THRESHOLD,
                max_sanity: MAX_SANITY_THRESHOLD,
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
    /// Mean absolute error over every R, G, B sample.
    pub mean: f64,
    /// The 99.9th percentile of the per-sample absolute error: the smallest error `e` such that
    /// at least 99.9% of samples have error ≤ `e`.
    pub p999: f64,
    /// The largest per-sample absolute error.
    pub max: f64,
    pub size_ok: bool,
}

/// Per-channel absolute error over R, G, B on a 0..1 scale (alpha ignored). Exact: errors are
/// whole byte differences, so a 256-bin histogram gives the percentile.
pub fn compare(got: &RgbaImage, want: &RgbaImage) -> Comparison {
    if got.dimensions() != want.dimensions() {
        return Comparison {
            mean: f64::INFINITY,
            p999: f64::INFINITY,
            max: f64::INFINITY,
            size_ok: false,
        };
    }
    let mut hist = [0u64; 256];
    for (a, b) in got.pixels().zip(want.pixels()) {
        for c in 0..3 {
            hist[a.0[c].abs_diff(b.0[c]) as usize] += 1;
        }
    }
    let n: u64 = hist.iter().sum();
    if n == 0 {
        return Comparison {
            mean: 0.0,
            p999: 0.0,
            max: 0.0,
            size_ok: true,
        };
    }
    let sum: u64 = hist.iter().enumerate().map(|(d, &k)| d as u64 * k).sum();
    let max = hist.iter().rposition(|&k| k > 0).unwrap_or(0);
    // At least 99.9% of n samples, in integers: count * 1000 >= n * 999.
    let mut count = 0u64;
    let mut p999 = max;
    for (d, &k) in hist.iter().enumerate() {
        count += k;
        if count * 1000 >= n * 999 {
            p999 = d;
            break;
        }
    }
    Comparison {
        mean: sum as f64 / n as f64 / 255.0,
        p999: p999 as f64 / 255.0,
        max: max as f64 / 255.0,
        size_ok: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A `width`×1 pair whose red channel differs by 10 at the first `bad` pixels (3 samples per
    /// pixel, so 334 pixels give 1002 samples, the closest to 1000).
    fn pair(width: u32, bad: u32) -> (RgbaImage, RgbaImage) {
        let want = RgbaImage::from_pixel(width, 1, Rgba([100, 100, 100, 255]));
        let mut got = want.clone();
        for x in 0..bad {
            got.put_pixel(x, 0, Rgba([110, 100, 100, 255]));
        }
        (got, want)
    }

    #[test]
    fn one_outlier_in_a_thousand_samples_is_under_the_percentile() {
        // 334 pixels = 1002 samples; one at 10/255: 1001/1002 >= 99.9% are 0.
        let (got, want) = pair(334, 1);
        let c = compare(&got, &want);
        assert_eq!(c.p999, 0.0);
        assert_eq!(c.max, 10.0 / 255.0);
        assert!((c.mean - 10.0 / 255.0 / 1002.0).abs() < 1e-12);
        assert!(c.size_ok);
    }

    #[test]
    fn two_outliers_in_a_thousand_samples_set_the_percentile() {
        // 334 pixels = 1002 samples; two at 10/255: only 1000/1002 < 99.9% are 0.
        let (got, want) = pair(334, 2);
        let c = compare(&got, &want);
        assert_eq!(c.p999, 10.0 / 255.0);
        assert_eq!(c.max, 10.0 / 255.0);
        assert!((c.mean - 20.0 / 255.0 / 1002.0).abs() < 1e-12);
    }

    #[test]
    fn identical_images_compare_to_zero_and_sizes_must_match() {
        let (_, want) = pair(10, 0);
        assert_eq!(
            compare(&want, &want),
            Comparison {
                mean: 0.0,
                p999: 0.0,
                max: 0.0,
                size_ok: true
            }
        );
        let other = RgbaImage::new(11, 1);
        assert!(!compare(&want, &other).size_ok);
    }
}
