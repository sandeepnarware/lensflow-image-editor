//! The crop tool's composition overlays, cycled with `O` (as Lightroom's): the rule of thirds,
//! a grid, the golden ratio, diagonals, triangles and the golden spiral. Each is lines in the
//! crop frame's own fractions (0..1 across and down), drawn over the frame by
//! [`super::paint_crop`]; the spiral is a polyline of quarter arcs.

/// The golden ratio's smaller part (1 − 1/φ).
const SHORT: f32 = 0.381_966;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    Thirds,
    Grid,
    GoldenRatio,
    Diagonals,
    Triangles,
    Spiral,
}

impl Overlay {
    pub const ALL: [Overlay; 6] = [
        Overlay::Thirds,
        Overlay::Grid,
        Overlay::GoldenRatio,
        Overlay::Diagonals,
        Overlay::Triangles,
        Overlay::Spiral,
    ];

    /// The next overlay (`O`), round again after the last.
    pub fn next(self) -> Overlay {
        let i = Overlay::ALL.iter().position(|o| *o == self).unwrap_or(0);
        Overlay::ALL[(i + 1) % Overlay::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            Overlay::Thirds => "Thirds",
            Overlay::Grid => "Grid",
            Overlay::GoldenRatio => "Golden ratio",
            Overlay::Diagonals => "Diagonals",
            Overlay::Triangles => "Triangles",
            Overlay::Spiral => "Golden spiral",
        }
    }

    /// The overlay's lines in a frame `aspect` wide per unit of height, each a polyline of
    /// points in the frame's fractions.
    pub fn lines(self, aspect: f32) -> Vec<Vec<[f32; 2]>> {
        let vertical = |x: f32| vec![[x, 0.0], [x, 1.0]];
        let horizontal = |y: f32| vec![[0.0, y], [1.0, y]];
        let at = |ks: &[f32]| {
            ks.iter()
                .flat_map(|&k| [vertical(k), horizontal(k)])
                .collect::<Vec<_>>()
        };
        match self {
            Overlay::Thirds => at(&[1.0 / 3.0, 2.0 / 3.0]),
            Overlay::Grid => at(&[0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875]),
            Overlay::GoldenRatio => at(&[SHORT, 1.0 - SHORT]),
            Overlay::Diagonals => {
                // From each corner at 45° (in the photo, not the fractions) to the far side.
                let a = aspect.max(1e-3);
                let (dx, dy) = if a >= 1.0 { (1.0 / a, 1.0) } else { (1.0, a) };
                vec![
                    vec![[0.0, 0.0], [dx, dy]],
                    vec![[1.0, 0.0], [1.0 - dx, dy]],
                    vec![[0.0, 1.0], [dx, 1.0 - dy]],
                    vec![[1.0, 1.0], [1.0 - dx, 1.0 - dy]],
                ]
            }
            Overlay::Triangles => {
                // The diagonal, and from the other two corners square to it.
                let a = aspect.max(1e-3);
                // The foot of the perpendicular from (1, 0) onto the line (0,0)-(1,1), in
                // photo units (x × aspect), back in fractions.
                let (w, h) = (a, 1.0f32);
                let t = (w * w) / (w * w + h * h);
                let foot = [t, t];
                vec![
                    vec![[0.0, 0.0], [1.0, 1.0]],
                    vec![[1.0, 0.0], foot],
                    vec![[0.0, 1.0], [1.0 - foot[0], 1.0 - foot[1]]],
                ]
            }
            Overlay::Spiral => vec![spiral()],
        }
    }
}

/// The golden spiral: quarter arcs through squares cut from the frame in turn (left, top,
/// right, bottom, …), each arc 16 points.
fn spiral() -> Vec<[f32; 2]> {
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, 0.0f32, 1.0f32, 1.0f32);
    let mut points = Vec::new();
    for step in 0..10 {
        let (w, h) = (x1 - x0, y1 - y0);
        // Each square takes the golden share of the remaining rectangle's long way.
        let (cx, cy, r, start) = match step % 4 {
            0 => {
                let s = w * (1.0 - SHORT);
                let c = (x0 + s, y1, s, h);
                x0 += s;
                c
            }
            1 => {
                let s = h * (1.0 - SHORT);
                let c = (x0, y0 + s, w, s);
                y0 += s;
                c
            }
            2 => {
                let s = w * (1.0 - SHORT);
                let c = (x1 - s, y0, s, h);
                x1 -= s;
                c
            }
            _ => {
                let s = h * (1.0 - SHORT);
                let c = (x1, y1 - s, w, s);
                y1 -= s;
                c
            }
        };
        // A quarter ellipse about the square's inner corner (cx, cy), radii (r, start).
        let base = std::f32::consts::FRAC_PI_2 * (step % 4) as f32 + std::f32::consts::PI;
        for i in 0..=16 {
            let t = base + std::f32::consts::FRAC_PI_2 * i as f32 / 16.0;
            points.push([cx + r * t.cos(), cy + start * t.sin()]);
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_cycles_through_every_overlay_and_back() {
        let mut o = Overlay::default();
        let mut seen = vec![o];
        for _ in 0..Overlay::ALL.len() - 1 {
            o = o.next();
            seen.push(o);
        }
        assert_eq!(seen, Overlay::ALL.to_vec());
        assert_eq!(o.next(), Overlay::Thirds);
    }

    #[test]
    fn every_line_stays_in_the_frame() {
        for o in Overlay::ALL {
            for aspect in [1.5f32, 0.66, 1.0] {
                for line in o.lines(aspect) {
                    assert!(line.len() >= 2);
                    for [x, y] in line {
                        assert!(
                            (-1e-4..=1.0001).contains(&x) && (-1e-4..=1.0001).contains(&y),
                            "{o:?} {aspect}: {x}, {y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_golden_ratio_lines_sit_at_382_and_618() {
        let lines = Overlay::GoldenRatio.lines(1.5);
        assert!((lines[0][0][0] - 0.382).abs() < 1e-3);
        assert!((lines[2][0][0] - 0.618).abs() < 1e-3);
    }

    #[test]
    fn a_45_degree_diagonal_is_square_in_the_photo() {
        // On a 3:2 frame, from the top left down one height and across one height.
        let d = &Overlay::Diagonals.lines(1.5)[0];
        let (dx, dy) = (d[1][0] * 1.5, d[1][1]);
        assert!((dx - dy).abs() < 1e-5);
    }
}
