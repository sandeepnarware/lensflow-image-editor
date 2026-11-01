//! The example fit on synthetic pairs: a known look (a warm S-curve), a known vignette and a
//! known grain are applied to a colourful image, and the fit must find them again.

use lf_grade::example::{Fit, Image, delta_e2000, delta_e2000_lab, fit, radius, resize, vignette};

const W: usize = 256;
const H: usize = 192;

/// A colourful image covering the colour cube: 4 x 4 tiles inside a grey frame wider than the
/// fit's left-out border, blue rising tile by tile, red rising left to right and green top to
/// bottom inside each tile, every channel kept just inside 0..1 so nothing is clipped.
fn colourful() -> Image {
    const FRAME: usize = 16;
    let (tw, th) = ((W - 2 * FRAME) / 4, (H - 2 * FRAME) / 4);
    let level = |i: usize, n: usize| 0.01 + 0.98 * i as f32 / (n - 1) as f32;
    let mut data = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let inside = (FRAME..W - FRAME).contains(&x) && (FRAME..H - FRAME).contains(&y);
            if !inside {
                data.extend_from_slice(&[0.5; 3]);
                continue;
            }
            let (lx, ly) = (x - FRAME, y - FRAME);
            let tile = (ly / th) * 4 + lx / tw;
            data.extend_from_slice(&[level(lx % tw, tw), level(ly % th, th), level(tile, 16)]);
        }
    }
    Image { w: W, h: H, data }
}

/// Only reds: red 0.4..1, green and blue low.
fn reds() -> Image {
    let mut data = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let r = 0.4 + 0.6 * x as f32 / (W - 1) as f32;
            let g = 0.05 + 0.2 * y as f32 / (H - 1) as f32;
            let b = 0.05 + 0.15 * ((x / 16 + y / 16) % 4) as f32 / 3.0;
            data.extend_from_slice(&[r, g, b]);
        }
    }
    Image { w: W, h: H, data }
}

/// A different scene: rings of colour around an off-centre point.
fn rings() -> Image {
    let mut data = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let d = ((x as f32 - 60.0).powi(2) + (y as f32 - 150.0).powi(2)).sqrt();
            let r = 0.5 + 0.45 * (d / 9.0).sin();
            let g = 0.5 + 0.45 * (d / 23.0).cos();
            let b = 0.5 + 0.45 * (x as f32 / 31.0).sin();
            data.extend_from_slice(&[r, g, b]);
        }
    }
    Image { w: W, h: H, data }
}

/// The hand-made look: an S-curve on each channel, red lifted, blue lowered (stays in 0..1).
fn look(c: [f32; 3]) -> [f32; 3] {
    let s = |x: f32| x - 0.08 * (2.0 * std::f32::consts::PI * x).sin();
    [0.02 + 0.96 * s(c[0]), s(c[1]), 0.92 * s(c[2])]
}

fn map(img: &Image, f: impl Fn(usize, usize, [f32; 3]) -> [f32; 3]) -> Image {
    let mut data = Vec::with_capacity(img.data.len());
    for y in 0..img.h {
        for x in 0..img.w {
            let i = (y * img.w + x) * 3;
            let c = [img.data[i], img.data[i + 1], img.data[i + 2]];
            data.extend_from_slice(&f(x, y, c));
        }
    }
    Image {
        w: img.w,
        h: img.h,
        data,
    }
}

/// A repeatable noise value in -0.5..0.5 for a pixel and seed.
fn noise(x: usize, y: usize, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0 - 0.5
}

/// Grain as a stand-in for the shader's: the same noise on each channel, its size linear in
/// the amount.
fn grain(c: [f32; 3], x: usize, y: usize, amount: f64, seed: u32) -> [f32; 3] {
    let n = noise(x, y, seed) * amount as f32 / 100.0 * 0.22;
    c.map(|v| v + n)
}

/// The injected renderer: the fit's table, vignette and grain on `base`, as the pipeline
/// orders them.
fn effects(base: &Image) -> impl FnMut(&Fit) -> Image + '_ {
    move |f: &Fit| {
        map(base, |x, y, c| {
            let t = f.table.apply(c);
            let v = vignette(
                t,
                radius(x, y, base.w, base.h),
                f.vignette,
                f.vignette_midpoint,
            );
            grain(v, x, y, f.grain, 7).map(|v| v.clamp(0.0, 1.0))
        })
    }
}

