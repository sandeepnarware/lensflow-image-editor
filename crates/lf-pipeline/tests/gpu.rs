//! GPU foundation: device, textures with mip chains, copy and gaussian passes.
//! These run on the machine's adapter (lavapipe in CI).

use image::{Rgba, RgbaImage};
use lf_pipeline::error::PipelineError;
use lf_pipeline::gpu::{Gpu, GpuOptions};
use lf_pipeline::passes::{copy, gaussian};
use lf_pipeline::texture::{Source, Target, read_back};

#[test]
fn gpu_or_a_clear_error() {
    match Gpu::new() {
        Ok(g) => assert!(g.max_edge() >= 4096),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn forbidding_the_fallback_without_hardware_is_a_named_error() {
    let r = Gpu::new_with(GpuOptions {
        allow_fallback: false,
        backend: Some(wgpu::Backends::empty()),
    });
    assert!(matches!(r, Err(PipelineError::NoAdapter)));
    assert_eq!(
        PipelineError::NoAdapter.to_string(),
        "No graphics device found. lensflow-ImageEditor needs a GPU with Vulkan, Metal or DirectX 12."
    );
}

#[test]
fn a_photo_over_the_texture_limit_is_refused() {
    let gpu = Gpu::new().unwrap();
    let limit = gpu.max_edge();
    let err = Source::upload_size_check(&gpu, limit + 1, 10).unwrap_err();
    assert!(matches!(err, PipelineError::TooLarge { .. }));
    assert!(err.to_string().contains(&limit.to_string()));
}

#[test]
fn an_empty_image_is_refused() {
    let gpu = Gpu::new().unwrap();
    let err = Target::new(&gpu, 0, 5, false).err().unwrap();
    assert!(matches!(err, PipelineError::EmptyImage));
    assert_eq!(err.to_string(), "The photo has no pixels.");
    assert!(matches!(
        Source::upload_size_check(&gpu, 10, 0),
        Err(PipelineError::EmptyImage)
    ));

    let empty = RgbaImage::new(0, 10);
    let r = lf_pipeline::Renderer::new(&gpu, &empty, 0, 10);
    assert!(matches!(r.err(), Some(PipelineError::EmptyImage)));
    // A real image with a zero full size would give an infinite scale.
    let one = RgbaImage::new(4, 4);
    let r = lf_pipeline::Renderer::new(&gpu, &one, 0, 4);
    assert!(matches!(r.err(), Some(PipelineError::EmptyImage)));
}

#[test]
fn copy_round_trips_a_picture() {
    let gpu = Gpu::new().unwrap();
    let img = checker(37, 23);
    let src = Source::upload(&gpu, &img, false).unwrap();
    let dst = Target::new(&gpu, 37, 23, false).unwrap();
    copy::run(&gpu, &src, &dst);
    assert_eq!(read_back(&gpu, &dst).unwrap(), img);
}

#[test]
fn mip_level_one_is_the_box_average_of_an_even_picture() {
    let gpu = Gpu::new().unwrap();
    let img = gradient(8, 4); // values chosen so every 2x2 average is exact in 8 bits
    let src = Source::upload(&gpu, &img, true).unwrap();
    let level1 = read_back_level(&gpu, &src, 1).unwrap();
    assert_eq!(level1.dimensions(), (4, 2));
    for y in 0..2 {
        for x in 0..4 {
            assert_eq!(level1.get_pixel(x, y)[0], box_avg(&img, x, y));
        }
    }
}

#[test]
fn gaussian_matches_the_cpu_kernel() {
    let gpu = Gpu::new().unwrap();
    let img = noise(64, 48, 7);
    let (src, to, tmp) = (
        Source::upload(&gpu, &img, false).unwrap(),
        Target::new(&gpu, 64, 48, false).unwrap(),
        Target::new(&gpu, 64, 48, false).unwrap(),
    );
    gaussian::blur(&gpu, &src, &to, &tmp, 2.5);
    let got = read_back(&gpu, &to).unwrap();
    let want = cpu_gaussian(&img, 2.5); // same radius/weights, clamp-to-edge, bilinear-free (integer taps)
    assert!(max_abs_diff(&got, &want) <= 1); // 8-bit rounding only
}

#[test]
fn a_wide_gaussian_is_capped_at_64_taps_like_the_cpu_kernel() {
    let gpu = Gpu::new().unwrap();
    let img = noise(160, 96, 11);
    let (src, to, tmp) = (
        Source::upload(&gpu, &img, false).unwrap(),
        Target::new(&gpu, 160, 96, false).unwrap(),
        Target::new(&gpu, 160, 96, false).unwrap(),
    );
    gaussian::blur(&gpu, &src, &to, &tmp, 30.0); // ceil(3 * 30) = 90, capped to 64
    let got = read_back(&gpu, &to).unwrap();
    assert!(max_abs_diff(&got, &cpu_gaussian(&img, 30.0)) <= 1);
}

#[test]
fn mip_level_one_of_an_odd_picture_samples_the_larger_level_at_its_texel_centres() {
    let gpu = Gpu::new().unwrap();
    let img = noise(7, 5, 3);
    let src = Source::upload(&gpu, &img, true).unwrap();
    let level1 = read_back_level(&gpu, &src, 1).unwrap();
    assert_eq!(level1.dimensions(), (3, 2));
    let want = RgbaImage::from_fn(3, 2, |x, y| {
        bilinear(&img, (x as f32 + 0.5) / 3.0, (y as f32 + 0.5) / 2.0)
    });
    // Within 1: GPUs quantise bilinear weights (these are thirds, not exact in binary).
    assert!(max_abs_diff(&level1, &want) <= 1);
}

#[test]
fn every_wgsl_module_validates() {
    let gpu = Gpu::new().unwrap();
    for (name, source) in lf_pipeline::passes::SHADERS {
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl((*source).into()),
            });
        if let Some(e) = pollster::block_on(scope.pop()) {
            panic!("{name}: {e}");
        }
    }
}

