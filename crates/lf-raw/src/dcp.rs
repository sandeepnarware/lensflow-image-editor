//! DNG camera profiles (`.dcp`): the camera-matching colour Lightroom, Camera Raw and
//! RawTherapee use. A DCP is a small TIFF-like file (`IIRC` or `MMCR`) carrying, for one camera
//! model, forward matrices (white-balanced camera RGB to XYZ D50) for one or two calibration
//! illuminants, an optional HueSatMap (hue shift, saturation and value scales over a hue ×
//! saturation × value grid), an optional LookTable (the same, for the "look"), an optional
//! tone curve and a baseline exposure offset. No profile ships with the app: users import
//! their own (Adobe's DNG Converter installs Adobe Standard and Camera Matching profiles).
//!
//! [`Dcp::prepare`] fixes a profile for one photo: the matrices and HueSatMaps of the two
//! illuminants are blended by the photo's white-balance temperature (in inverse kelvin, as
//! the DNG spec does). [`Prepared::apply`] then takes white-balanced camera RGB through
//! forward matrix → linear ProPhoto (D50) → HueSatMap → LookTable → tone curve (hue
//! preserving, as Adobe applies it) → linear sRGB (D65). Values above 1 keep their headroom:
//! the tables are looked up with the value clamped, and their scales applied to the real one.
//! A profile without forward matrices uses the file's own colour matrix instead.
//!
//! Profiles are kept in a process-wide registry ([`register`], [`get`]) known by
//! [`lf_settings::dcp_id`] of their name, so a photo stores only the name.

use crate::color::{self, Mat3};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Why a file is not a usable DCP: a sentence to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DcpError {
    #[error("This is not a DNG camera profile (.dcp).")]
    NotDcp,
    #[error("The camera profile is damaged ({0}).")]
    Damaged(String),
    #[error("The camera profile has no colour matrices.")]
    NoMatrix,
}

/// A hue × saturation × value table of (hue shift in degrees, saturation scale, value scale).
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Hue, saturation and value divisions.
    pub dims: [usize; 3],
    /// Value-major, then hue, then saturation (the DNG order).
    pub data: Vec<[f32; 3]>,
    /// The value axis is indexed by sRGB-encoded value rather than linear.
    pub srgb_value: bool,
}

/// A parsed camera profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Dcp {
    pub name: String,
    pub camera: String,
    /// SHA-256 of the file: part of the frame cache's key.
    pub digest: [u8; 32],
    illuminants: [u16; 2],
    color: [Option<Mat3>; 2],
    forward: [Option<Mat3>; 2],
    hue_sat: [Option<Table>; 2],
    look: Option<Table>,
    tone: Option<Vec<[f32; 2]>>,
    exposure_offset: f32,
}

// TIFF tags.
const UNIQUE_CAMERA_MODEL: u16 = 50708;
const COLOR_MATRIX: [u16; 2] = [50721, 50722];
const CALIBRATION_ILLUMINANT: [u16; 2] = [50778, 50779];
const PROFILE_NAME: u16 = 50936;
const HUE_SAT_DIMS: u16 = 50937;
const HUE_SAT_DATA: [u16; 2] = [50938, 50939];
const TONE_CURVE: u16 = 50940;
const FORWARD_MATRIX: [u16; 2] = [50964, 50965];
const LOOK_DIMS: u16 = 50981;
const LOOK_DATA: u16 = 50982;
const HUE_SAT_ENCODING: u16 = 51107;
const LOOK_ENCODING: u16 = 51108;
const EXPOSURE_OFFSET: u16 = 51109;

/// One IFD entry's raw parts.
struct Entry {
    typ: u16,
    count: usize,
    at: usize,
}

