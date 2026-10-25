//! EXIF for exported JPEGs: the web's `insertExif`, and a small TIFF/IFD writer for sources
//! that carry no JPEG APP1 to copy (RAW, PNG, TIFF, WebP).
//!
//! Tags written (big-endian TIFF, one APP1 segment):
//! - IFD0: Make 0x010F, Model 0x0110, Orientation 0x0112 (always 1: the pixels are upright),
//!   Artist 0x013B, Copyright 0x8298, ExifIFD pointer 0x8769.
//! - Exif IFD: ExposureTime 0x829A, FNumber 0x829D, ISO 0x8827, DateTimeOriginal 0x9003,
//!   FocalLength 0x920A, LensModel 0xA434.
//!
//! "Copyright only" writes IFD0 with just Artist and Copyright.

use lf_raw::exif::ExifSummary;

const SOI: [u8; 2] = [0xff, 0xd8];

/// The JPEG with `app1` (a whole `FF E1 len Exif\0\0 ...` segment) right after SOI, any APP1
/// the encoder wrote dropped. A JFIF APP0 stays ahead of it, as the web's `insertExif` keeps
/// it (Exif readers look for APP1 near the start either way).
pub fn insert_exif(jpeg: &[u8], app1: &[u8]) -> Vec<u8> {
    if jpeg.get(..2) != Some(&SOI) {
        return jpeg.to_vec();
    }
    let mut at = 2usize;
    let mut out = SOI.to_vec();
    // Keep a leading APP0 (JFIF) before the Exif segment.
    if jpeg.get(2..4) == Some(&[0xff, 0xe0]) && jpeg.len() >= 6 {
        let end = 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
        if end <= jpeg.len() {
            out.extend(&jpeg[2..end]);
            at = end;
        }
    }
    out.extend(app1);
    // Copy the rest, skipping any other Exif APP1 segment before the scan.
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff {
        let marker = jpeg[at + 1];
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let end = (at + 2 + len).min(jpeg.len());
        let is_exif = marker == 0xe1 && jpeg.get(at + 4..at + 10) == Some(b"Exif\0\0".as_slice());
        if !is_exif {
            out.extend(&jpeg[at..end]);
        }
        at = end;
    }
    out.extend(&jpeg[at..]);
    out
}

struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    data: Vec<u8>,
}

const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;

fn ascii(tag: u16, s: &str) -> Entry {
    let mut data = s.as_bytes().to_vec();
    data.push(0);
    Entry {
        tag,
        kind: ASCII,
        count: data.len() as u32,
        data,
    }
}

fn short(tag: u16, v: u16) -> Entry {
    Entry {
        tag,
        kind: SHORT,
        count: 1,
        data: v.to_be_bytes().to_vec(),
    }
}

fn long(tag: u16, v: u32) -> Entry {
    Entry {
        tag,
        kind: LONG,
        count: 1,
        data: v.to_be_bytes().to_vec(),
    }
}

fn rational(tag: u16, (n, d): (u32, u32)) -> Entry {
    let mut data = n.to_be_bytes().to_vec();
    data.extend(d.to_be_bytes());
    Entry {
        tag,
        kind: RATIONAL,
        count: 1,
        data,
    }
}

/// `x` as a fraction with `den` as denominator (or 1/round(1/x) for a short exposure).
fn fraction(x: f32, den: u32) -> (u32, u32) {
    ((x.max(0.0) * den as f32).round() as u32, den)
}

fn exposure(x: f32) -> (u32, u32) {
    if x > 0.0 && x < 1.0 {
        (1, (1.0 / x).round().max(1.0) as u32)
    } else {
        fraction(x, 100)
    }
}

