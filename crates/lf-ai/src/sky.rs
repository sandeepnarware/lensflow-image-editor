//! Sky replacement: the photo's sky (SkySeg, [`crate::segment`]) swapped for another photo's.
//! The new sky is scaled to cover the frame's width (and the sky's depth), anchored at the
//! top, and blended in through the sky mask, its edge feathered so trees and rooftops keep a
//! soft line. The land is then nudged towards the new sky's colour and brightness
//! ([`HARMONY`]), as the light of a different sky would fall on it, so it does not look
//! pasted on.

use crate::{AiError, Engine, Gray, Rgb};

pub use crate::segment::SKY_MODELS;

/// How far the land moves towards the new sky's colour (0: not at all).
pub const HARMONY: f32 = 0.2;

/// The mean colour of `img` weighted by `w` (`None` when the weights are all 0).
fn mean(img: &Rgb, w: &[f32]) -> Option<[f32; 3]> {
    let mut s = [0.0f64; 3];
    let mut n = 0.0f64;
    for (i, &k) in w.iter().enumerate() {
        if k <= 0.0 {
            continue;
        }
        let p = img.at(i % img.w, i / img.w);
        for c in 0..3 {
            s[c] += f64::from(p[c] * k);
        }
        n += f64::from(k);
    }
    (n > 1e-6).then(|| s.map(|v| (v / n) as f32))
}

/// `sky` scaled to cover `w` wide and at least `depth` deep, its top kept, cut to `w × h`
/// (rows below its own bottom repeat its last row).
pub fn fit(sky: &Rgb, w: usize, h: usize, depth: usize) -> Rgb {
    let k = (w as f32 / sky.w.max(1) as f32).max(depth.max(1) as f32 / sky.h.max(1) as f32);
    let (sw, sh) = (
        ((sky.w as f32 * k).ceil() as usize).max(w),
        ((sky.h as f32 * k).ceil() as usize).max(1),
    );
    let scaled = sky.resized(sw, sh);
    let x0 = (sw - w) / 2;
    let mut out = Rgb::new(w, h);
    for y in 0..h {
        let sy = y.min(sh - 1);
        for x in 0..w {
            let i = (y * w + x) * 3;
            out.data[i..i + 3].copy_from_slice(&scaled.at(x0 + x, sy));
        }
    }
    out
}

/// A box blur of `m` (radius `r`), for the mask's soft edge.
fn soften(m: &Gray, r: usize) -> Gray {
    if r == 0 {
        return m.clone();
    }
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let (w, h) = (m.w, m.h);
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        let at = |line: usize, i: usize| {
            if horizontal {
                line * w + i
            } else {
                i * w + line
            }
        };
        let mut out = vec![0.0; w * h];
        for line in 0..lines {
            let mut sum = 0.0f32;
            for k in 0..=2 * r {
                sum += src[at(line, k.saturating_sub(r).min(len - 1))];
            }
            for i in 0..len {
                out[at(line, i)] = sum / (2 * r + 1) as f32;
                let leave = i.saturating_sub(r).min(len - 1);
                let enter = (i + r + 1).min(len - 1);
                sum += src[at(line, enter)] - src[at(line, leave)];
            }
        }
        out
    };
    Gray {
        w: m.w,
        h: m.h,
        data: pass(&pass(&m.data, true), false),
    }
}

/// `img` with the sky where `mask` (0..1, `img`'s size) says, taken from `sky`.
pub fn replace_with(img: &Rgb, mask: &Gray, sky: &Rgb) -> Rgb {
    let (w, h) = (img.w, img.h);
    let soft = soften(mask, (w.max(h) / 400).max(1));
    // How deep the sky reaches: the lowest row with any sky.
    let depth = (0..h)
        .rev()
        .find(|&y| (0..w).any(|x| soft.data[y * w + x] > 0.05))
        .map_or(h / 2, |y| y + 1);
    let new_sky = fit(sky, w, h, depth);
    // The land's colour moves towards the new sky's, by how the two skies differ.
    let cast = match (mean(img, &soft.data), mean(&new_sky, &soft.data)) {
        (Some(old), Some(new)) => std::array::from_fn(|c| {
            let k = (new[c] + 0.02) / (old[c] + 0.02);
            1.0 + (k.clamp(0.5, 2.0) - 1.0) * HARMONY
        }),
        _ => [1.0; 3],
    };
    let mut out = Rgb::new(w, h);
    for i in 0..w * h {
        let m = soft.data[i].clamp(0.0, 1.0);
        for (c, k) in cast.iter().enumerate() {
            let lit = (img.data[i * 3 + c] * k).clamp(0.0, 1.0);
            out.data[i * 3 + c] = lit + (new_sky.data[i * 3 + c] - lit) * m;
        }
    }
    out
}

/// `img` with its sky replaced by `sky` (the sky found by SkySeg at about 1024 pixels).
pub fn replace(engine: &Engine, img: &Rgb, sky: &Rgb) -> Result<Rgb, AiError> {
    let long = img.w.max(img.h).max(1);
    let k = (1024.0 / long as f32).min(1.0);
    let small = img.resized(
        ((img.w as f32 * k) as usize).max(1),
        ((img.h as f32 * k) as usize).max(1),
    );
    let mask = crate::segment::mask(engine, crate::segment::Target::Sky, &small, img.w, img.h)?;
    if mask.data.iter().all(|&v| v < 0.3) {
        return Err(AiError::Model("No sky was found in this photo.".to_owned()));
    }
    Ok(replace_with(img, &mask, sky))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, c: [f32; 3]) -> Rgb {
        let mut img = Rgb::new(w, h);
        for p in img.data.chunks_mut(3) {
            p.copy_from_slice(&c);
        }
        img
    }

    #[test]
    fn the_sky_is_swapped_and_the_land_kept_with_a_touch_of_the_new_light() {
        let (w, h) = (80, 60);
        // A grey sky over green land; the new sky is orange.
        let mut img = flat(w, h, [0.2, 0.5, 0.2]);
        let mut mask = Gray {
            w,
            h,
            data: vec![0.0; w * h],
        };
        for y in 0..30 {
            for x in 0..w {
                img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[0.6, 0.6, 0.6]);
                mask.data[y * w + x] = 1.0;
            }
        }
        let sky = flat(40, 20, [0.9, 0.5, 0.2]);
        let out = replace_with(&img, &mask, &sky);
        let top = out.at(40, 5);
        assert!(
            (top[0] - 0.9).abs() < 0.02 && (top[2] - 0.2).abs() < 0.02,
            "{top:?}"
        );
        let land = out.at(40, 55);
        assert!(land[1] > 0.4, "the land stays green: {land:?}");
        assert!(land[0] > 0.2, "warmed a little by the new sky: {land:?}");
        assert!(land[0] < 0.3, "only a little: {land:?}");
    }

    #[test]
    fn the_new_sky_covers_the_width_and_keeps_its_top() {
        // A sky whose top row is red and the rest blue, half as wide as the photo.
        let mut sky = flat(50, 40, [0.0, 0.0, 1.0]);
        for x in 0..50 {
            sky.data[x * 3..x * 3 + 3].copy_from_slice(&[1.0, 0.0, 0.0]);
        }
        let f = fit(&sky, 100, 60, 30);
        assert_eq!((f.w, f.h), (100, 60));
        assert!(f.at(50, 0)[0] > 0.5, "the top stays at the top");
        assert!(f.at(99, 40)[2] > 0.5);
    }
}