struct Reader<'a> {
    b: &'a [u8],
    le: bool,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&self, at: usize) -> Result<[u8; N], DcpError> {
        self.b
            .get(at..at + N)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| DcpError::Damaged("it ends early".to_owned()))
    }
    fn u16(&self, at: usize) -> Result<u16, DcpError> {
        let b = self.bytes::<2>(at)?;
        Ok(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32(&self, at: usize) -> Result<u32, DcpError> {
        let b = self.bytes::<4>(at)?;
        Ok(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn u64(&self, at: usize) -> Result<u64, DcpError> {
        let b = self.bytes::<8>(at)?;
        Ok(if self.le {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        })
    }

    /// An entry's values as numbers.
    fn numbers(&self, e: &Entry) -> Result<Vec<f64>, DcpError> {
        let size = match e.typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            t => return Err(DcpError::Damaged(format!("unknown type {t}"))),
        };
        if e.count > self.b.len() {
            return Err(DcpError::Damaged("an entry is too long".to_owned()));
        }
        (0..e.count)
            .map(|i| {
                let at = e.at + i * size;
                Ok(match e.typ {
                    1 | 7 => f64::from(self.bytes::<1>(at)?[0]),
                    6 => f64::from(self.bytes::<1>(at)?[0] as i8),
                    3 => f64::from(self.u16(at)?),
                    8 => f64::from(self.u16(at)? as i16),
                    4 => f64::from(self.u32(at)?),
                    9 => f64::from(self.u32(at)? as i32),
                    5 => {
                        let d = self.u32(at + 4)?;
                        f64::from(self.u32(at)?) / f64::from(d.max(1))
                    }
                    10 => {
                        let d = self.u32(at + 4)? as i32;
                        f64::from(self.u32(at)? as i32) / f64::from(if d == 0 { 1 } else { d })
                    }
                    11 => f64::from(f32::from_bits(self.u32(at)?)),
                    12 => f64::from_bits(self.u64(at)?),
                    _ => 0.0,
                })
            })
            .collect()
    }

    fn text(&self, e: &Entry) -> String {
        let end = (e.at + e.count).min(self.b.len());
        let raw = self.b.get(e.at..end).unwrap_or_default();
        String::from_utf8_lossy(raw)
            .trim_end_matches('\0')
            .trim()
            .to_owned()
    }
}

fn mat3(v: &[f64]) -> Option<Mat3> {
    (v.len() == 9).then(|| {
        [
            [v[0] as f32, v[1] as f32, v[2] as f32],
            [v[3] as f32, v[4] as f32, v[5] as f32],
            [v[6] as f32, v[7] as f32, v[8] as f32],
        ]
    })
}

fn table(dims: &[f64], data: &[f64], srgb_value: bool) -> Option<Table> {
    let dims = [
        *dims.first()? as usize,
        *dims.get(1)? as usize,
        dims.get(2).map_or(1, |v| *v as usize).max(1),
    ];
    let n = dims[0] * dims[1] * dims[2];
    (dims[0] >= 1 && dims[1] >= 2 && data.len() == n * 3).then(|| Table {
        dims,
        data: data
            .chunks(3)
            .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
            .collect(),
        srgb_value,
    })
}

impl Dcp {
    /// Reads a `.dcp` file's bytes.
    pub fn parse(bytes: &[u8]) -> Result<Dcp, DcpError> {
        let le = match bytes.get(..4) {
            Some(b"IIRC") | Some(b"II*\0") => true,
            Some(b"MMCR") | Some(b"MM\0*") => false,
            _ => return Err(DcpError::NotDcp),
        };
        let r = Reader { b: bytes, le };
        let ifd = r.u32(4)? as usize;
        let n = usize::from(r.u16(ifd)?);
        let mut entries: HashMap<u16, Entry> = HashMap::new();
        for i in 0..n {
            let at = ifd + 2 + i * 12;
            let (tag, typ, count) = (r.u16(at)?, r.u16(at + 2)?, r.u32(at + 4)? as usize);
            let size = match typ {
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => 1,
            };
            let at = if size * count <= 4 {
                at + 8
            } else {
                r.u32(at + 8)? as usize
            };
            entries.insert(tag, Entry { typ, count, at });
        }
        let num = |tag| -> Result<Option<Vec<f64>>, DcpError> {
            entries.get(&tag).map(|e| r.numbers(e)).transpose()
        };
        let text = |tag| entries.get(&tag).map(|e| r.text(e)).unwrap_or_default();
        let mut color = [None, None];
        let mut forward = [None, None];
        let mut illuminants = [0u16; 2];
        let mut hue_sat = [None, None];
        let hue_sat_dims = num(HUE_SAT_DIMS)?;
        let hue_sat_srgb = num(HUE_SAT_ENCODING)?.and_then(|v| v.first().copied()) == Some(1.0);
        for k in 0..2 {
            color[k] = num(COLOR_MATRIX[k])?.as_deref().and_then(mat3);
            forward[k] = num(FORWARD_MATRIX[k])?.as_deref().and_then(mat3);
            illuminants[k] = num(CALIBRATION_ILLUMINANT[k])?
                .and_then(|v| v.first().copied())
                .unwrap_or(0.0) as u16;
            if let (Some(d), Some(data)) = (&hue_sat_dims, num(HUE_SAT_DATA[k])?) {
                hue_sat[k] = table(d, &data, hue_sat_srgb);
            }
        }
        if color[0].is_none() && forward[0].is_none() {
            return Err(DcpError::NoMatrix);
        }
        let look_srgb = num(LOOK_ENCODING)?.and_then(|v| v.first().copied()) == Some(1.0);
        let look = match (num(LOOK_DIMS)?, num(LOOK_DATA)?) {
            (Some(d), Some(data)) => table(&d, &data, look_srgb),
            _ => None,
        };
        // A straight curve (RawTherapee's profiles carry one) is no curve: the base curve
        // stays.
        let tone = num(TONE_CURVE)?
            .filter(|v| v.len() >= 4)
            .filter(|v| v.chunks(2).any(|p| (p[0] - p[1]).abs() > 1e-4))
            .map(|v| v.chunks(2).map(|p| [p[0] as f32, p[1] as f32]).collect());
        Ok(Dcp {
            name: text(PROFILE_NAME),
            camera: text(UNIQUE_CAMERA_MODEL),
            digest: Sha256::digest(bytes).into(),
            illuminants,
            color,
            forward,
            hue_sat,
            look,
            tone,
            exposure_offset: num(EXPOSURE_OFFSET)?
                .and_then(|v| v.first().copied())
                .unwrap_or(0.0) as f32,
        })
    }

    /// Whether it carries a tone curve (which then replaces the base curve).
    pub fn has_tone_curve(&self) -> bool {
        self.tone.is_some()
    }

    /// The profile fixed for one photo: `kelvin` is its white balance's temperature (the two
    /// illuminants are blended by it), `fallback` the file's own white-balanced camera to
    /// linear sRGB matrix (used when the profile has no forward matrix).
    pub fn prepare(&self, kelvin: Option<f64>, fallback: &Mat3) -> Prepared {
        let w = blend_weight(self.illuminants, kelvin);
        let lerp_m = |a: &Option<Mat3>, b: &Option<Mat3>| match (a, b) {
            (Some(a), Some(b)) => Some(std::array::from_fn(|i| {
                std::array::from_fn(|j| a[i][j] * w + b[i][j] * (1.0 - w))
            })),
            (Some(a), None) | (None, Some(a)) => Some(*a),
            _ => None,
        };
        let cam_to_prophoto = match lerp_m(&self.forward[0], &self.forward[1]) {
            Some(fm) => mul(&XYZ_D50_TO_PROPHOTO, &fm),
            None => mul(&linear_srgb_to_prophoto(), fallback),
        };
        let hue_sat = match (&self.hue_sat[0], &self.hue_sat[1]) {
            (Some(a), Some(b)) if a.dims == b.dims => Some(Table {
                dims: a.dims,
                data: a
                    .data
                    .iter()
                    .zip(&b.data)
                    .map(|(x, y)| std::array::from_fn(|i| x[i] * w + y[i] * (1.0 - w)))
                    .collect(),
                srgb_value: a.srgb_value,
            }),
            (Some(a), _) | (None, Some(a)) => Some(a.clone()),
            _ => None,
        };
        Prepared {
            cam_to_prophoto,
            gain: 2f32.powf(self.exposure_offset),
            hue_sat,
            look: self.look.clone(),
            tone: self.tone.as_ref().map(|t| ToneLut::new(t)),
            prophoto_to_srgb: prophoto_to_linear_srgb(),
        }
    }
}

/// The temperature of a DNG calibration illuminant code.
fn illuminant_kelvin(code: u16) -> f64 {
    match code {
        17 | 3 => 2856.0,
        24 => 3200.0,
        14 | 15 => 3450.0,
        2 => 4150.0,
        13 => 4230.0,
        18 => 4874.0,
        23 => 5003.0,
        1 | 4 | 9 | 20 => 5503.0,
        12 => 6430.0,
        21 | 10 => 6504.0,
        19 => 6774.0,
        22 | 11 => 7504.0,
        _ => 5000.0,
    }
}

/// How much of illuminant 1's data to use for a photo at `kelvin` (DNG: linear in inverse
/// temperature between the two, clamped; all of it when there is one).
pub fn blend_weight(illuminants: [u16; 2], kelvin: Option<f64>) -> f32 {
    let (Some(k), true) = (kelvin, illuminants[1] != 0) else {
        return 1.0;
    };
    let (t1, t2) = (
        illuminant_kelvin(illuminants[0]),
        illuminant_kelvin(illuminants[1]),
    );
    if (t1 - t2).abs() < 1.0 {
        return 1.0;
    }
    (((1.0 / k) - (1.0 / t2)) / ((1.0 / t1) - (1.0 / t2))).clamp(0.0, 1.0) as f32
}

/// XYZ (D50) to linear ProPhoto RGB (ROMM).
const XYZ_D50_TO_PROPHOTO: Mat3 = [
    [1.345_943_3, -0.255_607_5, -0.051_111_8],
    [-0.544_598_9, 1.508_167_3, 0.020_535_1],
    [0.0, 0.0, 1.211_812_8],
];
const PROPHOTO_TO_XYZ_D50: Mat3 = [
    [0.797_674_9, 0.135_191_7, 0.031_353_4],
    [0.288_040_2, 0.711_874_1, 0.000_085_7],
    [0.0, 0.0, 0.825_21],
];

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// Rows scaled to sum to 1, so white stays white.
fn white_kept(mut m: Mat3) -> Mat3 {
    for row in &mut m {
        let s: f32 = row.iter().sum();
        if s.abs() > 1e-6 {
            row.iter_mut().for_each(|v| *v /= s);
        }
    }
    m
}

/// Linear ProPhoto (D50) to linear sRGB (D65), Bradford-adapted.
pub fn prophoto_to_linear_srgb() -> Mat3 {
    white_kept(mul(
        &color::XYZ_D65_TO_LINEAR_SRGB,
        &mul(
            &color::bradford(color::D50_WHITE, color::D65_WHITE),
            &PROPHOTO_TO_XYZ_D50,
        ),
    ))
}

fn linear_srgb_to_prophoto() -> Mat3 {
    let m = prophoto_to_linear_srgb();
    let d = m.map(|r| r.map(f64::from));
    let cols: [[f64; 3]; 3] = std::array::from_fn(|j| {
        let e: [f64; 3] = std::array::from_fn(|i| f64::from(u8::from(i == j)));
        color::solve3(d, e).unwrap_or(e)
    });
    std::array::from_fn(|i| std::array::from_fn(|j| cols[j][i] as f32))
}

/// A tone curve sampled for quick lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneLut {
    samples: Vec<f32>,
    /// The slope past 1, so values above white keep their headroom.
    end_slope: f32,
}

impl ToneLut {
    const N: usize = 4096;

    fn new(points: &[[f32; 2]]) -> ToneLut {
        let at = |x: f32| -> f32 {
            match points.iter().position(|p| p[0] >= x) {
                Some(0) => points[0][1],
                Some(i) => {
                    let (a, b) = (points[i - 1], points[i]);
                    let t = (x - a[0]) / (b[0] - a[0]).max(1e-9);
                    a[1] + (b[1] - a[1]) * t
                }
                None => points[points.len() - 1][1],
            }
        };
        let samples: Vec<f32> = (0..=Self::N)
            .map(|i| at(i as f32 / Self::N as f32))
            .collect();
        let n = points.len();
        let (a, b) = (points[n - 2], points[n - 1]);
        ToneLut {
            samples,
            end_slope: ((b[1] - a[1]) / (b[0] - a[0]).max(1e-9)).max(0.0),
        }
    }

    fn at(&self, x: f32) -> f32 {
        if x >= 1.0 {
            return self.samples[Self::N] + (x - 1.0) * self.end_slope;
        }
        if x <= 0.0 {
            return self.samples[0] + x;
        }
        let f = x * Self::N as f32;
        let i = f as usize;
        let t = f - i as f32;
        self.samples[i] + (self.samples[i + 1] - self.samples[i]) * t
    }

    /// The curve on an RGB triple as Adobe applies it: on the largest and smallest channel,
    /// the middle one keeping its place between them (so hue holds).
    pub fn apply_rgb(&self, c: [f32; 3]) -> [f32; 3] {
        let mut idx = [0usize, 1, 2];
        idx.sort_by(|&a, &b| c[b].total_cmp(&c[a]));
        let (hi, mid, lo) = (c[idx[0]], c[idx[1]], c[idx[2]]);
        let (hi2, lo2) = (self.at(hi), self.at(lo));
        let mid2 = if hi > lo {
            lo2 + (hi2 - lo2) * (mid - lo) / (hi - lo)
        } else {
            hi2
        };
        let mut out = [0.0; 3];
        out[idx[0]] = hi2;
        out[idx[1]] = mid2;
        out[idx[2]] = lo2;
        out
    }
}

fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == c[0] {
        60.0 * ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    let s = if max > 0.0 { d / max } else { 0.0 };
    [h, s, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

impl Table {
    /// The (hue shift, saturation scale, value scale) at `hsv`, trilinear, the hue wrapping.
    fn lookup(&self, [h, s, v]: [f32; 3]) -> [f32; 3] {
        let [hd, sd, vd] = self.dims;
        let hf = h.rem_euclid(360.0) / 360.0 * hd as f32;
        let (h0, ht) = (hf.floor() as usize % hd, hf.fract());
        let h1 = (h0 + 1) % hd;
        let sf = s.clamp(0.0, 1.0) * (sd - 1) as f32;
        let (s0, st) = ((sf.floor() as usize).min(sd - 2), 0.0f32);
        let st = if sd > 1 { sf - s0 as f32 } else { st };
        let vv = if self.srgb_value {
            srgb_encode(v.clamp(0.0, 1.0))
        } else {
            v.clamp(0.0, 1.0)
        };
        let (v0, v1, vt) = if vd > 1 {
            let vf = vv * (vd - 1) as f32;
            let v0 = (vf.floor() as usize).min(vd - 2);
            (v0, v0 + 1, vf - v0 as f32)
        } else {
            (0, 0, 0.0)
        };
        let at = |vi: usize, hi: usize, si: usize| self.data[(vi * hd + hi) * sd + si];
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
        };
        let plane = |vi| {
            let a = mix(at(vi, h0, s0), at(vi, h0, s0 + 1), st);
            let b = mix(at(vi, h1, s0), at(vi, h1, s0 + 1), st);
            mix(a, b, ht)
        };
        mix(plane(v0), plane(v1), vt)
    }

    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let [h, s, v] = rgb_to_hsv(c);
        let [dh, ks, kv] = self.lookup([h, s, v]);
        hsv_to_rgb([h + dh, (s * ks).clamp(0.0, 1.0), v * kv])
    }
}

/// A profile fixed for one photo ([`Dcp::prepare`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    cam_to_prophoto: Mat3,
    gain: f32,
    hue_sat: Option<Table>,
    look: Option<Table>,
    tone: Option<ToneLut>,
    prophoto_to_srgb: Mat3,
}

