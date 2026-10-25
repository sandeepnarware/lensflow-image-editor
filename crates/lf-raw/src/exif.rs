//! EXIF reading: a small summary for the UI, and the JPEG Exif segment kept for
//! re-embedding on export.

use exif::{In, Reader, Tag, Value};
use std::io::Cursor;

/// The handful of EXIF facts the editor shows. Everything is optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifSummary {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Seconds.
    pub exposure_time: Option<f32>,
    pub f_number: Option<f32>,
    /// Millimetres.
    pub focal_length: Option<f32>,
    /// ISO-8601 local time without zone, e.g. `2024-05-01T12:34:56`.
    pub captured_at: Option<String>,
    /// EXIF orientation 1..=8 (1 when absent or invalid).
    pub orientation: u8,
}

/// Read the summary from a JPEG, TIFF, PNG or WebP file's bytes (all empty, orientation
/// 1, when the file carries no EXIF).
pub fn read_summary(bytes: &[u8]) -> ExifSummary {
    let mut out = ExifSummary {
        orientation: 1,
        ..Default::default()
    };
    let Ok(exif) = Reader::new().read_from_container(&mut Cursor::new(bytes)) else {
        return out;
    };
    let get = |tag| exif.get_field(tag, In::PRIMARY).map(|f| &f.value);
    let text = |tag| match get(tag)? {
        Value::Ascii(v) => {
            let s = String::from_utf8_lossy(v.first()?).trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    };
    let rational = |tag| match get(tag)? {
        Value::Rational(v) => v.first().filter(|r| r.denom != 0).map(|r| r.to_f32()),
        _ => None,
    };
    out.make = text(Tag::Make);
    out.model = text(Tag::Model);
    out.lens = text(Tag::LensModel);
    out.iso = get(Tag::PhotographicSensitivity).and_then(|v| v.get_uint(0));
    out.exposure_time = rational(Tag::ExposureTime);
    out.f_number = rational(Tag::FNumber);
    out.focal_length = rational(Tag::FocalLength);
    out.captured_at = text(Tag::DateTimeOriginal)
        .or_else(|| text(Tag::DateTime))
        .and_then(|s| iso_date(&s));
    out.orientation = get(Tag::Orientation)
        .and_then(|v| v.get_uint(0))
        .filter(|v| (1..=8).contains(v))
        .map_or(1, |v| v as u8);
    out
}

/// `2024:05:01 12:34:56` to `2024-05-01T12:34:56`; None when it is not that shape.
pub fn iso_date(s: &str) -> Option<String> {
    let t = s.trim();
    let b = t.as_bytes();
    let ok = b.len() >= 19
        && b[..19].iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b':',
            10 => *c == b' ',
            _ => c.is_ascii_digit(),
        });
    ok.then(|| format!("{}-{}-{}T{}", &t[0..4], &t[5..7], &t[8..10], &t[11..19]))
}

/// The JPEG's APP1 Exif segment, marker and length included (`FF E1 len Exif\0\0 ...`),
/// patched exactly as the web editor's `extractExif` does: Orientation becomes 1 and
/// the IFD1 link (embedded thumbnail) is cleared. None when there is none.
pub fn extract_jpeg_exif(jpeg: &[u8]) -> Option<Vec<u8>> {
    if jpeg.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2usize;
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xff {
            return None;
        }
        let marker = jpeg[at + 1];
        if marker == 0xda || marker == 0xd9 {
            return None;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        if len < 2 || at + 2 + len > jpeg.len() {
            return None;
        }
        if marker == 0xe1 && jpeg.get(at + 4..at + 10) == Some(b"Exif\0\0".as_slice()) {
            let mut seg = jpeg[at..at + 2 + len].to_vec();
            patch_tiff(&mut seg, 10);
            return Some(seg);
        }
        at += 2 + len;
    }
    None
}

fn patch_tiff(seg: &mut [u8], tiff: usize) {
    if tiff + 8 > seg.len() {
        return;
    }
    let little = seg[tiff] == 0x49;
    let get16 = |s: &[u8], i: usize| {
        let b = [s[i], s[i + 1]];
        if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    };
    let o = [seg[tiff + 4], seg[tiff + 5], seg[tiff + 6], seg[tiff + 7]];
    let off = if little {
        u32::from_le_bytes(o)
    } else {
        u32::from_be_bytes(o)
    } as usize;
    let ifd0 = tiff.saturating_add(off);
    if ifd0 + 2 > seg.len() {
        return;
    }
    let count = get16(seg, ifd0) as usize;
    for i in 0..count {
        let entry = ifd0 + 2 + i * 12;
        if entry + 12 > seg.len() {
            return;
        }
        if get16(seg, entry) == 0x0112 {
            let one = if little {
                1u16.to_le_bytes()
            } else {
                1u16.to_be_bytes()
            };
            seg[entry + 8..entry + 10].copy_from_slice(&one);
        }
    }
    let next = ifd0 + 2 + count * 12;
    if next + 4 <= seg.len() {
        seg[next..next + 4].fill(0);
    }
}
