use std::collections::BTreeMap;
use std::ops::{Index, IndexMut};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const SETTINGS_VERSION: u32 = 2;
/// Spots per photo (the shader loops over a fixed-size list).
pub const MAX_SPOTS: usize = 48;
/// Largest desktop object (UTF-8 bytes of its compact JSON) that is kept.
pub const MAX_DESKTOP_JSON: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HslBand {
    Red,
    Orange,
    Yellow,
    Green,
    Aqua,
    Blue,
    Purple,
    Magenta,
}

pub const HSL_BANDS: [HslBand; 8] = [
    HslBand::Red,
    HslBand::Orange,
    HslBand::Yellow,
    HslBand::Green,
    HslBand::Aqua,
    HslBand::Blue,
    HslBand::Purple,
    HslBand::Magenta,
];

/// Centre hue (degrees) of each HSL band, in `HSL_BANDS` order.
pub const HSL_HUES: [f64; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 225.0, 270.0, 315.0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HslAdjust {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

/// One `HslAdjust` per band, serialised as an object keyed by band name in band order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hsl(pub [HslAdjust; 8]);

impl Index<HslBand> for Hsl {
    type Output = HslAdjust;
    fn index(&self, band: HslBand) -> &HslAdjust {
        &self.0[band as usize]
    }
}

impl IndexMut<HslBand> for Hsl {
    fn index_mut(&mut self, band: HslBand) -> &mut HslAdjust {
        &mut self.0[band as usize]
    }
}

impl Serialize for Hsl {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(HSL_BANDS.iter().map(|b| (b, &self[*b])))
    }
}

impl<'de> Deserialize<'de> for Hsl {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<HslBand, HslAdjust>::deserialize(d)?;
        let mut out = Hsl::default();
        for band in HSL_BANDS {
            out[band] = *map
                .get(&band)
                .ok_or_else(|| D::Error::custom(format!("missing hsl band {band:?}")))?;
        }
        Ok(out)
    }
}

/// A curve point: input, output, both 0..1.
pub type CurvePoint = [f64; 2];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curves {
    pub rgb: Vec<CurvePoint>,
    pub r: Vec<CurvePoint>,
    pub g: Vec<CurvePoint>,
    pub b: Vec<CurvePoint>,
}

impl Curves {
    /// Four straight diagonals: the curves leave the image unchanged.
    pub fn identity() -> Curves {
        Curves {
            rgb: identity_curve(),
            r: identity_curve(),
            g: identity_curve(),
            b: identity_curve(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradeWheel {
    /// 0..360
    pub hue: f64,
    /// 0..100
    pub sat: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grading {
    pub shadows: GradeWheel,
    pub midtones: GradeWheel,
    pub highlights: GradeWheel,
    pub balance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpotMode {
    Heal,
    Clone,
}

/// One heal or clone spot: target centre (x, y), source centre (sx, sy), radius r, all fractions of the image.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    pub x: f64,
    pub y: f64,
    pub sx: f64,
    pub sy: f64,
    pub r: f64,
    pub mode: SpotMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotate {
    R0 = 0,
    R90 = 90,
    R180 = 180,
    R270 = 270,
}

impl Serialize for Rotate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(*self as u16)
    }
}

impl<'de> Deserialize<'de> for Rotate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match u16::deserialize(d)? {
            0 => Ok(Rotate::R0),
            90 => Ok(Rotate::R90),
            180 => Ok(Rotate::R180),
            270 => Ok(Rotate::R270),
            n => Err(D::Error::custom(format!(
                "rotate must be 0, 90, 180 or 270, not {n}"
            ))),
        }
    }
}

/// One photo's develop settings, as the web editor holds them.
///
/// Read JSON with [`crate::normalise`] or [`EditSettings::from_json_str`]. The derived
/// `Deserialize` is strict (missing fields, `90.0` for `rotate`, out-of-range values are not
/// fixed up) and is for already-normal JSON only; never use it on web or untrusted input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSettings {
    pub v: u32,
    /// Adjustments made in the desktop editor, kept as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<serde_json::Map<String, serde_json::Value>>,
    // Light
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    // Colour
    pub temperature: f64,
    pub tint: f64,
    pub vibrance: f64,
    pub saturation: f64,
    pub hsl: Hsl,
    pub curve: Curves,
    pub grading: Grading,
    // Detail
    pub clarity: f64,
    pub texture: f64,
    pub dehaze: f64,
    pub sharpen: f64,
    pub sharpen_radius: f64,
    pub noise: f64,
    pub color_noise: f64,
    // Effects
    pub vignette: f64,
    pub vignette_midpoint: f64,
    pub grain: f64,
    pub grain_size: f64,
    pub bw: bool,
    // Geometry
    pub rotate: Rotate,
    pub flip_h: bool,
    pub flip_v: bool,
    pub angle: f64,
    pub crop: Crop,
    /// The crop's locked aspect ("original", "free", "1:1", ...).
    pub aspect: String,
    // Retouch
    pub spots: Vec<Spot>,
}

pub fn identity_curve() -> Vec<CurvePoint> {
    vec![[0.0, 0.0], [1.0, 1.0]]
}

impl EditSettings {
    /// Settings from JSON text of any origin: parsed, then [`crate::normalise`]d. Text that is
    /// not JSON gives the defaults.
    pub fn from_json_str(s: &str) -> EditSettings {
        serde_json::from_str(s)
            .map(|v| crate::normalise(&v))
            .unwrap_or_else(|_| EditSettings::default_settings())
    }

