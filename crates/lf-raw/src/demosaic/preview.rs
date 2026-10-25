//! `Quality::Preview`: binning instead of interpolation, for thumbnails and quick
//! looks. Each block of sensor samples becomes one RGB pixel holding the mean of the
//! block's samples of each colour, so the output is a fraction of the size (and of
//! the memory) of a full demosaic and nothing is interpolated:
//!
//! - Bayer: 2x2 blocks (R, the mean of the two G, B), half size;
//! - X-Trans: 3x3 blocks (2 R, 5 G, 2 B in the aligned blocks), a third of the size;
//! - `Cfa::Rgb`: 2x2 averages per channel, half size.
//!
//! Blocks start at the crop's top-left corner; a partial block at the right or bottom
//! edge is dropped. Samples are normalised as in the full demosaic (black per CFA
//! position subtracted, divided by `white - black`), straight from the `u16` data.

use crate::{RawFrame, Rect};
use rayon::prelude::*;

/// Bin the crop of a mosaic: `block × block` samples per output pixel, the colour of
/// sample (x, y) being `colour(x, y)` (0 = R, 1 = G, 2 = B). Returns (data, w, h).
pub(crate) fn mosaic(
    raw: &RawFrame,
    crop: Rect,
    block: usize,
    colour: impl Fn(usize, usize) -> usize + Sync,
) -> (Vec<f32>, u32, u32) {
    let w = raw.width as usize;
    let (ow, oh) = (crop.w as usize / block, crop.h as usize / block);
    let scale = raw.black.map(|b| 1.0 / (raw.white - b));
    let mut out = vec![0.0f32; ow * oh * 3];
    if ow > 0 {
        out.par_chunks_mut(ow * 3)
            .enumerate()
            .for_each(|(oy, line)| {
                let y0 = crop.y as usize + oy * block;
                for (ox, px) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let x0 = crop.x as usize + ox * block;
                    let mut sum = [0.0f32; 3];
                    let mut n = [0u32; 3];
                    for y in y0..y0 + block {
                        for x in x0..x0 + block {
                            let pos = (y & 1) * 2 + (x & 1);
                            let v = (raw.data[y * w + x] as f32 - raw.black[pos]) * scale[pos];
                            let c = colour(x, y);
                            sum[c] += v;
                            n[c] += 1;
                        }
                    }
                    // Every 2x2 Bayer block and every aligned 3x3 X-Trans block has all
                    // three colours; a missing one (an unaligned X-Trans crop) takes green.
                    let g = if n[1] > 0 { sum[1] / n[1] as f32 } else { 0.0 };
                    for c in 0..3 {
                        px[c] = if n[c] > 0 { sum[c] / n[c] as f32 } else { g };
                    }
                }
            });
    }
    (out, ow as u32, oh as u32)
}

/// 2x2 per-channel averages of the crop of an interleaved RGB frame. Returns
/// (data, w, h).
pub(crate) fn rgb(raw: &RawFrame, crop: Rect) -> (Vec<f32>, u32, u32) {
    let w = raw.width as usize;
    let (ow, oh) = (crop.w as usize / 2, crop.h as usize / 2);
    let scale = [0, 1, 2].map(|c| 1.0 / (raw.white - raw.black[c]));
    let mut out = vec![0.0f32; ow * oh * 3];
    if ow > 0 {
        out.par_chunks_mut(ow * 3)
            .enumerate()
            .for_each(|(oy, line)| {
                let y0 = crop.y as usize + oy * 2;
                for (ox, px) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let x0 = crop.x as usize + ox * 2;
                    for c in 0..3 {
                        let mut sum = 0.0f32;
                        for y in y0..y0 + 2 {
                            for x in x0..x0 + 2 {
                                sum += raw.data[(y * w + x) * 3 + c] as f32 - raw.black[c];
                            }
                        }
                        px[c] = sum / 4.0 * scale[c];
                    }
                }
            });
    }
    (out, ow as u32, oh as u32)
}
