//! Where each output pixel comes from, ported from the web editor's `geometry.ts`.
//!
//! The photo is turned (quarter turns, flips), straightened by `angle` about its centre, then
//! cropped; the crop is in fractions of the turned image. All of it is one affine map from
//! output UV (0..1 across the crop) to source UV (0..1 across the file).
//!
//! Matrices are 3x3 affine, row-major: `[a, b, c, d, e, f]` = `[[a b c] [d e f] [0 0 1]]`.

use crate::{Crop, EditSettings, Rotate};

pub type Affine = [f64; 6];

pub const IDENTITY: Affine = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

pub fn multiply(m: &Affine, n: &Affine) -> Affine {
    [
        m[0] * n[0] + m[1] * n[3],
        m[0] * n[1] + m[1] * n[4],
        m[0] * n[2] + m[1] * n[5] + m[2],
        m[3] * n[0] + m[4] * n[3],
        m[3] * n[1] + m[4] * n[4],
        m[3] * n[2] + m[4] * n[5] + m[5],
    ]
}

pub fn invert(m: &Affine) -> Affine {
    let det = m[0] * m[4] - m[1] * m[3];
    if det.abs() < 1e-12 {
        return IDENTITY;
    }
    let a = m[4] / det;
    let b = -m[1] / det;
    let d = -m[3] / det;
    let e = m[0] / det;
    [a, b, -(a * m[2] + b * m[5]), d, e, -(d * m[2] + e * m[5])]
}

pub fn apply(m: &Affine, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
}

fn translate(x: f64, y: f64) -> Affine {
    [1.0, 0.0, x, 0.0, 1.0, y]
}

fn scale(x: f64, y: f64) -> Affine {
    [x, 0.0, 0.0, 0.0, y, 0.0]
}

fn rotation(rad: f64) -> Affine {
    [rad.cos(), -rad.sin(), 0.0, rad.sin(), rad.cos(), 0.0]
}

/// The settings that decide the geometry (the web's `Pick<EditSettings, ...>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub rotate: Rotate,
    pub flip_h: bool,
    pub flip_v: bool,
    pub angle: f64,
    pub crop: Crop,
}

impl From<&EditSettings> for Geometry {
    fn from(s: &EditSettings) -> Self {
        Geometry {
            rotate: s.rotate,
            flip_h: s.flip_h,
            flip_v: s.flip_v,
            angle: s.angle,
            crop: s.crop,
        }
    }
}

/// Size of the photo after quarter turns.
pub fn turned_size(width: u32, height: u32, rotate: Rotate) -> (u32, u32) {
    if matches!(rotate, Rotate::R90 | Rotate::R270) {
        (height, width)
    } else {
        (width, height)
    }
}

/// Source pixels to turned pixels (quarter turns clockwise, then flips).
fn turn_matrix(width: u32, height: u32, g: &Geometry) -> Affine {
    let (w, h) = (width as f64, height as f64);
    let (tw, th) = turned_size(width, height, g.rotate);
    let (tw, th) = (tw as f64, th as f64);
    // Rotate about the origin, then move back into the positive quadrant.
    let mut m = match g.rotate {
        Rotate::R0 => IDENTITY,
        Rotate::R90 => [0.0, -1.0, h, 1.0, 0.0, 0.0],
        Rotate::R180 => [-1.0, 0.0, w, 0.0, -1.0, h],
        Rotate::R270 => [0.0, 1.0, 0.0, -1.0, 0.0, w],
    };
    if g.flip_h {
        m = multiply(&[-1.0, 0.0, tw, 0.0, 1.0, 0.0], &m);
    }
    if g.flip_v {
        m = multiply(&[1.0, 0.0, 0.0, 0.0, -1.0, th], &m);
    }
    m
}

/// Output UV to source UV. `crop` overrides the settings' crop (the crop tool shows the whole
/// turned frame while you drag).
pub fn output_to_source(width: u32, height: u32, g: &Geometry, crop: &Crop) -> Affine {
    let (tw, th) = turned_size(width, height, g.rotate);
    let (tw, th) = (tw as f64, th as f64);
    // Output UV to turned pixels (in the straightened frame).
    let from_output = multiply(
        &translate(crop.x * tw, crop.y * th),
        &scale(crop.w * tw, crop.h * th),
    );
    // Undo the straightening: rotate by -angle about the centre.
    let unrotate = multiply(
        &translate(tw / 2.0, th / 2.0),
        &multiply(
            &rotation((-g.angle * std::f64::consts::PI) / 180.0),
            &translate(-tw / 2.0, -th / 2.0),
        ),
    );
    // Turned pixels to source pixels to source UV.
    let unturn = invert(&turn_matrix(width, height, g));
    multiply(
        &scale(1.0 / width as f64, 1.0 / height as f64),
        &multiply(&unturn, &multiply(&unrotate, &from_output)),
    )
}

/// Pixel size of the output (crop of the turned photo at full resolution).
pub fn output_size(width: u32, height: u32, g: &Geometry) -> (u32, u32) {
    let (tw, th) = turned_size(width, height, g.rotate);
    (
        ((g.crop.w * tw as f64).round() as u32).max(1),
        ((g.crop.h * th as f64).round() as u32).max(1),
    )
}

