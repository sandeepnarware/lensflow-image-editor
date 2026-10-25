use crate::{Cfa, Color, Orientation, RawError, RawFrame, Rect};
use rawler::RawlerError;
use rawler::cfa::CFA;
use rawler::decoders::Orientation as RwOrientation;
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::RawImageData;
use std::path::Path;

/// Decode a RAW file (DNG or any camera format rawler knows) into sensor data.
pub fn decode_raw(path: &Path) -> Result<RawFrame, RawError> {
    let io = |source| RawError::Io {
        path: path.to_path_buf(),
        source,
    };
    std::fs::File::open(path).map_err(io)?;

    let mut raw = rawler::decode_file(path).map_err(|e| match e {
        RawlerError::DecoderFailed(reason) => RawError::Corrupt {
            path: path.to_path_buf(),
            reason,
        },
        RawlerError::Unsupported { what, .. } => {
            if looks_like_raw(path) {
                RawError::Unsupported {
                    path: path.to_path_buf(),
                    reason: what,
                }
            } else {
                RawError::NotAnImage {
                    path: path.to_path_buf(),
                }
            }
        }
    })?;
    let unsupported = |reason: &str| RawError::Unsupported {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };

    let data = match std::mem::replace(&mut raw.data, RawImageData::Integer(Vec::new())) {
        RawImageData::Integer(v) => v,
        RawImageData::Float(_) => return Err(unsupported("floating-point sensor data")),
    };
    let (width, height) = (raw.width as u32, raw.height as u32);

    let cfa = if raw.cpp == 3 {
        Cfa::Rgb
    } else if raw.cpp == 1 {
        cfa_from(&raw.camera.cfa)
            .ok_or_else(|| unsupported("an unrecognised colour filter layout"))?
    } else {
        return Err(unsupported("an unexpected number of samples per pixel"));
    };

    let mut black = [0.0f32; 4];
    let levels = raw.blacklevel.as_vec();
    if !levels.is_empty() {
        for (i, slot) in black.iter_mut().enumerate() {
            *slot = levels[i.min(levels.len() - 1)];
        }
    }
    let white = raw.whitelevel.as_vec().into_iter().fold(0.0f32, f32::max);

    let mut wb = raw.wb_coeffs;
    if wb.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        // 4th (second green) is NaN on 3-colour cameras; mirror green.
        wb[3] = wb[1];
    }
    if wb[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        wb = [1.0; 4];
    }
    let g = wb[1];
    wb = [wb[0] / g, 1.0, wb[2] / g, wb[3] / g];

    let xyz_to_cam = raw
        .color_matrix
        .get(&Illuminant::D65)
        .or_else(|| raw.color_matrix.values().next())
        .filter(|m| m.len() >= 9)
        .map(|m| [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]);

    let r = raw.crop_area.or(raw.active_area);
    let crop = match r {
        Some(r) => Rect {
            x: r.p.x as u32,
            y: r.p.y as u32,
            w: r.d.w as u32,
            h: r.d.h as u32,
        },
        None => Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
    };

    let orientation = Orientation::from_exif(match raw.orientation {
        RwOrientation::Normal | RwOrientation::Unknown => 1,
        RwOrientation::HorizontalFlip => 2,
        RwOrientation::Rotate180 => 3,
        RwOrientation::VerticalFlip => 4,
        RwOrientation::Transpose => 5,
        RwOrientation::Rotate90 => 6,
        RwOrientation::Transverse => 7,
        RwOrientation::Rotate270 => 8,
    });

    Ok(RawFrame {
        width,
        height,
        cfa,
        data,
        black,
        white,
        wb,
        xyz_to_cam,
        crop,
        orientation,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
    })
}

fn color(c: usize) -> Option<Color> {
    match c {
        0 => Some(Color::Red),
        1 | 7 => Some(Color::Green),
        2 => Some(Color::Blue),
        _ => None,
    }
}

fn cfa_from(cfa: &CFA) -> Option<Cfa> {
    match (cfa.width, cfa.height) {
        (2, 2) => {
            let mut pattern = [[Color::Green; 2]; 2];
            for (r, row) in pattern.iter_mut().enumerate() {
                for (c, slot) in row.iter_mut().enumerate() {
                    *slot = color(cfa.color_at(r, c))?;
                }
            }
            Some(Cfa::Bayer { pattern })
        }
        (6, 6) => {
            let mut pattern = [[Color::Green; 6]; 6];
            for (r, row) in pattern.iter_mut().enumerate() {
                for (c, slot) in row.iter_mut().enumerate() {
                    *slot = color(cfa.color_at(r, c))?;
                }
            }
            Some(Cfa::XTrans { pattern })
        }
        _ => None,
    }
}

/// Rawler reports "unsupported" both for unknown cameras and for files that are not
/// photos at all. A TIFF-based or other known RAW header means a real (but unsupported) photo.
fn looks_like_raw(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 12];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    let h = &head[..n];
    h.starts_with(b"II*\0")
        || h.starts_with(b"MM\0*")
        || h.starts_with(b"IIRO")
        || h.starts_with(b"FUJIFILM")
        || h.starts_with(b"IIU\0")
        || h.get(4..8) == Some(b"ftyp")
        || h.starts_with(b"\0MRM")
        || h.starts_with(b"FOVb")
}
