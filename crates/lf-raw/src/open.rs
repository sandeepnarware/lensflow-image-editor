//! Opening any supported file: sniff its kind from magic bytes (never the extension),
//! then develop it into a [`Developed`] picture.

use crate::decode::decode_raw;
use crate::demosaic::Quality;
use crate::develop::{Developed, box_downscale_rgba8, develop, orient_rgba8};
use crate::exif::{ExifSummary, extract_jpeg_exif, iso_date, read_summary};
use crate::{Orientation, RawError};
use image::{ImageReader, RgbaImage};
use lf_settings::DevelopSettings;
use std::io::{Cursor, Read};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Raw,
    Jpeg,
    Png,
    Tiff,
    WebP,
    Heic,
    Unknown,
}

/// A developed picture: 8-bit files stay 8-bit so they take the web-identical render
/// path, while RAW carries f16 headroom above 1.0.
#[derive(Debug, Clone)]
pub enum Picture {
    EightBit(RgbaImage),
    Float(Developed),
}

/// What [`open`] returns.
#[derive(Debug, Clone)]
pub struct Opened {
    /// Upright, and reduced to `max_edge` when larger.
    pub picture: Picture,
    pub kind: Kind,
    pub exif: ExifSummary,
    /// Size before any reduction (after orientation).
    pub full_width: u32,
    pub full_height: u32,
    /// True when the picture was box-downscaled to fit `max_edge`.
    pub reduced: bool,
    /// JPEG only: the original's APP1 Exif segment (`FF E1 len Exif\0\0 ...`) with
    /// Orientation set to 1 and the thumbnail link cleared, as the web editor's
    /// `extractExif` keeps it, ready to insert into an exported JPEG.
    pub jpeg_exif_bytes: Option<Vec<u8>>,
}

const HEIC_BRANDS: [&[u8]; 13] = [
    b"heic", b"heix", b"hevc", b"hevx", b"hevm", b"hevs", b"heim", b"heis", b"heif", b"mif1",
    b"msf1", b"avif", b"avis",
];

fn is_tiff(h: &[u8]) -> bool {
    h.starts_with(b"II*\0") || h.starts_with(b"MM\0*")
}

/// Kind from the leading bytes alone; a TIFF-magic file is reported as `Tiff` (the
/// caller decides whether it is a RAW container).
fn kind_of_head(h: &[u8]) -> Kind {
    if h.starts_with(&[0xff, 0xd8, 0xff]) {
        Kind::Jpeg
    } else if h.starts_with(&[0x89, b'P', b'N', b'G']) {
        Kind::Png
    } else if is_tiff(h) {
        Kind::Tiff
    } else if h.len() >= 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP" {
        Kind::WebP
    } else if h.len() >= 12 && &h[4..8] == b"ftyp" {
        let brand = &h[8..12];
        if brand == b"crx " {
            Kind::Raw
        } else if HEIC_BRANDS.contains(&brand) {
            Kind::Heic
        } else {
            Kind::Unknown
        }
    } else if h.starts_with(b"IIRO")
        || h.starts_with(b"MMOR")
        || h.starts_with(b"IIRS")
        || h.starts_with(b"FUJIFILM")
        || h.starts_with(b"IIU\0")
        || h.starts_with(b"\0MRM")
        || h.starts_with(b"FOVb")
        || (h.starts_with(b"II\x1a\0") && h.get(6..14) == Some(b"HEAPCCDR".as_slice()))
    {
        Kind::Raw
    } else {
        Kind::Unknown
    }
}

fn read_head(path: &Path) -> Result<Vec<u8>, RawError> {
    let io = |source| RawError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut head = Vec::new();
    std::fs::File::open(path)
        .map_err(io)?
        .take(16)
        .read_to_end(&mut head)
        .map_err(io)?;
    Ok(head)
}

