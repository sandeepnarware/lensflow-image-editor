//! Working a large picture through a model in overlapping tiles: each tile is run alone (so
//! memory stays bounded whatever the photo's size) and the results are blended where they
//! overlap with weights falling off towards each tile's edge, so no seams show.

use crate::{AiError, Rgb};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where each tile starts along an edge of `len`: tiles of `tile` overlapping by `overlap`,
/// the last one flush with the end.
pub fn starts(len: usize, tile: usize, overlap: usize) -> Vec<usize> {
    if len <= tile {
        return vec![0];
    }
    let step = tile - overlap;
    let mut out: Vec<usize> = (0..)
        .map(|i| i * step)
        .take_while(|&s| s + tile < len)
        .collect();
    out.push(len - tile);
    out
}

/// A weight for position `i` of `n` in a tile: rising over `ramp` from each edge that is not
/// the picture's own edge (`open_start`, `open_end`).
fn ramp(i: usize, n: usize, ramp: usize, open_start: bool, open_end: bool) -> f32 {
    let mut w = 1.0f32;
    if open_start && ramp > 0 {
        w = w.min((i as f32 + 0.5) / ramp as f32);
    }
    if open_end && ramp > 0 {
        w = w.min(((n - i) as f32 - 0.5) / ramp as f32);
    }
    w.clamp(0.0, 1.0)
}

/// How a picture is cut for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiling {
    /// The tile edge and how much neighbours overlap (input pixels).
    pub tile: usize,
    pub overlap: usize,
    /// The model returns each tile this many times larger.
    pub scale: usize,
    /// Tiles are padded (by repeating the edge) to a multiple of this first, and cropped
    /// back after.
    pub multiple: usize,
}

/// Runs `model` over `img` in tiles as `tiling` says. `progress(done, total)` per tile;
/// `stop` ends it between tiles.
pub fn run(
    img: &Rgb,
    tiling: Tiling,
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
    model: &mut dyn FnMut(&Rgb) -> Result<Rgb, AiError>,
) -> Result<Rgb, AiError> {
    let Tiling {
        tile,
        overlap,
        scale,
        multiple,
    } = tiling;
    let xs = starts(img.w, tile, overlap);
    let ys = starts(img.h, tile, overlap);
    let total = xs.len() * ys.len();
    let (ow, oh) = (img.w * scale, img.h * scale);
    let mut acc = vec![0.0f32; ow * oh * 3];
    let mut weight = vec![0.0f32; ow * oh];
    let mut done = 0;
    progress(0, total);
    for &y in &ys {
        for &x in &xs {
            if stop.load(Ordering::SeqCst) {
                return Err(AiError::Stopped);
            }
            let tw = tile.min(img.w);
            let th = tile.min(img.h);
            let part = padded(&img.crop(x, y, tw, th), multiple);
            let out = model(&part)?;
            if (out.w, out.h) != (part.w * scale, part.h * scale) {
                return Err(AiError::Model(format!(
                    "a tile came back {}×{}, not {}×{}",
                    out.w,
                    out.h,
                    part.w * scale,
                    part.h * scale
                )));
            }
            let (sw, sh) = (tw * scale, th * scale);
            let r = overlap * scale / 2;
            for row in 0..sh {
                let wy = ramp(row, sh, r, y > 0, y + th < img.h);
                for col in 0..sw {
                    let wx = ramp(col, sw, r, x > 0, x + tw < img.w);
                    let w = wx * wy;
                    let o = (y * scale + row) * ow + x * scale + col;
                    let s = (row * out.w + col) * 3;
                    acc[o * 3] += out.data[s] * w;
                    acc[o * 3 + 1] += out.data[s + 1] * w;
                    acc[o * 3 + 2] += out.data[s + 2] * w;
                    weight[o] += w;
                }
            }
            done += 1;
            progress(done, total);
        }
    }
    for (i, w) in weight.iter().enumerate() {
        let w = w.max(1e-6);
        for c in 0..3 {
            acc[i * 3 + c] /= w;
        }
    }
    Ok(Rgb {
        w: ow,
        h: oh,
        data: acc,
    })
}

/// `img` grown to a multiple of `m` on each side by repeating its last row and column.
pub fn padded(img: &Rgb, m: usize) -> Rgb {
    let m = m.max(1);
    let (w, h) = (img.w.div_ceil(m) * m, img.h.div_ceil(m) * m);
    if (w, h) == (img.w, img.h) {
        return img.clone();
    }
    let mut out = Rgb::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = img.at(x.min(img.w - 1), y.min(img.h - 1));
            out.data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_edge_and_end_flush() {
        assert_eq!(starts(100, 256, 32), vec![0]);
        assert_eq!(starts(500, 256, 32), vec![0, 224, 244]);
        assert_eq!(starts(480, 256, 32), vec![0, 224]);
        assert_eq!(starts(1000, 256, 32), vec![0, 224, 448, 672, 744]);
    }

    #[test]
    fn an_identity_model_gives_the_picture_back_seamlessly() {
        let mut img = Rgb::new(300, 170);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = ((i * 7919) % 1000) as f32 / 1000.0;
        }
        let mut calls = 0;
        let out = run(
            &img,
            Tiling {
                tile: 128,
                overlap: 32,
                scale: 1,
                multiple: 8,
            },
            &mut |_, _| {},
            &AtomicBool::new(false),
            &mut |t| {
                calls += 1;
                Ok(t.clone())
            },
        )
        .unwrap();
        assert_eq!(calls, 3 * 2);
        let worst = out
            .data
            .iter()
            .zip(&img.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn a_scaling_model_scales_the_whole() {
        let img = Rgb {
            w: 20,
            h: 10,
            data: vec![0.5; 600],
        };
        let out = run(
            &img,
            Tiling {
                tile: 16,
                overlap: 4,
                scale: 2,
                multiple: 8,
            },
            &mut |_, _| {},
            &AtomicBool::new(false),
            &mut |t| Ok(t.resized(t.w * 2, t.h * 2)),
        )
        .unwrap();
        assert_eq!((out.w, out.h), (40, 20));
        assert!(out.data.iter().all(|v| (v - 0.5).abs() < 1e-4));
    }
}
