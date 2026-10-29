//! Removing things (LaMa): each painted region is cut out with room around it, shrunk to the
//! model's 512 × 512, filled, grown back and blended in through a feathered copy of the
//! mask, so only what was painted changes and the rest keeps the photo's full resolution.
//! LaMa takes the picture 0..1 and the mask 0 or 1, and answers 0..255.
//!
//! [`fill_edges`] fills a panorama's empty edges instead: long thin wedges a region box would
//! squash, so it works in squares along them ([`edge_tiles`]), each filled from the picture
//! and the squares filled before it.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Gray, Rgb};
use std::sync::atomic::{AtomicBool, Ordering};

pub const INPAINT_MODELS: [Asset; 1] = [assets::INPAINT];

const EDGE: usize = 512;

/// A part of the picture to fill: its box (pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// The painted regions of `mask` (values above 0.5), each with room around it for the model
/// to see what to fill with: grouped on a coarse grid so specks next to each other are one
/// region, then grown by `max(the region's size, 64 px)` each way, kept inside the picture.
pub fn regions(mask: &Gray) -> Vec<Region> {
    const CELL: usize = 16;
    let (gw, gh) = (mask.w.div_ceil(CELL), mask.h.div_ceil(CELL));
    let mut on = vec![false; gw * gh];
    for y in 0..mask.h {
        for x in 0..mask.w {
            if mask.data[y * mask.w + x] > 0.5 {
                on[(y / CELL) * gw + x / CELL] = true;
            }
        }
    }
    let mut seen = vec![false; gw * gh];
    let mut out = Vec::new();
    for start in 0..gw * gh {
        if !on[start] || seen[start] {
            continue;
        }
        // Flood the 8-connected cells (a gap of one cell still joins).
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (cx, cy) = (i % gw, i / gw);
            (x0, y0, x1, y1) = (x0.min(cx), y0.min(cy), x1.max(cx), y1.max(cy));
            for dy in -2i64..=2 {
                for dx in -2i64..=2 {
                    let (nx, ny) = (cx as i64 + dx, cy as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= gw as i64 || ny >= gh as i64 {
                        continue;
                    }
                    let n = ny as usize * gw + nx as usize;
                    if on[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        let (px0, py0) = (x0 * CELL, y0 * CELL);
        let (px1, py1) = (((x1 + 1) * CELL).min(mask.w), ((y1 + 1) * CELL).min(mask.h));
        let grow = (px1 - px0).max(py1 - py0).max(64);
        // Square-ish, so shrinking to 512 × 512 does not squash it.
        let side = ((px1 - px0).max(py1 - py0) + 2 * grow).min(mask.w.max(mask.h));
        let (cx, cy) = ((px0 + px1) / 2, (py0 + py1) / 2);
        let w = side.min(mask.w);
        let h = side.min(mask.h);
        let x = cx.saturating_sub(w / 2).min(mask.w - w);
        let y = cy.saturating_sub(h / 2).min(mask.h - h);
        out.push(Region { x, y, w, h });
    }
    out
}

/// `mask` grown by `r` pixels (a square's reach) and softened over `r` more: what blends a
/// filled region back in.
pub fn feathered(mask: &Gray, r: usize) -> Gray {
    let grow = |src: &[f32], w: usize, h: usize, horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut m = 0.0f32;
                for k in 0..=2 * r {
                    let (sx, sy) = if horizontal {
                        ((x + k).saturating_sub(r).min(w - 1), y)
                    } else {
                        (x, (y + k).saturating_sub(r).min(h - 1))
                    };
                    m = m.max(src[sy * w + sx]);
                }
                out[y * w + x] = m;
            }
        }
        out
    };
    let blur = |src: &[f32], w: usize, h: usize, horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        let n = (2 * r + 1) as f32;
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0f32;
                for k in 0..=2 * r {
                    let (sx, sy) = if horizontal {
                        ((x + k).saturating_sub(r).min(w - 1), y)
                    } else {
                        (x, (y + k).saturating_sub(r).min(h - 1))
                    };
                    s += src[sy * w + sx];
                }
                out[y * w + x] = s / n;
            }
        }
        out
    };
    let (w, h) = (mask.w, mask.h);
    let hard: Vec<f32> = mask
        .data
        .iter()
        .map(|&v| if v > 0.5 { 1.0 } else { 0.0 })
        .collect();
    let grown = grow(&grow(&hard, w, h, true), w, h, false);
    let soft = blur(&blur(&grown, w, h, true), w, h, false);
    Gray { w, h, data: soft }
}

