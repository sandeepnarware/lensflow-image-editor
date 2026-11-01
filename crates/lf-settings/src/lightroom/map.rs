//! Lightroom's settings translated into ours: a read [`LrPreset`] becomes a partial settings
//! object (a preset) and a list of notes for the user.
//!
//! A preset carries only the groups Lightroom set: a key Lightroom left out means "not part of
//! this preset", so the matching group is left out too and applying the preset leaves it on the
//! photo as it is (`desktop.lightroom` makes `apply_look` apply it that way). Within a group,
//! too, only the keys Lightroom wrote are written: Lightroom ticks each slider into a preset on
//! its own (exposure without contrast, sharpening without noise reduction), and
//! `apply_groups` lays on only the keys present. Settings we hold as one value (the HSL
//! table, a curve, the grading wheels, the desktop edits) are written whole, with Lightroom's
//! defaults where it left a part out.
//!
//! Lightroom's Lens Corrections and Transform panels share our `desktop.lens` (one value, one
//! group), so a preset with only Transform settings also writes neutral distortion, vignetting
//! and fringe values, turning those off on the photo it is applied to.
//!
//! Values are copied where both editors use the same range and idea; the few that differ are
//! scaled here, and calibration against Lightroom's renders can refine the responses later
//! without changing what is stored. Every original key is kept under `desktop.lightroom` so a
//! better mapping can translate the preset again without the file.
//!
//! Notes are one sentence per family of settings that was left out or approximated. A
//! setting left at Lightroom's own default is not noted: Lightroom writes those for every
//! ticked group, and a note about a setting that changes nothing would only be noise.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::{LIGHTROOM_KEY, LrPreset, budget, curve};
use crate::adaptive::{self, AdaptiveMask};
use crate::calibration::Calibration;
use crate::defringe::{self, Defringe};
use crate::lens::Lens;
use crate::masks::{Local, MAX_MASKS};
use crate::point_colour::{MAX_POINTS, Point, Points};
use crate::shader_values::kelvin_wb;
use crate::{CurvePoint, EditSettings, SETTINGS_VERSION};

/// A Lightroom preset in our terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    /// Partial settings JSON (`"v": 2`) with `desktop.lightroom`.
    pub preset: Value,
    /// Ready-to-show sentences about what was left out or approximated.
    pub notes: Vec<String>,
}

pub const KELVIN_NOTE: &str = "The white balance in Kelvin was approximated as a shift from \
     daylight; it may need adjusting on each photo.";
pub const SHARPEN_EXTRA_NOTE: &str = "Sharpening detail and masking, and noise reduction's \
     detail, contrast and smoothness, are not carried.";
pub const VIGNETTE_EXTRA_NOTE: &str =
    "The vignette's feather, roundness, style and highlights are not carried.";
pub const GRAIN_FREQUENCY_NOTE: &str = "Grain roughness is not carried.";
pub const GRAY_MIXER_NOTE: &str = "The black and white mix is not carried; the photo turns \
     black and white with the standard mix.";
pub const GRADING_EXTRA_NOTE: &str =
    "Colour grading's luminance, global wheel and blending are not carried.";
pub const LENS_PROFILE_NOTE: &str =
    "Lens profile correction: turn it on with the Lens section's Profile button.";
pub const AUTO_CA_NOTE: &str = "Remove chromatic aberration is not carried: set it with the \
     Lens section's colour fringe sliders.";
pub const LENS_VIGNETTE_MIDPOINT_NOTE: &str = "The lens vignetting midpoint is not carried.";
pub const PERSPECTIVE_EXTRA_NOTE: &str =
    "Perspective rotate, scale, aspect and offsets are not carried.";
pub const UPRIGHT_NOTE: &str =
    "Upright is not carried: use the Lens section's Upright buttons on each photo.";
pub const CROP_NOTE: &str =
    "The crop and straighten angle are not carried: they belong to one photo.";
