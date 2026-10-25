//! Colour matrices: camera RGB to linear sRGB (D65), with Bradford adaptation.
//!
//! ## Camera to sRGB
//!
//! A DNG `ColorMatrix` maps XYZ (relative to its calibration illuminant's white) to
//! camera RGB. [`cam_to_srgb`] builds the matrix that takes white-balanced camera
//! RGB to linear sRGB:
//!
//! 1. `A = xyz_to_cam × bradford(D65 → illuminant white)`: XYZ (D65) to camera.
//!    For a D65 matrix the adaptation is the identity.
//! 2. Normalise each row of `A` so the D65 white point maps to camera (1, 1, 1):
//!    row `i` is divided by `A[i] · D65_WHITE`. Camera (1, 1, 1) after white balance
//!    is then neutral (the dcraw / DNG convention).
//! 3. `cam_to_srgb = XYZ_D65_TO_LINEAR_SRGB × inverse(A)`.
//! 4. Rows are rescaled to sum to exactly 1, removing rounding drift, so camera
//!    white maps to sRGB (1, 1, 1).
//!
//! Without a matrix, or with a degenerate one, the result is the identity (logged).

pub type Mat3 = [[f32; 3]; 3];
type Mat3d = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// XYZ (D65) to linear sRGB (IEC 61966-2-1, as tabulated by Bruce Lindbloom).
pub const XYZ_D65_TO_LINEAR_SRGB: Mat3 = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

/// Bradford cone response matrix (XYZ to LMS-like "sharpened" space).
pub const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

// Reference white points, XYZ with Y = 1 (CIE 1931 2°).
pub const D65_WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];
pub const D50_WHITE: [f32; 3] = [0.964_22, 1.0, 0.825_21];
pub const D55_WHITE: [f32; 3] = [0.956_82, 1.0, 0.921_49];
pub const D75_WHITE: [f32; 3] = [0.949_72, 1.0, 1.226_38];
pub const A_WHITE: [f32; 3] = [1.098_50, 1.0, 0.355_85];
pub const B_WHITE: [f32; 3] = [0.990_72, 1.0, 0.852_23];
pub const C_WHITE: [f32; 3] = [0.980_74, 1.0, 1.182_32];
/// F2 (cool white fluorescent), used for the fluorescent EXIF light sources.
pub const F2_WHITE: [f32; 3] = [0.991_86, 1.0, 0.673_93];

/// White point of an EXIF/DNG `LightSource` / `CalibrationIlluminant` code.
/// Unknown codes are treated as D65 (no adaptation).
pub fn white_of_exif_illuminant(code: u16) -> [f32; 3] {
    match code {
        17 | 3 | 24 => A_WHITE, // Standard light A, tungsten, ISO studio tungsten
        18 => B_WHITE,          // Standard light B
        19 => C_WHITE,          // Standard light C
        20 | 1 | 4 | 9 => D55_WHITE, // D55, daylight, flash, fine weather
        22 | 11 => D75_WHITE,   // D75, shade
        23 => D50_WHITE,        // D50
        2 | 13 | 14 | 15 => F2_WHITE, // fluorescent kinds
        _ => D65_WHITE,         // D65, cloudy, daylight fluorescent, unknown
    }
}

/// The matrix that takes white-balanced camera RGB to linear sRGB (see the module docs).
pub fn cam_to_srgb(xyz_to_cam: Option<Mat3>, illuminant_white: [f32; 3]) -> Mat3 {
    let Some(m) = xyz_to_cam else {
        log::warn!("no colour matrix: camera RGB is used as sRGB unchanged");
        return IDENTITY;
    };
    let d65 = to_f64v(D65_WHITE);
    let a = mul(&to_f64(m), &bradford_d(d65, to_f64v(illuminant_white)));
    let mut a = a;
    for row in &mut a {
        let s = dot(row, &d65);
        if !s.is_finite() || s.abs() < 1e-9 {
            log::warn!("degenerate colour matrix {m:?}: using the identity");
            return IDENTITY;
        }
        row.iter_mut().for_each(|v| *v /= s);
    }
    let Some(inv) = inverse(&a) else {
        log::warn!("singular colour matrix {m:?}: using the identity");
        return IDENTITY;
    };
    let mut out = mul(&to_f64(XYZ_D65_TO_LINEAR_SRGB), &inv);
    for row in &mut out {
        let s: f64 = row.iter().sum();
        if s.abs() > 1e-9 {
            row.iter_mut().for_each(|v| *v /= s);
        }
    }
    out.map(|r| r.map(|v| v as f32))
}

/// Bradford chromatic adaptation (XYZ to XYZ) from white `from` to white `to`.
pub fn bradford(from: [f32; 3], to: [f32; 3]) -> Mat3 {
    bradford_d(to_f64v(from), to_f64v(to)).map(|r| r.map(|v| v as f32))
}

fn bradford_d(from: [f64; 3], to: [f64; 3]) -> Mat3d {
    let b = to_f64(BRADFORD);
    // BRADFORD is well conditioned; the inverse always exists.
    let b_inv = inverse(&b).unwrap_or(to_f64(IDENTITY));
    let (src, dst) = (mul_v(&b, &from), mul_v(&b, &to));
    let scale: Mat3d = [
        [dst[0] / src[0], 0.0, 0.0],
        [0.0, dst[1] / src[1], 0.0],
        [0.0, 0.0, dst[2] / src[2]],
    ];
    mul(&b_inv, &mul(&scale, &b))
}

/// `m × v`.
#[inline]
pub fn apply(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn to_f64(m: Mat3) -> Mat3d {
    m.map(|r| r.map(f64::from))
}

fn to_f64v(v: [f32; 3]) -> [f64; 3] {
    v.map(f64::from)
}

fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn mul_v(m: &Mat3d, v: &[f64; 3]) -> [f64; 3] {
    [dot(&m[0], v), dot(&m[1], v), dot(&m[2], v)]
}

fn mul(a: &Mat3d, b: &Mat3d) -> Mat3d {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn inverse(m: &Mat3d) -> Option<Mat3d> {
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let cof = [
        [c(1, 1, 2, 2), -c(1, 0, 2, 2), c(1, 0, 2, 1)],
        [-c(0, 1, 2, 2), c(0, 0, 2, 2), -c(0, 0, 2, 1)],
        [c(0, 1, 1, 2), -c(0, 0, 1, 2), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * cof[0][0] + m[0][1] * cof[0][1] + m[0][2] * cof[0][2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    // Inverse = transpose of the cofactor matrix / det.
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = cof[j][i] / det;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bradford_maps_source_white_to_destination_white() {
        let m = bradford(A_WHITE, D65_WHITE);
        let w = apply(&m, A_WHITE);
        for c in 0..3 {
            assert!((w[c] - D65_WHITE[c]).abs() < 1e-5, "{w:?}");
        }
        assert_eq!(
            bradford(D65_WHITE, D65_WHITE).map(|r| r.map(|v| (v * 1e5).round())),
            IDENTITY.map(|r| r.map(|v| v * 1e5))
        );
    }

    #[test]
    fn inverse_inverts() {
        let m = to_f64(XYZ_D65_TO_LINEAR_SRGB);
        let p = mul(&m, &inverse(&m).unwrap());
        for (i, row) in p.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
        assert!(inverse(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }
}
