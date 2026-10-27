//! The last touches an export puts on a rendered photo before it is encoded: output
//! sharpening for the screen, a watermark (text or a logo), and the colour profile that tells
//! a colour-managed viewer the pixels are sRGB.
//!
//! All of it works on the [`Pixels`] an export holds after resizing, 8-bit or 16-bit alike,
//! so a TIFF gets the same finish as a JPEG.

use std::path::PathBuf;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::imageops::{FilterType, resize, unsharpen};
use image::{ImageBuffer, Rgb, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::db::{CatalogError, Result};

/// A rendered photo, 8 bits a channel (JPEG, PNG, WebP) or 16 (TIFF).
pub enum Pixels {
    Eight(RgbImage),
    Sixteen(ImageBuffer<Rgb<u16>, Vec<u16>>),
}

impl Pixels {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Pixels::Eight(i) => i.dimensions(),
            Pixels::Sixteen(i) => i.dimensions(),
        }
    }

    /// Resized with Lanczos to exactly `w`×`h`.
    pub fn resized(self, w: u32, h: u32) -> Pixels {
        if self.dimensions() == (w, h) {
            return self;
        }
        match self {
            Pixels::Eight(i) => Pixels::Eight(resize(&i, w, h, FilterType::Lanczos3)),
            Pixels::Sixteen(i) => Pixels::Sixteen(resize(&i, w, h, FilterType::Lanczos3)),
        }
    }
}

/// Sharpening for the screen, after resizing (a downsized photo looks softer than the one
/// edited). Lightroom's three amounts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sharpen {
    #[default]
    Off,
    Low,
    Standard,
    High,
}

impl Sharpen {
    pub const ALL: [Sharpen; 4] = [Sharpen::Off, Sharpen::Low, Sharpen::Standard, Sharpen::High];

    pub fn label(self) -> &'static str {
        match self {
            Sharpen::Off => "None",
            Sharpen::Low => "Low",
            Sharpen::Standard => "Standard",
            Sharpen::High => "High",
        }
    }

    /// The unsharp mask's radius, in pixels.
    fn sigma(self) -> Option<f32> {
        match self {
            Sharpen::Off => None,
            Sharpen::Low => Some(0.5),
            Sharpen::Standard => Some(0.8),
            Sharpen::High => Some(1.2),
        }
    }
}

/// Unsharp-masks `pixels` (a 1-level threshold, so flat skies stay smooth).
pub fn sharpen(pixels: Pixels, amount: Sharpen) -> Pixels {
    let Some(sigma) = amount.sigma() else {
        return pixels;
    };
    match pixels {
        Pixels::Eight(i) => Pixels::Eight(unsharpen(&i, sigma, 1)),
        Pixels::Sixteen(i) => Pixels::Sixteen(unsharpen(&i, sigma, 257)),
    }
}

/// Where a watermark sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
    Centre,
}

impl Corner {
    pub const ALL: [Corner; 5] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
        Corner::Centre,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Corner::TopLeft => "Top left",
            Corner::TopRight => "Top right",
            Corner::BottomLeft => "Bottom left",
            Corner::BottomRight => "Bottom right",
            Corner::Centre => "Centre",
        }
    }
}

/// What a watermark shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Stamp {
    /// White text with a soft dark edge, e.g. "© Sharma Studio".
    Text(String),
    /// A picture file (a studio logo, ideally a PNG with transparency).
    Image(PathBuf),
}

/// A watermark: what, how wide (a fraction of the photo's width), how opaque, and where.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    pub stamp: Stamp,
    /// 0.02..=1.0 of the photo's width.
    pub width: f32,
    /// 0.05..=1.0.
    pub opacity: f32,
    pub corner: Corner,
}

/// A watermark ready to stamp: a logo read (once per export run) or the text's font.
pub struct Prepared {
    mark: Watermark,
    logo: Option<RgbaImage>,
}

/// The font text watermarks are set in.
fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("the bundled font parses")
}

