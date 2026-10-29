//! Lens blur as an edit (desktop only, `desktop.lensBlur`): the photo's depth, found once by AI
//! and kept here as a small picture (0 far, 255 near, encoded as an AI mask's coverage is,
//! [`crate::masks::encode_coverage`]), with how strong the blur is, the depth in focus and how
//! deep the sharp band is. The develop shader blurs each pixel by its distance from the focus
//! (a disc of samples in linear light, so highlights bloom), so the blur stays editable and
//! follows every other edit, unlike AI → Lens blur, which writes a new photo.

use serde::{Deserialize, Serialize};

use crate::EditSettings;
use crate::masks::{MAX_IMAGE_EDGE, decode_coverage};

/// The widest blur at Amount 100, as a share of the photo's height (a disc's radius).
pub const MAX_RADIUS: f64 = 0.025;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LensBlur {
    /// 0..100.
    pub amount: f64,
    /// The depth in focus, 0 (farthest) .. 100 (nearest).
    pub focus: f64,
    /// How deep the sharp band around the focus is, 0..100.
    pub range: f64,
    pub width: u32,
    pub height: u32,
    /// The depth picture, `width` × `height`.
    pub depth: String,
}

impl Default for LensBlur {
    fn default() -> LensBlur {
        LensBlur {
            amount: 50.0,
            focus: 80.0,
            range: 15.0,
            width: 0,
            height: 0,
            depth: String::new(),
        }
    }
}

impl LensBlur {
    /// Read `desktop.lensBlur`; `None` when there is none or its depth cannot be read.
    pub fn from_settings(s: &EditSettings) -> Option<LensBlur> {
        let b: LensBlur = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("lensBlur"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())?;
        b.values()?;
        let fix = |v: f64, d: f64| {
            if v.is_finite() {
                v.clamp(0.0, 100.0)
            } else {
                d
            }
        };
        Some(LensBlur {
            amount: fix(b.amount, 50.0),
            focus: fix(b.focus, 80.0),
            range: fix(b.range, 15.0),
            ..b
        })
    }

    /// The depth picture's values (0 far .. 255 near), when they read as its size.
    pub fn values(&self) -> Option<Vec<u8>> {
        if self.width == 0 || self.height == 0 || self.width.max(self.height) > MAX_IMAGE_EDGE {
            return None;
        }
        decode_coverage(&self.depth, (self.width * self.height) as usize)
    }

    /// Store into `desktop.lensBlur` (`None` removes it; an emptied `desktop` is dropped).
    pub fn write_into(this: Option<&LensBlur>, s: &mut EditSettings) {
        match this {
            Some(b) => {
                s.desktop.get_or_insert_with(Default::default).insert(
                    "lensBlur".to_string(),
                    serde_json::to_value(b).expect("lens blur serialises"),
                );
            }
            None => {
                if let Some(desktop) = s.desktop.as_mut() {
                    desktop.remove("lensBlur");
                    if desktop.is_empty() {
                        s.desktop = None;
                    }
                }
            }
        }
    }

    /// The shader's `blur`: the radius at full blur (a share of the height), the focus and
    /// half the sharp band (0..1 depth), and 1 when on (0: none, the web's pass).
    pub fn uniform(&self) -> [f32; 4] {
        let on = self.amount > 0.0;
        [
            (self.amount / 100.0 * MAX_RADIUS) as f32,
            (self.focus / 100.0) as f32,
            (self.range / 100.0 * 0.5) as f32,
            f32::from(u8::from(on)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blur_round_trips_and_one_without_depth_is_none() {
        let mut s = EditSettings::default_settings();
        assert!(LensBlur::from_settings(&s).is_none());
        let b = LensBlur {
            width: 2,
            height: 1,
            depth: crate::masks::encode_coverage(&[0, 255]),
            ..LensBlur::default()
        };
        LensBlur::write_into(Some(&b), &mut s);
        assert_eq!(LensBlur::from_settings(&s), Some(b.clone()));
        assert_eq!(b.values(), Some(vec![0, 255]));
        let mut off = b.clone();
        off.amount = 0.0;
        assert_eq!(off.uniform()[3], 0.0);
        LensBlur::write_into(None, &mut s);
        assert!(s.desktop.is_none());
        let broken = LensBlur {
            depth: "x".into(),
            ..b
        };
        LensBlur::write_into(Some(&broken), &mut s);
        assert!(
            LensBlur::from_settings(&s).is_none(),
            "a depth that cannot be read"
        );
    }
}
