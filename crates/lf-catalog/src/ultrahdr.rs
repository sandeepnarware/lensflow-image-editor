//! HDR JPEG exports (Ultra HDR, the gain-map JPEG Android, Chrome and Lightroom read): an
//! ordinary JPEG for every screen, and a small grey JPEG after it, the gain map, saying how
//! much brighter each part may be on a screen with headroom. Viewers that know nothing of it
//! show the ordinary JPEG.
//!
//! The develop pass ends in 0..1, so the brighter rendition comes from a second render of the
//! photo [`HEADROOM_STOPS`] darker, brought back up in linear light: what clipped to white in
//! the ordinary render still has its detail there (a raw file's headroom). Only the bright
//! parts take a gain (from where the ordinary render passes [`BRIGHT_FROM`] to
//! [`BRIGHT_FULL`]), so midtones look the same on both kinds of screen and only highlights
//! (skies, lamps, specular light) glow.
//!
//! The file: the primary JPEG carries XMP saying `hdrgm:Version` and a GContainer directory
//! (Primary, then GainMap with its length), and an MPF index (CIPA DC-007) with both images'
//! sizes and offsets; the gain map carries its own XMP with the `hdrgm` parameters (gamma 1,
//! offsets 1/64, the base rendition SDR). Its single channel is the gain's log2 between 0 and
//! the largest log2 gain present, at half the photo's size.

/// The second render is this many stops darker (the most a highlight can be brightened is
/// 2^this).
pub const HEADROOM_STOPS: f32 = 2.0;
/// The ordinary render's brightness (its largest channel, 0..1) where a gain starts, and where
/// it applies fully.
pub const BRIGHT_FROM: f32 = 0.75;
pub const BRIGHT_FULL: f32 = 0.97;
/// `hdrgm:OffsetSDR` and `OffsetHDR`.
pub const OFFSET: f32 = 1.0 / 64.0;

const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The gain map of an ordinary render `sdr` (8-bit sRGB, RGB) and the darker render `dark`
/// (0..1 sRGB, RGB, the same size), as log2 gains (0 and up), and the largest of them.
pub fn log_gains(sdr: &[u8], dark: &[f32]) -> (Vec<f32>, f32) {
    let k = 2f32.powf(HEADROOM_STOPS);
    let mut max = 0.0f32;
    let gains: Vec<f32> = sdr
        .as_chunks::<3>()
        .0
        .iter()
        .zip(dark.as_chunks::<3>().0)
        .map(|(s, d)| {
            let s = s.map(|v| f32::from(v) / 255.0);
            let bright = s[0].max(s[1]).max(s[2]);
            let w = smoothstep(BRIGHT_FROM, BRIGHT_FULL, bright);
            if w <= 0.0 {
                return 0.0;
            }
            let y = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let ys = y(s.map(linear));
            let yh = y(d.map(linear)) * k;
            let ratio = ((yh + OFFSET) / (ys + OFFSET)).clamp(1.0, k);
            let g = (w * ratio.log2()).max(0.0);
            max = max.max(g);
            g
        })
        .collect();
    (gains, max)
}