impl Prepared {
    /// Reads the logo, if any. An unreadable logo or empty text fails here, once, before any
    /// photo is exported.
    pub fn new(mark: &Watermark) -> Result<Prepared> {
        let logo = match &mark.stamp {
            Stamp::Text(text) if text.trim().is_empty() => {
                return Err(CatalogError::Export(
                    "The watermark has no text. Type some, or turn the watermark off.".into(),
                ));
            }
            Stamp::Text(_) => None,
            Stamp::Image(path) => Some(
                image::open(path)
                    .map_err(|e| {
                        CatalogError::Export(format!(
                            "The watermark picture {} could not be read ({e}).",
                            path.display()
                        ))
                    })?
                    .to_rgba8(),
            ),
        };
        Ok(Prepared {
            mark: Watermark {
                width: mark.width.clamp(0.02, 1.0),
                opacity: mark.opacity.clamp(0.05, 1.0),
                ..mark.clone()
            },
            logo,
        })
    }

    /// The stamp for a photo `photo_w` wide: an RGBA image (premultiplied by nothing; its
    /// alpha is the coverage) of about `width × photo_w` pixels.
    fn stamp(&self, photo_w: u32) -> RgbaImage {
        let want = ((photo_w as f32 * self.mark.width).round() as u32).max(1);
        match (&self.mark.stamp, &self.logo) {
            (_, Some(logo)) => {
                let h = (logo.height() as f32 * want as f32 / logo.width().max(1) as f32)
                    .round()
                    .max(1.0) as u32;
                resize(logo, want, h, FilterType::Lanczos3)
            }
            (Stamp::Text(text), None) => text_stamp(text, want),
            (Stamp::Image(_), None) => RgbaImage::new(1, 1),
        }
    }
}

/// `text` set in white about `width` pixels wide, with a soft dark edge so it reads on light
/// and dark photos alike.
fn text_stamp(text: &str, width: u32) -> RgbaImage {
    let font = font();
    // Measure at a reference size, then scale so the line is `width` wide.
    let measure = |px: f32| {
        let f = font.as_scaled(PxScale::from(px));
        let mut x = 0.0;
        let mut last = None;
        for c in text.chars() {
            let id = f.glyph_id(c);
            if let Some(prev) = last {
                x += f.kern(prev, id);
            }
            x += f.h_advance(id);
            last = Some(id);
        }
        x
    };
    let px = (100.0 * width as f32 / measure(100.0).max(1.0)).max(4.0);
    let scaled = font.as_scaled(PxScale::from(px));
    let pad = (px * 0.08).ceil() as u32 + 1;
    let w = measure(px).ceil() as u32 + 2 * pad;
    let h = (scaled.ascent() - scaled.descent()).ceil() as u32 + 2 * pad;
    let mut cover = vec![0f32; (w * h) as usize];
    let mut x = pad as f32;
    let mut last = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = last {
            x += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(px, point(x, pad as f32 + scaled.ascent()));
        x += scaled.h_advance(id);
        last = Some(id);
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, v| {
                let (px_x, px_y) = (
                    b.min.x as i64 + i64::from(gx),
                    b.min.y as i64 + i64::from(gy),
                );
                if (0..i64::from(w)).contains(&px_x) && (0..i64::from(h)).contains(&px_y) {
                    let i = (px_y as u32 * w + px_x as u32) as usize;
                    cover[i] = (cover[i] + v).min(1.0);
                }
            });
        }
    }
    // The edge: the coverage spread by a pixel or two, drawn dark under the white.
    let reach = (px * 0.04).ceil().max(1.0) as i64;
    let mut out = RgbaImage::new(w, h);
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut edge = 0f32;
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (sx, sy) = (x + dx, y + dy);
                    if (0..i64::from(w)).contains(&sx) && (0..i64::from(h)).contains(&sy) {
                        edge = edge.max(cover[(sy as u32 * w + sx as u32) as usize]);
                    }
                }
            }
            let c = cover[(y as u32 * w + x as u32) as usize];
            // White over a 45 % black edge.
            let a = c + edge * 0.45 * (1.0 - c);
            let v = if a > 0.0 { c / a } else { 0.0 };
            out.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    (v * 255.0).round() as u8,
                    (v * 255.0).round() as u8,
                    (v * 255.0).round() as u8,
                    (a * 255.0).round() as u8,
                ]),
            );
        }
    }
    out
}