    pub fn default_settings() -> EditSettings {
        let wheel = |hue| GradeWheel { hue, sat: 0.0 };
        EditSettings {
            v: SETTINGS_VERSION,
            desktop: None,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temperature: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            hsl: Hsl::default(),
            curve: Curves {
                rgb: identity_curve(),
                r: identity_curve(),
                g: identity_curve(),
                b: identity_curve(),
            },
            grading: Grading {
                shadows: wheel(220.0),
                midtones: wheel(30.0),
                highlights: wheel(45.0),
                balance: 0.0,
            },
            clarity: 0.0,
            texture: 0.0,
            dehaze: 0.0,
            sharpen: 0.0,
            sharpen_radius: 1.0,
            noise: 0.0,
            color_noise: 0.0,
            vignette: 0.0,
            vignette_midpoint: 50.0,
            grain: 0.0,
            grain_size: 25.0,
            bw: false,
            rotate: Rotate::R0,
            flip_h: false,
            flip_v: false,
            angle: 0.0,
            crop: Crop {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            aspect: "original".to_string(),
            spots: Vec::new(),
        }
    }

    /// The JSON the web editor would hold: whole numbers are written as integers (`0`, not `0.0`),
    /// as JavaScript does, so values compare equal to ones parsed from web-written JSON.
    pub fn to_value(&self) -> serde_json::Value {
        // Plain data with string keys: serialisation cannot fail.
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(map) = v.as_object_mut() {
            for (key, value) in map.iter_mut() {
                if key != "desktop" {
                    whole_numbers_as_integers(value);
                }
            }
        }
        v
    }

    /// True when the settings render exactly as the original (after normalising, as the web does).
    pub fn is_default(&self) -> bool {
        crate::normalise(&self.to_value()) == EditSettings::default_settings()
    }
}

fn whole_numbers_as_integers(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(f) = n
                .as_f64()
                .filter(|f| n.is_f64() && f.fract() == 0.0 && f.abs() < 9e15)
            {
                *v = serde_json::Value::from(f as i64);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(whole_numbers_as_integers),
        serde_json::Value::Object(map) => map.values_mut().for_each(whole_numbers_as_integers),
        _ => {}
    }
}

/// How a RAW file's sensor data is turned into a picture (desktop only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// A gentle camera-like tone curve (toe and shoulder).
    #[default]
    Camera,
    /// No tone curve: scene-linear values, gamma encoded.
    Linear,
}

/// What happens to colours brighter than the sensor's range (desktop only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Highlights {
    /// Clamp white-balanced camera RGB at 1.0 per channel before the colour matrix, so
    /// clipped whites stay neutral.
    Clip,
    /// Blend clipped pixels towards their luma, driven by sensor clipping measured before
    /// white balance, so unclipped colours are untouched.
    #[default]
    Reconstruct,
}