// --- helpers -----------------------------------------------------------------------------

fn read_back_level(gpu: &Gpu, t: &Target, level: u32) -> Result<RgbaImage, PipelineError> {
    lf_pipeline::texture::read_back_level(gpu, t, level)
}

/// Distinct values in every channel, so a flipped or shifted copy cannot pass.
fn checker(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let on = ((x / 3) + (y / 3)) % 2 == 1;
        Rgba([
            (x * 7) as u8,
            (y * 11) as u8,
            if on { 230 } else { 20 },
            255,
        ])
    })
}

/// A non-linear pattern whose every value is a multiple of 16, so each 2x2 sum divides by 4.
fn gradient(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let v = (16 * ((x * 5 + y * 3 + x * y) % 13)) as u8;
        Rgba([v, 255 - v, v / 2, 255])
    })
}

fn box_avg(img: &RgbaImage, x: u32, y: u32) -> u8 {
    let s: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
        .iter()
        .map(|(dx, dy)| img.get_pixel(2 * x + dx, 2 * y + dy)[0] as u32)
        .sum();
    assert_eq!(s % 4, 0);
    (s / 4) as u8
}

/// Deterministic xorshift noise, opaque.
fn noise(w: u32, h: u32, seed: u32) -> RgbaImage {
    let mut state = seed.wrapping_mul(2_654_435_761).max(1);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 24) as u8
    };
    RgbaImage::from_fn(w, h, |_, _| Rgba([next(), next(), next(), 255]))
}

/// The `GAUSSIAN` shader on the CPU: across into an 8-bit intermediate, then down.
fn cpu_gaussian(img: &RgbaImage, sigma: f32) -> RgbaImage {
    let radius = (sigma * 3.0).ceil().min(64.0) as i32;
    let weights: Vec<f32> = (0..=radius)
        .map(|i| {
            if i == 0 {
                1.0
            } else {
                (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()
            }
        })
        .collect();
    let total: f32 = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    let run = |src: &RgbaImage, dx: i32, dy: i32| -> RgbaImage {
        let (w, h) = src.dimensions();
        RgbaImage::from_fn(w, h, |x, y| {
            let tap = |i: i32, c: usize| {
                let sx = (x as i32 + dx * i).clamp(0, w as i32 - 1) as u32;
                let sy = (y as i32 + dy * i).clamp(0, h as i32 - 1) as u32;
                src.get_pixel(sx, sy)[c] as f32 / 255.0
            };
            let mut out = [0u8, 0, 0, 255];
            for (c, o) in out.iter_mut().take(3).enumerate() {
                let mut sum = tap(0, c);
                for i in 1..=radius {
                    sum += (tap(i, c) + tap(-i, c)) * weights[i as usize];
                }
                *o = (sum / total * 255.0).round() as u8;
            }
            Rgba(out)
        })
    };
    run(&run(img, 1, 0), 0, 1)
}

/// A clamp-to-edge bilinear sample of `img` at normalised `(u, v)`, rounded to 8 bits.
fn bilinear(img: &RgbaImage, u: f32, v: f32) -> Rgba<u8> {
    let (w, h) = img.dimensions();
    let (tx, ty) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
    let (x0, y0) = (tx.floor(), ty.floor());
    let (fx, fy) = (tx - x0, ty - y0);
    let at = |x: f32, y: f32, c: usize| {
        let x = (x as i32).clamp(0, w as i32 - 1) as u32;
        let y = (y as i32).clamp(0, h as i32 - 1) as u32;
        img.get_pixel(x, y)[c] as f32
    };
    let mut out = [0u8; 4];
    for (c, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1.0, y0, c) * fx;
        let bottom = at(x0, y0 + 1.0, c) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0, c) * fx;
        *o = (top * (1.0 - fy) + bottom * fy).round() as u8;
    }
    Rgba(out)
}

fn max_abs_diff(a: &RgbaImage, b: &RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}