fn shift(img: &Image, dx: i32, dy: i32) -> Image {
    map(img, |x, y, _| {
        let sx = (x as i32 - dx).clamp(0, img.w as i32 - 1) as usize;
        let sy = (y as i32 - dy).clamp(0, img.h as i32 - 1) as usize;
        let i = (sy * img.w + sx) * 3;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    })
}

/// A small repeatable random generator.
fn lcg(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*state >> 40) as f32) / (1u64 << 24) as f32
}

#[test]
fn a_known_table_is_recovered() {
    let ours = colourful();
    let theirs = map(&ours, |_, _, c| look(c));
    let f = fit(&ours, &theirs, &mut effects(&ours));
    assert!(f.aligned);
    assert_eq!(f.table.size, 17);
    let mut seed = 1u64;
    let mut worst = 0f64;
    for _ in 0..200 {
        let c = [0; 3].map(|_| 0.05 + 0.9 * lcg(&mut seed));
        worst = worst.max(delta_e2000(f.table.apply(c), look(c)));
    }
    assert!(worst < 1.0, "worst difference {worst}");
    assert!(f.vignette.abs() < 2.0, "no vignette, found {}", f.vignette);
    assert!(f.delta_e_mean < 1.0, "mean {}", f.delta_e_mean);
}

#[test]
fn unseen_colours_map_smoothly() {
    let ours = reds();
    let theirs = map(&ours, |_, _, c| look(c));
    let f = fit(&ours, &theirs, &mut effects(&ours));
    let blue = [0.1, 0.2, 0.9];
    let d = delta_e2000(f.table.apply(blue), blue);
    assert!(d < 10.0, "blue moved by {d}");
    // From the reds that were seen to the blue that was not, in small steps, without jumps.
    let red = [0.8, 0.15, 0.1];
    let mut last = f.table.apply(red);
    for i in 1..=100 {
        let t = i as f32 / 100.0;
        let c = [0, 1, 2].map(|k| red[k] + (blue[k] - red[k]) * t);
        let now = f.table.apply(c);
        let step = delta_e2000(now, last);
        assert!(step < 3.0, "a jump of {step} at {t}");
        last = now;
    }
}

#[test]
fn a_vignette_is_recovered() {
    let ours = colourful();
    let theirs = map(&ours, |x, y, c| {
        vignette(look(c), radius(x, y, W, H), -30.0, 50.0)
    });
    let f = fit(&ours, &theirs, &mut effects(&ours));
    assert!(
        (f.vignette + 30.0).abs() <= 5.0,
        "vignette {} (midpoint {})",
        f.vignette,
        f.vignette_midpoint
    );
    assert!(f.delta_e_mean < 2.0, "mean {}", f.delta_e_mean);
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// The shader's value noise: smooth between hashed lattice points.
fn value_noise(p: [f32; 2]) -> f32 {
    let hash = |p: [f32; 2]| {
        let mut q = [fract(p[0] * 443.897), fract(p[1] * 441.423)];
        let d = q[0] * (q[1] + 19.19) + q[1] * (q[0] + 19.19);
        q[0] += d;
        q[1] += d;
        fract((q[0] + q[1]) * q[0])
    };
    let i = [p[0].floor(), p[1].floor()];
    let f = [fract(p[0]), fract(p[1])].map(|t| t * t * (3.0 - 2.0 * t));
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    mix(
        mix(hash(i), hash([i[0] + 1.0, i[1]]), f[0]),
        mix(
            hash([i[0], i[1] + 1.0]),
            hash([i[0] + 1.0, i[1] + 1.0]),
            f[0],
        ),
        f[1],
    )
}

/// Grain as the develop shader makes it: two octaves of value noise in cells set by the size,
/// strongest in the midtones, offset by `seed` (another seed is another grain of the same kind).
fn shader_grain(c: [f32; 3], x: usize, y: usize, amount: f64, size: f64, seed: f32) -> [f32; 3] {
    if amount <= 0.0 {
        return c;
    }
    let cell = 0.7 + (size / 100.0) as f32 * 3.0;
    let p = [
        (x as f32 + 0.5) / cell + seed,
        (y as f32 + 0.5) / cell + seed,
    ];
    let n =
        value_noise(p) * 0.6 + value_noise([p[0] * 2.13 + 17.0, p[1] * 2.13 + 17.0]) * 0.4 - 0.5;
    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let a = n * (amount / 100.0) as f32 * 0.22 * (0.4 + 0.6 * (1.0 - (2.0 * l - 1.0).abs()));
    c.map(|v| v + a)
}

/// The injected renderer with the shader's grain.
fn shader_effects(base: &Image) -> impl FnMut(&Fit) -> Image + '_ {
    move |f: &Fit| {
        map(base, |x, y, c| {
            let t = f.table.apply(c);
            let v = vignette(
                t,
                radius(x, y, base.w, base.h),
                f.vignette,
                f.vignette_midpoint,
            );
            shader_grain(v, x, y, f.grain, f.grain_size, 0.0).map(|v| v.clamp(0.0, 1.0))
        })
    }
}