/// Stamps the watermark onto `pixels`, a margin of 3 % of the short side in from its corner.
pub fn watermark(pixels: &mut Pixels, mark: &Prepared) {
    let (w, h) = pixels.dimensions();
    let stamp = mark.stamp(w);
    let (sw, sh) = stamp.dimensions();
    let margin = (w.min(h) as f32 * 0.03).round() as i64;
    let (x0, y0) = match mark.mark.corner {
        Corner::TopLeft => (margin, margin),
        Corner::TopRight => (i64::from(w) - i64::from(sw) - margin, margin),
        Corner::BottomLeft => (margin, i64::from(h) - i64::from(sh) - margin),
        Corner::BottomRight => (
            i64::from(w) - i64::from(sw) - margin,
            i64::from(h) - i64::from(sh) - margin,
        ),
        Corner::Centre => (
            (i64::from(w) - i64::from(sw)) / 2,
            (i64::from(h) - i64::from(sh)) / 2,
        ),
    };
    let opacity = mark.mark.opacity;
    for sy in 0..sh {
        for sx in 0..sw {
            let (x, y) = (x0 + i64::from(sx), y0 + i64::from(sy));
            if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
                continue;
            }
            let p = stamp.get_pixel(sx, sy).0;
            let a = f32::from(p[3]) / 255.0 * opacity;
            if a <= 0.0 {
                continue;
            }
            let (x, y) = (x as u32, y as u32);
            match pixels {
                Pixels::Eight(img) => {
                    let d = img.get_pixel_mut(x, y);
                    for (dc, pc) in d.0.iter_mut().zip(p) {
                        let v = f32::from(*dc) * (1.0 - a) + f32::from(pc) * a;
                        *dc = v.round() as u8;
                    }
                }
                Pixels::Sixteen(img) => {
                    let d = img.get_pixel_mut(x, y);
                    for (dc, pc) in d.0.iter_mut().zip(p) {
                        let v = f32::from(*dc) * (1.0 - a) + f32::from(pc) * 257.0 * a;
                        *dc = v.round() as u16;
                    }
                }
            }
        }
    }
}

