//! The chain on float textures: values above 1.0 reach the develop pass, an 8-bit photo
//! renders the same through the float path as through its own 8-bit chain (which
//! `Renderer::new` keeps for parity with the web's RGBA8 blurs), and the develop pass draws
//! into any colour target format.

use half::f16;
use image::RgbaImage;
use lf_fixtures::{case_json, load_png, root};
use lf_pipeline::error::PipelineError;
use lf_pipeline::renderer::DrawOptions;
use lf_pipeline::test_support::shared_gpu;
use lf_pipeline::texture::{FLOAT_FORMAT, Source, Target, read_back, read_back_f16};
use lf_pipeline::{FloatFrame, Renderer};
use lf_settings::normalise;
use serde_json::json;

/// RGBA of pixel `(x, y)` as f32.
fn px(f: &FloatFrame, x: u32, y: u32) -> [f32; 4] {
    let i = 4 * (y * f.width + x) as usize;
    [0, 1, 2, 3].map(|c| f.data[i + c].to_f32())
}

/// The 8-bit image as half floats (`v / 255`, rounded to nearest).
fn to_f16(img: &RgbaImage) -> FloatFrame {
    FloatFrame {
        width: img.width(),
        height: img.height(),
        data: img
            .as_raw()
            .iter()
            .map(|&v| f16::from_f32(f32::from(v) / 255.0))
            .collect(),
    }
}

#[test]
fn an_f16_source_above_one_survives_to_the_chain_input() {
    let gpu = shared_gpu();
    // A 4x4 grey frame with one pixel at 2.0.
    let (hot_x, hot_y) = (2, 1);
    let mut data = vec![f16::from_f32(0.5); 4 * 16];
    for c in 0..3 {
        data[4 * (hot_y * 4 + hot_x) + c] = f16::from_f32(2.0);
    }
    for a in data.iter_mut().skip(3).step_by(4) {
        *a = f16::ONE;
    }
    let frame = FloatFrame {
        width: 4,
        height: 4,
        data,
    };
    let source = Source::upload_f16(&gpu, &frame, true).unwrap();
    assert_eq!(source.format, FLOAT_FORMAT);
    let mut r = Renderer::from_texture(gpu.clone(), source, 4, 4).unwrap();

    // The chain's input (the source, or the spotted image) holds the value unclamped.
    let s = normalise(&json!({}));
    let input = read_back_f16(&gpu, r.prepare(&s).unwrap()).unwrap();
    assert_eq!(px(&input, 2, 1), [2.0, 2.0, 2.0, 1.0]);
    assert_eq!(px(&input, 0, 0), [0.5, 0.5, 0.5, 1.0]);
    let spotted = normalise(&json!({ "spots": [
        { "x": 0.125, "y": 0.875, "r": 0.1, "sx": 0.375, "sy": 0.875, "mode": "clone" }
    ] }));
    let input = read_back_f16(&gpu, r.prepare(&spotted).unwrap()).unwrap();
    assert_eq!(px(&input, 2, 1), [2.0, 2.0, 2.0, 1.0]);

    // Drawn into a float target: "before" ends at the output clamp (the chain's own) ...
    let canvas = Target::new_with(&gpu, 4, 4, false, FLOAT_FORMAT).unwrap();
    let before = DrawOptions {
        before: true,
        ..DrawOptions::default()
    };
    r.draw(&s, &before, &canvas).unwrap();
    let out = read_back_f16(&gpu, &canvas).unwrap();
    assert_eq!(px(&out, 2, 1), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(px(&out, 0, 0), [0.5, 0.5, 0.5, 1.0]);

    // ... and exposure -3 pulls the value back from above 1.0: 2 * 8^(-1/2.2) = 0.777, where a
    // value clamped to 1.0 on the way in would give 8^(-1/2.2) = 0.389.
    let darker = normalise(&json!({ "exposure": -3 }));
    r.draw(&darker, &DrawOptions::default(), &canvas).unwrap();
    let out = read_back_f16(&gpu, &canvas).unwrap();
    let want = 2.0 * 8f32.powf(-1.0 / 2.2);
    for c in &px(&out, 2, 1)[..3] {
        assert!((c - want).abs() < 0.01, "{c} is not {want}");
    }
}

/// `Renderer::new` (8-bit chain) and `Renderer::from_texture` on the same photo converted to
/// half floats draw the identity case byte for byte alike.
#[test]
fn eight_bit_input_renders_identically_through_the_float_path() {
    let gpu = shared_gpu();
    let src = load_png(root().join("sources/portrait.png")).unwrap();
    let (w, h) = src.dimensions();
    let s = normalise(&case_json("identity").unwrap());
    let via_new = Renderer::new(gpu.clone(), &src, w, h)
        .unwrap()
        .render(&s)
        .unwrap();
    assert!(
        via_new == src,
        "identity through Renderer::new is not the source"
    );

    let source = Source::upload_f16(&gpu, &to_f16(&src), true).unwrap();
    let via_float = Renderer::from_texture(gpu.clone(), source, w, h)
        .unwrap()
        .render(&s)
        .unwrap();
    assert!(
        via_new.as_raw() == via_float.as_raw(),
        "the two paths differ"
    );

    // from_texture refuses an 8-bit source, and a float one without mips.
    let eight = Source::upload(&gpu, &src, true).unwrap();
    let err = Renderer::from_texture(gpu.clone(), eight, w, h)
        .err()
        .unwrap();
    assert!(matches!(err, PipelineError::WrongFormat { .. }));
    assert_eq!(
        err.to_string(),
        "The image texture is Rgba8Unorm with mipmaps; the renderer needs Rgba16Float with mipmaps."
    );
    let flat = Source::upload_f16(&gpu, &to_f16(&src), false).unwrap();
    let err = Renderer::from_texture(gpu.clone(), flat, w, h)
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "The image texture is Rgba16Float without mipmaps; the renderer needs Rgba16Float with mipmaps."
    );
}