/// The id a DNG camera profile is known by in `lf_raw::dcp`'s registry: FNV-1a (64 bit) of
/// its name, case ignored.
pub fn dcp_id(name: &str) -> u64 {
    name.to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// The RAW develop choices, kept in `desktop.develop` as lowercase strings:
/// `{"profile": "camera" | "linear", "highlights": "clip" | "reconstruct", "dcp": "<name>"}`
/// (`dcp`, a camera profile imported into the catalogue, only when one is chosen).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DevelopSettings {
    pub profile: Profile,
    pub highlights: Highlights,
    /// The camera profile, by [`dcp_id`] of its name ([`DevelopSettings::dcp_name`] reads
    /// the name itself).
    pub dcp: Option<u64>,
}

impl DevelopSettings {
    /// Read `desktop.develop`; anything missing or unknown gives the default.
    pub fn from_settings(s: &EditSettings) -> DevelopSettings {
        let develop = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("develop"))
            .and_then(|d| d.as_object());
        let field = |key: &str| develop.and_then(|d| d.get(key)).and_then(|v| v.as_str());
        DevelopSettings {
            profile: match field("profile") {
                Some("linear") => Profile::Linear,
                _ => Profile::Camera,
            },
            highlights: match field("highlights") {
                Some("clip") => Highlights::Clip,
                _ => Highlights::Reconstruct,
            },
            dcp: field("dcp").filter(|n| !n.trim().is_empty()).map(dcp_id),
        }
    }

    /// The camera profile's name in `desktop.develop`, when one is chosen.
    pub fn dcp_name(s: &EditSettings) -> Option<String> {
        s.desktop
            .as_ref()
            .and_then(|d| d.get("develop"))
            .and_then(|d| d.get("dcp"))
            .and_then(|v| v.as_str())
            .filter(|n| !n.trim().is_empty())
            .map(str::to_owned)
    }

    /// Chooses the camera profile `name` (`None`: the built-in colour), keeping the rest.
    pub fn set_dcp(s: &mut EditSettings, name: Option<&str>) {
        let mut d = DevelopSettings::from_settings(s);
        d.dcp = name.map(dcp_id);
        d.write_with(s, name.map(str::to_owned));
    }

    /// Store into `desktop.develop`. Defaults are stored as no `develop` key, and an
    /// emptied `desktop` is dropped, so a default export carries no `desktop` key.
    pub fn write_into(&self, s: &mut EditSettings) {
        // The profile's name is kept from `s` (the id alone cannot give it back).
        let name = DevelopSettings::dcp_name(s).filter(|n| Some(dcp_id(n)) == self.dcp);
        self.write_with(s, name);
    }

    fn write_with(&self, s: &mut EditSettings, dcp: Option<String>) {
        let mut this = *self;
        if dcp.is_none() {
            this.dcp = None;
        }
        if this == DevelopSettings::default() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("develop");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        let profile = match self.profile {
            Profile::Camera => "camera",
            Profile::Linear => "linear",
        };
        let highlights = match self.highlights {
            Highlights::Clip => "clip",
            Highlights::Reconstruct => "reconstruct",
        };
        let mut develop = serde_json::json!({ "profile": profile, "highlights": highlights });
        if let Some(name) = dcp {
            develop["dcp"] = serde_json::Value::String(name);
        }
        s.desktop
            .get_or_insert_with(Default::default)
            .insert("develop".to_string(), develop);
    }
}
