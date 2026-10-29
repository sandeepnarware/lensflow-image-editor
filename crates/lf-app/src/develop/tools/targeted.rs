//! The targeted adjustment for Colour mix (Lightroom's): with the tool on, a drag up or down
//! on a colour in the photo moves the hue, saturation or luminance (the panel's chosen
//! property) of the bands that colour belongs to, each by its share of it, as the develop
//! shader weighs bands ([`weights`]). The colour is the edited photo's under the pointer (the
//! colour readout's picture). One drag is one history step.

use lf_settings::{EditSettings, HSL_BANDS, HSL_HUES};

use crate::develop::panels::hsl::Mode;

/// Slider units per point dragged.
pub const PER_POINT: f64 = 0.5;
pub const HINT: &str = "Drag up or down on a colour in the photo to change it.";
pub const GREY: &str = "That spot has no colour to change (it is grey); try a more colourful part.";

/// How much each band (in [`HSL_BANDS`] order) takes a colour of `hue` degrees: the shader's
/// smoothstep of the hue distance over the band's width, scaled so the strongest is 1.
pub fn weights(hue: f64) -> [f64; 8] {
    let gap = |a: f64, b: f64| ((a - b + 540.0) % 360.0 - 180.0).abs();
    let n = HSL_HUES.len();
    let mut w = [0.0; 8];
    for (i, h) in HSL_HUES.iter().enumerate() {
        let width = gap(*h, HSL_HUES[(i + n - 1) % n]).max(gap(*h, HSL_HUES[(i + 1) % n]));
        let t = (1.0 - gap(hue, *h) / width).clamp(0.0, 1.0);
        w[i] = t * t * (3.0 - 2.0 * t);
    }
    let top = w.iter().copied().fold(0.0, f64::max);
    if top > 0.0 {
        for v in &mut w {
            *v /= top;
        }
    }
    w
}

/// A drag in progress: the bands' shares and their values when it began.
#[derive(Debug, Clone)]
struct Drag {
    weights: [f64; 8],
    start: [f64; 8],
    y0: f32,
}

/// The open targeted tool.
#[derive(Debug, Clone)]
pub struct TargetedTool {
    pub mode: Mode,
    drag: Option<Drag>,
}

fn value(s: &mut EditSettings, i: usize, mode: Mode) -> &mut f64 {
    let a = &mut s.hsl[HSL_BANDS[i]];
    match mode {
        Mode::Hue => &mut a.h,
        Mode::Saturation => &mut a.s,
        Mode::Luminance => &mut a.l,
    }
}

impl TargetedTool {
    pub fn new(mode: Mode) -> TargetedTool {
        TargetedTool { mode, drag: None }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A drag starts at screen height `y` on a colour of `hue` (none: grey, nothing moves).
    pub fn start(&mut self, s: &EditSettings, hue: Option<f64>, y: f32) -> bool {
        let Some(hue) = hue else {
            return false;
        };
        let mut copy = s.clone();
        let start = std::array::from_fn(|i| *value(&mut copy, i, self.mode));
        self.drag = Some(Drag {
            weights: weights(hue),
            start,
            y0: y,
        });
        true
    }

    /// The pointer is at height `y`: up raises, down lowers. True when the settings changed.
    pub fn drag_to(&mut self, s: &mut EditSettings, y: f32) -> bool {
        let Some(d) = &self.drag else {
            return false;
        };
        let delta = f64::from(d.y0 - y) * PER_POINT;
        let mut changed = false;
        for i in 0..8 {
            if d.weights[i] <= 0.0 {
                continue;
            }
            let v = (d.start[i] + delta * d.weights[i])
                .clamp(-100.0, 100.0)
                .round();
            let slot = value(s, i, self.mode);
            if *slot != v {
                *slot = v;
                changed = true;
            }
        }
        changed
    }

    /// The drag ended; true when there was one (commit it).
    pub fn release(&mut self) -> bool {
        self.drag.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_settings::HslBand;

    #[test]
    fn a_colour_between_two_bands_moves_both() {
        let w = weights(15.0);
        // Both bands take it (the shader's widths: red reaches further than orange).
        assert!(w[0] == 1.0 && w[1] > 0.5, "red and orange: {w:?}");
        assert_eq!(w[3], 0.0, "not green");
        let blue = weights(225.0);
        assert_eq!(blue[5], 1.0);
        assert!(blue[0] == 0.0);
    }

    #[test]
    fn dragging_up_on_a_blue_raises_the_blue_bands_and_one_step_ends_it() {
        let mut s = EditSettings::default_settings();
        let mut t = TargetedTool::new(Mode::Saturation);
        assert!(!t.start(&s, None, 100.0), "grey: nothing to move");
        assert!(t.start(&s, Some(225.0), 100.0));
        assert!(t.drag_to(&mut s, 60.0));
        assert_eq!(s.hsl[HslBand::Blue].s, 20.0);
        assert_eq!(s.hsl[HslBand::Red].s, 0.0);
        assert_eq!(s.hsl[HslBand::Blue].h, 0.0, "only saturation");
        // Far down: kept within -100.
        t.drag_to(&mut s, 1000.0);
        assert_eq!(s.hsl[HslBand::Blue].s, -100.0);
        assert!(t.release());
        assert!(!t.release());
    }
}