/// `img` with what `mask` paints (same size, above 0.5) filled in. `progress(done, total)`
/// per region; `stop` ends it between regions.
pub fn remove(
    engine: &Engine,
    img: &Rgb,
    mask: &Gray,
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<Rgb, AiError> {
    let parts = regions(mask);
    let mut session = engine.session(&assets::INPAINT)?;
    let mut out = img.clone();
    progress(0, parts.len());
    for (n, r) in parts.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err(AiError::Stopped);
        }
        let part = out.crop(r.x, r.y, r.w, r.h);
        let part_mask = crop_gray(mask, r);
        // A few pixels wider than painted, so the object's fringe goes too.
        let reach = (r.w.max(r.h) / 200).max(3);
        let feather = feathered(&part_mask, reach);
        let small = part.resized(EDGE, EDGE);
        let small_mask = feather.resized(EDGE, EDGE);
        let filled = fill(&mut session, &small, &small_mask)?.resized(r.w, r.h);
        for y in 0..r.h {
            for x in 0..r.w {
                let a = feather.data[y * r.w + x];
                if a <= 0.0 {
                    continue;
                }
                let o = ((r.y + y) * out.w + r.x + x) * 3;
                let f = (y * r.w + x) * 3;
                for c in 0..3 {
                    out.data[o + c] += (filled.data[f + c] - out.data[o + c]) * a;
                }
            }
        }
        progress(n + 1, parts.len());
    }
    Ok(out)
}

/// The squares [`fill_edges`] works in, in order, for holes (`true`) in a `w` × `h` picture:
/// squares of a side four times the holes' depth (at least 512, at most 1536 px, and no
/// larger than the picture), on a grid three quarters of a side apart, those with a hole.
pub fn edge_tiles(holes: &[bool], w: usize, h: usize) -> Vec<Region> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    // How deep the holes reach: from the top and bottom, and from the sides; a wedge along
    // the top also runs far from the side, so the shallower of the two is its depth.
    let (mut down, mut across) = (0usize, 0usize);
    for x in 0..w {
        let top = (0..h).take_while(|&y| holes[y * w + x]).count();
        let bottom = (0..h).rev().take_while(|&y| holes[y * w + x]).count();
        down = down.max(top.max(bottom));
    }
    for y in 0..h {
        let row = &holes[y * w..(y + 1) * w];
        let left = row.iter().take_while(|&&v| v).count();
        let right = row.iter().rev().take_while(|&&v| v).count();
        across = across.max(left.max(right));
    }
    let side = (down.min(across) * 4).clamp(EDGE, 1536).min(w).min(h);
    let step = (side * 3 / 4).max(1);
    let starts = |len: usize| -> Vec<usize> {
        let mut v: Vec<usize> = (0..len.saturating_sub(side)).step_by(step).collect();
        v.push(len - side);
        v.dedup();
        v
    };
    let mut tiles = Vec::new();
    for &y in &starts(h) {
        for &x in &starts(w) {
            let any = (y..y + side).any(|yy| holes[yy * w + x..yy * w + x + side].contains(&true));
            if any {
                tiles.push(Region {
                    x,
                    y,
                    w: side,
                    h: side,
                });
            }
        }
    }
    tiles
}

/// `img` with its `holes` (one per pixel, `true`: nothing there) filled in square by square
/// ([`edge_tiles`]); what was there is kept as it was. `progress(done, total)` per square;
/// `stop` ends it between squares.
pub fn fill_edges(
    engine: &Engine,
    img: &Rgb,
    holes: &[bool],
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<Rgb, AiError> {
    let (w, h) = (img.w, img.h);
    let tiles = edge_tiles(holes, w, h);
    let mut session = engine.session(&assets::INPAINT)?;
    let mut out = img.clone();
    let mut left: Vec<bool> = holes.to_vec();
    progress(0, tiles.len());
    for (n, r) in tiles.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err(AiError::Stopped);
        }
        let mask = Gray {
            w: r.w,
            h: r.h,
            data: (r.y..r.y + r.h)
                .flat_map(|y| (r.x..r.x + r.w).map(move |x| (y, x)))
                .map(|(y, x)| f32::from(u8::from(left[y * w + x])))
                .collect(),
        };
        if mask.data.iter().all(|&v| v == 0.0) {
            progress(n + 1, tiles.len());
            continue;
        }
        let small = out.crop(r.x, r.y, r.w, r.h).resized(EDGE, EDGE);
        // A hole a pixel wider at the model's size, so no dark rim is left at the border.
        let small_mask = feathered(&mask, (r.w / EDGE).max(1)).resized(EDGE, EDGE);
        let filled = fill(&mut session, &small, &small_mask)?.resized(r.w, r.h);
        for y in 0..r.h {
            for x in 0..r.w {
                let i = (r.y + y) * w + r.x + x;
                if left[i] {
                    let (o, f) = (i * 3, (y * r.w + x) * 3);
                    out.data[o..o + 3].copy_from_slice(&filled.data[f..f + 3]);
                    left[i] = false;
                }
            }
        }
        progress(n + 1, tiles.len());
    }
    Ok(out)
}

