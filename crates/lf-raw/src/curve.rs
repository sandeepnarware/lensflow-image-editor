//! The base tone curve applied to scene-linear sRGB before gamma encoding.
//!
//! [`camera`] is a monotone cubic Hermite spline through fixed knots
//! (linear input `x`, linear output `y`, slope `m`):
//!
//! | x    | y                     | m    | role                                       |
//! |------|-----------------------|------|--------------------------------------------|
//! | 0.00 | 0.000                 | 1.40 | black stays black                          |
//! | 0.02 | 0.025                 | 1.10 | end of the toe: a gentle lift of the blacks |
//! | 0.18 | 0.181 16 (= 0.46^2.2) | 1.05 | mid-grey: 0.46 after the 1/2.2 gamma       |
//! | 0.80 | 0.840                 | 1.00 | start of the shoulder                      |
//! | 1.00 | 1.000                 | 0.60 | sensor white reaches display white         |
//!
//! Every segment meets the Fritsch–Carlson condition (slopes within 3× the secant), so
//! the spline is strictly increasing. Above 1.0 it continues as the line
//! `1 + 0.6 (x - 1)` (the shoulder's end slope), so reconstructed highlights keep their
//! headroom; below 0 it continues with the slope at 0.

use lf_settings::Profile;

const KNOTS: [(f32, f32, f32); 5] = [
    (0.0, 0.0, 1.40),
    (0.02, 0.025, 1.10),
    (0.18, 0.181_16, 1.05),
    (0.80, 0.840, 1.00),
    (1.00, 1.000, 0.60),
];

/// The `Camera` base curve on linear input.
pub fn camera(x: f32) -> f32 {
    let (x0, y0, m0) = KNOTS[0];
    if x <= x0 {
        return y0 + m0 * (x - x0);
    }
    let (xn, yn, mn) = KNOTS[KNOTS.len() - 1];
    if x >= xn {
        return yn + mn * (x - xn);
    }
    for pair in KNOTS.windows(2) {
        let ((xa, ya, ma), (xb, yb, mb)) = (pair[0], pair[1]);
        if x <= xb {
            let h = xb - xa;
            let t = (x - xa) / h;
            let (t2, t3) = (t * t, t * t * t);
            return (2.0 * t3 - 3.0 * t2 + 1.0) * ya
                + (t3 - 2.0 * t2 + t) * h * ma
                + (-2.0 * t3 + 3.0 * t2) * yb
                + (t3 - t2) * h * mb;
        }
    }
    yn
}

/// The base curve for `profile` (`Linear` is the identity).
#[inline]
pub fn apply(profile: Profile, x: f32) -> f32 {
    match profile {
        Profile::Camera => camera(x),
        Profile::Linear => x,
    }
}
