use serde_json::Value;

use crate::settings::*;

fn clamp(v: Option<&Value>, min: f64, max: f64, fallback: f64) -> f64 {
    let n = v
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(fallback);
    n.max(min).min(max)
}

fn unit(n: f64) -> f64 {
    n.clamp(0.0, 1.0)
}

fn normalise_points(v: Option<&Value>) -> Vec<CurvePoint> {
    let Some(Value::Array(items)) = v else {
        return identity_curve();
    };
    let mut points: Vec<CurvePoint> = items
        .iter()
        .filter_map(|p| match p.as_array()?.as_slice() {
            [x, y] => {
                let (x, y) = (x.as_f64()?, y.as_f64()?);
                (x.is_finite() && y.is_finite()).then(|| [unit(x), unit(y)])
            }
            _ => None,
        })
        .collect();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    points.truncate(16);
    if points.len() >= 2 {
        points
    } else {
        identity_curve()
    }
}

fn wheel(w: Option<&Value>, fallback: GradeWheel) -> GradeWheel {
    GradeWheel {
        hue: clamp(w.and_then(|w| w.get("hue")), 0.0, 360.0, fallback.hue),
        sat: clamp(w.and_then(|w| w.get("sat")), 0.0, 100.0, 0.0),
    }
}

const BAND_NAMES: [&str; 8] = [
    "red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta",
];

/// Anything saved (or pasted, or a preset) made safe: unknown keys dropped,
/// missing ones defaulted, numbers clamped to their slider range.
pub fn normalise(input: &Value) -> EditSettings {
    let d = EditSettings::default_settings();
    let Some(s) = input.as_object() else {
        return d;
    };
    let get = |k: &str| s.get(k);
    let n = |key: &str, min: f64, max: f64, fallback: f64| clamp(get(key), min, max, fallback);
    let flag = |key: &str| get(key) == Some(&Value::Bool(true));

    let crop = get("crop");
    let cf = |k: &str| crop.and_then(|c| c.get(k));
    let cx = clamp(cf("x"), 0.0, 1.0, 0.0);
    let cy = clamp(cf("y"), 0.0, 1.0, 0.0);

    let mut hsl = Hsl::default();
    for (band, name) in HSL_BANDS.into_iter().zip(BAND_NAMES) {
        let b = get("hsl").and_then(|h| h.get(name));
        let f = |k: &str| clamp(b.and_then(|b| b.get(k)), -100.0, 100.0, 0.0);
        hsl[band] = HslAdjust {
            h: f("h"),
            s: f("s"),
            l: f("l"),
        };
    }

    let curve = get("curve");
    let cv = |k: &str| normalise_points(curve.and_then(|c| c.get(k)));
    let grading = get("grading");
    let gw = |k: &str, fb: GradeWheel| wheel(grading.and_then(|g| g.get(k)), fb);

    let rotate = match get("rotate").and_then(Value::as_f64) {
        Some(90.0) => Rotate::R90,
        Some(180.0) => Rotate::R180,
        Some(270.0) => Rotate::R270,
        _ => Rotate::R0,
    };

    let spots = match get("spots") {
        Some(Value::Array(items)) => items
            .iter()
            .take(MAX_SPOTS)
            .map(|p| {
                let f = |k: &str, min, max, fb| clamp(p.get(k), min, max, fb);
                Spot {
                    x: f("x", 0.0, 1.0, 0.5),
                    y: f("y", 0.0, 1.0, 0.5),
                    sx: f("sx", 0.0, 1.0, 0.5),
                    sy: f("sy", 0.0, 1.0, 0.5),
                    r: f("r", 0.002, 0.25, 0.02),
                    mode: if p.get("mode").and_then(Value::as_str) == Some("clone") {
                        SpotMode::Clone
                    } else {
                        SpotMode::Heal
                    },
                }
            })
            .collect(),
        _ => Vec::new(),
    };

    // An empty object is dropped, so a desktop export with nothing extra shows no notice.
    let desktop = get("desktop")
        .and_then(Value::as_object)
        .filter(|o| !o.is_empty())
        .filter(|o| serde_json::to_vec(o).is_ok_and(|b| b.len() <= MAX_DESKTOP_JSON))
        .cloned();

    EditSettings {
        v: SETTINGS_VERSION,
        desktop,
        exposure: n("exposure", -5.0, 5.0, d.exposure),
        contrast: n("contrast", -100.0, 100.0, d.contrast),
        highlights: n("highlights", -100.0, 100.0, d.highlights),
        shadows: n("shadows", -100.0, 100.0, d.shadows),
        whites: n("whites", -100.0, 100.0, d.whites),
        blacks: n("blacks", -100.0, 100.0, d.blacks),
        temperature: n("temperature", -100.0, 100.0, d.temperature),
        tint: n("tint", -100.0, 100.0, d.tint),
        vibrance: n("vibrance", -100.0, 100.0, d.vibrance),
        saturation: n("saturation", -100.0, 100.0, d.saturation),
        hsl,
        curve: Curves {
            rgb: cv("rgb"),
            r: cv("r"),
            g: cv("g"),
            b: cv("b"),
        },
        grading: Grading {
            shadows: gw("shadows", d.grading.shadows),
            midtones: gw("midtones", d.grading.midtones),
            highlights: gw("highlights", d.grading.highlights),
            balance: clamp(grading.and_then(|g| g.get("balance")), -100.0, 100.0, 0.0),
        },
        clarity: n("clarity", -100.0, 100.0, d.clarity),
        texture: n("texture", -100.0, 100.0, d.texture),
        dehaze: n("dehaze", -100.0, 100.0, d.dehaze),
        sharpen: n("sharpen", 0.0, 150.0, d.sharpen),
        sharpen_radius: n("sharpenRadius", 0.5, 3.0, d.sharpen_radius),
        noise: n("noise", 0.0, 100.0, d.noise),
        color_noise: n("colorNoise", 0.0, 100.0, d.color_noise),
        vignette: n("vignette", -100.0, 100.0, d.vignette),
        vignette_midpoint: n("vignetteMidpoint", 0.0, 100.0, d.vignette_midpoint),
        grain: n("grain", 0.0, 100.0, d.grain),
        grain_size: n("grainSize", 0.0, 100.0, d.grain_size),
        bw: flag("bw"),
        rotate,
        flip_h: flag("flipH"),
        flip_v: flag("flipV"),
        angle: n("angle", -45.0, 45.0, d.angle),
        crop: Crop {
            x: cx,
            y: cy,
            w: clamp(cf("w"), 0.01, 1.0 - cx, 1.0 - cx),
            h: clamp(cf("h"), 0.01, 1.0 - cy, 1.0 - cy),
        },
        aspect: match get("aspect").and_then(Value::as_str) {
            Some(a) if a.encode_utf16().count() <= 12 => a.to_string(),
            _ => d.aspect,
        },
        spots,
    }
}