/// The kind of file at `path`, from its magic bytes. A TIFF-magic file is `Raw` only
/// on RAW evidence in its structure ([`crate::container`]: DNGVersion, the CR2
/// signature, or CFA / LinearRaw sensor data), never because of its `Make`.
pub fn sniff(path: &Path) -> Result<Kind, RawError> {
    let head = read_head(path)?;
    Ok(match kind_of_head(&head) {
        Kind::Tiff => {
            let src = rawler::rawsource::RawSource::new(path).map_err(|source| RawError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let raw = crate::decode::guarded(path, || {
                crate::decode::maybe_inject_panic();
                Ok(crate::container::tiff_is_raw(src.buf()))
            })?;
            if raw { Kind::Raw } else { Kind::Tiff }
        }
        k => k,
    })
}

/// What [`summary`] returns: kind, EXIF facts and the size when the headers give it.
pub type Summary = (Kind, ExifSummary, Option<(u32, u32)>);

/// Kind, EXIF facts and (when known, after orientation) the size of a file, without decoding
/// any pixels. A RAW's size comes from rawler's dummy decode (headers only, cropped to the
/// usable area); it is `None` only when that fails.
///
/// The size is that of the developed picture's long and short sides before any reduction.
pub fn summary(path: &Path) -> Result<Summary, RawError> {
    let kind = sniff(path)?;
    match kind {
        Kind::Raw => {
            let mut exif = raw_summary(path)?;
            let size = raw_dimensions(path).map(|(w, h, o)| {
                exif.orientation = o;
                if o >= 5 { (h, w) } else { (w, h) }
            });
            Ok((kind, exif, size))
        }
        Kind::Heic => Err(RawError::Unsupported {
            path: path.to_path_buf(),
            reason: "HEIC files aren’t supported yet.".to_string(),
        }),
        Kind::Unknown => Err(RawError::NotAnImage {
            path: path.to_path_buf(),
        }),
        Kind::Jpeg | Kind::Png | Kind::Tiff | Kind::WebP => {
            let bytes = std::fs::read(path).map_err(|source| RawError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let corrupt = |reason: String| RawError::Corrupt {
                path: path.to_path_buf(),
                reason,
            };
            let (w, h) = ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|e| corrupt(e.to_string()))?
                .into_dimensions()
                .map_err(|e| corrupt(e.to_string()))?;
            let exif = read_summary(&bytes);
            let size = if exif.orientation >= 5 {
                (h, w)
            } else {
                (w, h)
            };
            Ok((kind, exif, Some(size)))
        }
    }
}

/// Open and develop any supported file, to at most `max_edge` on the long side
/// (8-bit files by an integer box filter).
pub fn open(
    path: &Path,
    quality: Quality,
    settings: &DevelopSettings,
    max_edge: u32,
) -> Result<Opened, RawError> {
    let kind = sniff(path)?;
    match kind {
        Kind::Raw => {
            let frame = decode_raw(path)?;
            let mut exif = raw_summary(path)?;
            exif.orientation = frame.orientation.exif();
            if exif.make.is_none() && !frame.make.is_empty() {
                exif.make = Some(frame.make.clone());
            }
            if exif.model.is_none() && !frame.model.is_empty() {
                exif.model = Some(frame.model.clone());
            }
            let developed = develop(&frame, quality, settings, max_edge)?;
            Ok(Opened {
                full_width: developed.full_width,
                full_height: developed.full_height,
                reduced: developed.reduced,
                picture: Picture::Float(developed),
                kind,
                exif,
                jpeg_exif_bytes: None,
            })
        }
        Kind::Heic => {
            let head = read_head(path)?;
            let avif = matches!(head.get(8..12), Some(b"avif" | b"avis"));
            Err(RawError::Unsupported {
                path: path.to_path_buf(),
                reason: if avif {
                    "AVIF files aren’t supported yet."
                } else {
                    "HEIC files aren’t supported yet."
                }
                .to_string(),
            })
        }
        Kind::Unknown => Err(RawError::NotAnImage {
            path: path.to_path_buf(),
        }),
        Kind::Jpeg | Kind::Png | Kind::Tiff | Kind::WebP => {
            let bytes = std::fs::read(path).map_err(|source| RawError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let corrupt = |reason: String| RawError::Corrupt {
                path: path.to_path_buf(),
                reason,
            };
            // `ImageReader` does not apply EXIF orientation (that is an explicit
            // `DynamicImage::apply_orientation` call), so it is applied once, below.
            let img = ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|e| corrupt(e.to_string()))?
                .decode()
                .map_err(|e| corrupt(e.to_string()))?
                .to_rgba8();
            let exif = read_summary(&bytes);
            let orientation = Orientation::from_exif(exif.orientation as u16);
            let (w, h) = img.dimensions();
            let (full_width, full_height) = if orientation.exif() >= 5 {
                (h, w)
            } else {
                (w, h)
            };
            // Reduce first, then turn: orienting the smaller picture needs less memory
            // (the box factor depends only on the long edge, which turning keeps).
            let small = box_downscale_rgba8(&img, max_edge);
            let reduced = small.is_some();
            let picture = Picture::EightBit(orient_rgba8(small.unwrap_or(img), orientation));
            let jpeg_exif_bytes = if kind == Kind::Jpeg {
                extract_jpeg_exif(&bytes)
            } else {
                None
            };
            Ok(Opened {
                picture,
                kind,
                exif,
                full_width,
                full_height,
                reduced,
                jpeg_exif_bytes,
            })
        }
    }
}

/// `(width, height, exif orientation)` of a RAW's usable area from rawler's dummy decode,
/// which parses headers and allocates no pixel data. Panics inside rawler are caught.
/// The colour temperature (kelvin) of a RAW file's as-shot white balance, from its headers:
/// the camera's neutral (one over each white-balance multiplier) taken to XYZ through the
/// colour matrix, and McCamy's estimate from its chromaticity. `None` for other files, or a
/// file without the metadata.
pub fn as_shot_kelvin(path: &Path) -> Option<f64> {
    use rawler::decoders::RawDecodeParams;
    if sniff(path).ok()? != Kind::Raw {
        return None;
    }
    let src = rawler::rawsource::RawSource::new(path).ok()?;
    let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rawler::get_decoder(&src)?.raw_image(&src, &RawDecodeParams::default(), true)
    }))
    .ok()?
    .ok()?;
    let wb = img.wb_coeffs;
    if wb[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let m = img
        .color_matrix
        .iter()
        .filter(|(_, m)| m.len() >= 9)
        .min_by_key(|(ill, _)| (**ill != rawler::imgop::xyz::Illuminant::D65, **ill))
        .map(|(_, m)| m.clone())?;
    let neutral = [f64::from(wb[1] / wb[0]), 1.0, f64::from(wb[1] / wb[2])];
    let xyz = crate::color::solve3(
        [
            [f64::from(m[0]), f64::from(m[1]), f64::from(m[2])],
            [f64::from(m[3]), f64::from(m[4]), f64::from(m[5])],
            [f64::from(m[6]), f64::from(m[7]), f64::from(m[8])],
        ],
        neutral,
    )?;
    let sum = xyz[0] + xyz[1] + xyz[2];
    if sum.abs() < 1e-9 {
        return None;
    }
    Some(lf_settings::shader_values::kelvin_of_xy(
        xyz[0] / sum,
        xyz[1] / sum,
    ))
}

