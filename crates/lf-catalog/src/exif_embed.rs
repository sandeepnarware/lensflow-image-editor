//! EXIF for exported JPEGs: the web's `insertExif`, and a small TIFF/IFD writer for sources
//! that carry no JPEG APP1 to copy (RAW, PNG, TIFF, WebP).
//!
//! Tags written (big-endian TIFF, one APP1 segment):
//! - IFD0: Make 0x010F, Model 0x0110, Orientation 0x0112 (always 1: the pixels are upright),
//!   DateTime 0x0132, Artist 0x013B, Copyright 0x8298, ExifIFD pointer 0x8769.
//! - Exif IFD: ExifVersion 0x9000 ("0232"), ExposureTime 0x829A, FNumber 0x829D, ISO 0x8827, DateTimeOriginal 0x9003,
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

/// Longest text field written; longer ones are cut (with a warning) so the segment, which is
/// capped at 64 KB, always fits.
const MAX_TEXT: usize = 1000;

fn ascii(tag: u16, s: &str) -> Entry {
    let mut end = s.len().min(MAX_TEXT);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    if end < s.len() {
        log::warn!(
            "EXIF tag {tag:#06x} is {} bytes long; cut to {end}",
            s.len()
        );
    }
    let mut data = s.as_bytes()[..end].to_vec();
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
            ifd0.push(ascii(0x0132, &v));
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
        exif.push(Entry {
            tag: 0x9000,
            kind: 7,
            count: 4,
            data: b"0232".to_vec(),
        });
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
    let Ok(len) = u16::try_from(len) else {
        log::warn!("EXIF segment of {len} bytes does not fit in an APP1; skipped");
        return None;
    };
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

/// The signature that opens an XMP APP1 segment.
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // XML 1.0 has no place for other control characters.
            c if c.is_control() && !matches!(c, '\n' | '\t') => {}
            c => out.push(c),
        }
    }
    out
}

/// A whole APP1 XMP segment with `meta`'s title (`dc:title`), caption (`dc:description`),
/// creator (`dc:creator`), copyright (`dc:rights`) and keywords (`dc:subject`), or (when
/// `copyright_only`) just the creator and copyright. None when there is nothing to write,
/// or when it would not fit one segment (64 KB).
pub fn xmp_segment(meta: &crate::meta::Meta, copyright_only: bool) -> Option<Vec<u8>> {
    let alt = |tag: &str, v: &str| {
        format!(
            "<dc:{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:{tag}>",
            xml_escape(v)
        )
    };
    let mut body = String::new();
    if !copyright_only {
        if let Some(t) = &meta.title {
            body.push_str(&alt("title", t));
        }
        if let Some(c) = &meta.caption {
            body.push_str(&alt("description", c));
        }
    }
    if let Some(c) = &meta.creator {
        body.push_str(&format!(
            "<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>",
            xml_escape(c)
        ));
    }
    if let Some(c) = &meta.copyright {
        body.push_str(&alt("rights", c));
    }
    if !copyright_only && !meta.keywords.is_empty() {
        body.push_str("<dc:subject><rdf:Bag>");
        for k in &meta.keywords {
            body.push_str(&format!("<rdf:li>{}</rdf:li>", xml_escape(k)));
        }
        body.push_str("</rdf:Bag></dc:subject>");
    }
    if body.is_empty() {
        return None;
    }
    let packet = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
         {body}</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    );
    let len = 2 + XMP_HEADER.len() + packet.len();
    if len > usize::from(u16::MAX) {
        return None;
    }
    let mut seg = vec![0xff, 0xe1];
    seg.extend((len as u16).to_be_bytes());
    seg.extend(XMP_HEADER);
    seg.extend(packet.as_bytes());
    Some(seg)
}

/// The JPEG with `segment` (a whole XMP APP1) after its leading APP0/APP1 segments, any XMP
/// segment already there dropped (the source's XMP, copied with its EXIF, would contradict
/// the catalogue's).
pub fn insert_xmp(jpeg: &[u8], segment: &[u8]) -> Vec<u8> {
    if jpeg.get(..2) != Some(&SOI) {
        return jpeg.to_vec();
    }
    let mut out = SOI.to_vec();
    let mut at = 2usize;
    let mut placed = false;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff {
        let marker = jpeg[at + 1];
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if !placed && !matches!(marker, 0xe0 | 0xe1) {
            out.extend(segment);
            placed = true;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let end = (at + 2 + len).min(jpeg.len());
        let is_xmp = marker == 0xe1 && jpeg[at + 4..end].starts_with(XMP_HEADER);
        if !is_xmp {
            out.extend(&jpeg[at..end]);
        }
        at = end;
    }
    if !placed {
        out.extend(segment);
    }
    out.extend(&jpeg[at..]);
    out
}

#[cfg(test)]
mod xmp_tests {
    use super::*;
    use crate::meta::Meta;

    fn meta() -> Meta {
        Meta {
            title: Some("Sunset <Goa>".into()),
            caption: Some("Fish & chips".into()),
            creator: Some("Asha".into()),
            copyright: Some("© Studio".into()),
            keywords: vec!["beach".into(), "wedding".into()],
        }
    }

    #[test]
    fn the_packet_carries_every_field_escaped() {
        let seg = xmp_segment(&meta(), false).unwrap();
        assert_eq!(&seg[..2], &[0xff, 0xe1]);
        let len = u16::from_be_bytes([seg[2], seg[3]]) as usize;
        assert_eq!(len, seg.len() - 2);
        let text = String::from_utf8_lossy(&seg[4 + XMP_HEADER.len()..]);
        for want in [
            "Sunset &lt;Goa&gt;",
            "Fish &amp; chips",
            "<rdf:li>Asha</rdf:li>",
            "© Studio",
            "<rdf:li>beach</rdf:li><rdf:li>wedding</rdf:li>",
        ] {
            assert!(text.contains(want), "{want} in {text}");
        }
        let short = xmp_segment(&meta(), true).unwrap();
        let text = String::from_utf8_lossy(&short);
        assert!(text.contains("Asha") && !text.contains("Sunset") && !text.contains("beach"));
        assert_eq!(xmp_segment(&Meta::default(), false), None);
    }

    #[test]
    fn the_segment_goes_after_the_app_segments_and_replaces_an_old_one() {
        let seg = xmp_segment(&meta(), false).unwrap();
        // SOI, APP0 (len 4), an old XMP APP1, DQT (len 3), SOS.
        let mut old = vec![0xff, 0xe1, 0x00, 0x00];
        old.extend(XMP_HEADER);
        old.extend(b"<old/>");
        let n = (old.len() - 2) as u16;
        old[2..4].copy_from_slice(&n.to_be_bytes());
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 1, 2];
        jpeg.extend(&old);
        jpeg.extend([
            0xff, 0xdb, 0x00, 0x03, 9, 0xff, 0xda, 0x00, 0x02, 0xff, 0xd9,
        ]);
        let out = insert_xmp(&jpeg, &seg);
        let mut want = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 1, 2];
        want.extend(&seg);
        want.extend([
            0xff, 0xdb, 0x00, 0x03, 9, 0xff, 0xda, 0x00, 0x02, 0xff, 0xd9,
        ]);
        assert_eq!(out, want);
    }
}
