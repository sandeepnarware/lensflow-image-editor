//! Local adjustments ("masks"): areas of the photo (a linear or radial gradient, or painted
//! with a brush), each with its own exposure, contrast, tone, white balance, saturation and
//! clarity, as Lightroom's masking panel has them. A brush mask's strokes are drawn into a
//! coverage map ([`rasterize`]) that the develop pass samples.
//!
//! They are desktop-only and live in `desktop.masks` (as `desktop.develop` does), so the web
//! editor keeps them in a sidecar it rewrites but does not draw them. Read them with
//! [`Masks::from_settings`] (anything unreadable is dropped, never an error) and store them
//! with [`Masks::write_into`] (no masks, no key).
//!
//! Shapes are in source-photo fractions (the space the spots use), so they stay on the same
//! part of the photo when it is cropped, turned or flipped. Lengths that must be the same in
//! both directions (a radial's radii, nothing else) are fractions of the photo's *height*;
//! the shader multiplies x by the photo's aspect to make its space square.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::EditSettings;

/// The most masks a photo can have (the shader's array).
pub const MAX_MASKS: usize = 8;

/// The area a mask covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Shape {
    /// A graduated filter: full effect behind the start line (`x0`, `y0`), none past the end
    /// line (`x1`, `y1`), fading between them. Points in photo fractions.
    Linear { x0: f64, y0: f64, x1: f64, y1: f64 },
    /// An ellipse around (`cx`, `cy`) (photo fractions) with radii `rx`, `ry` (fractions of
    /// the photo's height), turned by `angle` degrees. Full effect inside; `feather` sets how
    /// far in from the edge the fade starts.
    Radial {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        angle: f64,
        feather: f64,
    },
    /// Painted by hand: the strokes, in the order they were made (an erasing stroke takes
    /// away what the ones before it painted).
    Brush { strokes: Vec<Stroke> },
    /// Found by AI ("subject", "sky", "people", "hair", "face-skin", "body-skin",
    /// "clothes"): coverage as a picture of the photo (lens-corrected,
    /// before crop and turn), `width` × `height` 8-bit values, run-length coded then base64
    /// ([`encode_coverage`]).
    Image {
        what: String,
        width: u32,
        height: u32,
        data: String,
    },
}

/// The longest edge an AI mask's picture keeps (the brush layers' size).
pub const MAX_IMAGE_EDGE: u32 = 512;

/// `values` (`width` × `height`, row by row) as an AI mask's `data`: runs of (count, value)
/// pairs, counts 1..=255, in base64. A mask is mostly long runs of 0 and 255, so it stays
/// small in the sidecar.
pub fn encode_coverage(values: &[u8]) -> String {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < values.len() {
        let v = values[i];
        let mut n = 1;
        while n < 255 && i + n < values.len() && values[i + n] == v {
            n += 1;
        }
        runs.push(n as u8);
        runs.push(v);
        i += n;
    }
    base64_encode(&runs)
}

