//! Local adjustments (desktop-only masks): the develop pass changes what a mask covers and
//! nothing else, and its weights follow `lf_settings::masks::weight`.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::masks::{Local, Mask, Masks, Shape, weight};

const W: u32 = 300;
const H: u32 = 200;

fn render(s: &EditSettings) -> RgbaImage {
    let img = RgbaImage::from_pixel(W, H, Rgba([128, 128, 128, 255]));
    let mut r = Renderer::new(shared_gpu(), &img, W, H).unwrap();
    r.render(s).unwrap()
}

fn with(masks: Vec<Mask>) -> EditSettings {
    let mut s = EditSettings::default_settings();
    Masks(masks).write_into(&mut s);
    s
}

fn radial(exposure: f64, invert: bool) -> Mask {
    Mask {
        shape: Shape::Radial {
            cx: 0.5,
            cy: 0.5,
            rx: 0.25,
            ry: 0.25,
            angle: 0.0,
            feather: 0.0,
        },
        invert,
        range: lf_settings::masks::Range::All,
        local: Local {
            exposure,
            ..Local::default()
        },
    }
}

#[test]
fn a_radial_mask_brightens_inside_and_leaves_the_rest() {
    let plain = render(&EditSettings::default_settings());
    let lit = render(&with(vec![radial(1.0, false)]));
    let centre = lit.get_pixel(W / 2, H / 2).0[0];
    assert!(centre > plain.get_pixel(W / 2, H / 2).0[0] + 30, "{centre}");
    assert_eq!(
        lit.get_pixel(5, 5),
        plain.get_pixel(5, 5),
        "a corner is untouched"
    );
    assert_eq!(lit.get_pixel(W - 5, H / 2), plain.get_pixel(W - 5, H / 2));

    // Inverted: the other way round.
    let out = render(&with(vec![radial(1.0, true)]));
    assert_eq!(out.get_pixel(W / 2, H / 2), plain.get_pixel(W / 2, H / 2));
    assert!(out.get_pixel(5, 5).0[0] > plain.get_pixel(5, 5).0[0] + 30);
}

#[test]
fn the_gpu_weight_is_the_cpu_weight() {
    // A soft radial on a 3:2 photo: where the CPU says half, the GPU is half way too.
    let mask = Mask {
        shape: Shape::Radial {
            cx: 0.5,
            cy: 0.5,
            rx: 0.3,
            ry: 0.3,
            angle: 0.0,
            feather: 100.0,
        },
        ..radial(1.0, false)
    };
    let plain = render(&EditSettings::default_settings());
    let full = render(&with(vec![Mask {
        shape: Shape::Linear {
            x0: 0.5,
            y0: 1.5,
            x1: 0.5,
            y1: 2.0,
        },
        ..radial(1.0, false)
    }]));
    let lit = render(&with(vec![mask.clone()]));
    let aspect = f64::from(W) / f64::from(H);
    for x in [150u32, 170, 190, 210, 230] {
        let (u, v) = (
            (f64::from(x) + 0.5) / f64::from(W),
            (f64::from(H / 2) + 0.5) / f64::from(H),
        );
        let want = weight(&mask, (u, v), aspect);
        let p = |img: &RgbaImage| f64::from(img.get_pixel(x, H / 2).0[1]);
        // Exposure is not linear in the output, so compare with loose bounds: inside the
        // span from the plain to the fully lit grey, in the right order.
        let got = (p(&lit) - p(&plain)) / (p(&full) - p(&plain));
        assert!(
            (0.0..=1.0).contains(&got) && (want < 0.05) == (got < 0.1),
            "x {x}: weight {want}, gpu {got}"
        );
    }
    // The fully covered photo (a linear mask whose start line is below it) is lit everywhere.
    assert!(full.get_pixel(5, 5).0[0] > plain.get_pixel(5, 5).0[0] + 30);
}

#[test]
fn a_warm_linear_mask_warms_the_top_only_and_masks_add_up() {
    let warm = Mask {
        shape: Shape::Linear {
            x0: 0.5,
            y0: 0.1,
            x1: 0.5,
            y1: 0.4,
        },
        invert: false,
        range: lf_settings::masks::Range::All,
        local: Local {
            temperature: 60.0,
            ..Local::default()
        },
    };
    let plain = render(&EditSettings::default_settings());
    let out = render(&with(vec![warm.clone()]));
    let top = out.get_pixel(W / 2, 3).0;
    assert!(top[0] > top[2] + 20, "the top is warm: {top:?}");
    assert_eq!(out.get_pixel(W / 2, H - 3), plain.get_pixel(W / 2, H - 3));
    // Two of the same go further than one.
    let twice = render(&with(vec![warm.clone(), warm]));
    let t2 = twice.get_pixel(W / 2, 3).0;
    assert!(t2[0] - t2[2] > top[0] - top[2], "{t2:?} vs {top:?}");
}

#[test]
fn local_contrast_and_saturation_stay_in_range() {
    // A colour photo with the global sliders at the end and a mask pushing further: the
    // result is clamped to the slider's range, so it matches the global end alone.
    let img = RgbaImage::from_fn(W, H, |x, _| Rgba([(x % 256) as u8, 90, 200, 255]));
    let mut r = Renderer::new(shared_gpu(), &img, W, H).unwrap();
    let mut s = EditSettings::default_settings();
    s.contrast = 100.0;
    s.saturation = 100.0;
    let global = r.render(&s).unwrap();
    let everywhere = Mask {
        shape: Shape::Linear {
            x0: 0.5,
            y0: 1.5,
            x1: 0.5,
            y1: 2.0,
        },
        invert: false,
        range: lf_settings::masks::Range::All,
        local: Local {
            contrast: 100.0,
            saturation: 100.0,
            ..Local::default()
        },
    };
    Masks(vec![everywhere]).write_into(&mut s);
    assert_eq!(r.render(&s).unwrap(), global);
}

