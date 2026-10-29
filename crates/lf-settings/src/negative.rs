//! Film negative conversion (desktop only, `desktop.negative`): a scanned or photographed
//! colour negative turned into a positive, before every other adjustment, so white balance,
//! exposure and the rest then work on the positive as on any photo.
//!
//! Film records light as density: the brighter the scene, the darker the negative, and the
//! film's own base (the orange mask of colour film) lets most light through where nothing was
//! exposed. Each channel is measured against that base — the light the film let through, as a
//! share of what its base lets through — and taken to density (`−log₁₀`), which is the scene's
//! brightness in log steps; each channel's density is then scaled by the photo's own range
//! ([`Negative::measure`]: the base from the brightest values, the range from the darkest),
//! which also cancels the orange mask. The shader does the same per pixel, in linear light.

use serde::{Deserialize, Serialize};

use crate::EditSettings;

/// A conversion: the film base (linear RGB, what unexposed film reads as) and each channel's
/// largest density in the photo (its white point).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Negative {
    pub base: [f64; 3],
    pub dmax: [f64; 3],
}

/// The smallest share of the base's light a pixel is taken to pass (a density cap of 4).
const MIN_PASSED: f64 = 1e-4;

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

impl Negative {
    /// Measured from the photo's pixels (encoded values, as the shader receives them): the
    /// base from each channel's 99.5th percentile, the range from its 0.5th. `None` for an
    /// empty or flat picture.
    pub fn measure(pixels: &[[f32; 3]]) -> Option<Negative> {
        if pixels.len() < 16 {
            return None;
        }
        let mut base = [0.0; 3];
        let mut dmax = [0.0; 3];
        for c in 0..3 {
            let mut v: Vec<f64> = pixels
                .iter()
                .map(|p| f64::from(p[c]).max(0.0).powf(2.2))
                .collect();
            base[c] = percentile(&mut v, 0.995);
            let dark = percentile(&mut v, 0.005);
            if base[c] <= 1e-6 {
                return None;
            }
            dmax[c] = density(dark, base[c]);
            if dmax[c] < 0.05 {
                return None;
            }
        }
        Some(Negative { base, dmax })
    }

    /// The positive of one pixel (linear in, linear out, 0..1 for the photo's range).
    pub fn convert(&self, lin: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|c| (density(lin[c], self.base[c]) / self.dmax[c]).max(0.0))
    }

    /// Read `desktop.negative`; anything missing or unusable is no conversion.
    pub fn from_settings(s: &EditSettings) -> Option<Negative> {
        let n: Negative = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("negative"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())?;
        let ok = n
            .base
            .iter()
            .chain(&n.dmax)
            .all(|v| v.is_finite() && *v > 0.0);
        ok.then_some(n)
    }

    /// Store `n` into `desktop.negative` (`None`: no conversion, no key; an emptied `desktop`
    /// is dropped).
    pub fn write_into(n: Option<&Negative>, s: &mut EditSettings) {
        match n {
            Some(n) => {
                s.desktop.get_or_insert_with(Default::default).insert(
                    "negative".to_string(),
                    serde_json::to_value(n).expect("negative serialises"),
                );
            }
            None => {
                if let Some(desktop) = s.desktop.as_mut() {
                    desktop.remove("negative");
                    if desktop.is_empty() {
                        s.desktop = None;
                    }
                }
            }
        }
    }
}

/// The density of a pixel passing `lin` of light where the base passes `base`.
fn density(lin: f64, base: f64) -> f64 {
    -(lin / base).clamp(MIN_PASSED, 1.0).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A negative of `scene` (linear, 0..1 scene light) on film with an orange `base`: denser
    /// (darker) where the scene was brighter, by a different contrast per channel.
    fn negative_of(scene: f64, base: [f64; 3]) -> [f32; 3] {
        let gamma = [0.6, 0.7, 0.8];
        std::array::from_fn(|c| {
            let d = gamma[c] * (scene.max(0.001) / 0.001).log10() / 3.0;
            ((base[c] * 10f64.powf(-d)).powf(1.0 / 2.2)) as f32
        })
    }

    #[test]
    fn a_negative_becomes_a_neutral_positive_whatever_the_orange_mask() {
        let base = [0.8, 0.45, 0.25];
        let scenes: Vec<f64> = (0..400).map(|i| 0.001 + 0.999 * i as f64 / 399.0).collect();
        let pixels: Vec<[f32; 3]> = scenes.iter().map(|&v| negative_of(v, base)).collect();
        // The base is read from the clearest film there is; the same scene point sets every
        // channel's reference, so the colours still agree after scaling.
        let n = Negative::measure(&pixels).unwrap();
        let lin = |p: [f32; 3]| p.map(|v| f64::from(v).powf(2.2));
        // Unexposed film is black; the brightest scene is white; a grey scene is grey (the
        // channels agree although the film's colours differ).
        let black = n.convert(lin(negative_of(0.001, base)));
        assert!(black.iter().all(|v| *v < 0.02), "{black:?}");
        let white = n.convert(lin(negative_of(1.0, base)));
        assert!(white.iter().all(|v| *v > 0.97), "{white:?}");
        let mid = n.convert(lin(negative_of(0.05, base)));
        assert!(
            (mid[0] - mid[1]).abs() < 0.03 && (mid[1] - mid[2]).abs() < 0.03,
            "grey stays grey: {mid:?}"
        );
        assert!(mid[0] > 0.3 && mid[0] < 0.8, "{mid:?}");
    }

    #[test]
    fn it_round_trips_and_a_flat_picture_measures_nothing() {
        let n = Negative {
            base: [0.8, 0.5, 0.3],
            dmax: [2.0, 2.1, 2.2],
        };
        let mut s = EditSettings::default_settings();
        Negative::write_into(Some(&n), &mut s);
        assert_eq!(Negative::from_settings(&s), Some(n));
        Negative::write_into(None, &mut s);
        assert_eq!(s.desktop, None);
        assert_eq!(Negative::measure(&[[0.5, 0.5, 0.5]; 100]), None);
    }
}