/// Serialise one IFD that starts at `start` (an offset from the TIFF header): the entry table,
/// then the values that do not fit in four bytes.
fn write_ifd(mut entries: Vec<Entry>, start: u32) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);
    let table = 2 + 12 * entries.len() + 4;
    let mut out = (entries.len() as u16).to_be_bytes().to_vec();
    let mut overflow = Vec::new();
    for e in &entries {
        out.extend(e.tag.to_be_bytes());
        out.extend(e.kind.to_be_bytes());
        out.extend(e.count.to_be_bytes());
        if e.data.len() <= 4 {
            out.extend(&e.data);
            out.extend(std::iter::repeat_n(0, 4 - e.data.len()));
        } else {
            out.extend((start + (table + overflow.len()) as u32).to_be_bytes());
            overflow.extend(&e.data);
            if overflow.len() % 2 == 1 {
                overflow.push(0);
            }
        }
    }
    out.extend([0, 0, 0, 0]); // no next IFD
    out.extend(overflow);
    out
}

/// `2024-05-01T12:34:56` as EXIF's `2024:05:01 12:34:56`.
fn exif_date(iso: &str) -> Option<String> {
    let b = iso.as_bytes();
    (b.len() >= 19 && b[10] == b'T').then(|| {
        let mut s = iso[..19].to_string();
        s.replace_range(4..5, ":");
        s.replace_range(7..8, ":");
        s.replace_range(10..11, " ");
        s
    })
}

/// A whole APP1 Exif segment built from `s`: everything the summary carries, or (when
/// `copyright_only`) just Artist and Copyright. None when there is nothing to write.
pub fn build_exif(s: &ExifSummary, copyright_only: bool) -> Option<Vec<u8>> {
    let mut ifd0 = Vec::new();
    let mut exif = Vec::new();
    if let Some(a) = &s.artist {
        ifd0.push(ascii(0x013b, a));
    }
    if let Some(c) = &s.copyright {
        ifd0.push(ascii(0x8298, c));
    }
    if !copyright_only {
        if let Some(v) = &s.make {
            ifd0.push(ascii(0x010f, v));
        }
        if let Some(v) = &s.model {
            ifd0.push(ascii(0x0110, v));
        }
        if let Some(v) = s.exposure_time {
            exif.push(rational(0x829a, exposure(v)));
        }
        if let Some(v) = s.f_number {
            exif.push(rational(0x829d, fraction(v, 10)));
        }
        if let Some(v) = s.iso {
            exif.push(short(0x8827, v.min(u16::MAX as u32) as u16));
        }
        if let Some(v) = s.captured_at.as_deref().and_then(exif_date) {
            exif.push(ascii(0x9003, &v));
        }
        if let Some(v) = s.focal_length {
            exif.push(rational(0x920a, fraction(v, 10)));
        }
        if let Some(v) = &s.lens {
            exif.push(ascii(0xa434, v));
        }
        if !(ifd0.is_empty() && exif.is_empty()) {
            ifd0.push(short(0x0112, 1));
        }
    }
    if ifd0.is_empty() && exif.is_empty() {
        return None;
    }
    let has_exif = !exif.is_empty();
    if has_exif {
        ifd0.push(long(0x8769, 0));
    }
    // The pointer's value does not change the IFD's length, so size it first.
    let ifd0_len = write_ifd(ifd0.iter().map(clone_entry).collect(), 8).len() as u32;
    if has_exif {
        let at = ifd0.iter().position(|e| e.tag == 0x8769).unwrap();
        ifd0[at] = long(0x8769, 8 + ifd0_len);
    }
    let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
    tiff.extend(write_ifd(ifd0, 8));
    if has_exif {
        let start = tiff.len() as u32;
        tiff.extend(write_ifd(exif, start));
    }
    let len = 2 + 6 + tiff.len();
    let len = u16::try_from(len).ok()?;
    let mut seg = vec![0xff, 0xe1];
    seg.extend(len.to_be_bytes());
    seg.extend(b"Exif\0\0");
    seg.extend(tiff);
    Some(seg)
}

fn clone_entry(e: &Entry) -> Entry {
    Entry {
        tag: e.tag,
        kind: e.kind,
        count: e.count,
        data: e.data.clone(),
    }
}
