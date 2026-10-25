use crate::{Cfa, Color, Orientation, RawError, RawFrame, Rect};
use rawler::RawlerError;
use rawler::cfa::CFA;
use rawler::decoders::Orientation as RwOrientation;
use rawler::decoders::RawDecodeParams;
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::RawImageData;
use std::path::Path;

/// Reason given when rawler panics on a file.
pub(crate) const DECODER_PANICKED: &str = "the decoder failed on this file";

/// Run `f`, turning a panic inside it into [`RawError::Corrupt`] naming `path`.
/// rawler can panic on malformed input; see the crate docs for why this needs
/// `panic = "unwind"`.
pub(crate) fn guarded<T>(
    path: &Path,
    f: impl FnOnce() -> Result<T, RawError>,
) -> Result<T, RawError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(RawError::Corrupt {
            path: path.to_path_buf(),
            reason: DECODER_PANICKED.to_string(),
        })
    })
}

#[cfg(test)]
thread_local! {
    /// Test hook: when set, the guarded decode step panics, as rawler can.
    pub(crate) static INJECT_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn maybe_inject_panic() {
    if INJECT_PANIC.with(|p| p.get()) {
        panic!("injected decoder panic");
    }
}

#[cfg(not(test))]
#[inline(always)]
pub(crate) fn maybe_inject_panic() {}

/// "Make Model", without the make twice when the model already starts with it
/// ("Canon" + "Canon EOS R5" is "Canon EOS R5"). None when both are empty.
pub(crate) fn camera_name(make: &str, model: &str) -> Option<String> {
    let (make, model) = (make.trim(), model.trim());
    let name = if !make.is_empty()
        && model
            .get(..make.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(make))
    {
        model.to_string()
    } else {
        format!("{make} {model}").trim().to_string()
    };
    (!name.is_empty()).then_some(name)
}

/// rawler's error as ours. `Unsupported` names the camera when rawler knows it. A
/// `DecoderFailed` whose message says the variant is not implemented ("not
/// supported", "Unsupported", "Don't know", "unimplemented") is `Unsupported` too
/// (e.g. Nikon HE NEF); any other is `Corrupt`.
fn from_rawler(path: &Path, e: RawlerError) -> RawError {
    match e {
        RawlerError::DecoderFailed(reason) => {
            let lower = reason.to_lowercase();
            let gap = [
                "not supported",
                "unsupported",
                "don't know",
                "unimplemented",
            ]
            .iter()
            .any(|k| lower.contains(k));
            if gap {
                RawError::Unsupported {
                    path: path.to_path_buf(),
                    reason,
                }
            } else {
                RawError::Corrupt {
                    path: path.to_path_buf(),
                    reason,
                }
            }
        }
        RawlerError::Unsupported {
            what, make, model, ..
        } => {
            if looks_like_raw(path) {
                RawError::Unsupported {
                    path: path.to_path_buf(),
                    reason: match camera_name(&make, &model) {
                        Some(name) => format!("{name} RAW files aren’t supported yet."),
                        None => what,
                    },
                }
            } else {
                RawError::NotAnImage {
                    path: path.to_path_buf(),
                }
            }
        }
    }
}

/// Decode a RAW file (DNG or any camera format rawler knows) into sensor data.
///
/// Refused before decoding: a file whose header points past its end
/// ([`crate::container`]: "the file is incomplete"). rawler panics become
/// `Corrupt`; a frame whose white level is not above its black level is `Corrupt`.
pub fn decode_raw(path: &Path) -> Result<RawFrame, RawError> {
    let io = |source| RawError::Io {
        path: path.to_path_buf(),
        source,
    };
    std::fs::File::open(path).map_err(io)?;
    let corrupt = |reason: &str| RawError::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if std::fs::metadata(path).map_err(io)?.len() == 0 {
        return Err(RawError::NotAnImage {
            path: path.to_path_buf(),
        });
    }
    let src = rawler::rawsource::RawSource::new(path).map_err(io)?;
    if let Some(reason) = crate::container::incomplete(src.buf()) {
        return Err(corrupt(reason));
    }
    let mut raw = guarded(path, || {
        let decoder = rawler::get_decoder(&src).map_err(|e| from_rawler(path, e))?;
        maybe_inject_panic();
        decoder
            .raw_image(&src, &RawDecodeParams::default(), false)
            .map_err(|e| from_rawler(path, e))
    })?;
    drop(src);
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
    // Canon sRAW / mRAW: rawler's YUV to RGB conversion has already removed the
    // black offset, so the levels it reports would be subtracted twice.
    if cfa == Cfa::Rgb && raw.clean_make.eq_ignore_ascii_case("canon") {
        black = [0.0; 4];
    }
    let white = raw.whitelevel.as_vec().into_iter().fold(0.0f32, f32::max);
    let max_black = black.iter().copied().fold(f32::MIN, f32::max);
    if !(white.is_finite() && white > max_black) {
        return Err(corrupt("the white level is not above the black level"));
    }

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

    // Prefer D65; otherwise the daylight-like matrices (adapting from them is the
    // smallest step), then anything (the order of the others is deterministic).
    let preference = |ill: &Illuminant| match ill {
        Illuminant::D65 => 0,
        Illuminant::D55 => 1,
        Illuminant::D50 => 2,
        Illuminant::D75 => 3,
        _ => 4,
    };
    let (xyz_to_cam, matrix_white) = raw
        .color_matrix
        .iter()
        .filter(|(_, m)| m.len() >= 9)
        .min_by_key(|(ill, _)| (preference(ill), **ill))
        .map(|(ill, m)| {
            (
                Some([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]),
                crate::color::white_of_exif_illuminant(*ill as u16),
            )
        })
        .unwrap_or((None, crate::color::D65_WHITE));

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
        matrix_white,
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

// The integration tests' DNG writer, for the panic-hook tests below. (A top-level
// `#[path]` resolves from `src/`; inside `mod tests` it would go through the
// non-existent `src/decode/tests/`, which only Windows tolerates.)
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/support/synthetic_dng.rs"]
mod synthetic_dng;

#[cfg(test)]
mod tests {
    use super::synthetic_dng;
    use super::*;
    use crate::demosaic::Quality;
    use crate::open::open;
    use image::{Rgb, Rgb32FImage};

    fn dng(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("lf-raw-guard-{}-{name}.dng", std::process::id()));
        let img = Rgb32FImage::from_fn(32, 32, |_, _| Rgb([0.3, 0.3, 0.3]));
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        synthetic_dng::write(&path, &img, synthetic_dng::RGGB, 64, 4095, [1.0; 3], m);
        path
    }

    fn with_injected_panic<T>(f: impl FnOnce() -> T) -> T {
        INJECT_PANIC.with(|p| p.set(true));
        let out = f();
        INJECT_PANIC.with(|p| p.set(false));
        out
    }

    #[test]
    fn a_panic_in_the_decoder_is_corrupt_not_a_crash() {
        let path = dng("panic");
        assert!(decode_raw(&path).is_ok());
        for r in [
            with_injected_panic(|| decode_raw(&path).map(|_| ())),
            with_injected_panic(|| open(&path, Quality::Fast, &Default::default(), 64).map(|_| ())),
        ] {
            match r {
                Err(RawError::Corrupt { reason, .. }) => assert_eq!(reason, DECODER_PANICKED),
                other => panic!("{other:?}"),
            }
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn guarded_maps_any_panic_to_corrupt() {
        let r: Result<(), RawError> = guarded(Path::new("x.nef"), || panic!("boom"));
        let e = r.unwrap_err();
        assert!(matches!(e, RawError::Corrupt { .. }));
        assert!(e.to_string().contains("x.nef"), "{e}");
    }

    #[test]
    fn camera_names_drop_a_repeated_make() {
        assert_eq!(
            camera_name("Canon", "Canon EOS R5").as_deref(),
            Some("Canon EOS R5")
        );
        assert_eq!(
            camera_name("NIKON", "NIKON Z 8").as_deref(),
            Some("NIKON Z 8")
        );
        assert_eq!(
            camera_name("Sony", "ILCE-7M4").as_deref(),
            Some("Sony ILCE-7M4")
        );
        assert_eq!(camera_name(" ", "").as_deref(), None);
    }

    #[test]
    fn decoder_gaps_are_unsupported_and_other_failures_corrupt() {
        let p = Path::new("a.nef");
        for msg in [
            "NEF compression HighEfficiency is not supported",
            "Unsupported JPEG-XL tile",
            "ARW: Don't know how to decode type 7",
            "not yet UNIMPLEMENTED",
        ] {
            let e = from_rawler(p, RawlerError::DecoderFailed(msg.into()));
            assert!(matches!(e, RawError::Unsupported { .. }), "{msg}: {e:?}");
        }
        let e = from_rawler(p, RawlerError::DecoderFailed("bitstream ended".into()));
        assert!(matches!(e, RawError::Corrupt { .. }), "{e:?}");
    }

    #[test]
    fn a_white_level_at_or_below_black_is_corrupt() {
        let path =
            std::env::temp_dir().join(format!("lf-raw-guard-{}-white.dng", std::process::id()));
        let img = Rgb32FImage::from_fn(32, 32, |_, _| Rgb([0.3, 0.3, 0.3]));
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        synthetic_dng::write(&path, &img, synthetic_dng::RGGB, 4095, 4095, [1.0; 3], m);
        match decode_raw(&path) {
            Err(RawError::Corrupt { reason, .. }) => {
                assert_eq!(reason, "the white level is not above the black level")
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_file(path);
    }
}