/// An sRGB (IEC 61966-2-1) ICC v2 display profile, built here so no profile file needs
/// shipping: D50-adapted primaries (Bradford), the D50 white, and the sRGB curve as a
/// 1024-entry table. Embedded in every export so a colour-managed viewer shows the colours
/// as the editor does.
pub fn srgb_icc() -> Vec<u8> {
    fn s15(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz(v: [f64; 3]) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            t.extend(s15(c));
        }
        t
    }
    fn text_desc(s: &str) -> Vec<u8> {
        let mut t = b"desc\0\0\0\0".to_vec();
        t.extend(((s.len() + 1) as u32).to_be_bytes());
        t.extend(s.as_bytes());
        t.push(0);
        // No Unicode or ScriptCode descriptions (counts of zero, 67 bytes of Macintosh name).
        t.extend([0u8; 4 + 4 + 2 + 1 + 67]);
        t
    }
    fn text(s: &str) -> Vec<u8> {
        let mut t = b"text\0\0\0\0".to_vec();
        t.extend(s.as_bytes());
        t.push(0);
        t
    }
    let curve = {
        let mut t = b"curv\0\0\0\0".to_vec();
        let n = 1024u32;
        t.extend(n.to_be_bytes());
        for i in 0..n {
            let v = f64::from(i) / f64::from(n - 1);
            let lin = if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            };
            t.extend(((lin * 65535.0).round() as u16).to_be_bytes());
        }
        t
    };
    const D50: [f64; 3] = [0.9642, 1.0, 0.8249];
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", text_desc("sRGB IEC61966-2.1")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz(D50)),
        (b"rXYZ", xyz([0.436_074_7, 0.222_504_5, 0.013_932_2])),
        (b"gXYZ", xyz([0.385_064_9, 0.716_878_6, 0.097_104_5])),
        (b"bXYZ", xyz([0.143_080_4, 0.060_616_9, 0.714_173_3])),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];
    let table = 4 + 12 * tags.len();
    let mut body = Vec::new();
    let mut entries = Vec::new();
    for (sig, data) in &tags {
        let offset = 128 + table + body.len();
        entries.extend(*sig);
        entries.extend((offset as u32).to_be_bytes());
        entries.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }
    let size = 128 + table + body.len();
    let mut out = Vec::with_capacity(size);
    out.extend((size as u32).to_be_bytes());
    out.extend([0u8; 4]); // preferred CMM
    out.extend([0x02, 0x10, 0x00, 0x00]); // version 2.1
    out.extend(b"mntr");
    out.extend(b"RGB ");
    out.extend(b"XYZ ");
    out.extend([0u8; 12]); // date
    out.extend(b"acsp");
    out.extend([0u8; 4 + 4 + 4 + 4 + 8]); // platform, flags, maker, model, attributes
    out.extend(0u32.to_be_bytes()); // perceptual
    for c in D50 {
        out.extend(s15(c));
    }
    out.extend([0u8; 4 + 16 + 28]); // creator, profile id, reserved
    debug_assert_eq!(out.len(), 128);
    out.extend((tags.len() as u32).to_be_bytes());
    out.extend(entries);
    out.extend(body);
    out
}