pub const RETOUCH_NOTE: &str = "Spot removal is not carried: it belongs to one photo.";
pub const MASKS_NOTE: &str = "Masks are not carried: they belong to one photo.";
pub const MASK_KIND_NOTE: &str = "Only subject, sky and people masks are carried; masks drawn      as brushes, gradients, ranges or combined with others are not, because they belong to one photo.";
pub const MASK_LOCAL_NOTE: &str = "A mask's texture and dehaze are not carried.";
/// What replaces the shapes Lightroom stored for masks and spot removal.
const OMITTED: &str = "(omitted)";
pub const CURVE_UNREADABLE_NOTE: &str = "A tone curve could not be read and was left straight.";
pub const POINT_COLOUR_RANGE_NOTE: &str = "Point colour's separate hue, saturation and \
     luminance ranges were approximated by one range.";
pub const POINT_COLOUR_UNREADABLE_NOTE: &str = "A point colour could not be read and was left out.";
pub const POINT_COLOUR_EXTRA_NOTE: &str = "Only the first four point colours are carried.";
/// Followed by the profile's name in quotes and a full stop.
pub const PROFILE_NOTE: &str = "This preset uses the Lightroom profile";
/// Followed by the keys' names, comma-separated, and a full stop.
pub const UNMAPPED_NOTE: &str = "These Lightroom settings are not carried:";

/// Keys that describe the preset rather than change the photo; never noted.
const BOOKKEEPING: &[&str] = &[
    "Version",
    "HasSettings",
    "AlreadyApplied",
    "UUID",
    "Copyright",
    "ContactInfo",
    "Description",
    "Cluster",
    "ShortName",
    "SortName",
    "CameraModelRestriction",
    "CameraProfileDigest",
    "SupportsAmount",
    "SupportsAmount2",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "ToneCurveName",
    "ToneCurveName2012",
    "OverrideLookVignette",
];

const HSL_KEYS: [(&str, &str); 8] = [
    ("Red", "red"),
    ("Orange", "orange"),
    ("Yellow", "yellow"),
    ("Green", "green"),
    ("Aqua", "aqua"),
    ("Blue", "blue"),
    ("Purple", "purple"),
    ("Magenta", "magenta"),
];

/// The as-shot white balance a Kelvin target is measured from (daylight).
const DAYLIGHT_KELVIN: f64 = 5500.0;
/// Lightroom's tint runs -150..150; ours -100..100.
const LR_TINT_RANGE: f64 = 150.0;

/// Translate a read Lightroom preset.
pub fn map(p: &LrPreset) -> Mapped {
    let mut m = Mapper {
        s: &p.settings,
        defaults: EditSettings::default_settings().to_value(),
        out: Map::new(),
        desktop: Map::new(),
        notes: Vec::new(),
        handled: BOOKKEEPING.iter().map(|k| (*k).to_string()).collect(),
        adaptive: Vec::new(),
    };
    m.out.insert("v".into(), json!(SETTINGS_VERSION));
    m.light();
    let auto_wb = m.colour();
    let auto_tone = m.auto_tone();
    m.detail();
    m.hsl();
    m.curve();
    m.grading();
    m.sharpen();
    m.effects();
    m.bw();
    m.lens();
    m.calibration();
    m.point_colour();
    m.left_out();
    m.unmapped();

    m.desktop.insert(
        LIGHTROOM_KEY.into(),
        json!({
            "name": p.name,
            "group": p.group,
            "processVersion": p.process_version,
            "settings": stored_settings(&p.settings),
            "auto": {"wb": auto_wb, "tone": auto_tone},
        }),
    );
    let Mapper {
        mut out,
        desktop,
        notes,
        adaptive: adaptive_masks,
        ..
    } = m;
    out.insert("desktop".into(), Value::Object(desktop));
    let mut preset = Value::Object(out);
    adaptive::write_into_preset(&mut preset, &adaptive_masks);
    let mut notes = notes;
    if let Some(note) = budget::fit_budget(&mut preset) {
        notes.push(note.to_string());
    }
    Mapped { preset, notes }
}

/// Lightroom's settings as kept under `desktop.lightroom`: the mask and spot-removal geometry
/// is large and belongs to one photo, so it is replaced by a marker.
fn stored_settings(settings: &Map<String, Value>) -> Map<String, Value> {
    let mut kept = settings.clone();
    for key in ["MaskGroupBasedCorrections", "RetouchInfo"] {
        if let Some(v) = kept.get_mut(key) {
            *v = json!(OMITTED);
        }
    }
    kept
}

