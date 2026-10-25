//! X-Trans (6x6 CFA) demosaic. PROVISIONAL: a correct but simple interpolation,
//! to be replaced by a proper X-Trans method in a later part.
//!
//! Each missing channel is a weighted average of the samples of that colour near
//! the pixel: `Mean3x3` takes the plain mean over the 3x3 neighbourhood (`Fast`),
//! `InverseDistance5x5` weights the 5x5 neighbourhood by 1 / distance (`Best`).
//! Out-of-frame neighbours are skipped (mirroring would break the 6x6 phase); if a
//! window holds no sample of a colour, the next larger window is tried.

use super::Plane;
use crate::Rect;
use rayon::prelude::*;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Weighting {
    Mean3x3,
    InverseDistance5x5,
}

pub(crate) fn run(p: &Plane, pattern: &[[usize; 6]; 6], crop: Rect, how: Weighting) -> Vec<f32> {
    let cw = crop.w as usize;
    let mut out = vec![0.0f32; cw * crop.h as usize * 3];
    if cw == 0 {
        return out;
    }
    let colour = |x: usize, y: usize| pattern[y % 6][x % 6];
    out.par_chunks_mut(cw * 3)
        .enumerate()
        .for_each(|(row, line)| {
            let y = crop.y as usize + row;
            for (i, px) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let x = crop.x as usize + i;
                let own = colour(x, y);
                let start = match how {
                    Weighting::Mean3x3 => 1,
                    Weighting::InverseDistance5x5 => 2,
                };
                for (c, slot) in px.iter_mut().enumerate() {
                    if c == own {
                        *slot = p.data[y * p.w + x];
                        continue;
                    }
                    *slot = 0.0;
                    for r in start..=4isize {
                        let (mut sum, mut wsum) = (0.0f32, 0.0f32);
                        for dy in -r..=r {
                            for dx in -r..=r {
                                let (nx, ny) = (x as isize + dx, y as isize + dy);
                                if nx < 0 || ny < 0 || nx >= p.w as isize || ny >= p.h as isize {
                                    continue;
                                }
                                let (nx, ny) = (nx as usize, ny as usize);
                                if colour(nx, ny) != c {
                                    continue;
                                }
                                let wgt = match how {
                                    Weighting::Mean3x3 => 1.0,
                                    Weighting::InverseDistance5x5 => {
                                        1.0 / ((dx * dx + dy * dy) as f32).sqrt()
                                    }
                                };
                                sum += wgt * p.data[ny * p.w + nx];
                                wsum += wgt;
                            }
                        }
                        if wsum > 0.0 {
                            *slot = sum / wsum;
                            break;
                        }
                    }
                }
            }
        });
    out
}