/// `img` as an 8-bit export stores it.
fn eight_bit(img: &Image) -> Image {
    map(img, |_, _, c| {
        c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0)
    })
}

#[test]
fn grain_is_measured() {
    let ours = colourful();
    let grainy = map(&ours, |x, y, c| grain(look(c), x, y, 40.0, 99));
    let shader = map(&ours, |x, y, c| {
        shader_grain(look(c), x, y, 40.0, 25.0, 31.7)
    });
    for (what, theirs, white) in [
        ("float", grainy.clone(), true),
        ("8-bit", eight_bit(&grainy), true),
        ("shader float", shader.clone(), false),
        ("shader 8-bit", eight_bit(&shader), false),
    ] {
        let f = if white {
            fit(&ours, &theirs, &mut effects(&ours))
        } else {
            fit(&ours, &theirs, &mut shader_effects(&ours))
        };
        assert!((f.grain - 40.0).abs() <= 6.0, "{what}: grain {}", f.grain);
    }
}

#[test]
fn a_grain_free_export_gets_no_grain() {
    let ours = colourful();
    let clean = map(&ours, |_, _, c| look(c));
    let ours8 = eight_bit(&ours);
    for shader in [false, true] {
        let render = |base: &Image, f: &Fit| {
            if shader {
                shader_effects(base)(f)
            } else {
                effects(base)(f)
            }
        };
        let f = fit(&ours, &clean, &mut |f| render(&ours, f));
        assert_eq!(f.grain, 0.0, "float export (shader grain {shader})");
        // An 8-bit export re-quantises smooth gradients: that is not grain.
        let f = fit(&ours, &eight_bit(&clean), &mut |f| render(&ours, f));
        assert!(
            f.grain < 5.0,
            "8-bit export (shader {shader}): grain {}",
            f.grain
        );
        // An 8-bit original (a JPEG or PNG), and Lightroom's 8-bit export of it.
        let export = eight_bit(&map(&ours8, |_, _, c| look(c)));
        let f = fit(&ours8, &export, &mut |f| render(&ours8, f));
        assert!(
            f.grain < 5.0,
            "8-bit pair (shader {shader}): grain {}",
            f.grain
        );
    }
}

#[test]
fn a_few_pixels_of_shift_are_tolerated() {
    let ours = colourful();
    let theirs = shift(&map(&ours, |_, _, c| look(c)), 3, -2);
    let f = fit(&ours, &theirs, &mut effects(&ours));
    assert!(f.aligned);
    assert!(f.delta_e_mean < 2.0, "mean {}", f.delta_e_mean);
}

#[test]
fn a_mismatched_pair_says_so() {
    let ours = colourful();
    let other = map(&rings(), |_, _, c| look(c));
    let f = fit(&ours, &other, &mut effects(&ours));
    assert!(
        !f.aligned || f.delta_e_mean > 10.0,
        "different image: aligned, mean {}",
        f.delta_e_mean
    );
    assert!(
        f.residual > 0.03,
        "different image: residual {}",
        f.residual
    );

    // Lightroom's export cropped by 20 % (the middle 80 %), at another size.
    let looked = map(&ours, |_, _, c| look(c));
    let (cw, ch) = (W * 8 / 10, H * 8 / 10);
    let (ox, oy) = ((W - cw) / 2, (H - ch) / 2);
    let mut data = Vec::with_capacity(cw * ch * 3);
    for y in 0..ch {
        for x in 0..cw {
            let i = ((y + oy) * W + x + ox) * 3;
            data.extend_from_slice(&looked.data[i..i + 3]);
        }
    }
    let cropped = Image { w: cw, h: ch, data };
    let f = fit(&ours, &cropped, &mut effects(&ours));
    assert!(
        !f.aligned || f.delta_e_mean > 10.0,
        "cropped: aligned, mean {}",
        f.delta_e_mean
    );
}

