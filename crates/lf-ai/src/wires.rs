//! Wires and power lines against the sky, found for removal (Remove objects → "Select wires").
//!
//! No model draws wires, so they are found by their shape: within the sky (SkySeg,
//! [`crate::segment`], widened so a wire's ends where it meets a pole or a roof come too), a
//! wire is a thin line darker than the sky about it. A black-hat (the brightness closed by a
//! square a few pixels wide, minus the brightness) is large exactly on such thin dark lines and
//! near zero on the sky's smooth gradient and on anything wider than the square. What stands
//! out is grouped into connected pieces, and only long, thin pieces are kept: a bird or a cloud's
//! dark edge is short or wide, a wire is long and thin. A branch against the sky may be kept
//! too; it can be rubbed out before removing.

use crate::{AiError, Engine, Gray, Rgb};

/// How much darker than the sky about it a wire must be (0..1 brightness).
pub const CONTRAST: f32 = 0.04;
/// The longest a wire is across, as a share of the picture's long edge (the closing's reach).
const REACH: f32 = 1.0 / 250.0;
/// The shortest piece kept, as a share of the long edge (its box's diagonal).
const MIN_LENGTH: f32 = 0.08;

/// The running maximum (or minimum) of `src` over a window of `2r + 1` along each line.
fn sweep(src: &[f32], w: usize, h: usize, r: usize, horizontal: bool, max: bool) -> Vec<f32> {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| {
        if horizontal {
            line * w + i
        } else {
            i * w + line
        }
    };
    let better = |a: f32, b: f32| if max { a >= b } else { a <= b };
    let mut out = vec![0.0; w * h];
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for line in 0..lines {
        queue.clear();
        // A monotonic queue over the window [i - r, i + r]: its front is the extreme.
        let mut next = 0usize;
        for i in 0..len {
            let hi = (i + r).min(len - 1);
            while next <= hi {
                let v = src[at(line, next)];
                while queue.back().is_some_and(|&b| better(v, src[at(line, b)])) {
                    queue.pop_back();
                }
                queue.push_back(next);
                next += 1;
            }
            while queue.front().is_some_and(|&f| f + r < i) {
                queue.pop_front();
            }
            out[at(line, i)] = src[at(line, *queue.front().expect("the window has one"))];
        }
    }
    out
}

/// The brightness closed by a square of radius `r`: thin dark lines are filled in.
fn closing(lum: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let grown = sweep(&sweep(lum, w, h, r, true, true), w, h, r, false, true);
    sweep(&sweep(&grown, w, h, r, true, false), w, h, r, false, false)
}

/// `on` widened by `r` pixels each way (a square).
fn widen(on: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let f: Vec<f32> = on.iter().map(|&v| f32::from(u8::from(v))).collect();
    let grown = sweep(&sweep(&f, w, h, r, true, true), w, h, r, false, true);
    grown.iter().map(|&v| v > 0.5).collect()
}

/// The wires in `img`, where `sky` (its size, 0..1) is sky: 1 on a wire, else 0.
pub fn lines(img: &Rgb, sky: &Gray) -> Gray {
    let (w, h) = (img.w, img.h);
    let long = w.max(h);
    let r = ((long as f32 * REACH).round() as usize).max(2);
    let lum: Vec<f32> = img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect();
    let closed = closing(&lum, w, h, r);
    let skyish = widen(
        &sky.data.iter().map(|&v| v >= 0.5).collect::<Vec<_>>(),
        w,
        h,
        r * 3,
    );
    let dark: Vec<bool> = (0..w * h)
        .map(|i| skyish[i] && closed[i] - lum[i] > CONTRAST)
        .collect();
    // Connected pieces (8-way), kept when long and thin.
    let mut seen = vec![false; w * h];
    let mut out = vec![0.0f32; w * h];
    let mut stack = Vec::new();
    let mut piece = Vec::new();
    for start in 0..w * h {
        if !dark[start] || seen[start] {
            continue;
        }
        piece.clear();
        stack.push(start);
        seen[start] = true;
        while let Some(i) = stack.pop() {
            piece.push(i);
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for (dx, dy) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if dark[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for &i in &piece {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let length = ((x1 - x0) as f32).hypot((y1 - y0) as f32);
        // Thin: on average no wider across than the closing reaches.
        let thickness = piece.len() as f32 / length.max(1.0);
        if length >= long as f32 * MIN_LENGTH && thickness <= (2 * r) as f32 {
            for &i in &piece {
                out[i] = 1.0;
            }
        }
    }
    Gray { w, h, data: out }
}

/// The wires against the sky in `img` (found at its own size): 1 on a wire, else 0.
pub fn find(engine: &Engine, img: &Rgb) -> Result<Gray, AiError> {
    let sky = crate::segment::mask(engine, crate::segment::Target::Sky, img, img.w, img.h)?;
    Ok(lines(img, &sky))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sky brightening upwards over dark ground, with a wire, a bird and a thick pole.
    fn scene() -> (Rgb, Gray) {
        let (w, h) = (400usize, 300usize);
        let mut img = Rgb::new(w, h);
        let mut sky = Gray {
            w,
            h,
            data: vec![0.0; w * h],
        };
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let v = if y < 200 {
                    sky.data[y * w + x] = 1.0;
                    0.9 - 0.2 * y as f32 / 200.0
                } else {
                    0.2
                };
                img.data[i..i + 3].copy_from_slice(&[v * 0.8, v * 0.9, v]);
            }
        }
        let mut paint = |x: usize, y: usize, v: f32| {
            let i = (y * w + x) * 3;
            img.data[i..i + 3].copy_from_slice(&[v, v, v]);
        };
        // A sagging wire across the sky, two pixels thick.
        for x in 0..w {
            let t = x as f32 / w as f32 - 0.5;
            let y = (60.0 + 80.0 * t * t * 4.0) as usize;
            paint(x, y, 0.3);
            paint(x, y + 1, 0.3);
        }
        // A bird: a small blob.
        for y in 30..35 {
            for x in 300..306 {
                paint(x, y, 0.2);
            }
        }
        // A thick pole, 30 px wide: not a wire.
        for y in 100..200 {
            for x in 50..80 {
                paint(x, y, 0.25);
            }
        }
        (img, sky)
    }

    #[test]
    fn a_wire_is_found_and_a_bird_a_pole_and_the_ground_are_not() {
        let (img, sky) = scene();
        let m = lines(&img, &sky);
        let at = |x: usize, y: usize| m.data[y * 400 + x];
        assert_eq!(at(200, 60), 1.0, "the wire's middle");
        assert_eq!(
            at(20, (60.0 + 320.0 * 0.45 * 0.45) as usize),
            1.0,
            "near its end"
        );
        assert_eq!(at(302, 32), 0.0, "the bird");
        assert_eq!(at(65, 150), 0.0, "the pole");
        assert!(m.data[250 * 400..].iter().all(|&v| v == 0.0), "the ground");
        assert_eq!(at(200, 20), 0.0, "plain sky");
    }

    #[test]
    fn the_running_extremes_match_a_plain_scan() {
        let (w, h) = (17usize, 5usize);
        let src: Vec<f32> = (0..w * h).map(|i| ((i * 37) % 11) as f32).collect();
        let got = sweep(&src, w, h, 3, true, true);
        for y in 0..h {
            for x in 0..w {
                let want = (x.saturating_sub(3)..=(x + 3).min(w - 1))
                    .map(|xx| src[y * w + xx])
                    .fold(f32::MIN, f32::max);
                assert_eq!(got[y * w + x], want);
            }
        }
    }
}