#[test]
fn draw_into_bgra_target_works() {
    let gpu = shared_gpu();
    let src = load_png(root().join("sources/portrait.png")).unwrap();
    let s = normalise(&case_json("look-golden").unwrap());
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    let (w, h) = (src.width() / 2, src.height() / 2);
    let draw = |r: &mut Renderer, format| {
        let canvas = Target::new_with(&gpu, w, h, false, format).unwrap();
        r.draw(&s, &DrawOptions::default(), &canvas).unwrap();
        canvas
    };
    let rgba = draw(&mut r, wgpu::TextureFormat::Rgba8Unorm);
    let bgra = draw(&mut r, wgpu::TextureFormat::Bgra8Unorm);
    assert_eq!(bgra.format, wgpu::TextureFormat::Bgra8Unorm);
    let want = read_back(&gpu, &rgba).unwrap();
    // read_back returns RGBA whatever the byte order on the device.
    let got = read_back(&gpu, &bgra).unwrap();
    assert!(got.as_raw() == want.as_raw(), "the BGRA draw differs");
    assert!(
        want.pixels().any(|p| p[0] != p[2]),
        "the picture has no red/blue difference, so a swap would go unnoticed"
    );
}

#[test]
fn render_f16_matches_render_within_one_level() {
    let gpu = shared_gpu();
    let src = load_png(root().join("sources/portrait.png")).unwrap();
    let s = normalise(&case_json("look-golden").unwrap());
    let mut r = Renderer::new(gpu, &src, src.width(), src.height()).unwrap();
    let eight = r.render(&s).unwrap();
    let float = r.render_f16(&s).unwrap();
    assert_eq!((float.width, float.height), eight.dimensions());
    let mut worst = 0i32;
    for y in 0..eight.height() {
        for x in 0..eight.width() {
            let f = px(&float, x, y);
            let e = eight.get_pixel(x, y).0;
            for c in 0..3 {
                let v = (f[c].clamp(0.0, 1.0) * 255.0).round() as i32;
                worst = worst.max((v - i32::from(e[c])).abs());
            }
        }
    }
    assert!(worst <= 1, "max channel difference {worst}");
    // Orientation: the picture is not symmetric, so a flip would show at these corners.
    let (w, h) = eight.dimensions();
    for (x, y) in [
        (w / 5, h / 5),
        (w - 1 - w / 5, h / 5),
        (w / 5, h - 1 - h / 5),
    ] {
        let f = px(&float, x, y);
        let e = eight.get_pixel(x, y).0;
        assert!((f[0] * 255.0 - f32::from(e[0])).abs() <= 1.5, "({x},{y})");
    }
    let (a, b) = (
        eight.get_pixel(w / 5, h / 5),
        eight.get_pixel(w - 1 - w / 5, h - 1 - h / 5),
    );
    assert_ne!(
        a, b,
        "the probe pixels are alike, so a flip would go unnoticed"
    );
}