fn raw_dimensions(path: &Path) -> Option<(u32, u32, u8)> {
    use rawler::decoders::{Orientation as O, RawDecodeParams};
    let src = rawler::rawsource::RawSource::new(path).ok()?;
    let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rawler::get_decoder(&src)?.raw_image(&src, &RawDecodeParams::default(), true)
    }))
    .ok()?
    .ok()?;
    let (w, h) = match img.crop_area.or(img.active_area) {
        Some(r) => (r.d.w as u32, r.d.h as u32),
        None => (img.width as u32, img.height as u32),
    };
    if w == 0 || h == 0 {
        return None;
    }
    let o = match img.orientation {
        O::Normal | O::Unknown => 1,
        O::HorizontalFlip => 2,
        O::Rotate180 => 3,
        O::VerticalFlip => 4,
        O::Transpose => 5,
        O::Rotate90 => 6,
        O::Transverse => 7,
        O::Rotate270 => 8,
    };
    Some((w, h, o))
}

/// EXIF facts for a RAW file from rawler's metadata (empty when unavailable). Only a
/// rawler panic is an error (`Corrupt`).
fn raw_summary(path: &Path) -> Result<ExifSummary, RawError> {
    let empty = ExifSummary {
        orientation: 1,
        ..Default::default()
    };
    let Ok(src) = rawler::rawsource::RawSource::new(path) else {
        return Ok(empty);
    };
    let meta = crate::decode::guarded(path, || {
        crate::decode::maybe_inject_panic();
        Ok(rawler::get_decoder(&src)
            .ok()
            .and_then(|d| d.raw_metadata(&src, &Default::default()).ok()))
    })?;
    let Some(meta) = meta else {
        return Ok(empty);
    };
    let mut out = empty;
    let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    let x = &meta.exif;
    out.make = non_empty(&meta.make);
    out.model = non_empty(&meta.model);
    out.lens = meta
        .lens
        .as_ref()
        .and_then(|l| non_empty(&l.lens_model))
        .or_else(|| x.lens_model.as_deref().and_then(non_empty));
    out.iso = x
        .iso_speed_ratings
        .map(u32::from)
        .or(x.iso_speed)
        .or(x.recommended_exposure_index);
    let f = |r: rawler::formats::tiff::Rational| (r.d != 0).then(|| r.n as f32 / r.d as f32);
    out.exposure_time = x.exposure_time.and_then(f);
    out.f_number = x.fnumber.and_then(f);
    out.focal_length = x.focal_length.and_then(f);
    out.artist = x.artist.as_deref().and_then(non_empty);
    out.copyright = x.copyright.as_deref().and_then(non_empty);
    out.captured_at = x
        .date_time_original
        .as_deref()
        .or(x.create_date.as_deref())
        .and_then(iso_date);
    Ok(out)
}