impl Prepared {
    /// Whether the profile's own tone curve was applied (the base curve then is not).
    pub fn has_tone_curve(&self) -> bool {
        self.tone.is_some()
    }

    /// White-balanced camera RGB to linear sRGB through the profile.
    pub fn apply(&self, cam: [f32; 3]) -> [f32; 3] {
        let mut p = color::apply(&self.cam_to_prophoto, cam).map(|v| v * self.gain);
        // The tables work on colours; negative (out of gamut) parts are left as they are.
        if let Some(t) = &self.hue_sat
            && p.iter().all(|v| *v >= 0.0)
        {
            p = t.apply(p);
        }
        if let Some(t) = &self.look
            && p.iter().all(|v| *v >= 0.0)
        {
            p = t.apply(p);
        }
        if let Some(t) = &self.tone {
            p = t.apply_rgb(p);
        }
        color::apply(&self.prophoto_to_srgb, p)
    }
}

/// The registry of loaded profiles, by [`lf_settings::dcp_id`] of their name.
/// Each profile's name and itself, by id.
type Profiles = HashMap<u64, (String, Arc<Dcp>)>;

static REGISTRY: RwLock<Option<Profiles>> = RwLock::new(None);

/// Loads `dcp` as `name` (replacing one of the same name).
pub fn register(name: &str, dcp: Dcp) {
    let mut r = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    r.get_or_insert_with(HashMap::new)
        .insert(lf_settings::dcp_id(name), (name.to_owned(), Arc::new(dcp)));
}

