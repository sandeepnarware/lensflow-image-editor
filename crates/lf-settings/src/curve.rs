//! Tone curves as lookup tables, ported from the web editor's `curve.ts`.

use crate::{CurvePoint, Curves};

/// A tone curve through its points as a lookup table (0..1), using a monotone cubic
/// (Fritsch-Carlson) so the curve never overshoots between points.
pub fn curve_table(points: &[CurvePoint], size: usize) -> Vec<f32> {
    let mut sorted: Vec<CurvePoint> = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // Drop points within 1e-4 in x of the previous one (compared against the unfiltered list, as the web does).
    let pts: Vec<CurvePoint> = sorted
        .iter()
        .enumerate()
        .filter(|(i, p)| *i == 0 || p[0] > sorted[i - 1][0] + 1e-4)
        .map(|(_, p)| *p)
        .collect();

    let mut out = vec![0.0f32; size];
    if pts.len() < 2 {
        for (i, v) in out.iter_mut().enumerate() {
            *v = (i as f64 / (size - 1) as f64) as f32;
        }
        return out;
    }

    let n = pts.len();
    let xs: Vec<f64> = pts.iter().map(|p| p[0]).collect();
    let ys: Vec<f64> = pts.iter().map(|p| p[1]).collect();
    let slopes: Vec<f64> = (0..n - 1)
        .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
        .collect();
    let mut tangents: Vec<f64> = vec![slopes[0]];
    for i in 1..n - 1 {
        tangents.push(if slopes[i - 1] * slopes[i] <= 0.0 {
            0.0
        } else {
            (slopes[i - 1] + slopes[i]) / 2.0
        });
    }
    tangents.push(slopes[n - 2]);
    for i in 0..n - 1 {
        if slopes[i] == 0.0 {
            tangents[i] = 0.0;
            tangents[i + 1] = 0.0;
            continue;
        }
        let a = tangents[i] / slopes[i];
        let b = tangents[i + 1] / slopes[i];
        let h = a * a + b * b;
        if h > 9.0 {
            let t = 3.0 / h.sqrt();
            tangents[i] = t * a * slopes[i];
            tangents[i + 1] = t * b * slopes[i];
        }
    }

    let mut seg = 0;
    for (i, slot) in out.iter_mut().enumerate() {
        let x = i as f64 / (size - 1) as f64;
        let y = if x <= xs[0] {
            ys[0]
        } else if x >= xs[n - 1] {
            ys[n - 1]
        } else {
            while seg < n - 2 && x > xs[seg + 1] {
                seg += 1;
            }
            let h = xs[seg + 1] - xs[seg];
            let t = (x - xs[seg]) / h;
            let t2 = t * t;
            let t3 = t2 * t;
            (2.0 * t3 - 3.0 * t2 + 1.0) * ys[seg]
                + (t3 - 2.0 * t2 + t) * h * tangents[seg]
                + (-2.0 * t3 + 3.0 * t2) * ys[seg + 1]
                + (t3 - t2) * h * tangents[seg + 1]
        };
        *slot = y.clamp(0.0, 1.0) as f32;
    }
    out
}

/// The four curves packed for the shader: 256 RGBA texels where R, G and B are each channel's
/// curve applied after the master (RGB) curve, so the shader needs one lookup per channel.
pub fn pack_curves(c: &Curves) -> [u8; 1024] {
    let master = curve_table(&c.rgb, 256);
    let channels = [
        curve_table(&c.r, 256),
        curve_table(&c.g, 256),
        curve_table(&c.b, 256),
    ];
    let mut data = [0u8; 1024];
    for i in 0..256 {
        let index = (master[i] as f64 * 255.0).round() as usize;
        for (k, ch) in channels.iter().enumerate() {
            data[i * 4 + k] = (ch[index] as f64 * 255.0).round() as u8;
        }
        data[i * 4 + 3] = 255;
    }
    data
}