fn brush(strokes: Vec<lf_settings::masks::Stroke>, exposure: f64) -> Mask {
    Mask {
        shape: Shape::Brush { strokes },
        invert: false,
        range: lf_settings::masks::Range::All,
        local: Local {
            exposure,
            ..Local::default()
        },
    }
}

fn stroke(points: &[[f64; 2]], size: f64, erase: bool) -> lf_settings::masks::Stroke {
    lf_settings::masks::Stroke {
        points: points.to_vec(),
        size,
        feather: 0.0,
        flow: 100.0,
        erase,
    }
}

#[test]
fn a_brush_mask_brightens_where_it_is_painted() {
    let plain = render(&EditSettings::default_settings());
    // A band across the top third, with a hole rubbed out of its middle.
    let painted = brush(
        vec![
            stroke(&[[0.0, 0.2], [1.0, 0.2]], 0.1, false),
            stroke(&[[0.5, 0.2]], 0.08, true),
        ],
        1.0,
    );
    // A gradient before it, so the brush is not the first mask (its own layer is read).
    let none = Mask {
        local: Local::default(),
        ..Mask::linear()
    };
    let out = render(&with(vec![none, painted]));
    let at = |img: &RgbaImage, u: f64, v: f64| {
        img.get_pixel((u * f64::from(W)) as u32, (v * f64::from(H)) as u32)
            .0[0]
    };
    assert!(at(&out, 0.2, 0.2) > at(&plain, 0.2, 0.2) + 30, "painted");
    assert!(at(&out, 0.8, 0.2) > at(&plain, 0.8, 0.2) + 30, "painted");
    assert_eq!(at(&out, 0.5, 0.2), at(&plain, 0.5, 0.2), "rubbed out");
    assert_eq!(at(&out, 0.2, 0.7), at(&plain, 0.2, 0.7), "never painted");
}

#[test]
fn every_layer_is_read_by_its_own_mask() {
    // Eight brush masks, each a dot in its own column: each darkens only its own.
    let plain = render(&EditSettings::default_settings());
    let masks: Vec<Mask> = (0..8)
        .map(|i| {
            let u = (f64::from(i) + 0.5) / 8.0;
            brush(vec![stroke(&[[u, 0.5]], 0.04, false)], -1.0)
        })
        .collect();
    let out = render(&with(masks));
    for i in 0..8u32 {
        let x = (f64::from(i) + 0.5) / 8.0 * f64::from(W);
        let p = out.get_pixel(x as u32, H / 2).0[0];
        assert!(p + 20 < plain.get_pixel(x as u32, H / 2).0[0], "dot {i}");
        assert_eq!(
            out.get_pixel(x as u32, H / 2 + 30),
            plain.get_pixel(x as u32, H / 2 + 30),
            "below dot {i}"
        );
    }
}

/// A mask over the whole photo, narrowed by `range`, lifting exposure a stop.
fn everywhere(range: lf_settings::masks::Range) -> Mask {
    Mask {
        shape: Shape::Linear {
            x0: 0.5,
            y0: 1.5,
            x1: 0.5,
            y1: 2.0,
        },
        invert: false,
        range,
        local: Local {
            exposure: 1.0,
            ..Local::default()
        },
    }
}

#[test]
fn a_range_narrows_the_mask_to_tones_or_colours() {
    use lf_settings::masks::Range;
    // Four patches: dark grey, light grey, sky blue, red.
    let patches = [
        [40u8, 40, 40],
        [200, 200, 200],
        [70, 120, 200],
        [200, 50, 40],
    ];
    let img = RgbaImage::from_fn(W, H, |x, _| {
        let [r, g, b] = patches[(x * 4 / W) as usize];
        Rgba([r, g, b, 255])
    });
    let at = |i: u32| (W / 8 + i * W / 4, H / 2);
    let mut r = Renderer::new(shared_gpu(), &img, W, H).unwrap();
    let plain = r.render(&EditSettings::default_settings()).unwrap();
    let lifted = |out: &RgbaImage, i: u32| {
        let (x, y) = at(i);
        out.get_pixel(x, y).0[1] > plain.get_pixel(x, y).0[1] + 8
    };

    let bright = r
        .render(&with(vec![everywhere(Range::luminance())]))
        .unwrap();
    assert!(!lifted(&bright, 0), "the dark patch moved");
    assert!(lifted(&bright, 1), "the light patch did not move");

    let blue = r.render(&with(vec![everywhere(Range::colour())])).unwrap();
    assert!(!lifted(&blue, 0) && !lifted(&blue, 1), "a grey moved");
    assert!(lifted(&blue, 2), "the blue did not move");
    assert!(!lifted(&blue, 3), "the red moved");

    // The CPU's weights agree.
    let rgb = |p: [u8; 3]| p.map(|v| f64::from(v) / 255.0);
    assert_eq!(Range::colour().weight(rgb(patches[2])), 1.0);
    assert_eq!(Range::colour().weight(rgb(patches[3])), 0.0);
    assert_eq!(Range::luminance().weight(rgb(patches[0])), 0.0);
}
