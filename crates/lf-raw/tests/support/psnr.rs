//! Image-quality metrics for the demosaic tests.

/// Peak signal-to-noise ratio in dB for 0..1 data (peak 1.0), over the pixels of an
/// interleaved RGB image of `width` x `height` that lie at least `border` pixels from
/// every edge. `a` and `b` are both `width * height * 3` samples.
pub fn psnr(a: &[f32], b: &[f32], width: usize, height: usize, border: usize) -> f64 {
    assert_eq!(a.len(), width * height * 3);
    assert_eq!(b.len(), a.len());
    let mut sum = 0.0f64;
    let mut n = 0usize;
    for y in border..height - border {
        for x in border..width - border {
            for c in 0..3 {
                let i = (y * width + x) * 3 + c;
                let d = (a[i] - b[i]) as f64;
                sum += d * d;
                n += 1;
            }
        }
    }
    let mse = sum / n as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (1.0 / mse).log10()
    }
}