/// The coverage an AI mask's `data` holds, if it reads as exactly `len` values.
pub fn decode_coverage(data: &str, len: usize) -> Option<Vec<u8>> {
    let runs = base64_decode(data)?;
    if runs.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    for pair in runs.chunks(2) {
        if pair[0] == 0 || out.len() + pair[0] as usize > len {
            return None;
        }
        out.extend(std::iter::repeat_n(pair[1], pair[0] as usize));
    }
    (out.len() == len).then_some(out)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// An AI mask's coverage at photo fraction `(u, v)`, bilinear, 0..1.
pub fn image_weight(width: u32, height: u32, values: &[u8], (u, v): (f64, f64)) -> f64 {
    if width == 0 || height == 0 || values.len() != (width * height) as usize {
        return 0.0;
    }
    let (w, h) = (f64::from(width), f64::from(height));
    let x = (u * w - 0.5).clamp(0.0, w - 1.0);
    let y = (v * h - 0.5).clamp(0.0, h - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = (
        (x0 + 1).min(width as usize - 1),
        (y0 + 1).min(height as usize - 1),
    );
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let at = |xx: usize, yy: usize| f64::from(values[yy * width as usize + xx]) / 255.0;
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// The most strokes a brush mask keeps, and the most points one stroke keeps: a sidecar
/// stays small and a redraw stays quick however long someone paints.
pub const MAX_STROKES: usize = 400;
pub const MAX_POINTS: usize = 4000;

/// One drag of the brush: a line through `points` (photo fractions), `size` across on each
/// side (a radius, as a fraction of the photo's height), soft for its outer `feather`
/// percent, laid down at `flow` percent strength (or taken away, when `erase`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    pub points: Vec<[f64; 2]>,
    pub size: f64,
    #[serde(default = "half")]
    pub feather: f64,
    #[serde(default = "full")]
    pub flow: f64,
    #[serde(default)]
    pub erase: bool,
}

fn half() -> f64 {
    50.0
}

fn full() -> f64 {
    100.0
}

impl Stroke {
    /// The same stroke made safe: finite points (a little outside the photo is allowed),
    /// a size and percentages in range; `None` with no point left.
    fn clamped(mut self) -> Option<Stroke> {
        self.points.retain(|p| p[0].is_finite() && p[1].is_finite());
        self.points.truncate(MAX_POINTS);
        for p in &mut self.points {
            *p = [p[0].clamp(-1.0, 2.0), p[1].clamp(-1.0, 2.0)];
        }
        if self.points.is_empty() {
            return None;
        }
        let pct = |v: f64, default: f64| {
            if v.is_finite() {
                v.clamp(0.0, 100.0)
            } else {
                default
            }
        };
        self.size = if self.size.is_finite() {
            self.size.clamp(0.001, 1.0)
        } else {
            0.05
        };
        self.feather = pct(self.feather, 50.0);
        self.flow = pct(self.flow, 100.0);
        Some(self)
    }

    /// How much of this stroke reaches `p` (square space: x × aspect), 0..=1 (before its
    /// flow): full within the hard core, fading to nothing at `size`.
    fn reach(&self, p: (f64, f64), aspect: f64) -> f64 {
        let d = self.distance(p, aspect);
        let inner = self.size * (1.0 - self.feather / 100.0);
        if d >= self.size {
            0.0
        } else {
            1.0 - smoothstep(inner, self.size + 1e-9, d)
        }
    }

    /// The distance from `p` (square space) to the stroke's line.
    fn distance(&self, p: (f64, f64), aspect: f64) -> f64 {
        let pts: Vec<(f64, f64)> = self.points.iter().map(|q| (q[0] * aspect, q[1])).collect();
        if pts.len() == 1 {
            return (p.0 - pts[0].0).hypot(p.1 - pts[0].1);
        }
        pts.windows(2)
            .map(|w| segment_distance(p, w[0], w[1]))
            .fold(f64::INFINITY, f64::min)
    }
}

fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let d = (b.0 - a.0, b.1 - a.1);
    let len2 = d.0 * d.0 + d.1 * d.1;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * d.0 + (p.1 - a.1) * d.1) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - a.0 - t * d.0).hypot(p.1 - a.1 - t * d.1)
}

/// Lays `stroke` over `amount` (what the strokes before it left): painting adds its flow
/// towards full, erasing takes its flow away.
fn lay(amount: f64, stroke: &Stroke, reach: f64) -> f64 {
    let k = reach * stroke.flow / 100.0;
    if stroke.erase {
        amount * (1.0 - k)
    } else {
        amount + (1.0 - amount) * k
    }
}

/// How much `strokes` cover `p` (photo fractions) of a photo `aspect` wide: 0..=1.
pub fn brush_weight(strokes: &[Stroke], (u, v): (f64, f64), aspect: f64) -> f64 {
    let p = (u * aspect, v);
    strokes
        .iter()
        .fold(0.0, |a, s| lay(a, s, s.reach(p, aspect)))
}

