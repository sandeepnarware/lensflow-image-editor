//! Denoising (SCUNet, blind real-world) and upscaling (Real-ESRGAN general x4v3), both in
//! tiles ([`crate::tiles`]). Both models take RGB 0..1 at any size (SCUNet a multiple of 8)
//! and give the same back, SCUNet at the same size and Real-ESRGAN four times larger.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Rgb, run_one, tiles};
use std::sync::atomic::AtomicBool;

/// What denoising downloads.
pub const DENOISE_MODELS: [Asset; 2] = [assets::DENOISE, assets::DENOISE_DATA];
/// What upscaling downloads.
pub const UPSCALE_MODELS: [Asset; 1] = [assets::UPSCALE];

/// The tile edge for denoising and its overlap (pixels).
const DENOISE_TILE: usize = 256;
const DENOISE_OVERLAP: usize = 32;
/// The tile edge for upscaling (input pixels) and its overlap.
const UPSCALE_TILE: usize = 192;
const UPSCALE_OVERLAP: usize = 16;

/// The largest photo upscaling takes, in megapixels of output.
pub const MAX_UPSCALED_MP: f64 = 120.0;

/// `img` denoised, mixed with the original by `strength` (0..1: 1 is the model's result).
pub fn denoise(
    engine: &Engine,
    img: &Rgb,
    strength: f32,
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<Rgb, AiError> {
    let mut session = engine.session(&assets::DENOISE)?;
    let mut out = tiles::run(
        img,
        tiles::Tiling {
            tile: DENOISE_TILE,
            overlap: DENOISE_OVERLAP,
            scale: 1,
            multiple: 8,
        },
        progress,
        stop,
        &mut |t| model(&mut session, "image", t, 1),
    )?;
    mix(&mut out, img, strength);
    Ok(out)
}

/// `img` upscaled `factor` times (2 or 4): the model's 4×, halved again for 2×.
pub fn upscale(
    engine: &Engine,
    img: &Rgb,
    factor: usize,
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<Rgb, AiError> {
    let mp = (img.w * factor) as f64 * (img.h * factor) as f64 / 1e6;
    if mp > MAX_UPSCALED_MP {
        return Err(AiError::Model(format!(
            "the result would be {mp:.0} MP; the largest is {MAX_UPSCALED_MP:.0} MP"
        )));
    }
    let mut session = engine.session(&assets::UPSCALE)?;
    let four = tiles::run(
        img,
        tiles::Tiling {
            tile: UPSCALE_TILE,
            overlap: UPSCALE_OVERLAP,
            scale: 4,
            multiple: 1,
        },
        progress,
        stop,
        &mut |t| model(&mut session, "input", t, 4),
    )?;
    Ok(if factor == 4 {
        four
    } else {
        four.resized(img.w * factor, img.h * factor)
    })
}

/// One tile through a model taking and giving NCHW RGB, `scale` times larger.
fn model(
    session: &mut ort::session::Session,
    input: &str,
    tile: &Rgb,
    scale: usize,
) -> Result<Rgb, AiError> {
    let (shape, values) = run_one(session, input, [1, 3, tile.h, tile.w], tile.planar())?;
    let (h, w) = (tile.h * scale, tile.w * scale);
    if shape != [1, 3, h, w] {
        return Err(AiError::Model(format!("unexpected output {shape:?}")));
    }
    Ok(Rgb::from_planar(w, h, &values))
}

/// `out` moved back towards `original` by `1 − strength`.
pub fn mix(out: &mut Rgb, original: &Rgb, strength: f32) {
    let s = strength.clamp(0.0, 1.0);
    for (o, v) in out.data.iter_mut().zip(&original.data) {
        *o = (v + (*o - v) * s).clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_mixes_with_the_original() {
        let original = Rgb {
            w: 1,
            h: 1,
            data: vec![0.0, 0.5, 1.0],
        };
        let mut out = Rgb {
            w: 1,
            h: 1,
            data: vec![1.0, 0.5, 0.0],
        };
        mix(&mut out, &original, 0.25);
        assert_eq!(out.data, vec![0.25, 0.5, 0.75]);
    }
}