/// The TIFF-structured EXIF inside a JPEG APP1 segment (`FF E1 len "Exif\0\0" ...`), which
/// is what PNG's `eXIf` chunk and WebP's `EXIF` chunk hold.
pub fn exif_payload(app1: &[u8]) -> Option<Vec<u8>> {
    let rest = app1.strip_prefix(&[0xff, 0xe1])?;
    let rest = rest.get(2..)?;
    rest.strip_prefix(b"Exif\0\0").map(<[u8]>::to_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: u32, h: u32, v: u8) -> Pixels {
        Pixels::Eight(RgbImage::from_pixel(w, h, Rgb([v, v, v])))
    }

    #[test]
    fn the_icc_profile_is_well_formed() {
        let icc = srgb_icc();
        let be = |at: usize| u32::from_be_bytes(icc[at..at + 4].try_into().unwrap());
        assert_eq!(be(0) as usize, icc.len(), "the size field is the size");
        assert_eq!(&icc[36..40], b"acsp");
        assert_eq!(&icc[12..20], b"mntrRGB ");
        let count = be(128) as usize;
        assert_eq!(count, 9);
        let mut sum = [0f64; 3];
        for i in 0..count {
            let e = 132 + 12 * i;
            let (offset, size) = (be(e + 4) as usize, be(e + 8) as usize);
            assert_eq!(offset % 4, 0, "tags start on 4-byte boundaries");
            assert!(offset + size <= icc.len());
            if matches!(&icc[e..e + 4], b"rXYZ" | b"gXYZ" | b"bXYZ") {
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += f64::from(be(offset + 8 + 4 * c) as i32) / 65536.0;
                }
            }
        }
        // The primaries add up to the D50 white.
        for (got, want) in sum.iter().zip([0.9642, 1.0, 0.8249]) {
            assert!((got - want).abs() < 2e-3, "{sum:?}");
        }
    }

    #[test]
    fn sharpening_leaves_flat_areas_and_lifts_edges() {
        let flat = sharpen(grey(32, 32, 120), Sharpen::High);
        let Pixels::Eight(flat) = flat else { panic!() };
        assert!(flat.pixels().all(|p| p.0 == [120, 120, 120]));
        let mut edge = RgbImage::from_pixel(32, 32, Rgb([60, 60, 60]));
        for y in 0..32 {
            for x in 16..32 {
                edge.put_pixel(x, y, Rgb([180, 180, 180]));
            }
        }
        let Pixels::Eight(out) = sharpen(Pixels::Eight(edge), Sharpen::Standard) else {
            panic!()
        };
        assert!(
            out.get_pixel(16, 10).0[0] > 180,
            "the bright side of the edge brightens"
        );
        assert!(out.get_pixel(15, 10).0[0] < 60, "the dark side darkens");
        assert!(matches!(
            sharpen(grey(4, 4, 1), Sharpen::Off),
            Pixels::Eight(_)
        ));
    }

    #[test]
    fn a_text_watermark_lands_in_its_corner_at_its_width() {
        let mark = Watermark {
            stamp: Stamp::Text("© Sharma Studio".into()),
            width: 0.3,
            opacity: 1.0,
            corner: Corner::BottomRight,
        };
        let prepared = Prepared::new(&mark).unwrap();
        let mut px = grey(1000, 600, 128);
        watermark(&mut px, &prepared);
        let Pixels::Eight(img) = px else { panic!() };
        let changed = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1).any(|y| (x0..x1).any(|x| img.get_pixel(x, y).0 != [128, 128, 128]))
        };
        assert!(changed(650, 1000, 450, 600), "the bottom right is stamped");
        assert!(!changed(0, 600, 0, 600), "the left is untouched");
        assert!(!changed(0, 1000, 0, 400), "the top is untouched");
        // It is about 30 % of the width (plus its padding and edge).
        let cols: Vec<u32> = (0..1000)
            .filter(|&x| (0..600).any(|y| img.get_pixel(x, y).0 != [128, 128, 128]))
            .collect();
        let span = cols.last().unwrap() - cols.first().unwrap();
        assert!((250..=360).contains(&span), "{span}");
    }

    #[test]
    fn a_watermark_is_as_opaque_as_asked_on_sixteen_bits_too() {
        let logo_dir = tempfile::tempdir().unwrap();
        let logo = logo_dir.path().join("logo.png");
        RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]))
            .save(&logo)
            .unwrap();
        let mark = Watermark {
            stamp: Stamp::Image(logo),
            width: 0.5,
            opacity: 0.5,
            corner: Corner::Centre,
        };
        let prepared = Prepared::new(&mark).unwrap();
        let mut px = Pixels::Sixteen(ImageBuffer::from_pixel(100, 100, Rgb([0u16, 0, 0])));
        watermark(&mut px, &prepared);
        let Pixels::Sixteen(img) = px else { panic!() };
        let mid = img.get_pixel(50, 50).0[0];
        assert!((32_000..=33_600).contains(&mid), "half of white: {mid}");
        assert_eq!(img.get_pixel(5, 5).0, [0, 0, 0], "outside the logo");
    }

    #[test]
    fn bad_watermarks_fail_up_front() {
        let text = Watermark {
            stamp: Stamp::Text("   ".into()),
            width: 0.2,
            opacity: 0.5,
            corner: Corner::TopLeft,
        };
        assert!(Prepared::new(&text).is_err());
        let missing = Watermark {
            stamp: Stamp::Image("/no/such/logo.png".into()),
            ..text
        };
        let e = Prepared::new(&missing).err().unwrap().to_string();
        assert!(e.contains("could not be read"), "{e}");
    }

    #[test]
    fn the_exif_payload_is_the_tiff_part_of_app1() {
        let app1 = [
            0xff, 0xe1, 0x00, 0x0e, b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0,
        ];
        assert_eq!(exif_payload(&app1).unwrap(), [b'I', b'I', 42, 0]);
        assert_eq!(exif_payload(b"nope"), None);
    }
}
