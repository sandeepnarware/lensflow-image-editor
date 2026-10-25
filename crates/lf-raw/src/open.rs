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

const HEIC_BRANDS: [&[u8]; 10] = [
    b"heic", b"heix", b"hevc", b"hevx", b"mif1", b"msf1", b"heim", b"heis", b"avif", b"avis",
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
        || h.starts_with(b"IIRS")
        || h.starts_with(b"FUJIFILM")
        || h.starts_with(b"IIU\0")
        || h.starts_with(b"\0MRM")
        || h.starts_with(b"FOVb")
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

/// Is this TIFF-magic file a RAW container (DNG, NEF, CR2, ARW, PEF...)? Decided by
/// asking rawler whether it recognises the camera.
fn rawler_recognises(path: &Path) -> bool {
    rawler::rawsource::RawSource::new(path)
        .ok()
        .is_some_and(|src| rawler::get_decoder(&src).is_ok())
}

/// The kind of file at `path`, from its magic bytes.
pub fn sniff(path: &Path) -> Result<Kind, RawError> {
    let head = read_head(path)?;
    Ok(match kind_of_head(&head) {
        Kind::Tiff if rawler_recognises(path) => Kind::Raw,
        k => k,
    })
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
            let mut exif = raw_summary(path);
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
            // `ImageReader` does not apply EXIF orientation (that is an explicit
            // `DynamicImage::apply_orientation` call), so it is applied once, below.
            let img = ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|e| corrupt(e.to_string()))?
                .decode()
                .map_err(|e| corrupt(e.to_string()))?
                .to_rgba8();
            let exif = read_summary(&bytes);
            let upright = orient_rgba8(&img, Orientation::from_exif(exif.orientation as u16));
            let (full_width, full_height) = upright.dimensions();
            let small = box_downscale_rgba8(&upright, max_edge);
            let reduced = small.is_some();
            let picture = Picture::EightBit(small.unwrap_or(upright));
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

/// EXIF facts for a RAW file from rawler's metadata (empty when unavailable).
fn raw_summary(path: &Path) -> ExifSummary {
    let mut out = ExifSummary {
        orientation: 1,
        ..Default::default()
    };
    let Ok(src) = rawler::rawsource::RawSource::new(path) else {
        return out;
    };
    let Ok(decoder) = rawler::get_decoder(&src) else {
        return out;
    };
    let Ok(meta) = decoder.raw_metadata(&src, &Default::default()) else {
        return out;
    };
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
    out.captured_at = x
        .date_time_original
        .as_deref()
        .or(x.create_date.as_deref())
        .and_then(iso_date);
    out
}