/// `strokes` drawn into a `width`×`height` coverage map (row by row, 0 none to 255 full),
/// each pixel sampled at its centre, as [`brush_weight`] would give it. The develop pass
/// samples this; the photo's aspect is `width / height`. Each stroke only visits the pixels
/// within its reach.
pub fn rasterize(strokes: &[Stroke], width: usize, height: usize) -> Vec<u8> {
    let mut amount = vec![0f32; width * height];
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let aspect = width as f64 / height as f64;
    let mut reach = vec![0f32; width * height];
    for stroke in strokes {
        // The stroke's box, in pixels, grown by its size.
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &stroke.points {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let grow_x = stroke.size / aspect;
        let px = |v: f64, n: usize| (v * n as f64).floor().clamp(0.0, n as f64) as usize;
        let (ix0, ix1) = (px(x0 - grow_x, width), px(x1 + grow_x, width) + 1);
        let (iy0, iy1) = (
            px(y0 - stroke.size, height),
            px(y1 + stroke.size, height) + 1,
        );
        let (ix1, iy1) = (ix1.min(width), iy1.min(height));
        // Each segment visits its own box, so a long stroke does not visit every pixel of
        // its whole box once per segment.
        let pts: Vec<[f64; 2]> = if stroke.points.len() == 1 {
            vec![stroke.points[0], stroke.points[0]]
        } else {
            stroke.points.clone()
        };
        for w in pts.windows(2) {
            let (a, b) = ((w[0][0] * aspect, w[0][1]), (w[1][0] * aspect, w[1][1]));
            let sx0 = px(w[0][0].min(w[1][0]) - grow_x, width).max(ix0);
            let sx1 = (px(w[0][0].max(w[1][0]) + grow_x, width) + 1).min(ix1);
            let sy0 = px(w[0][1].min(w[1][1]) - stroke.size, height).max(iy0);
            let sy1 = (px(w[0][1].max(w[1][1]) + stroke.size, height) + 1).min(iy1);
            let inner = stroke.size * (1.0 - stroke.feather / 100.0);
            for y in sy0..sy1 {
                let v = (y as f64 + 0.5) / height as f64;
                for x in sx0..sx1 {
                    let u = (x as f64 + 0.5) / width as f64;
                    let d = segment_distance((u * aspect, v), a, b);
                    if d >= stroke.size {
                        continue;
                    }
                    let r = (1.0 - smoothstep(inner, stroke.size + 1e-9, d)) as f32;
                    let cell = &mut reach[y * width + x];
                    *cell = cell.max(r);
                }
            }
        }
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                let i = y * width + x;
                if reach[i] > 0.0 {
                    amount[i] = lay(f64::from(amount[i]), stroke, f64::from(reach[i])) as f32;
                    reach[i] = 0.0;
                }
            }
        }
    }
    amount
        .into_iter()
        .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// What a mask does where it applies: the global sliders' units (exposure in stops, the rest
/// -100..100), added to the global values there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Local {
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    pub temperature: f64,
    pub tint: f64,
    pub saturation: f64,
    pub clarity: f64,
}

impl Local {
    /// The sliders, in panel order: label, value, and whether it is the exposure (stops).
    pub fn sliders(&mut self) -> [(&'static str, &mut f64); 10] {
        [
            ("Exposure", &mut self.exposure),
            ("Contrast", &mut self.contrast),
            ("Highlights", &mut self.highlights),
            ("Shadows", &mut self.shadows),
            ("Whites", &mut self.whites),
            ("Blacks", &mut self.blacks),
            ("Temperature", &mut self.temperature),
            ("Tint", &mut self.tint),
            ("Saturation", &mut self.saturation),
            ("Clarity", &mut self.clarity),
        ]
    }

    fn clamped(mut self) -> Local {
        let fix = |v: f64, lo: f64, hi: f64| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        self.exposure = fix(self.exposure, -4.0, 4.0);
        for (_, v) in self.sliders().into_iter().skip(1) {
            *v = fix(*v, -100.0, 100.0);
        }
        self
    }
}

/// Narrows a mask to part of the photo's tones or colours, inside its shape: Lightroom's range
/// masks. Measured on the photo as shot at each pixel (before the edits), so moving a slider
/// never moves the mask. Numbers are percentages; the hue is in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Range {
    /// The whole shape.
    #[default]
    All,
    /// Brightness between `low` and `high` (0..100), fading over `smooth` beyond them.
    Luminance { low: f64, high: f64, smooth: f64 },
    /// Colours whose hue is within `width` degrees of `hue`, fading over `smooth` degrees;
    /// greys never match.
    Colour { hue: f64, width: f64, smooth: f64 },
}