#[test]
fn resizing_keeps_a_flat_image_flat_and_sizes_right() {
    let img = Image {
        w: 10,
        h: 6,
        data: [0.25f32, 0.5, 0.75].repeat(60),
    };
    let out = resize(&img, 25, 4);
    assert_eq!((out.w, out.h, out.data.len()), (25, 4, 300));
    assert!(out.data.chunks(3).all(|p| (p[0] - 0.25).abs() < 1e-6
        && (p[1] - 0.5).abs() < 1e-6
        && (p[2] - 0.75).abs() < 1e-6));
}

/// `img` sampled bilinearly at `(x, y)` in pixels (pixel centres at whole numbers), clamped.
fn sample(img: &Image, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (img.w - 1) as f32);
    let y = y.clamp(0.0, (img.h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.w - 1), (y0 + 1).min(img.h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize, c: usize| img.data[(yy * img.w + xx) * 3 + c];
    [0, 1, 2].map(|c| {
        let top = at(x0, y0, c) + (at(x1, y0, c) - at(x0, y0, c)) * tx;
        let bottom = at(x0, y1, c) + (at(x1, y1, c) - at(x0, y1, c)) * tx;
        top + (bottom - top) * ty
    })
}

/// `img` warped about its centre: each output pixel reads from where `from` sends it.
fn warp(img: &Image, from: impl Fn(f32, f32) -> (f32, f32)) -> Image {
    let (cx, cy) = ((img.w - 1) as f32 / 2.0, (img.h - 1) as f32 / 2.0);
    map(img, |x, y, _| {
        let (sx, sy) = from(x as f32 - cx, y as f32 - cy);
        sample(img, sx + cx, sy + cy)
    })
}

/// A colour-for-colour look.
type Look = fn([f32; 3]) -> [f32; 3];

#[test]
fn strong_looks_stay_aligned() {
    let ours = colourful();
    let looks: [(&str, Look); 2] = [
        ("brighter", |c| c.map(|v| v.powf(0.6))),
        ("darker", |c| c.map(|v| v * 0.45)),
    ];
    for (name, f) in looks {
        let theirs = map(&ours, |_, _, c| f(c));
        let fitted = fit(&ours, &theirs, &mut effects(&ours));
        assert!(fitted.aligned, "{name}: said not aligned");
        assert!(
            fitted.residual < 0.01,
            "{name}: residual {}",
            fitted.residual
        );
        assert!(
            fitted.delta_e_mean < 1.0,
            "{name}: mean {}",
            fitted.delta_e_mean
        );
    }
}

#[test]
fn vivid_colours_are_learned() {
    // Fully saturated colours: green at zero, red and blue across their range.
    let ours = map(&colourful(), |x, y, c| {
        let inside = (16..W - 16).contains(&x) && (16..H - 16).contains(&y);
        if inside {
            [
                (x - 16) as f32 / (W - 33) as f32,
                0.0,
                (y - 16) as f32 / (H - 33) as f32,
            ]
        } else {
            c
        }
    });
    let theirs = map(&ours, |_, _, c| look(c));
    let f = fit(&ours, &theirs, &mut effects(&ours));
    let mut seed = 5u64;
    let mut worst = 0f64;
    for _ in 0..100 {
        let c = [
            0.2 + 0.75 * lcg(&mut seed),
            0.0,
            0.2 + 0.75 * lcg(&mut seed),
        ];
        worst = worst.max(delta_e2000(f.table.apply(c), look(c)));
    }
    assert!(worst < 1.5, "worst difference {worst}");
}

#[test]
fn a_turned_or_warped_pair_says_so() {
    let looked = map(&colourful(), |_, _, c| look(c));
    let ours = colourful();
    let (sin, cos) = 2f32.to_radians().sin_cos();
    let turned = warp(&looked, |x, y| (x * cos - y * sin, x * sin + y * cos));
    let f = fit(&ours, &turned, &mut effects(&ours));
    assert!(
        !f.aligned || f.delta_e_mean > 2.0,
        "turned 2 degrees: aligned, mean {}",
        f.delta_e_mean
    );

    let half = (W * W + H * H) as f32 / 4.0;
    let barrel = warp(&looked, |x, y| {
        let k = 1.0 + 0.04 * (x * x + y * y) / half;
        (x * k, y * k)
    });
    let f = fit(&ours, &barrel, &mut effects(&ours));
    assert!(
        !f.aligned || f.delta_e_mean > 2.0,
        "barrel: aligned, mean {}",
        f.delta_e_mean
    );
}

#[test]
fn shrinking_averages_away_fine_detail() {
    // Fine lines, one pixel lit in three: across red's columns and green's rows. Each output
    // pixel covers 6 x 6, so it must be exactly a third (sampling would give 0.5, 0 or 1).
    let (w, h) = (120, 96);
    let data = (0..w * h)
        .flat_map(|i| {
            let lit = |v: usize| if v.is_multiple_of(3) { 1.0 } else { 0.0 };
            [lit(i % w), lit(i / w), 1.0 / 3.0]
        })
        .collect();
    let out = resize(&Image { w, h, data }, w / 6, h / 6);
    assert_eq!((out.w, out.h), (20, 16));
    let worst = out
        .data
        .iter()
        .map(|v| (v - 1.0 / 3.0).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-4, "fine lines shrunk 6x are not flat: {worst}");
}

#[test]
fn shrinking_by_an_uneven_factor_covers_each_source_pixel_once() {
    // 7 x 5 to 3 x 2: each output pixel covers 7/3 source columns and 5/2 source rows, so
    // pixels on a footprint's edge count by the share covered.
    let (w, h) = (7, 5);
    let flat = Image {
        w,
        h,
        data: [0.3f32, 0.6, 0.9].repeat(w * h),
    };
    let out = resize(&flat, 3, 2);
    assert_eq!((out.w, out.h, out.data.len()), (3, 2, 18));
    for p in out.data.chunks(3) {
        assert!(
            (p[0] - 0.3).abs() < 1e-6 && (p[1] - 0.6).abs() < 1e-6 && (p[2] - 0.9).abs() < 1e-6
        );
    }

    // Red rises with the column (0..6), green with the row (0..4).
    let ramp = Image {
        w,
        h,
        data: (0..w * h)
            .flat_map(|i| [(i % w) as f32, (i / w) as f32, 1.0])
            .collect(),
    };
    let out = resize(&ramp, 3, 2);
    // The mean of floor(t) over [from, to): each source pixel's value by the length covered.
    let covered = |from: f64, to: f64| {
        let mut sum = 0.0;
        let mut s = from.floor();
        while s < to {
            sum += s * (to.min(s + 1.0) - from.max(s));
            s += 1.0;
        }
        sum / (to - from)
    };
    let (sx, sy) = (7.0 / 3.0, 5.0 / 2.0);
    for y in 0..2 {
        for x in 0..3 {
            let p = &out.data[(y * 3 + x) * 3..(y * 3 + x) * 3 + 3];
            let want_r = covered(x as f64 * sx, (x + 1) as f64 * sx);
            let want_g = covered(y as f64 * sy, (y + 1) as f64 * sy);
            assert!(
                (p[0] as f64 - want_r).abs() < 1e-5,
                "red at {x}: {} not {want_r}",
                p[0]
            );
            assert!(
                (p[1] as f64 - want_g).abs() < 1e-5,
                "green at {y}: {} not {want_g}",
                p[1]
            );
            assert!((p[2] - 1.0).abs() < 1e-6);
        }
    }
    // The edges by hand: the first column covers pixels 0, 1 and a third of 2; the last a
    // third of 4, then 5 and 6.
    assert!((out.data[0] - (0.0 + 1.0 + 2.0 / 3.0) / sx as f32).abs() < 1e-5);
    assert!((out.data[6] - (4.0 / 3.0 + 5.0 + 6.0) / sx as f32).abs() < 1e-5);
}

#[test]
fn delta_e2000_matches_published_pairs() {
    // Sharma, Wu and Dalal (2005), "The CIEDE2000 color-difference formula", Table 1,
    // pairs 1, 4 (a' of zero in the second sample), 7, 11 and 15 (hues on either side of
    // the wrap, whose mean and difference go round through 0), 17 and 25.
    let pairs = [
        ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
        ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0011], 7.2195),
        ([50.0, -0.001, 2.49], [50.0, 0.0011, -2.49], 4.7461),
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        (
            [60.2574, -34.0099, 36.2677],
            [60.4626, -34.1751, 39.4387],
            1.2644,
        ),
    ];
    for (a, b, want) in pairs {
        let got = delta_e2000_lab(a, b);
        assert!((got - want).abs() < 1e-4, "{a:?} {b:?}: {got} not {want}");
        let back = delta_e2000_lab(b, a);
        assert!((back - want).abs() < 1e-4, "symmetry: {back}");
    }
    // sRGB in: the same colour is no difference, white to black is the whole lightness range.
    assert_eq!(delta_e2000([0.3, 0.6, 0.2], [0.3, 0.6, 0.2]), 0.0);
    assert!((delta_e2000([1.0; 3], [0.0; 3]) - 100.0).abs() < 0.01);
}
