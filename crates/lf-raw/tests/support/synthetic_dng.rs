//! Writes a minimal uncompressed 16-bit CFA DNG from a linear RGB image, so the
//! decoder can be tested without shipping a camera file.

use image::Rgb32FImage;
use std::path::Path;

pub const RGGB: [u8; 4] = [0, 1, 1, 2];

struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    data: Vec<u8>,
}

fn short(tag: u16, v: &[u16]) -> Entry {
    Entry {
        tag,
        kind: 3,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn long(tag: u16, v: &[u32]) -> Entry {
    Entry {
        tag,
        kind: 4,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn bytes(tag: u16, v: &[u8]) -> Entry {
    Entry {
        tag,
        kind: 1,
        count: v.len() as u32,
        data: v.to_vec(),
    }
}
fn ascii(tag: u16, s: &str) -> Entry {
    let mut d = s.as_bytes().to_vec();
    d.push(0);
    Entry {
        tag,
        kind: 2,
        count: d.len() as u32,
        data: d,
    }
}
fn rationals(tag: u16, kind: u16, v: &[(i32, u32)]) -> Entry {
    let data = v
        .iter()
        .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
        .collect();
    Entry {
        tag,
        kind,
        count: v.len() as u32,
        data,
    }
}

/// Mosaic `image` through `pattern` (0 = R, 1 = G, 2 = B, over a 2x2 tile) and
/// write a DNG. Each channel is divided by its white-balance multiplier first, so
/// decoding and multiplying by the as-shot WB restores the image.
pub fn write(
    path: &Path,
    image: &Rgb32FImage,
    pattern: [u8; 4],
    black: u16,
    white: u16,
    wb: [f32; 3],
    matrix: [[f32; 3]; 3],
) {
    let (w, h) = image.dimensions();
    let mut pixels = Vec::with_capacity((w * h * 2) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = pattern[((y % 2) * 2 + x % 2) as usize] as usize;
            let v = (image.get_pixel(x, y).0[c] / wb[c]).clamp(0.0, 1.0);
            let s = black as f32 + v * (white - black) as f32;
            pixels.extend_from_slice(&(s.round() as u16).to_le_bytes());
        }
    }

    let flat: Vec<(i32, u32)> = matrix
        .iter()
        .flatten()
        .map(|v| ((v * 10000.0).round() as i32, 10000))
        .collect();
    let neutral: Vec<(i32, u32)> = wb
        .iter()
        .map(|m| ((10000.0 / m).round() as i32, 10000))
        .collect();

    let data_off = 8u32;
    let mut entries = vec![
        long(254, &[0]),
        long(256, &[w]),
        long(257, &[h]),
        short(258, &[16]),
        short(259, &[1]),
        short(262, &[32803]),
        ascii(271, "Synthetic"),
        ascii(272, "Synthetic"),
        long(273, &[data_off]),
        short(274, &[1]),
        short(277, &[1]),
        long(278, &[h]),
        long(279, &[pixels.len() as u32]),
        short(284, &[1]),
        short(33421, &[2, 2]),
        bytes(33422, &pattern),
        bytes(50706, &[1, 4, 0, 0]),
        ascii(50708, "Synthetic"),
        short(50713, &[2, 2]),
        short(50714, &[black; 4]),
        long(50717, &[white as u32]),
        rationals(50721, 10, &flat),
        rationals(
            50728,
            5,
            &neutral.iter().map(|(n, d)| (*n, *d)).collect::<Vec<_>>(),
        ),
        short(50778, &[21]),
    ];
    entries.sort_by_key(|e| e.tag);

    let ifd_off = data_off + pixels.len() as u32;
    let ifd_len = 2 + entries.len() as u32 * 12 + 4;
    let mut extra_off = ifd_off + ifd_len;
    let mut ifd = Vec::new();
    ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut extra = Vec::new();
    for e in &entries {
        ifd.extend_from_slice(&e.tag.to_le_bytes());
        ifd.extend_from_slice(&e.kind.to_le_bytes());
        ifd.extend_from_slice(&e.count.to_le_bytes());
        if e.data.len() <= 4 {
            let mut v = e.data.clone();
            v.resize(4, 0);
            ifd.extend_from_slice(&v);
        } else {
            ifd.extend_from_slice(&extra_off.to_le_bytes());
            extra.extend_from_slice(&e.data);
            if e.data.len() % 2 == 1 {
                extra.push(0);
            }
            extra_off = ifd_off + ifd_len + extra.len() as u32;
        }
    }
    ifd.extend_from_slice(&0u32.to_le_bytes());

    let mut file = Vec::new();
    file.extend_from_slice(b"II");
    file.extend_from_slice(&42u16.to_le_bytes());
    file.extend_from_slice(&ifd_off.to_le_bytes());
    file.extend_from_slice(&pixels);
    file.extend_from_slice(&ifd);
    file.extend_from_slice(&extra);
    std::fs::write(path, file).unwrap();
}