impl Range {
    /// A brightness range taking the highlights.
    pub fn luminance() -> Range {
        Range::Luminance {
            low: 60.0,
            high: 100.0,
            smooth: 10.0,
        }
    }

    /// A colour range taking blues (skies).
    pub fn colour() -> Range {
        Range::Colour {
            hue: 215.0,
            width: 25.0,
            smooth: 20.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Range::All => "Whole shape",
            Range::Luminance { .. } => "Brightness range",
            Range::Colour { .. } => "Colour range",
        }
    }

    /// The same range with safe numbers.
    fn clamped(self) -> Range {
        let fix =
            |v: f64, lo: f64, hi: f64, or: f64| if v.is_finite() { v.clamp(lo, hi) } else { or };
        match self {
            Range::All => Range::All,
            Range::Luminance { low, high, smooth } => {
                let low = fix(low, 0.0, 100.0, 0.0);
                Range::Luminance {
                    low,
                    high: fix(high, low, 100.0, 100.0),
                    smooth: fix(smooth, 1.0, 50.0, 10.0),
                }
            }
            Range::Colour { hue, width, smooth } => Range::Colour {
                hue: if hue.is_finite() {
                    hue.rem_euclid(360.0)
                } else {
                    0.0
                },
                width: fix(width, 1.0, 180.0, 25.0),
                smooth: fix(smooth, 1.0, 90.0, 20.0),
            },
        }
    }

    /// How much of the mask a colour (sRGB, 0..1) keeps: the GPU's `range_weight`.
    pub fn weight(self, rgb: [f64; 3]) -> f64 {
        match self {
            Range::All => 1.0,
            Range::Luminance { low, high, smooth } => {
                let y = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]) * 100.0;
                let below = if low <= 0.0 {
                    1.0
                } else {
                    smoothstep(low - smooth, low, y)
                };
                let above = if high >= 100.0 {
                    1.0
                } else {
                    1.0 - smoothstep(high, high + smooth, y)
                };
                below * above
            }
            Range::Colour { hue, width, smooth } => {
                let (a, b) = opponent(rgb);
                let chroma = a.hypot(b);
                let (ta, tb) = (hue.to_radians().cos(), hue.to_radians().sin());
                let cos = ((a * ta + b * tb) / chroma.max(1e-5)).clamp(-1.0, 1.0);
                let off = cos.acos().to_degrees();
                (1.0 - smoothstep(width, width + smooth, off)) * smoothstep(0.02, 0.08, chroma)
            }
        }
    }
}

/// A colour's place on the colour wheel: red along `a`, green and blue a third of a turn
/// either side (the hue angle is `atan2(b, a)`, 0° red, 120° green, 240° blue).
pub fn opponent(rgb: [f64; 3]) -> (f64, f64) {
    (
        rgb[0] - 0.5 * (rgb[1] + rgb[2]),
        0.866_025_4 * (rgb[1] - rgb[2]),
    )
}

/// The hue of a colour in degrees (0 red, 120 green, 240 blue), if it has one.
pub fn hue_of(rgb: [f64; 3]) -> Option<f64> {
    let (a, b) = opponent(rgb);
    (a.hypot(b) > 0.02).then(|| b.atan2(a).to_degrees().rem_euclid(360.0))
}

/// One local adjustment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mask {
    pub shape: Shape,
    /// Everywhere except the shape instead.
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub local: Local,
    /// Only part of the shape's tones or colours ([`Range`]).
    #[serde(default)]
    pub range: Range,
}

impl Mask {
    /// A new graduated filter from the top down to the middle.
    pub fn linear() -> Mask {
        Mask {
            shape: Shape::Linear {
                x0: 0.5,
                y0: 0.15,
                x1: 0.5,
                y1: 0.5,
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        }
    }

    /// A new ellipse in the middle, a third of the photo's height across.
    pub fn radial() -> Mask {
        Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.3,
                ry: 0.2,
                angle: 0.0,
                feather: 50.0,
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        }
    }

