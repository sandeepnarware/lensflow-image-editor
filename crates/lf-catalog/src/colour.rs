//! Colour spaces for exports and soft-proofing, through Little CMS (`lcms2`).
//!
//! The develop pipeline renders sRGB. An export can be converted to Display P3, Adobe RGB
//! (1998) or any RGB ICC profile (a print lab's) with that profile embedded. The photo's
//! colours stay those of the sRGB render, so a wider space does not add colours the render
//! did not have; it puts them where a lab or a wide-gamut workflow expects them.
//!
//! Soft-proofing builds a 3D LUT that sends each sRGB colour through a printer or paper
//! profile and back ([`proof_lut`]): the preview applies it, so the photo shows as it will
//! print. Exports never carry it.

use lcms2::{CIExyY, CIExyYTRIPLE, Flags, Intent, PixelFormat, Profile, ToneCurve, Transform};
use lf_settings::lut::Lut;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where an export's colours are written.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputSpace {
    #[default]
    Srgb,
    DisplayP3,
    AdobeRgb,
    /// An RGB ICC profile from a file (a lab's printer and paper).
    Profile(PathBuf),
}

impl OutputSpace {
    pub fn name(&self) -> String {
        match self {
            OutputSpace::Srgb => "sRGB (web, most labs)".to_owned(),
            OutputSpace::DisplayP3 => "Display P3".to_owned(),
            OutputSpace::AdobeRgb => "Adobe RGB (1998)".to_owned(),
            OutputSpace::Profile(p) => p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
        }
    }
}

/// Why a profile cannot be used: a sentence to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColourError {
    #[error("The profile {0} could not be read.")]
    Unreadable(String),
    #[error(
        "The profile {0} is not an RGB profile, so photos cannot be exported in it. It can still be used to soft-proof."
    )]
    NotRgb(String),
    #[error("The colours could not be converted ({0}).")]
    Transform(String),
}

fn xyy(x: f64, y: f64) -> CIExyY {
    CIExyY { x, y, Y: 1.0 }
}

const D65: (f64, f64) = (0.3127, 0.3290);

/// An RGB matrix-and-curve profile from primaries and a tone curve.
fn matrix_profile(primaries: [(f64, f64); 3], curve: &ToneCurve, name: &str) -> Profile {
    let triple = CIExyYTRIPLE {
        Red: xyy(primaries[0].0, primaries[0].1),
        Green: xyy(primaries[1].0, primaries[1].1),
        Blue: xyy(primaries[2].0, primaries[2].1),
    };
    let mut p = Profile::new_rgb(&xyy(D65.0, D65.1), &triple, &[curve, curve, curve])
        .expect("a matrix profile");
    let mut desc = lcms2::MLU::new(1);
    desc.set_text_ascii(name, lcms2::Locale::none());
    p.remove_tag(lcms2::TagSignature::ProfileDescriptionTag);
    p.write_tag(
        lcms2::TagSignature::ProfileDescriptionTag,
        lcms2::Tag::MLU(&desc),
    );
    p
}

/// The sRGB tone curve (IEC 61966-2-1): type 4 parametric.
fn srgb_curve() -> ToneCurve {
    ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
        .expect("the sRGB curve")
}

