//! Bilinear Bayer demosaic: each missing channel is the mean of the samples of
//! that colour among the 8 neighbours (2 or 4 of them, depending on the site).
//! Edges are mirrored, which keeps the CFA phase.

use super::Plane;
use crate::Rect;
use rayon::prelude::*;

pub(crate) fn bayer(p: &Plane, pattern: [[usize; 2]; 2], crop: Rect) -> Vec<f32> {
    let cw = crop.w as usize;
    let mut out = vec![0.0f32; cw * crop.h as usize * 3];
    if cw == 0 {
        return out;
    }
    out.par_chunks_mut(cw * 3)
        .enumerate()
        .for_each(|(row, line)| {
            let y = (crop.y as usize + row) as isize;
            for (i, px) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let x = (crop.x as usize + i) as isize;
                let mut sum = [0.0f32; 3];
                let mut n = [0u32; 3];
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let c = pattern[((y + dy) & 1) as usize][((x + dx) & 1) as usize];
                        sum[c] += p.mirrored(x + dx, y + dy);
                        n[c] += 1;
                    }
                }
                let own = pattern[(y & 1) as usize][(x & 1) as usize];
                for c in 0..3 {
                    px[c] = if c == own {
                        p.mirrored(x, y)
                    } else {
                        sum[c] / n[c] as f32
                    };
                }
            }
        });
    out
}