    /// A new brush mask with nothing painted yet.
    pub fn brush() -> Mask {
        Mask {
            shape: Shape::Brush {
                strokes: Vec::new(),
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        }
    }

    /// What the mask is called in the panel.
    pub fn kind_name(&self) -> &'static str {
        match &self.shape {
            Shape::Linear { .. } => "Linear gradient",
            Shape::Radial { .. } => "Radial gradient",
            Shape::Brush { .. } => "Brush",
            Shape::Image { what, .. } => match what.as_str() {
                "sky" => "Sky",
                "people" => "People",
                "hair" => "Hair",
                "face-skin" => "Face skin",
                "body-skin" => "Body skin",
                "clothes" => "Clothes",
                _ => "Subject",
            },
        }
    }

    /// The same mask with every number made safe (finite, in range): points may sit a little
    /// outside the photo, radii are positive, angles wrap.
    fn clamped(mut self) -> Option<Mask> {
        let pos = |v: f64| {
            if v.is_finite() {
                v.clamp(-1.0, 2.0)
            } else {
                0.5
            }
        };
        self.shape = match self.shape {
            Shape::Linear { x0, y0, x1, y1 } => {
                let (x0, y0, x1, y1) = (pos(x0), pos(y0), pos(x1), pos(y1));
                // Two lines on top of each other say nothing about a direction.
                if (x1 - x0).hypot(y1 - y0) < 1e-6 {
                    return None;
                }
                Shape::Linear { x0, y0, x1, y1 }
            }
            Shape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
            } => {
                let radius = |v: f64| {
                    if v.is_finite() {
                        v.clamp(0.002, 4.0)
                    } else {
                        0.2
                    }
                };
                Shape::Radial {
                    cx: pos(cx),
                    cy: pos(cy),
                    rx: radius(rx),
                    ry: radius(ry),
                    angle: if angle.is_finite() {
                        angle.rem_euclid(360.0)
                    } else {
                        0.0
                    },
                    feather: if feather.is_finite() {
                        feather.clamp(0.0, 100.0)
                    } else {
                        50.0
                    },
                }
            }
            Shape::Brush { strokes } => Shape::Brush {
                strokes: strokes
                    .into_iter()
                    .filter_map(Stroke::clamped)
                    .take(MAX_STROKES)
                    .collect(),
            },
            Shape::Image {
                what,
                width,
                height,
                data,
            } => {
                // A picture that does not read as its size is no mask at all.
                if width == 0
                    || height == 0
                    || width.max(height) > MAX_IMAGE_EDGE
                    || decode_coverage(&data, (width * height) as usize).is_none()
                {
                    return None;
                }
                Shape::Image {
                    what,
                    width,
                    height,
                    data,
                }
            }
        };
        self.local = self.local.clamped();
        self.range = self.range.clamped();
        Some(self)
    }
}

/// A photo's masks, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Masks(pub Vec<Mask>);

impl Masks {
    /// Read `desktop.masks`; a mask that cannot be read (or past [`MAX_MASKS`]) is left out.
    pub fn from_settings(s: &EditSettings) -> Masks {
        let Some(Value::Array(items)) = s.desktop.as_ref().and_then(|d| d.get("masks")) else {
            return Masks::default();
        };
        Masks(
            items
                .iter()
                .filter_map(|v| serde_json::from_value::<Mask>(v.clone()).ok())
                .filter_map(Mask::clamped)
                .take(MAX_MASKS)
                .collect(),
        )
    }

    /// Store into `desktop.masks`; none is stored as no key, and an emptied `desktop` is
    /// dropped.
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.0.is_empty() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("masks");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        let value = serde_json::to_value(&self.0).expect("masks serialise");
        s.desktop
            .get_or_insert_with(Default::default)
            .insert("masks".to_string(), value);
    }
}