/// The AI mask a correction stands for, when every mask in it is one of a supported kind.
/// Lightroom's `MaskSubType`: 1 subject, 2 sky, 3 people (the numbering of people is unverified
/// against real files); a subject mask that is inverted is the background.
fn ai_correction(c: &Value) -> Option<AdaptiveMask> {
    let masks = c.get("CorrectionMasks")?.as_array()?;
    let [mask] = masks.as_slice() else {
        return None;
    };
    if mask.get("What").and_then(Value::as_str) != Some("Mask/Image") {
        return None;
    }
    let what = match mask.get("MaskSubType").and_then(Value::as_i64)? {
        1 => "subject",
        2 => "sky",
        3 => "people",
        _ => return None,
    };
    let invert = mask.get("MaskInverted").and_then(Value::as_bool) == Some(true);
    let f = |key: &str| c.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
    // Lightroom's local sliders run -1..1, ours -100..100; exposure is in stops in both.
    let pct = |key: &str| f(key).map_or(0.0, |v| v * 100.0);
    let local = Local {
        exposure: f("LocalExposure2012").unwrap_or(0.0),
        contrast: pct("LocalContrast2012"),
        highlights: pct("LocalHighlights2012"),
        shadows: pct("LocalShadows2012"),
        whites: pct("LocalWhites2012"),
        blacks: pct("LocalBlacks2012"),
        temperature: pct("LocalTemperature"),
        tint: pct("LocalTint"),
        saturation: pct("LocalSaturation"),
        clarity: pct("LocalClarity2012"),
    };
    Some(AdaptiveMask {
        what: what.to_owned(),
        invert,
        opacity: 100.0,
        local,
        range: Default::default(),
    })
}

struct Mapper<'a> {
    s: &'a Map<String, Value>,
    defaults: Value,
    out: Map<String, Value>,
    desktop: Map<String, Value>,
    notes: Vec<String>,
    /// Keys mapped, noted in a family, or bookkeeping; the rest are listed as not carried.
    handled: BTreeSet<String>,
    /// AI masks carried over from `MaskGroupBasedCorrections`.
    adaptive: Vec<AdaptiveMask>,
}