fn crop_gray(mask: &Gray, r: &Region) -> Gray {
    let mut data = Vec::with_capacity(r.w * r.h);
    for y in r.y..r.y + r.h {
        data.extend_from_slice(&mask.data[y * mask.w + r.x..y * mask.w + r.x + r.w]);
    }
    Gray {
        w: r.w,
        h: r.h,
        data,
    }
}

/// One 512 × 512 square through LaMa.
fn fill(session: &mut ort::session::Session, img: &Rgb, mask: &Gray) -> Result<Rgb, AiError> {
    let model = |e: ort::Error| AiError::Model(e.to_string());
    let image =
        ort::value::Tensor::from_array(([1usize, 3, EDGE, EDGE], img.planar())).map_err(model)?;
    let holes: Vec<f32> = mask
        .data
        .iter()
        .map(|&v| if v > 0.01 { 1.0 } else { 0.0 })
        .collect();
    let mask = ort::value::Tensor::from_array(([1usize, 1, EDGE, EDGE], holes)).map_err(model)?;
    let outputs = session
        .run(ort::inputs!["image" => image, "mask" => mask])
        .map_err(model)?;
    let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(model)?;
    if shape.iter().map(|&d| d as usize).collect::<Vec<_>>() != [1, 3, EDGE, EDGE] {
        return Err(AiError::Model(format!("unexpected output {shape:?}")));
    }
    let scaled: Vec<f32> = values.iter().map(|v| v / 255.0).collect();
    Ok(Rgb::from_planar(EDGE, EDGE, &scaled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_tiles_cover_every_hole_in_squares_sized_by_the_holes() {
        // A wide picture with a wedge missing along the top (up to 100 px deep).
        let (w, h) = (3000usize, 900usize);
        let holes: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                y < 100 * x / w
            })
            .collect();
        let tiles = edge_tiles(&holes, w, h);
        assert!(!tiles.is_empty());
        assert!(tiles.iter().all(|t| t.w == 512 && t.h == 512), "{tiles:?}");
        for (i, _) in holes.iter().enumerate().filter(|(_, v)| **v) {
            let (x, y) = (i % w, i / w);
            assert!(
                tiles
                    .iter()
                    .any(|t| (t.x..t.x + t.w).contains(&x) && (t.y..t.y + t.h).contains(&y)),
                "({x}, {y}) is in no square"
            );
        }
        // Squares only where there are holes: none along the bottom.
        assert!(tiles.iter().all(|t| t.y == 0));
        assert!(edge_tiles(&vec![false; w * h], w, h).is_empty());
    }

    fn painted(w: usize, h: usize, boxes: &[(usize, usize, usize, usize)]) -> Gray {
        let mut m = Gray {
            w,
            h,
            data: vec![0.0; w * h],
        };
        for &(x, y, bw, bh) in boxes {
            for yy in y..y + bh {
                for xx in x..x + bw {
                    m.data[yy * w + xx] = 1.0;
                }
            }
        }
        m
    }

    #[test]
    fn far_apart_strokes_are_separate_regions_with_room_around_them() {
        let m = painted(2000, 1500, &[(100, 100, 40, 40), (1800, 1300, 30, 30)]);
        let rs = regions(&m);
        assert_eq!(rs.len(), 2);
        for r in &rs {
            assert!(r.x + r.w <= 2000 && r.y + r.h <= 1500);
            assert!(r.w >= 40 + 128 && r.h >= 40 + 128, "{r:?}");
        }
        // The painted box is inside its region.
        let first = rs.iter().find(|r| r.x <= 100 && r.y <= 100).unwrap();
        assert!(first.x + first.w >= 140 && first.y + first.h >= 140);
        // Near each other: one region.
        assert_eq!(
            regions(&painted(
                1000,
                800,
                &[(100, 100, 20, 20), (140, 100, 20, 20)]
            ))
            .len(),
            1
        );
        assert!(regions(&painted(100, 100, &[])).is_empty());
    }

    #[test]
    fn the_feather_covers_the_paint_and_fades_out() {
        let m = painted(60, 60, &[(25, 25, 10, 10)]);
        let f = feathered(&m, 3);
        assert_eq!(f.data[30 * 60 + 30], 1.0);
        assert!(f.data[30 * 60 + 37] > 0.0 && f.data[30 * 60 + 37] < 1.0);
        assert_eq!(f.data[5 * 60 + 5], 0.0);
    }
}