/// How much `mask` applies at `(u, v)` (photo fractions) of a photo `aspect` wide per unit of
/// height: 0..=1. The shader's `mask_weight`, for tests and for the panel's overlay.
pub fn weight(mask: &Mask, (u, v): (f64, f64), aspect: f64) -> f64 {
    let p = (u * aspect, v);
    let w = match &mask.shape {
        Shape::Brush { strokes } => brush_weight(strokes, (u, v), aspect),
        Shape::Image {
            width,
            height,
            data,
            ..
        } => decode_coverage(data, (*width * *height) as usize)
            .map_or(0.0, |values| image_weight(*width, *height, &values, (u, v))),
        &Shape::Linear { x0, y0, x1, y1 } => {
            let a = (x0 * aspect, y0);
            let d = (x1 * aspect - a.0, y1 - a.1);
            let t = ((p.0 - a.0) * d.0 + (p.1 - a.1) * d.1) / (d.0 * d.0 + d.1 * d.1);
            1.0 - smoothstep(0.0, 1.0, t)
        }
        &Shape::Radial {
            cx,
            cy,
            rx,
            ry,
            angle,
            feather,
        } => {
            let (s, c) = angle.to_radians().sin_cos();
            let q = (p.0 - cx * aspect, p.1 - cy);
            // Into the ellipse's own axes.
            let q = (q.0 * c + q.1 * s, -q.0 * s + q.1 * c);
            let r = (q.0 / rx).hypot(q.1 / ry);
            1.0 - smoothstep(1.0 - feather / 100.0, 1.0 + 1e-4, r)
        }
    };
    if mask.invert { 1.0 - w } else { w }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ai_mask_round_trips_and_samples_between_pixels() {
        let values: Vec<u8> = (0..6u8).map(|i| if i % 3 == 0 { 0 } else { 255 }).collect();
        let data = encode_coverage(&values);
        assert_eq!(decode_coverage(&data, 6), Some(values.clone()));
        assert_eq!(decode_coverage(&data, 7), None);
        assert_eq!(decode_coverage("!!", 6), None);
        let long = vec![7u8; 600];
        assert_eq!(decode_coverage(&encode_coverage(&long), 600), Some(long));
        // 3 × 2: left column clear, the rest covered.
        assert_eq!(image_weight(3, 2, &values, (0.1, 0.25)), 0.0);
        assert_eq!(image_weight(3, 2, &values, (0.9, 0.75)), 1.0);
        let half = image_weight(3, 2, &values, (1.0 / 3.0, 0.25));
        assert!((half - 0.5).abs() < 1e-9, "{half}");
        let m = Mask {
            shape: Shape::Image {
                what: "sky".into(),
                width: 3,
                height: 2,
                data,
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        };
        assert_eq!(m.kind_name(), "Sky");
        assert!(m.clone().clamped().is_some());
        let broken = Mask {
            shape: Shape::Image {
                what: "subject".into(),
                width: 4,
                height: 2,
                data: encode_coverage(&values),
            },
            ..m
        };
        assert!(broken.clamped().is_none());
    }

    #[test]
    fn a_range_reads_back_safe_and_old_masks_take_the_whole_shape() {
        let m: Mask = serde_json::from_value(serde_json::json!({
            "shape": { "kind": "linear", "x0": 0.5, "y0": 0.1, "x1": 0.5, "y1": 0.5 },
        }))
        .unwrap();
        assert_eq!(m.range, Range::All);
        let wild = Range::Luminance {
            low: 80.0,
            high: 20.0,
            smooth: 0.0,
        }
        .clamped();
        assert_eq!(
            wild,
            Range::Luminance {
                low: 80.0,
                high: 80.0,
                smooth: 1.0
            }
        );
        let hue = Range::Colour {
            hue: -30.0,
            width: 500.0,
            smooth: f64::NAN,
        }
        .clamped();
        assert_eq!(
            hue,
            Range::Colour {
                hue: 330.0,
                width: 180.0,
                smooth: 20.0
            }
        );
        assert_eq!(hue_of([1.0, 0.0, 0.0]).map(f64::round), Some(0.0));
        assert_eq!(hue_of([0.0, 0.0, 1.0]).map(f64::round), Some(240.0));
        assert_eq!(hue_of([0.5, 0.5, 0.5]), None);
    }

    #[test]
    fn masks_round_trip_through_desktop_and_none_leaves_no_key() {
        let mut s = EditSettings::default_settings();
        assert_eq!(Masks::from_settings(&s), Masks::default());
        let mut radial = Mask::radial();
        radial.local.exposure = 0.7;
        radial.invert = true;
        let masks = Masks(vec![Mask::linear(), radial]);
        masks.write_into(&mut s);
        assert_eq!(Masks::from_settings(&s), masks);
        // Survives the trip through JSON and normalise, as a sidecar would carry it.
        let json = serde_json::to_string(&s).unwrap();
        let back = crate::normalise(&serde_json::from_str(&json).unwrap());
        assert_eq!(Masks::from_settings(&back), masks);
        // Other desktop keys stay; no masks, no key; nothing left, no desktop.
        Masks::default().write_into(&mut s);
        assert!(s.desktop.is_none());
    }

    #[test]
    fn unreadable_and_out_of_range_masks_are_made_safe() {
        let mut s = EditSettings::default_settings();
        let mut desktop = serde_json::Map::new();
        desktop.insert(
            "masks".into(),
            serde_json::json!([
                {"shape": {"kind": "radial", "cx": 0.5, "cy": 0.5, "rx": -3, "ry": 1e9,
                           "angle": 400, "feather": 250},
                 "local": {"exposure": 9, "contrast": -500}},
                {"shape": {"kind": "star"}},
                "junk",
                {"shape": {"kind": "linear", "x0": 0.2, "y0": 0.2, "x1": 0.2, "y1": 0.2}},
                {"shape": {"kind": "linear", "x0": 0, "y0": 0, "x1": 0, "y1": 1}}
            ]),
        );
        s.desktop = Some(desktop);
        let m = Masks::from_settings(&s);
        assert_eq!(
            m.0.len(),
            2,
            "the star, the junk and the zero-length line are dropped"
        );
        let Shape::Radial {
            rx,
            ry,
            angle,
            feather,
            ..
        } = m.0[0].shape
        else {
            panic!()
        };
        assert_eq!((rx, ry, angle, feather), (0.002, 4.0, 40.0, 100.0));
        assert_eq!(
            (m.0[0].local.exposure, m.0[0].local.contrast),
            (4.0, -100.0)
        );
        assert!(matches!(m.0[1].shape, Shape::Linear { .. }));
    }

    #[test]
    fn no_more_than_eight() {
        let mut s = EditSettings::default_settings();
        Masks(vec![Mask::radial(); 12]).write_into(&mut s);
        assert_eq!(Masks::from_settings(&s).0.len(), MAX_MASKS);
    }

    #[test]
    fn a_linear_mask_fades_from_its_start_to_its_end() {
        let m = Mask {
            shape: Shape::Linear {
                x0: 0.5,
                y0: 0.2,
                x1: 0.5,
                y1: 0.6,
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        };
        assert_eq!(weight(&m, (0.3, 0.1), 1.5), 1.0, "behind the start");
        assert_eq!(weight(&m, (0.9, 0.7), 1.5), 0.0, "past the end");
        let mid = weight(&m, (0.5, 0.4), 1.5);
        assert!((mid - 0.5).abs() < 1e-9, "{mid}");
        let inverted = Mask { invert: true, ..m };
        assert_eq!(weight(&inverted, (0.3, 0.1), 1.5), 0.0);
    }

    #[test]
    fn a_radial_mask_is_round_on_a_wide_photo_and_feathers_at_its_edge() {
        // A circle 0.2 of the height across on a 3:2 photo.
        let m = Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.2,
                ry: 0.2,
                angle: 0.0,
                feather: 0.0,
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        };
        let aspect = 1.5;
        assert_eq!(weight(&m, (0.5, 0.5), aspect), 1.0);
        // 0.19 of the height to the right is 0.19 / 1.5 of the width: inside.
        assert_eq!(weight(&m, (0.5 + 0.19 / aspect, 0.5), aspect), 1.0);
        assert_eq!(weight(&m, (0.5 + 0.21 / aspect, 0.5), aspect), 0.0);
        assert_eq!(weight(&m, (0.5, 0.71), aspect), 0.0);
        let soft = Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.2,
                ry: 0.2,
                angle: 0.0,
                feather: 50.0,
            },
            ..m
        };
        let edge = weight(&soft, (0.5, 0.5 + 0.15), aspect);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
        assert_eq!(
            weight(&soft, (0.5, 0.5 + 0.09), aspect),
            1.0,
            "the core is full"
        );
    }

    #[test]
    fn a_turned_ellipse_follows_its_angle() {
        let m = Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.3,
                ry: 0.05,
                angle: 90.0,
                feather: 0.0,
            },
            invert: false,
            range: Range::All,
            local: Local::default(),
        };
        // Turned a quarter: long up and down, narrow across.
        assert_eq!(weight(&m, (0.5, 0.75), 1.0), 1.0);
        assert_eq!(weight(&m, (0.6, 0.5), 1.0), 0.0);
    }
}