/// The profile for `space` (an RGB one).
pub fn profile(space: &OutputSpace) -> Result<Profile, ColourError> {
    Ok(match space {
        OutputSpace::Srgb => Profile::new_srgb(),
        OutputSpace::DisplayP3 => matrix_profile(
            [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
            &srgb_curve(),
            "Display P3 (LensFlow)",
        ),
        OutputSpace::AdobeRgb => matrix_profile(
            [(0.6400, 0.3300), (0.2100, 0.7100), (0.1500, 0.0600)],
            &ToneCurve::new(563.0 / 256.0),
            "Adobe RGB (1998) compatible (LensFlow)",
        ),
        OutputSpace::Profile(path) => {
            let name = path.display().to_string();
            let p = read(path)?;
            if p.color_space() != lcms2::ColorSpaceSignature::RgbData {
                return Err(ColourError::NotRgb(name));
            }
            p
        }
    })
}

/// Reads an ICC profile file.
pub fn read(path: &Path) -> Result<Profile, ColourError> {
    Profile::new_file(path).map_err(|_| ColourError::Unreadable(path.display().to_string()))
}

/// 8-bit sRGB `pixels` (RGB, row by row) converted to `space`, with that space's profile to
/// embed. sRGB comes back as it was.
pub fn convert8(pixels: &mut [u8], space: &OutputSpace) -> Result<Vec<u8>, ColourError> {
    let dest = profile(space)?;
    let icc = dest
        .icc()
        .map_err(|e| ColourError::Transform(e.to_string()))?;
    if *space != OutputSpace::Srgb {
        let t: Transform<[u8; 3], [u8; 3]> = Transform::new(
            &Profile::new_srgb(),
            PixelFormat::RGB_8,
            &dest,
            PixelFormat::RGB_8,
            Intent::Perceptual,
        )
        .map_err(|e| ColourError::Transform(e.to_string()))?;
        let px: &mut [[u8; 3]] = pixels.as_chunks_mut::<3>().0;
        t.transform_in_place(px);
    }
    Ok(icc)
}

/// 16-bit sRGB `pixels` converted to `space`, with that space's profile to embed.
pub fn convert16(pixels: &mut [u16], space: &OutputSpace) -> Result<Vec<u8>, ColourError> {
    let dest = profile(space)?;
    let icc = dest
        .icc()
        .map_err(|e| ColourError::Transform(e.to_string()))?;
    if *space != OutputSpace::Srgb {
        let t: Transform<[u16; 3], [u16; 3]> = Transform::new(
            &Profile::new_srgb(),
            PixelFormat::RGB_16,
            &dest,
            PixelFormat::RGB_16,
            Intent::Perceptual,
        )
        .map_err(|e| ColourError::Transform(e.to_string()))?;
        let px: &mut [[u16; 3]] = pixels.as_chunks_mut::<3>().0;
        t.transform_in_place(px);
    }
    Ok(icc)
}

/// The grid size of a soft-proof LUT.
pub const PROOF_SIZE: usize = 33;

/// A LUT that shows sRGB colours as the printer or paper of `proof` (any ICC profile, RGB or
/// CMYK) would print them, back in sRGB. `look` (a LUT and its amount) is applied first, so
/// one LUT stands for both on the preview.
pub fn proof_lut(proof: &Path, look: Option<(&Lut, f32)>) -> Result<Lut, ColourError> {
    let printer = read(proof)?;
    let srgb = Profile::new_srgb();
    let t: Transform<[u16; 3], [u16; 3]> = Transform::new_proofing(
        &srgb,
        PixelFormat::RGB_16,
        &srgb,
        PixelFormat::RGB_16,
        &printer,
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Flags::SOFT_PROOFING,
    )
    .map_err(|e| ColourError::Transform(e.to_string()))?;
    let n = PROOF_SIZE;
    // The LUT's order: red fastest, then green, then blue (`.cube`'s).
    let mut grid: Vec<[u16; 3]> = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let mut c = [r, g, b].map(|v| v as f32 / (n - 1) as f32);
                if let Some((lut, amount)) = look {
                    let looked = lut.apply(c);
                    for i in 0..3 {
                        c[i] += (looked[i] - c[i]) * amount;
                    }
                }
                grid.push(c.map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16));
            }
        }
    }
    t.transform_in_place(&mut grid);
    let data = grid
        .into_iter()
        .map(|c| c.map(|v| f32::from(v) / 65535.0))
        .collect();
    Ok(Lut {
        size: n,
        data,
        title: Some("Soft proof".to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_stays_and_wider_spaces_pull_colours_in() {
        let mut px = vec![255u8, 0, 0, 128, 128, 128];
        let icc = convert8(&mut px, &OutputSpace::Srgb).unwrap();
        assert_eq!(px, [255, 0, 0, 128, 128, 128]);
        assert!(!icc.is_empty());
        let mut p3 = px.clone();
        let icc = convert8(&mut p3, &OutputSpace::DisplayP3).unwrap();
        // The name is stored as UTF-16 (big-endian).
        let name: Vec<u8> = "Display P3"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert!(icc.windows(name.len()).any(|w| w == name.as_slice()));
        // sRGB red is inside P3: less than full red there, with a little green.
        assert!(p3[0] < 250 && p3[1] > 0, "{p3:?}");
        // A neutral grey stays neutral.
        assert!(
            p3[3].abs_diff(p3[4]) <= 1 && p3[4].abs_diff(p3[5]) <= 1,
            "{p3:?}"
        );
        let mut wide = vec![0u16, 65535, 0];
        convert16(&mut wide, &OutputSpace::AdobeRgb).unwrap();
        // sRGB green sits inside Adobe RGB's wider green: some red and blue join it.
        assert!(wide[0] > 10_000 && wide[2] > 1_000, "{wide:?}");
    }

    #[test]
    fn a_missing_profile_says_so() {
        let e = profile(&OutputSpace::Profile("Z:/nowhere/lab.icc".into())).unwrap_err();
        assert!(matches!(e, ColourError::Unreadable(_)));
    }

    #[test]
    fn proofing_through_srgb_changes_little_and_a_look_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("srgb.icc");
        std::fs::write(&path, Profile::new_srgb().icc().unwrap()).unwrap();
        let lut = proof_lut(&path, None).unwrap();
        assert_eq!(lut.data.len(), PROOF_SIZE.pow(3));
        let mid = lut.apply([0.5, 0.25, 0.75]);
        assert!(
            (mid[0] - 0.5).abs() < 0.02 && (mid[2] - 0.75).abs() < 0.02,
            "{mid:?}"
        );
        // A look that inverts, at full amount, comes through.
        let mut invert = Lut::identity(2);
        for c in &mut invert.data {
            *c = c.map(|v| 1.0 - v);
        }
        let both = proof_lut(&path, Some((&invert, 1.0))).unwrap();
        let black = both.apply([0.0, 0.0, 0.0]);
        assert!(black[0] > 0.95, "{black:?}");
    }
}