/// `gains` (log2) as 8-bit recovery values over 0..`max`.
pub fn encode_gains(gains: &[f32], max: f32) -> Vec<u8> {
    let max = max.max(1e-3);
    gains
        .iter()
        .map(|&g| ((g / max).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// The gain map's own XMP APP1 segment.
fn gain_map_xmp(max: f32) -> Vec<u8> {
    let packet = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" \
         hdrgm:Version=\"1.0\" hdrgm:GainMapMin=\"0\" hdrgm:GainMapMax=\"{max:.6}\" \
         hdrgm:Gamma=\"1\" hdrgm:OffsetSDR=\"{OFFSET:.6}\" hdrgm:OffsetHDR=\"{OFFSET:.6}\" \
         hdrgm:HDRCapacityMin=\"0\" hdrgm:HDRCapacityMax=\"{max:.6}\" \
         hdrgm:BaseRenditionIsHDR=\"False\"/>\
         </rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    );
    segment(&packet)
}

/// The primary's XMP: the gain-map version and the container directory, beside `meta` (the
/// photo's own XMP packet, title, keywords and so on, when there is one).
fn primary_xmp(gain_len: usize, meta: Option<&[u8]>) -> Vec<u8> {
    let ours = format!(
        "<rdf:Description rdf:about=\"\" \
         xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" \
         xmlns:Container=\"http://ns.google.com/photos/1.0/container/\" \
         xmlns:Item=\"http://ns.google.com/photos/1.0/container/item/\" hdrgm:Version=\"1.0\">\
         <Container:Directory><rdf:Seq>\
         <rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/></rdf:li>\
         <rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"GainMap\" Item:Mime=\"image/jpeg\" Item:Length=\"{gain_len}\"/></rdf:li>\
         </rdf:Seq></Container:Directory></rdf:Description>"
    );
    // A second description inside the photo's own packet keeps both.
    let packet = match meta
        .and_then(|m| std::str::from_utf8(m).ok())
        .filter(|m| m.contains("</rdf:RDF>"))
    {
        Some(m) => m.replacen("</rdf:RDF>", &format!("{ours}</rdf:RDF>"), 1),
        None => format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
             <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{ours}\
             </rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
        ),
    };
    segment(&packet)
}

fn segment(packet: &str) -> Vec<u8> {
    let len = 2 + XMP_HEADER.len() + packet.len();
    let mut seg = vec![0xff, 0xe1];
    seg.extend((len.min(usize::from(u16::MAX)) as u16).to_be_bytes());
    seg.extend(XMP_HEADER);
    seg.extend(packet.as_bytes());
    seg
}

/// The MPF APP2 segment's length, marker included.
const MPF_LEN: usize = 2 + 2 + 4 + 8 + 42 + 32;

/// The MPF APP2 segment: two images, the primary `primary_len` bytes from offset 0, the gain
/// map `gain_len` bytes right after it; offsets count from the MPF TIFF header, which starts
/// `tiff_at` bytes into the primary.
fn mpf(primary_len: usize, gain_len: usize, tiff_at: usize) -> Vec<u8> {
    let mut s = vec![0xff, 0xe2];
    s.extend(((MPF_LEN - 2) as u16).to_be_bytes());
    s.extend(b"MPF\0");
    // Big-endian TIFF header; the index IFD right after it.
    s.extend(b"MM\0\x2a");
    s.extend(8u32.to_be_bytes());
    s.extend(3u16.to_be_bytes());
    let entry = |s: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        s.extend(tag.to_be_bytes());
        s.extend(kind.to_be_bytes());
        s.extend(count.to_be_bytes());
        s.extend(value);
    };
    entry(&mut s, 0xb000, 7, 4, *b"0100");
    entry(&mut s, 0xb001, 4, 1, 2u32.to_be_bytes());
    // The entries follow the IFD: 8 (header) + 2 + 3 × 12 + 4 (next IFD) = 50.
    entry(&mut s, 0xb002, 7, 32, 50u32.to_be_bytes());
    s.extend(0u32.to_be_bytes());
    let image = |s: &mut Vec<u8>, attr: u32, size: usize, offset: usize| {
        s.extend(attr.to_be_bytes());
        s.extend((size as u32).to_be_bytes());
        s.extend((offset as u32).to_be_bytes());
        s.extend(0u16.to_be_bytes());
        s.extend(0u16.to_be_bytes());
    };
    // The primary (baseline MP primary image), then the gain map.
    image(&mut s, 0x0003_0000, primary_len, 0);
    image(&mut s, 0, gain_len, primary_len - tiff_at);
    debug_assert_eq!(s.len(), MPF_LEN);
    s
}

/// Where the leading APPn segments of `jpeg` end (the first other marker).
fn after_app_segments(jpeg: &[u8]) -> usize {
    let mut at = 2usize;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff && (0xe0..=0xef).contains(&jpeg[at + 1]) {
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        at += 2 + len;
    }
    at.min(jpeg.len())
}

/// The Ultra HDR file: `primary` (a finished JPEG, its own XMP `meta` packet already taken out
/// and given here) with the gain map `gain` (a plain grey JPEG) after it; `max` is the
/// largest log2 gain.
pub fn assemble(primary: &[u8], meta: Option<&[u8]>, gain: &[u8], max: f32) -> Vec<u8> {
    let gain = crate::exif_embed::insert_xmp(gain, &gain_map_xmp(max));
    let primary = crate::exif_embed::insert_xmp(primary, &primary_xmp(gain.len(), meta));
    let at = after_app_segments(&primary);
    let primary_len = primary.len() + MPF_LEN;
    // The MPF TIFF header starts after the marker, the length and "MPF\0".
    let seg = mpf(primary_len, gain.len(), at + 8);
    let mut out = Vec::with_capacity(primary_len + gain.len());
    out.extend(&primary[..at]);
    out.extend(&seg);
    out.extend(&primary[at..]);
    out.extend(&gain);
    out
}

/// The gain map's JPEG inside an Ultra HDR `file`, found through its MPF index.
pub fn find_gain_map(file: &[u8]) -> Option<&[u8]> {
    let at = file.windows(4).position(|w| w == b"MPF\0")?;
    let tiff = at + 4;
    let entries = tiff + 50;
    let second = entries + 16;
    let read = |i: usize| -> Option<u32> {
        Some(u32::from_be_bytes(file.get(i..i + 4)?.try_into().ok()?))
    };
    let size = read(second + 4)? as usize;
    let offset = read(second + 8)? as usize;
    file.get(tiff + offset..tiff + offset + size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_highlights_with_headroom_take_a_gain() {
        // Mid grey; a clipped white whose darker render still has room (a lamp); a white
        // that was white in the darker render too would be at the most gain.
        let sdr = [128u8, 128, 128, 255, 255, 255, 255, 255, 255];
        let dark = [0.35f32, 0.35, 0.35, 0.6, 0.6, 0.6, 1.0, 1.0, 1.0];
        let (g, max) = log_gains(&sdr, &dark);
        assert_eq!(g[0], 0.0, "midtones are the same on both screens");
        assert!(g[1] > 0.3 && g[1] < HEADROOM_STOPS, "{g:?}");
        assert!(
            (g[2] - HEADROOM_STOPS).abs() < 0.03,
            "the offsets aside: {g:?}"
        );
        assert_eq!(max, g[2]);
        assert_eq!(
            encode_gains(&g, max),
            vec![0, (g[1] / max * 255.0).round() as u8, 255]
        );
    }

    #[test]
    fn the_file_holds_both_images_found_through_its_index() {
        let jpeg = |w: u32, gray: bool| {
            let mut bytes = Vec::new();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
            if gray {
                image::ImageEncoder::write_image(
                    enc,
                    &vec![200u8; (w * w) as usize],
                    w,
                    w,
                    image::ExtendedColorType::L8,
                )
                .unwrap();
            } else {
                image::ImageEncoder::write_image(
                    enc,
                    &vec![100u8; (w * w * 3) as usize],
                    w,
                    w,
                    image::ExtendedColorType::Rgb8,
                )
                .unwrap();
            }
            bytes
        };
        let (primary, gain) = (jpeg(32, false), jpeg(16, true));
        let meta = b"<?xpacket begin=\"\"?><x:xmpmeta><rdf:RDF><rdf:Description dc:title=\"Haldi\"/></rdf:RDF></x:xmpmeta>";
        let file = assemble(&primary, Some(meta), &gain, 1.5);
        // The primary still opens as it was.
        let shown = image::load_from_memory(&file).unwrap();
        assert_eq!((shown.width(), shown.height()), (32, 32));
        // The gain map is where the index says, a JPEG with its parameters.
        let found = find_gain_map(&file).unwrap();
        let map = image::load_from_memory(found).unwrap();
        assert_eq!((map.width(), map.height()), (16, 16));
        let text = String::from_utf8_lossy(found);
        assert!(text.contains("hdrgm:GainMapMax=\"1.500000\""));
        assert!(text.contains("hdrgm:BaseRenditionIsHDR=\"False\""));
        // The primary says where to look, and keeps the photo's own XMP.
        let head = String::from_utf8_lossy(&file[..file.len() - found.len()]);
        assert!(head.contains("hdrgm:Version=\"1.0\""));
        assert!(head.contains(&format!("Item:Length=\"{}\"", found.len())));
        assert!(head.contains("dc:title=\"Haldi\""));
        assert!(file.ends_with(found), "the gain map comes last");
    }
}