#[cfg(test)]
mod brush_tests {
    use super::*;

    fn stroke(points: &[[f64; 2]], size: f64, erase: bool) -> Stroke {
        Stroke {
            points: points.to_vec(),
            size,
            feather: 0.0,
            flow: 100.0,
            erase,
        }
    }

    #[test]
    fn a_stroke_covers_its_line_and_an_eraser_takes_it_back() {
        let line = stroke(&[[0.2, 0.5], [0.8, 0.5]], 0.1, false);
        let aspect = 1.0;
        assert_eq!(
            brush_weight(std::slice::from_ref(&line), (0.5, 0.5), aspect),
            1.0
        );
        assert_eq!(
            brush_weight(std::slice::from_ref(&line), (0.5, 0.58), aspect),
            1.0
        );
        assert_eq!(
            brush_weight(std::slice::from_ref(&line), (0.5, 0.7), aspect),
            0.0
        );
        assert_eq!(
            brush_weight(std::slice::from_ref(&line), (0.95, 0.5), aspect),
            0.0
        );
        let rubber = stroke(&[[0.5, 0.5]], 0.05, true);
        let both = [line.clone(), rubber];
        assert_eq!(brush_weight(&both, (0.5, 0.5), aspect), 0.0);
        assert_eq!(brush_weight(&both, (0.3, 0.5), aspect), 1.0);
        // Half flow, twice over, builds up to three quarters.
        let soft = Stroke { flow: 50.0, ..line };
        let w = brush_weight(&[soft.clone(), soft], (0.5, 0.5), aspect);
        assert!((w - 0.75).abs() < 1e-9, "{w}");
    }