/// Width / height for an aspect name, given the turned photo's own ratio ("original"); `None` = free.
pub fn aspect_ratio(aspect: &str, turned_ratio: f64) -> Option<f64> {
    if aspect == "free" {
        return None;
    }
    if aspect == "original" {
        return Some(turned_ratio);
    }
    // The web matches /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.
    let number = |s: &str| -> Option<f64> {
        let mut parts = s.split('.');
        let whole = parts.next()?;
        let frac = parts.next();
        let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if parts.next().is_some() || !digits(whole) || frac.is_some_and(|f| !digits(f)) {
            return None;
        }
        s.parse().ok()
    };
    if let Some((a, b)) = aspect.split_once(':')
        && let (Some(a), Some(b)) = (number(a), number(b))
    {
        return Some(a / b);
    }
    Some(turned_ratio)
}

/// True when the crop (fractions of the turned frame) lies inside the straightened photo.
pub fn crop_inside(crop: &Crop, tw: f64, th: f64, angle: f64) -> bool {
    let rad = (angle * std::f64::consts::PI) / 180.0;
    let cos = rad.cos();
    let sin = rad.sin();
    let corners = [
        (crop.x, crop.y),
        (crop.x + crop.w, crop.y),
        (crop.x, crop.y + crop.h),
        (crop.x + crop.w, crop.y + crop.h),
    ];
    corners.iter().all(|&(u, v)| {
        let x = u * tw - tw / 2.0;
        let y = v * th - th / 2.0;
        // Rotate the corner back into the photo's own frame.
        let px = cos * x + sin * y;
        let py = -sin * x + cos * y;
        px.abs() <= tw / 2.0 + 0.5 && py.abs() <= th / 2.0 + 0.5
    })
}

/// The largest crop of the given aspect (width / height in pixels) that fits inside the photo
/// straightened by `angle`, centred.
pub fn largest_crop(tw: f64, th: f64, angle: f64, ratio: Option<f64>) -> Crop {
    let rad = ((angle * std::f64::consts::PI) / 180.0).abs();
    let cos = rad.cos();
    let sin = rad.sin();
    let a = ratio.unwrap_or(tw / th);
    // h fits when: a*h*cos + h*sin <= tw and a*h*sin + h*cos <= th.
    let mut h = (tw / (a * cos + sin)).min(th / (a * sin + cos)).min(th);
    let mut w = a * h;
    if w > tw {
        w = tw;
        h = w / a;
    }
    let cw = w / tw;
    let ch = h / th;
    Crop {
        x: (1.0 - cw) / 2.0,
        y: (1.0 - ch) / 2.0,
        w: cw,
        h: ch,
    }
}

/// Keeps a crop valid after a straighten change: same centre and aspect when it still fits,
/// else shrunk about its centre until it does, else the largest centred crop.
pub fn fit_crop(crop: &Crop, tw: f64, th: f64, angle: f64) -> Crop {
    if crop_inside(crop, tw, th, angle) {
        return *crop;
    }
    let cx = crop.x + crop.w / 2.0;
    let cy = crop.y + crop.h / 2.0;
    let scaled = |k: f64| Crop {
        x: cx - (crop.w * k) / 2.0,
        y: cy - (crop.h * k) / 2.0,
        w: crop.w * k,
        h: crop.h * k,
    };
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..24 {
        let k = (lo + hi) / 2.0;
        if crop_inside(&scaled(k), tw, th, angle) {
            lo = k;
        } else {
            hi = k;
        }
    }
    if lo > 0.05 {
        return scaled(lo);
    }
    largest_crop(tw, th, angle, Some((crop.w * tw) / (crop.h * th)))
}

/// Which way a quarter turn goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnDir {
    Clockwise,
    Anticlockwise,
}

/// Settings after a quarter turn: the crop turns with the photo.
pub fn turn_settings(g: &Geometry, dir: TurnDir) -> Geometry {
    let step: i32 = match dir {
        TurnDir::Clockwise => 1,
        TurnDir::Anticlockwise => -1,
    };
    let rotate = match (((g.rotate as i32 + step * 90) % 360) + 360) % 360 {
        0 => Rotate::R0,
        90 => Rotate::R90,
        180 => Rotate::R180,
        _ => Rotate::R270,
    };
    let c = g.crop;
    // Turning the frame clockwise moves (x, y) to (1 - y - h, x).
    let crop = match dir {
        TurnDir::Clockwise => Crop {
            x: 1.0 - c.y - c.h,
            y: c.x,
            w: c.h,
            h: c.w,
        },
        TurnDir::Anticlockwise => Crop {
            x: c.y,
            y: 1.0 - c.x - c.w,
            w: c.h,
            h: c.w,
        },
    };
    // A flip seen through a quarter turn is the other flip.
    Geometry {
        rotate,
        crop,
        flip_h: g.flip_v,
        flip_v: g.flip_h,
        angle: g.angle,
    }
}