impl Mapper<'_> {
    /// The raw value of `key`, marking it handled.
    fn get(&mut self, key: &str) -> Option<&Value> {
        self.handled.insert(key.to_string());
        self.s.get(key)
    }

    fn num(&mut self, key: &str) -> Option<f64> {
        self.get(key)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    }

    /// A Lightroom switch: a bool, or a number (0 off).
    fn flag(&mut self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => n.as_f64().map(|v| v != 0.0),
            Value::String(s) => Some(s.eq_ignore_ascii_case("true")),
            _ => None,
        }
    }

    /// Whether `key` is set to something other than Lightroom's `default` (marks it handled).
    fn differs(&mut self, key: &str, default: f64) -> bool {
        match self.get(key) {
            None => false,
            Some(v) => v.as_f64().is_none_or(|n| (n - default).abs() > 1e-9),
        }
    }

    /// Whether any of `pairs` (key, Lightroom default) differs; every key is marked handled.
    fn any_differs(&mut self, pairs: &[(&str, f64)]) -> bool {
        let mut any = false;
        for (key, default) in pairs {
            any |= self.differs(key, *default);
        }
        any
    }

    /// Every key starting with `prefix`, marked handled.
    fn with_prefix(&mut self, prefix: &str) -> Vec<String> {
        let keys: Vec<String> = self
            .s
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        self.handled.extend(keys.iter().cloned());
        keys
    }

    fn note(&mut self, note: &str) {
        if !self.notes.iter().any(|n| n == note) {
            self.notes.push(note.to_string());
        }
    }

    /// Writes `values` (our keys, already in our scale).
    fn put(&mut self, values: &[(&str, f64)]) {
        for (key, v) in values {
            self.out.insert((*key).to_string(), number(*v));
        }
    }

    /// Maps `pairs` of (Lightroom key, our key) value for value: only the keys Lightroom set,
    /// as Lightroom ticks each slider into a preset on its own.
    fn copy(&mut self, pairs: &[(&str, &str)]) {
        for (lr, ours) in pairs {
            if let Some(v) = self.num(lr) {
                self.out.insert((*ours).to_string(), number(v));
            }
        }
    }

    fn light(&mut self) {
        self.copy(&[
            ("Exposure2012", "exposure"),
            ("Contrast2012", "contrast"),
            ("Highlights2012", "highlights"),
            ("Shadows2012", "shadows"),
            ("Whites2012", "whites"),
            ("Blacks2012", "blacks"),
        ]);
    }

    /// White balance, vibrance and saturation. Returns whether Lightroom asks for Auto WB.
    fn colour(&mut self) -> bool {
        let wb = self
            .get("WhiteBalance")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let kelvin = self.num("Temperature");
        let lr_tint = self.num("Tint");
        let inc_temp = self.num("IncrementalTemperature");
        let inc_tint = self.num("IncrementalTint");
        let auto = wb.as_deref() == Some("Auto");
        let as_shot = wb.as_deref() == Some("As Shot");
        if !auto && !as_shot {
            let clamp = |v: f64| v.clamp(-100.0, 100.0);
            // A preset has one or the other; should both appear, the relative `Incremental*`
            // win, as they need no guess about the photo's own balance.
            if inc_temp.is_some() || inc_tint.is_some() {
                // Non-RAW presets: already relative, the same scale.
                if let Some(t) = inc_temp {
                    self.put(&[("temperature", clamp(t))]);
                }
                if let Some(n) = inc_tint {
                    self.put(&[("tint", clamp(n))]);
                }
            } else if kelvin.is_some() || lr_tint.is_some() {
                // Ours is relative to the photo's own balance: the Kelvin target is balanced
                // as our white balance presets are, on a photo taken to be shot in daylight.
                // `desktop.lightroom` keeps the target for a later per-photo conversion.
                let (t, wb_tint) = kelvin
                    .and_then(|k| kelvin_wb(DAYLIGHT_KELVIN, k))
                    .unwrap_or((0.0, 0.0));
                let n = clamp(wb_tint + lr_tint.unwrap_or(0.0) * 100.0 / LR_TINT_RANGE);
                if kelvin.is_some() {
                    self.put(&[("temperature", clamp(t))]);
                }
                self.put(&[("tint", n)]);
                self.note(KELVIN_NOTE);
            }
        }
        self.copy(&[("Vibrance", "vibrance"), ("Saturation", "saturation")]);
        auto
    }

    /// Whether Lightroom asks for Auto tone (any of its Auto switches).
    fn auto_tone(&mut self) -> bool {
        [
            "AutoTone",
            "AutoExposure",
            "AutoContrast",
            "AutoShadows",
            "AutoBrightness",
        ]
        .iter()
        .map(|k| self.flag(k) == Some(true))
        .collect::<Vec<_>>()
        .contains(&true)
    }

    fn detail(&mut self) {
        self.copy(&[
            ("Clarity2012", "clarity"),
            ("Texture", "texture"),
            ("Dehaze", "dehaze"),
        ]);
    }

    fn hsl(&mut self) {
        let mut hsl = self.defaults["hsl"].clone();
        let mut any = false;
        for (lr, ours) in HSL_KEYS {
            for (prefix, field) in [
                ("HueAdjustment", "h"),
                ("SaturationAdjustment", "s"),
                ("LuminanceAdjustment", "l"),
            ] {
                if let Some(v) = self.num(&format!("{prefix}{lr}")) {
                    hsl[ours][field] = number(v);
                    any = true;
                }
            }
        }
        if any {
            self.out.insert("hsl".into(), hsl);
        }
    }

    /// A point curve (`"x, y"` strings, 0..255) in 0..1; `Err` when it can't be read.
    fn point_curve(&mut self, key: &str) -> Option<Result<Vec<CurvePoint>, ()>> {
        let items = self.get(key)?;
        let points: Option<Vec<CurvePoint>> = items.as_array().and_then(|items| {
            items
                .iter()
                .map(|item| {
                    let s = item.as_str()?;
                    let (x, y) = s.split_once(',')?;
                    let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?);
                    (x.is_finite() && y.is_finite()).then_some([x / 255.0, y / 255.0])
                })
                .collect()
        });
        Some(match points {
            Some(mut p) if p.len() >= 2 => {
                p.sort_by(|a, b| a[0].total_cmp(&b[0]));
                Ok(p)
            }
            _ => Err(()),
        })
    }

    fn curve(&mut self) {
        let params: Vec<Option<f64>> = [
            "ParametricShadows",
            "ParametricDarks",
            "ParametricLights",
            "ParametricHighlights",
            "ParametricShadowSplit",
            "ParametricMidtoneSplit",
            "ParametricHighlightSplit",
        ]
        .iter()
        .map(|k| self.num(k))
        .collect();
        let mut channels = Vec::new();
        for (key, ours) in [
            ("ToneCurvePV2012", "rgb"),
            ("ToneCurvePV2012Red", "r"),
            ("ToneCurvePV2012Green", "g"),
            ("ToneCurvePV2012Blue", "b"),
        ] {
            let read = self.point_curve(key);
            if matches!(read, Some(Err(()))) {
                self.note(CURVE_UNREADABLE_NOTE);
            }
            channels.push((ours, read));
        }
        if params.iter().all(Option::is_none) && channels.iter().all(|(_, c)| c.is_none()) {
            return;
        }
        let mut curve = self.defaults["curve"].clone();
        for (ours, read) in channels {
            let points = match read {
                Some(Ok(p)) => p,
                _ => crate::identity_curve(),
            };
            let shape: &[Option<f64>] = if ours == "rgb" { &params } else { &[] };
            curve[ours] = json!(curve::shaped(points, shape));
        }
        self.out.insert("curve".into(), curve);
    }

    fn grading(&mut self) {
        // Lightroom 10+ keeps the shadow and highlight wheels in the split toning keys and
        // adds the rest as `ColorGrade*`; older presets have only split toning. A `ColorGrade`
        // key wins when both are present.
        let mut pick = |grade: &str, split: Option<&str>| {
            self.num(grade).or_else(|| split.and_then(|k| self.num(k)))
        };
        let fields = [
            (
                "shadows",
                pick("ColorGradeShadowHue", Some("SplitToningShadowHue")),
                pick("ColorGradeShadowSat", Some("SplitToningShadowSaturation")),
            ),
            (
                "midtones",
                pick("ColorGradeMidtoneHue", None),
                pick("ColorGradeMidtoneSat", None),
            ),
            (
                "highlights",
                pick("ColorGradeHighlightHue", Some("SplitToningHighlightHue")),
                pick(
                    "ColorGradeHighlightSat",
                    Some("SplitToningHighlightSaturation"),
                ),
            ),
        ];
        let balance = pick("ColorGradeBalance", Some("SplitToningBalance"));
        let extras = [
            ("ColorGradeShadowLum", 0.0),
            ("ColorGradeMidtoneLum", 0.0),
            ("ColorGradeHighlightLum", 0.0),
            ("ColorGradeGlobalSat", 0.0),
            ("ColorGradeGlobalLum", 0.0),
            ("ColorGradeBlending", 50.0),
        ];
        if self.any_differs(&extras) {
            self.note(GRADING_EXTRA_NOTE);
        }
        // The global hue does nothing without global saturation.
        self.get("ColorGradeGlobalHue");
        if balance.is_none() && fields.iter().all(|(_, h, s)| h.is_none() && s.is_none()) {
            return;
        }
        let mut grading = self.defaults["grading"].clone();
        for (wheel, hue, sat) in fields {
            if let Some(h) = hue {
                grading[wheel]["hue"] = number(h);
            }
            if let Some(s) = sat {
                grading[wheel]["sat"] = number(s);
            }
        }
        // Lightroom's balance above 0 favours the highlights; ours favours the shadows
        // (the develop pass moves its pivot up by balance).
        if let Some(b) = balance {
            grading["balance"] = number(-b);
        }
        self.out.insert("grading".into(), grading);
    }

    fn sharpen(&mut self) {
        self.copy(&[
            ("Sharpness", "sharpen"),
            ("SharpenRadius", "sharpenRadius"),
            ("LuminanceSmoothing", "noise"),
            ("ColorNoiseReduction", "colorNoise"),
        ]);
        let extras = [
            ("SharpenDetail", 25.0),
            ("SharpenEdgeMasking", 0.0),
            ("LuminanceNoiseReductionDetail", 50.0),
            ("LuminanceNoiseReductionContrast", 0.0),
            ("ColorNoiseReductionDetail", 50.0),
            ("ColorNoiseReductionSmoothness", 50.0),
        ];
        if self.any_differs(&extras) {
            self.note(SHARPEN_EXTRA_NOTE);
        }
    }

    fn effects(&mut self) {
        self.copy(&[
            ("PostCropVignetteAmount", "vignette"),
            ("PostCropVignetteMidpoint", "vignetteMidpoint"),
            ("GrainAmount", "grain"),
            ("GrainSize", "grainSize"),
        ]);
        let extras = [
            ("PostCropVignetteFeather", 50.0),
            ("PostCropVignetteRoundness", 0.0),
            ("PostCropVignetteStyle", 1.0),
            ("PostCropVignetteHighlightContrast", 0.0),
        ];
        if self.any_differs(&extras) {
            self.note(VIGNETTE_EXTRA_NOTE);
        }
        if self.differs("GrainFrequency", 50.0) {
            self.note(GRAIN_FREQUENCY_NOTE);
        }
    }

    fn bw(&mut self) {
        let grey = self.flag("ConvertToGrayscale");
        let mixer = self.with_prefix("GrayMixer");
        if let Some(bw) = grey {
            self.out.insert("bw".into(), json!(bw));
            if bw && !mixer.is_empty() {
                self.note(GRAY_MIXER_NOTE);
            }
        }
    }

    /// `desktop.lens` and `desktop.defringe` (one group), built through their own types.
    fn lens(&mut self) {
        let lens_keys = [
            "LensManualDistortionAmount",
            "VignetteAmount",
            "PerspectiveVertical",
            "PerspectiveHorizontal",
            "ChromaticAberrationR",
            "ChromaticAberrationB",
        ];
        let [
            distortion,
            vignetting,
            vertical,
            horizontal,
            ca_red,
            ca_blue,
        ] = lens_keys.map(|k| self.num(k));
        // Lightroom's fringe hues are 0..100 sliders, each a low and a high end; ours is one
        // hue per colour, in degrees within its own span. The middle of Lightroom's pair,
        // placed along our span.
        let mut hue = |lo: &str, hi: &str, (from, to): (f64, f64)| {
            let (lo, hi) = (self.num(lo), self.num(hi));
            (lo.is_some() || hi.is_some()).then(|| {
                let mid = (lo.unwrap_or(0.0) + hi.or(lo).unwrap_or(100.0)) / 2.0;
                from + (to - from) * (mid / 100.0).clamp(0.0, 1.0)
            })
        };
        let purple_hue = hue(
            "DefringePurpleHueLo",
            "DefringePurpleHueHi",
            defringe::PURPLE_HUES,
        );
        let green_hue = hue(
            "DefringeGreenHueLo",
            "DefringeGreenHueHi",
            defringe::GREEN_HUES,
        );
        let purple = self.num("DefringePurpleAmount");
        let green = self.num("DefringeGreenAmount");

        if self.flag("LensProfileEnable") == Some(true) {
            self.note(LENS_PROFILE_NOTE);
        }
        self.with_prefix("LensProfile");
        if self.flag("AutoLateralCA") == Some(true) {
            self.note(AUTO_CA_NOTE);
        }
        if self.differs("VignetteMidpoint", 50.0) {
            self.note(LENS_VIGNETTE_MIDPOINT_NOTE);
        }
        let perspective = [
            ("PerspectiveRotate", 0.0),
            ("PerspectiveScale", 100.0),
            ("PerspectiveAspect", 0.0),
            ("PerspectiveX", 0.0),
            ("PerspectiveY", 0.0),
        ];
        if self.any_differs(&perspective) {
            self.note(PERSPECTIVE_EXTRA_NOTE);
        }
        self.with_prefix("Upright");
        if self.differs("PerspectiveUpright", 0.0) {
            self.note(UPRIGHT_NOTE);
        }

        let carried = [
            distortion, vignetting, vertical, horizontal, ca_red, ca_blue, purple, green,
        ]
        .iter()
        .any(Option::is_some)
            || purple_hue.is_some()
            || green_hue.is_some();
        if !carried {
            return;
        }
        let lens = Lens {
            distortion: distortion.unwrap_or(0.0),
            vignetting: vignetting.unwrap_or(0.0),
            // Lightroom straightens a building shot from below with a negative Vertical; ours
            // with a positive one.
            vertical: -vertical.unwrap_or(0.0),
            // Same sign as Lightroom's; unverified until the LF_LR_SAMPLES check compares a
            // real Lightroom export.
            horizontal: horizontal.unwrap_or(0.0),
            // The legacy manual chromatic aberration sliders, -100..100 like ours.
            ca_red: ca_red.unwrap_or(0.0),
            ca_blue: ca_blue.unwrap_or(0.0),
        };
        let fringe = Defringe {
            purple_amount: purple.unwrap_or(0.0),
            purple_hue: purple_hue.unwrap_or(defringe::PURPLE_HUE),
            green_amount: green.unwrap_or(0.0),
            green_hue: green_hue.unwrap_or(defringe::GREEN_HUE),
        };
        let mut s = EditSettings::default_settings();
        lens.write_into(&mut s);
        fringe.write_into(&mut s);
        self.desktop_group(&s, &["lens", "defringe"]);
    }

    /// Copies `keys` of `s.desktop` into the preset; an edit `s` lacks is `null`, so a carried
    /// group that is off turns the photo's off (as a picked group does).
    fn desktop_group(&mut self, s: &EditSettings, keys: &[&str]) {
        for key in keys {
            let v = s
                .desktop
                .as_ref()
                .and_then(|d| d.get(*key))
                .cloned()
                .unwrap_or(Value::Null);
            self.desktop.insert((*key).to_string(), v);
        }
    }

    fn calibration(&mut self) {
        let [
            red_hue,
            red_saturation,
            green_hue,
            green_saturation,
            blue_hue,
            blue_saturation,
            shadows_tint,
        ] = [
            "RedHue",
            "RedSaturation",
            "GreenHue",
            "GreenSaturation",
            "BlueHue",
            "BlueSaturation",
            "ShadowTint",
        ]
        .map(|k| self.num(k));
        let all = [
            red_hue,
            red_saturation,
            green_hue,
            green_saturation,
            blue_hue,
            blue_saturation,
            shadows_tint,
        ];
        if all.iter().all(Option::is_none) {
            return;
        }
        let v = |o: Option<f64>| o.unwrap_or(0.0);
        let c = Calibration {
            shadows_tint: v(shadows_tint),
            red_hue: v(red_hue),
            red_saturation: v(red_saturation),
            green_hue: v(green_hue),
            green_saturation: v(green_saturation),
            blue_hue: v(blue_hue),
            blue_saturation: v(blue_saturation),
        };
        let mut s = EditSettings::default_settings();
        c.write_into(&mut s);
        self.desktop_group(&s, &["calibration"]);
    }

    /// `PointColors`: each entry a string of numbers, in recent Lightroom's order: source hue
    /// (radians), saturation and luminance (0..1), hue shift, saturation and luminance scale
    /// (-1..1), range amount (0..1), then separate hue, saturation and luminance range curves,
    /// which our one range stands in for.
    fn point_colour(&mut self) {
        let Some(entries) = self.get("PointColors").cloned() else {
            return;
        };
        let entries = match entries {
            Value::Array(items) => items,
            other => vec![other],
        };
        let mut points = Vec::new();
        for entry in &entries {
            let numbers: Option<Vec<f64>> = entry.as_str().and_then(|s| {
                s.split(',')
                    .map(|n| n.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
                    .collect()
            });
            match numbers {
                Some(n) if n.len() >= 7 && n[0] >= 0.0 => {
                    if !default_ranges(&n[7..]) {
                        self.note(POINT_COLOUR_RANGE_NOTE);
                    }
                    points.push(Point {
                        hue: n[0].to_degrees(),
                        saturation: n[1],
                        brightness: n[2],
                        hue_shift: n[3] * 100.0,
                        saturation_shift: n[4] * 100.0,
                        luminance_shift: n[5] * 100.0,
                        range: n[6] * 100.0,
                    });
                }
                _ => self.note(POINT_COLOUR_UNREADABLE_NOTE),
            }
        }
        if points.len() > MAX_POINTS {
            points.truncate(MAX_POINTS);
            self.note(POINT_COLOUR_EXTRA_NOTE);
        }
        let mut s = EditSettings::default_settings();
        Points(points).write_into(&mut s);
        self.desktop_group(&s, &["pointColour"]);
    }

    /// What belongs to one photo, and the profiles, noted.
    fn left_out(&mut self) {
        let crop = [
            ("CropTop", 0.0),
            ("CropLeft", 0.0),
            ("CropBottom", 1.0),
            ("CropRight", 1.0),
            ("CropAngle", 0.0),
        ];
        let cropped = self.any_differs(&crop);
        let has_crop = self.flag("HasCrop") == Some(true);
        self.with_prefix("Crop");
        if cropped || has_crop {
            self.note(CROP_NOTE);
        }
        let non_empty = |v: Option<&Value>| match v {
            None | Some(Value::Null) => false,
            Some(Value::Array(a)) => !a.is_empty(),
            Some(Value::String(s)) => !s.is_empty(),
            Some(_) => true,
        };
        let retouch = ["RetouchInfo", "RetouchAreas"]
            .iter()
            .map(|k| non_empty(self.get(k)))
            .collect::<Vec<_>>()
            .contains(&true);
        if retouch {
            self.note(RETOUCH_NOTE);
        }
        self.ai_masks();
        let masks = [
            "PaintBasedCorrections",
            "GradientBasedCorrections",
            "CircularGradientBasedCorrections",
        ]
        .iter()
        .map(|k| non_empty(self.get(k)))
        .collect::<Vec<_>>()
        .contains(&true);
        if masks {
            self.note(MASKS_NOTE);
        }
        let profile = self
            .get("CameraProfile")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let look = self.get("Look").and_then(|l| match l {
            Value::String(s) => Some(s.clone()),
            other => other.get("Name").and_then(Value::as_str).map(str::to_owned),
        });
        for name in [profile, look].into_iter().flatten() {
            if !name.trim().is_empty() {
                let note = format!("{PROFILE_NOTE} \"{}\".", name.trim());
                self.note(&note);
            }
        }
    }

    /// Carries the corrections that are one AI mask; notes the others.
    fn ai_masks(&mut self) {
        let corrections = match self.get("MaskGroupBasedCorrections") {
            Some(Value::Array(a)) => a.clone(),
            _ => return,
        };
        for c in corrections {
            if c.get("CorrectionActive").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            match ai_correction(&c) {
                Some(mask) if self.adaptive.len() < MAX_MASKS => {
                    if ["LocalTexture", "LocalDehaze"]
                        .iter()
                        .any(|k| c.get(*k).and_then(Value::as_f64).is_some_and(|v| v != 0.0))
                    {
                        self.note(MASK_LOCAL_NOTE);
                    }
                    self.adaptive.push(mask);
                }
                _ => self.note(MASK_KIND_NOTE),
            }
        }
    }

    /// One note naming every key nothing above read.
    fn unmapped(&mut self) {
        let keys: Vec<&str> = self
            .s
            .keys()
            .filter(|k| !self.handled.contains(*k))
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !keys.is_empty() {
            let note = format!("{UNMAPPED_NOTE} {}.", keys.join(", "));
            self.notes.push(note);
        }
    }
}

/// The hue, saturation and luminance range curves Lightroom writes for a point whose ranges
/// were left alone, four numbers each, as recalled from Lightroom's own files; unverified
/// until the LF_LR_SAMPLES check reads real ones.
const LR_DEFAULT_RANGE: [f64; 4] = [0.0, 0.166667, 0.583333, 1.0];

/// Whether a point colour's range curves (after its first seven numbers) are Lightroom's
/// defaults (or absent), which our one range already stands for.
fn default_ranges(ranges: &[f64]) -> bool {
    ranges
        .iter()
        .enumerate()
        .all(|(i, v)| (v - LR_DEFAULT_RANGE[i % 4]).abs() < 1e-3)
}

/// A number as the web writes it: whole numbers as integers (`10`, not `10.0`), so values
/// compare equal to the rest of the settings JSON.
fn number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}