    #[test]
    fn the_map_is_the_weight_at_each_pixel_centre() {
        let strokes = vec![
            Stroke {
                feather: 60.0,
                ..stroke(&[[0.1, 0.2], [0.6, 0.7], [0.9, 0.3]], 0.12, false)
            },
            Stroke {
                flow: 70.0,
                feather: 30.0,
                ..stroke(&[[0.55, 0.55]], 0.08, true)
            },
        ];
        let (w, h) = (90, 60);
        let map = rasterize(&strokes, w, h);
        assert_eq!(map.len(), w * h);
        let aspect = w as f64 / h as f64;
        for y in 0..h {
            for x in 0..w {
                let uv = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
                let want = (brush_weight(&strokes, uv, aspect) * 255.0).round() as i32;
                let got = i32::from(map[y * w + x]);
                assert!((want - got).abs() <= 1, "({x}, {y}): {got} vs {want}");
            }
        }
        assert!(map.contains(&255) && map.contains(&0));
    }

    #[test]
    fn brush_masks_round_trip_and_bad_strokes_are_dropped() {
        let mut s = EditSettings::default_settings();
        let mut m = Mask::brush();
        m.shape = Shape::Brush {
            strokes: vec![
                stroke(&[[0.1, 0.1], [0.2, 0.2]], 0.05, false),
                stroke(&[], 0.05, false),
                stroke(&[[0.3, 0.3]], 9.0, true),
            ],
        };
        Masks(vec![m]).write_into(&mut s);
        let back = Masks::from_settings(&s);
        let Shape::Brush { strokes } = &back.0[0].shape else {
            panic!("a brush")
        };
        assert_eq!(strokes.len(), 2, "the stroke with no usable point is gone");
        assert_eq!(strokes[1].size, 1.0);
        assert!(strokes[1].erase);
        assert_eq!(back.0[0].kind_name(), "Brush");
        // An empty brush (just added) is kept.
        Masks(vec![Mask::brush()]).write_into(&mut s);
        assert_eq!(Masks::from_settings(&s).0.len(), 1);
    }
}
