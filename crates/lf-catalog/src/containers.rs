//! Metadata in the export formats other than JPEG ([`crate::exif_embed`] does JPEG): XMP in a
//! PNG's `iTXt` chunk, a WebP assembled as an extended file (`VP8X` with `ICCP`, `EXIF` and
//! `XMP ` chunks around the encoder's bitstream, so lossy and lossless WebP carry the same),
//! and a 16-bit TIFF written with its profile, XMP and the baseline descriptive tags (artist,
//! copyright, description, make and model).

use std::io::Cursor;

/// The PNG with an `iTXt` chunk holding `xmp` (the XMP packet) before its first `IDAT`.
pub fn png_with_xmp(png: &[u8], xmp: &[u8]) -> Vec<u8> {
    const SIGNATURE: usize = 8;
    let mut data = b"XML:com.adobe.xmp".to_vec();
    // Keyword end, uncompressed, no language, no translated keyword.
    data.extend([0, 0, 0, 0, 0]);
    data.extend(xmp);
    let chunk = png_chunk(b"iTXt", &data);
    let mut at = SIGNATURE;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().expect("4 bytes")) as usize;
        if &png[at + 4..at + 8] == b"IDAT" {
            let mut out = png[..at].to_vec();
            out.extend(chunk);
            out.extend(&png[at..]);
            return out;
        }
        at += 12 + len;
    }
    png.to_vec()
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend(crc.finalize().to_be_bytes());
    out
}

/// The image chunk (`VP8 ` or `VP8L`, header and padding included) of a simple WebP file.
fn webp_bitstream(webp: &[u8]) -> Option<&[u8]> {
    if webp.get(..4) != Some(b"RIFF") || webp.get(8..12) != Some(b"WEBP") {
        return None;
    }
    let mut at = 12;
    while at + 8 <= webp.len() {
        let len = u32::from_le_bytes(webp[at + 4..at + 8].try_into().ok()?) as usize;
        let end = (at + 8 + len + (len & 1)).min(webp.len());
        if matches!(&webp[at..at + 4], b"VP8 " | b"VP8L") {
            return Some(&webp[at..end]);
        }
        at = end;
    }
    None
}

fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// A `width` × `height` WebP (simple, from an encoder) as an extended file carrying `icc`,
/// `exif` (the TIFF part, from `II*`/`MM*`) and `xmp` (the packet), each when given. The
/// encoder's own file comes back unchanged when it cannot be read.
pub fn webp_with_metadata(
    webp: &[u8],
    width: u32,
    height: u32,
    icc: Option<&[u8]>,
    exif: Option<&[u8]>,
    xmp: Option<&[u8]>,
) -> Vec<u8> {
    let Some(bitstream) = webp_bitstream(webp) else {
        return webp.to_vec();
    };
    let mut flags = 0u8;
    if icc.is_some() {
        flags |= 0x20;
    }
    if exif.is_some() {
        flags |= 0x08;
    }
    if xmp.is_some() {
        flags |= 0x04;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend(&(width - 1).to_le_bytes()[..3]);
    vp8x.extend(&(height - 1).to_le_bytes()[..3]);
    let mut body = b"WEBP".to_vec();
    body.extend(riff_chunk(b"VP8X", &vp8x));
    if let Some(icc) = icc {
        body.extend(riff_chunk(b"ICCP", icc));
    }
    body.extend(bitstream);
    if let Some(exif) = exif {
        body.extend(riff_chunk(b"EXIF", exif));
    }
    if let Some(xmp) = xmp {
        body.extend(riff_chunk(b"XMP ", xmp));
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

/// The descriptive tags a TIFF export carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TiffTags {
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub description: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
}

/// A 16-bit RGB TIFF of `data` (`width` × `height` × 3) with `icc`, `xmp` and `tags`.
pub fn tiff16(
    width: u32,
    height: u32,
    data: &[u16],
    icc: &[u8],
    xmp: Option<&[u8]>,
    tags: &TiffTags,
) -> Result<Vec<u8>, String> {
    use tiff::encoder::{TiffEncoder, colortype::RGB16};
    use tiff::tags::Tag;
    let text = |e: tiff::TiffError| e.to_string();
    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).map_err(text)?;
        let mut image = encoder.new_image::<RGB16>(width, height).map_err(text)?;
        let dir = image.encoder();
        dir.write_tag(Tag::Unknown(34675), icc).map_err(text)?;
        if let Some(xmp) = xmp {
            dir.write_tag(Tag::Unknown(700), xmp).map_err(text)?;
        }
        for (tag, value) in [
            (Tag::Artist, &tags.artist),
            (Tag::Copyright, &tags.copyright),
            (Tag::ImageDescription, &tags.description),
            (Tag::Make, &tags.make),
            (Tag::Model, &tags.model),
        ] {
            // ASCII only: a tag the format cannot hold is left out rather than garbled.
            if let Some(v) = value.as_deref().filter(|v| v.is_ascii() && !v.is_empty()) {
                dir.write_tag(tag, v).map_err(text)?;
            }
        }
        image.write_data(data).map_err(text)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder as _;

    #[test]
    fn a_png_gains_its_xmp_and_still_decodes() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[10, 20, 30], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let out = png_with_xmp(&png, b"<x:xmpmeta>hello</x:xmpmeta>");
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("XML:com.adobe.xmp") && text.contains("hello"));
        let back = image::load_from_memory(&out).unwrap().to_rgb8();
        assert_eq!(back.get_pixel(0, 0).0, [10, 20, 30]);
    }

    #[test]
    fn a_webp_is_wrapped_with_its_profile_exif_and_xmp_and_still_decodes() {
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .write_image(
                &[200, 100, 50, 200, 100, 50],
                2,
                1,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        let out = webp_with_metadata(
            &webp,
            2,
            1,
            Some(b"icc!"),
            Some(b"II*\0exif"),
            Some(b"<xmp/>"),
        );
        assert_eq!(&out[12..16], b"VP8X");
        assert_eq!(out[20], 0x20 | 0x08 | 0x04);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("ICCP") && text.contains("EXIF") && text.contains("XMP "));
        let back = image::load_from_memory(&out).unwrap().to_rgb8();
        assert_eq!(back.get_pixel(1, 0).0, [200, 100, 50]);
    }

    #[test]
    fn a_tiff_carries_its_tags_and_still_decodes() {
        let tags = TiffTags {
            artist: Some("Asha".into()),
            copyright: Some("(c) Sharma Studio".into()),
            make: Some("Nikon".into()),
            ..TiffTags::default()
        };
        let bytes = tiff16(1, 1, &[65535, 0, 32768], b"icc", Some(b"<xmp/>"), &tags).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Asha") && text.contains("Sharma Studio") && text.contains("<xmp/>"));
        let back = image::load_from_memory(&bytes).unwrap();
        assert!(matches!(back, image::DynamicImage::ImageRgb16(_)));
    }
}