/// Forgets every profile (another catalogue opened).
pub fn clear() {
    *REGISTRY.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The profile known by `id`.
pub fn get(id: u64) -> Option<Arc<Dcp>> {
    REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(&id)
        .map(|(_, d)| d.clone())
}

/// Every loaded profile's name and camera, by name.
pub fn names() -> Vec<(String, String)> {
    let r = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<(String, String)> = r
        .as_ref()
        .map(|m| {
            m.values()
                .map(|(n, d)| (n.clone(), d.camera.clone()))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

/// A minimal DCP writer, for tests here and in other crates.
pub mod testing {
    /// One tag: number, TIFF type and its little-endian value bytes.
    pub struct Tag(pub u16, pub u16, pub u32, pub Vec<u8>);

    pub fn srational(v: &[f64]) -> Vec<u8> {
        v.iter()
            .flat_map(|x| {
                let n = (x * 10_000.0).round() as i32;
                [n.to_le_bytes(), 10_000i32.to_le_bytes()].concat()
            })
            .collect()
    }

    pub fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub fn ascii(s: &str) -> Vec<u8> {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    }

    /// An `IIRC` file holding `tags` (sorted by number, values after the IFD).
    pub fn write(mut tags: Vec<Tag>) -> Vec<u8> {
        tags.sort_by_key(|t| t.0);
        let ifd = 8usize;
        let mut data_at = ifd + 2 + tags.len() * 12 + 4;
        let mut head = b"IIRC".to_vec();
        head.extend((ifd as u32).to_le_bytes());
        head.extend((tags.len() as u16).to_le_bytes());
        let mut tail: Vec<u8> = Vec::new();
        for Tag(tag, typ, count, value) in &tags {
            head.extend(tag.to_le_bytes());
            head.extend(typ.to_le_bytes());
            head.extend(count.to_le_bytes());
            if value.len() <= 4 {
                let mut v = value.clone();
                v.resize(4, 0);
                head.extend(v);
            } else {
                head.extend((data_at as u32).to_le_bytes());
                tail.extend(value);
                data_at += value.len();
            }
        }
        head.extend(0u32.to_le_bytes());
        head.extend(tail);
        head
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    /// ProPhoto's own primaries as a forward matrix: camera RGB is then ProPhoto RGB.
    fn prophoto_forward() -> Vec<f64> {
        PROPHOTO_TO_XYZ_D50
            .iter()
            .flatten()
            .map(|v| f64::from(*v))
            .collect()
    }

    fn profile(extra: Vec<Tag>) -> Vec<u8> {
        let mut tags = vec![
            Tag(UNIQUE_CAMERA_MODEL, 2, 11, ascii("Test Cam 1")),
            Tag(PROFILE_NAME, 2, 9, ascii("Standard")),
            Tag(
                CALIBRATION_ILLUMINANT[0],
                3,
                1,
                21u16.to_le_bytes().to_vec(),
            ),
            Tag(
                COLOR_MATRIX[0],
                10,
                9,
                srational(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            ),
            Tag(FORWARD_MATRIX[0], 10, 9, srational(&prophoto_forward())),
        ];
        tags.extend(extra);
        write(tags)
    }

    #[test]
    fn a_profile_is_read() {
        let d = Dcp::parse(&profile(vec![])).unwrap();
        assert_eq!(d.name, "Standard");
        assert_eq!(d.camera, "Test Cam 1");
        assert_eq!(d.illuminants, [21, 0]);
        assert!(d.forward[0].is_some() && d.hue_sat[0].is_none() && d.tone.is_none());
        assert_eq!(
            Dcp::parse(b"II*\0garbage"),
            Err(DcpError::Damaged("it ends early".into()))
        );
        assert_eq!(Dcp::parse(b"PK\x03\x04"), Err(DcpError::NotDcp));
    }

    #[test]
    fn neutral_stays_neutral_and_white_stays_white() {
        let p = Dcp::parse(&profile(vec![]))
            .unwrap()
            .prepare(None, &color::IDENTITY);
        for v in [0.18f32, 1.0, 2.5] {
            let out = p.apply([v, v, v]);
            for c in out {
                assert!((c - v).abs() < 2e-3, "{v}: {out:?}");
            }
        }
    }

    #[test]
    fn the_hue_sat_map_takes_saturation_away() {
        // 2 hues × 2 saturations × 1 value, every entry: no shift, saturation × 0.
        let data: Vec<f32> = (0..4).flat_map(|_| [0.0, 0.0, 1.0]).collect();
        let d = Dcp::parse(&profile(vec![
            Tag(HUE_SAT_DIMS, 4, 3, longs(&[2, 2, 1])),
            Tag(HUE_SAT_DATA[0], 11, 12, floats(&data)),
        ]))
        .unwrap();
        let p = d.prepare(None, &color::IDENTITY);
        let out = p.apply([0.6, 0.2, 0.1]);
        assert!(
            (out[0] - out[1]).abs() < 1e-3 && (out[1] - out[2]).abs() < 1e-3,
            "grey: {out:?}"
        );
    }

    #[test]
    fn the_tone_curve_keeps_hue_and_headroom() {
        // Brightens: 0.25 -> 0.5, ends at white, slope 2/3 at the end.
        let curve = [0.0f32, 0.0, 0.25, 0.5, 1.0, 1.0];
        let d = Dcp::parse(&profile(vec![Tag(TONE_CURVE, 11, 6, floats(&curve))])).unwrap();
        assert!(d.has_tone_curve());
        let t = ToneLut::new(&d.tone.clone().unwrap());
        assert!((t.at(0.25) - 0.5).abs() < 1e-3);
        assert!(t.at(2.0) > 1.5, "headroom kept: {}", t.at(2.0));
        let c = t.apply_rgb([0.25, 0.125, 0.0]);
        assert!((c[0] - 0.5).abs() < 1e-3 && (c[1] - 0.25).abs() < 1e-3 && c[2] == 0.0);
        // A straight curve is no curve.
        let straight = [0.0f32, 0.0, 1.0, 1.0];
        let d = Dcp::parse(&profile(vec![Tag(TONE_CURVE, 11, 4, floats(&straight))])).unwrap();
        assert!(!d.has_tone_curve());
    }

    #[test]
    fn two_illuminants_blend_by_temperature() {
        let ill = [17, 21];
        assert_eq!(blend_weight(ill, None), 1.0);
        assert_eq!(blend_weight(ill, Some(2856.0)), 1.0);
        assert_eq!(blend_weight(ill, Some(6504.0)), 0.0);
        let mid = blend_weight(ill, Some(4000.0));
        assert!(mid > 0.3 && mid < 0.7, "{mid}");
        assert_eq!(blend_weight([21, 0], Some(3000.0)), 1.0);
    }

    #[test]
    fn hsv_round_trips() {
        for c in [
            [0.6f32, 0.2, 0.1],
            [0.1, 0.7, 0.3],
            [0.2, 0.3, 0.9],
            [0.5, 0.5, 0.5],
        ] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-5, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn profiles_are_registered_by_name() {
        register("Test Standard", Dcp::parse(&profile(vec![])).unwrap());
        let id = lf_settings::dcp_id("Test Standard");
        assert_eq!(get(id).unwrap().camera, "Test Cam 1");
        assert!(names().iter().any(|(n, _)| n == "Test Standard"));
    }
}
