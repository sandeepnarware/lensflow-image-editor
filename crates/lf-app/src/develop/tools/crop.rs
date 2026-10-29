//! The crop tool (`R`): the frame over the whole turned, straightened photo, its eight handles
//! and inside-drag move, the aspect ratios, quarter turns, flips and straighten. Ported from
//! the web editor's `CropOverlay.vue` and `CropPanel.vue`.
//!
//! The crop is in fractions of the turned frame (`tw`×`th` pixels). It never leaves the
//! straightened photo: a drag goes as far as the photo allows along its direction, and
//! [`fit_crop`] pulls in anything still outside.
//!
//! Commits: the frame, the aspect and "Reset crop & rotation" are one crop session,
//! committed once on `Enter` or when the tool is left; `Esc` puts back the settings the tool
//! was entered with. A quarter turn, a flip and the end of a straighten drag each commit at
//! once (one history step each), as the other panels' controls do.

use super::ToolOutput;
use crate::develop::edit_state::EditState;
use lf_settings::geometry::{
    Geometry, TurnDir, aspect_ratio, crop_inside, fit_crop, largest_crop, turn_settings,
    turned_size,
};
use lf_settings::{Crop, EditSettings};

/// The smallest crop side, as a fraction of the turned frame (the web's `MIN`).
pub const MIN: f64 = 0.03;

/// The whole turned frame: what the preview draws while the tool is open.
pub const WHOLE: Crop = Crop {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

/// The aspect choices (value, label): the web's `ASPECTS` (`geometry.ts`), same order.
pub const ASPECTS: [(&str, &str); 14] = [
    ("original", "Original"),
    ("free", "Free"),
    ("1:1", "1:1"),
    ("4:5", "4:5"),
    ("5:4", "5:4"),
    ("2:3", "2:3"),
    ("3:2", "3:2"),
    ("3:4", "3:4"),
    ("4:3", "4:3"),
    ("9:16", "9:16"),
    ("16:9", "16:9"),
    ("5:7", "5:7"),
    ("8:10", "8×10"),
    ("12:18", "12×18"),
];

/// Turns a locked aspect between portrait and landscape.
pub const SWITCH: &str = "Switch portrait / landscape";
pub const RESET: &str = "Reset crop & rotation";
pub const STRAIGHTEN: &str = "Straighten";

/// What a drag on the frame does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// Inside the frame: moves it.
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    /// The eight resize handles, corners first (they win where they overlap an edge).
    pub const RESIZE: [Handle; 8] = [
        Handle::NW,
        Handle::NE,
        Handle::SW,
        Handle::SE,
        Handle::N,
        Handle::S,
        Handle::W,
        Handle::E,
    ];

    fn north(self) -> bool {
        matches!(self, Handle::N | Handle::NE | Handle::NW)
    }

    fn south(self) -> bool {
        matches!(self, Handle::S | Handle::SE | Handle::SW)
    }

    fn east(self) -> bool {
        matches!(self, Handle::E | Handle::NE | Handle::SE)
    }

    fn west(self) -> bool {
        matches!(self, Handle::W | Handle::NW | Handle::SW)
    }

    fn corner(self) -> bool {
        matches!(self, Handle::NE | Handle::NW | Handle::SE | Handle::SW)
    }

    /// Where the handle sits on the frame, as fractions of the crop (0, 0 = top left).
    pub fn anchor(self) -> (f64, f64) {
        let x = if self.west() {
            0.0
        } else if self.east() {
            1.0
        } else {
            0.5
        };
        let y = if self.north() {
            0.0
        } else if self.south() {
            1.0
        } else {
            0.5
        };
        (x, y)
    }
}

/// Inside the turned frame, at least [`MIN`] on each side, and inside the straightened photo.
fn valid(c: &Crop, tw: f64, th: f64, angle: f64) -> bool {
    c.w >= MIN
        && c.h >= MIN
        && c.x >= -1e-6
        && c.y >= -1e-6
        && c.x + c.w <= 1.0 + 1e-6
        && c.y + c.h <= 1.0 + 1e-6
        && crop_inside(c, tw, th, angle)
}

/// The frame for a drag of `(dx, dy)` (fractions of the turned frame) on `handle`, before
/// any limit: the web's `resize`.
fn resize(
    start: Crop,
    handle: Handle,
    (dx, dy): (f64, f64),
    ratio: Option<f64>,
    tw: f64,
    th: f64,
) -> Crop {
    if handle == Handle::Move {
        return Crop {
            x: start.x + dx,
            y: start.y + dy,
            ..start
        };
    }
    let Crop {
        mut x,
        mut y,
        mut w,
        mut h,
    } = start;
    let right = x + w;
    let bottom = y + h;
    if handle.east() {
        w = start.w + dx;
    }
    if handle.west() {
        w = start.w - dx;
        x = right - w;
    }
    if handle.south() {
        h = start.h + dy;
    }
    if handle.north() {
        h = start.h - dy;
        y = bottom - h;
    }
    if let Some(ratio) = ratio {
        // In fractions: w·tw ÷ (h·th) = ratio.
        let k = ratio * th / tw;
        if matches!(handle, Handle::E | Handle::W) {
            let nh = w / k;
            y = start.y + (start.h - nh) / 2.0;
            h = nh;
        } else if matches!(handle, Handle::N | Handle::S) {
            let nw = h * k;
            x = start.x + (start.w - nw) / 2.0;
            w = nw;
        } else if handle.corner() {
            // Follow whichever side moved more, anchored at the opposite corner.
            if (w - start.w).abs() * tw >= (h - start.h).abs() * th {
                h = w / k;
            } else {
                w = h * k;
            }
            if handle.west() {
                x = right - w;
            }
            if handle.north() {
                y = bottom - h;
            }
        }
    }
    Crop { x, y, w, h }
}

/// The frame after dragging `handle` by `delta` (fractions of the turned frame, from where
/// the drag started on `crop`). A locked `ratio` (width ÷ height in pixels) keeps its shape;
/// the frame never leaves the turned frame or the photo straightened by `angle`: the drag
/// goes as far as it can along its direction (a move slides along an edge it meets), and
/// [`fit_crop`] pulls in a frame that started outside.
pub fn drag(
    crop: Crop,
    handle: Handle,
    delta: (f64, f64),
    ratio: Option<f64>,
    tw: f64,
    th: f64,
    angle: f64,
) -> Crop {
    let (mut dx, mut dy) = delta;
    if handle == Handle::Move {
        // Slide along the frame's edges rather than stopping where one is met.
        dx = (crop.x + dx).clamp(0.0, (1.0 - crop.w).max(0.0)) - crop.x;
        dy = (crop.y + dy).clamp(0.0, (1.0 - crop.h).max(0.0)) - crop.y;
    }
    let at = |t: f64| resize(crop, handle, (dx * t, dy * t), ratio, tw, th);
    let wanted = at(1.0);
    let moved = if valid(&wanted, tw, th, angle) {
        wanted
    } else {
        // As far as the photo allows along the drag.
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..24 {
            let t = (lo + hi) / 2.0;
            if valid(&at(t), tw, th, angle) {
                lo = t;
            } else {
                hi = t;
            }
        }
        if lo > 0.0 { at(lo) } else { crop }
    };
    fit_crop(&moved, tw, th, angle)
}

/// The handle under `p` (fractions of the turned frame), within `tol` (fractions, per axis)
/// of it; inside the frame is [`Handle::Move`]; `None` outside.
pub fn handle_at(crop: &Crop, p: (f64, f64), tol: (f64, f64)) -> Option<Handle> {
    for handle in Handle::RESIZE {
        let (ax, ay) = handle.anchor();
        let hx = crop.x + ax * crop.w;
        let hy = crop.y + ay * crop.h;
        // Edge handles reach along their whole edge.
        let reach_x = if ax == 0.5 { crop.w / 2.0 } else { tol.0 };
        let reach_y = if ay == 0.5 { crop.h / 2.0 } else { tol.1 };
        if (p.0 - hx).abs() <= reach_x && (p.1 - hy).abs() <= reach_y {
            return Some(handle);
        }
    }
    let inside = p.0 >= crop.x && p.0 <= crop.x + crop.w && p.1 >= crop.y && p.1 <= crop.y + crop.h;
    inside.then_some(Handle::Move)
}

/// The turned frame's size for `s` on a `width`×`height` photo.
pub fn turned(s: &EditSettings, width: u32, height: u32) -> (f64, f64) {
    let (tw, th) = turned_size(width, height, s.rotate);
    (f64::from(tw), f64::from(th))
}

/// The locked width ÷ height for `s`'s aspect, or `None` for free.
pub fn ratio(s: &EditSettings, tw: f64, th: f64) -> Option<f64> {
    aspect_ratio(&s.aspect, tw / th)
}

/// A numeric aspect whose sides differ ("3:2"): it can switch portrait / landscape.
pub fn flippable(aspect: &str) -> bool {
    match aspect.split_once(':') {
        Some((a, b)) => {
            a != b
                && a.parse::<f64>().is_ok_and(|v| v > 0.0)
                && b.parse::<f64>().is_ok_and(|v| v > 0.0)
        }
        None => false,
    }
}

/// The button for `value` shows as chosen when the photo's aspect is `current`.
/// A switched shape that is not listed itself (7:5) lights its listed twin (5:7).
pub fn aspect_lit(value: &str, current: &str) -> bool {
    current == value
        || (flippable(value)
            && swapped(value) == current
            && !ASPECTS.iter().any(|(v, _)| *v == current))
}

/// "3:2" → "2:3".
pub fn swapped(aspect: &str) -> String {
    match aspect.split_once(':') {
        Some((a, b)) => format!("{b}:{a}"),
        None => aspect.to_owned(),
    }
}

/// Picks `aspect`: the largest frame of that shape inside the current one, centred on it.
pub fn set_aspect(s: &mut EditSettings, aspect: &str, tw: f64, th: f64) {
    s.aspect = aspect.to_owned();
    if let Some(r) = aspect_ratio(aspect, tw / th) {
        let c = s.crop;
        let k = r * th / tw;
        let mut w = c.w;
        let mut h = w / k;
        if h > c.h {
            h = c.h;
            w = h * k;
        }
        let wanted = Crop {
            x: c.x + (c.w - w) / 2.0,
            y: c.y + (c.h - h) / 2.0,
            w,
            h,
        };
        s.crop = fit_crop(&wanted, tw, th, s.angle);
    }
}

fn apply_geometry(s: &mut EditSettings, g: &Geometry) {
    s.rotate = g.rotate;
    s.flip_h = g.flip_h;
    s.flip_v = g.flip_v;
    s.angle = g.angle;
    s.crop = g.crop;
}

/// A quarter turn: the crop turns with the photo, and a locked shape too (3:2 → 2:3).
pub fn turn(s: &mut EditSettings, dir: TurnDir) {
    let turned = turn_settings(&Geometry::from(&*s), dir);
    apply_geometry(s, &turned);
    if flippable(&s.aspect) {
        s.aspect = swapped(&s.aspect);
    }
}

/// Flips the photo horizontally or vertically.
pub fn flip(s: &mut EditSettings, horizontal: bool) {
    if horizontal {
        s.flip_h = !s.flip_h;
    } else {
        s.flip_v = !s.flip_v;
    }
}

/// Straightens by `angle` degrees; the crop shrinks about its centre until it fits.
pub fn straighten(s: &mut EditSettings, angle: f64, tw: f64, th: f64) {
    s.angle = angle;
    s.crop = fit_crop(&s.crop, tw, th, angle);
}

/// "Reset crop & rotation": level, unflipped, the original shape, the whole frame (the quarter
/// turn stays, as on the web).
pub fn reset(s: &mut EditSettings, tw: f64, th: f64) {
    s.angle = 0.0;
    s.flip_h = false;
    s.flip_v = false;
    s.aspect = "original".to_owned();
    s.crop = largest_crop(tw, th, 0.0, None);
}

/// A drag on the frame in progress.
#[derive(Debug, Clone, Copy)]
struct Drag {
    handle: Handle,
    start: Crop,
    origin: (f64, f64),
}

/// The open crop tool.
#[derive(Debug, Clone)]
pub struct CropTool {
    /// The settings when the tool was entered (`Esc` puts them back).
    entry: EditSettings,
    drag: Option<Drag>,
    /// The straighten slider is being dragged (the thirds grid shows).
    pub straightening: bool,
}

impl CropTool {
    pub fn enter(settings: &EditSettings) -> CropTool {
        CropTool {
            entry: settings.clone(),
            drag: None,
            straightening: false,
        }
    }

    pub fn entry(&self) -> &EditSettings {
        &self.entry
    }

    /// A drag on the frame is in progress.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The thirds grid shows (a frame or straighten drag).
    pub fn grid(&self) -> bool {
        self.dragging() || self.straightening
    }

    /// A press at `p` (fractions of the turned frame): starts a drag on the handle there
    /// (within `tol`), or a move inside the frame. False when it missed the frame.
    pub fn press(&mut self, s: &EditSettings, p: (f64, f64), tol: (f64, f64)) -> bool {
        self.drag = handle_at(&s.crop, p, tol).map(|handle| Drag {
            handle,
            start: s.crop,
            origin: p,
        });
        self.drag.is_some()
    }

    /// The handle a drag is on.
    pub fn handle(&self) -> Option<Handle> {
        self.drag.map(|d| d.handle)
    }

    /// The pointer is at `p`: the frame follows. True when it changed.
    pub fn drag_to(&self, s: &mut EditSettings, p: (f64, f64), tw: f64, th: f64) -> bool {
        let Some(d) = self.drag else {
            return false;
        };
        let delta = (p.0 - d.origin.0, p.1 - d.origin.1);
        let next = drag(d.start, d.handle, delta, ratio(s, tw, th), tw, th, s.angle);
        let changed = next != s.crop;
        s.crop = next;
        changed
    }

    /// The drag ended: part of the crop session, committed with it.
    pub fn release(&mut self) -> ToolOutput {
        self.drag = None;
        ToolOutput::default()
    }

    /// `Esc`: the geometry (turn, flips, straighten, frame and aspect) goes back to what it
    /// was when the tool was entered; any other change made meanwhile (an Exposure drag in
    /// the panels) stays. Nothing is committed unless something in the session was already
    /// committed, in which case putting it back is one more step (the catalogue must hold
    /// what is shown).
    pub fn cancel(&self, edit: &mut EditState) -> ToolOutput {
        let entry = &self.entry;
        edit.set(|s| {
            s.rotate = entry.rotate;
            s.flip_h = entry.flip_h;
            s.flip_v = entry.flip_v;
            s.angle = entry.angle;
            s.crop = entry.crop;
            s.aspect = entry.aspect.clone();
        });
        ToolOutput {
            changed: true,
            commit: edit.dirty,
        }
    }

    /// `Enter`, or the tool left another way: the session is committed (one history step).
    pub fn leave(&self) -> ToolOutput {
        ToolOutput {
            changed: false,
            commit: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::{Catalog, NewPhoto, edits};

    const TW: f64 = 6000.0;
    const TH: f64 = 4000.0;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn inside(c: &Crop, tw: f64, th: f64, angle: f64) -> bool {
        c.x >= -1e-6
            && c.y >= -1e-6
            && c.x + c.w <= 1.0 + 1e-6
            && c.y + c.h <= 1.0 + 1e-6
            && crop_inside(c, tw, th, angle)
    }

    fn shrunk(c: Crop, k: f64) -> Crop {
        let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        Crop {
            x: cx - c.w * k / 2.0,
            y: cy - c.h * k / 2.0,
            w: c.w * k,
            h: c.h * k,
        }
    }

    #[test]
    fn drag_corner_with_locked_ratio_keeps_ratio() {
        // 1:1 in pixels: 0.4 × 6000 = 2400 = 0.6 × 4000.
        let start = Crop {
            x: 0.2,
            y: 0.2,
            w: 0.4,
            h: 0.6,
        };
        for (handle, delta) in [
            (Handle::SE, (0.1, 0.02)),
            (Handle::SE, (-0.05, -0.2)),
            (Handle::NW, (-0.1, 0.03)),
            (Handle::NE, (0.02, -0.15)),
            (Handle::SW, (0.5, 0.5)),
        ] {
            let c = drag(start, handle, delta, Some(1.0), TW, TH, 0.0);
            assert!(close(c.w * TW / (c.h * TH), 1.0), "{handle:?}: {c:?}");
            assert!(inside(&c, TW, TH, 0.0), "{handle:?}: {c:?}");
        }
        // The opposite corner stays put.
        let c = drag(start, Handle::SE, (0.1, 0.02), Some(1.0), TW, TH, 0.0);
        assert!(close(c.x, 0.2) && close(c.y, 0.2), "{c:?}");
        let c = drag(start, Handle::NW, (-0.1, 0.03), Some(1.0), TW, TH, 0.0);
        assert!(close(c.x + c.w, 0.6) && close(c.y + c.h, 0.8), "{c:?}");
        // An edge keeps the shape about the frame's centre line.
        let c = drag(start, Handle::E, (0.05, 0.0), Some(1.0), TW, TH, 0.0);
        assert!(close(c.w * TW / (c.h * TH), 1.0), "{c:?}");
        assert!(close(c.y + c.h / 2.0, 0.5), "{c:?}");
    }

    #[test]
    fn drag_never_leaves_turned_frame() {
        let start = Crop {
            x: 0.3,
            y: 0.3,
            w: 0.4,
            h: 0.4,
        };
        // A portrait turned frame too (a quarter turn swaps the sides).
        for (tw, th) in [(TW, TH), (TH, TW)] {
            for angle in [0.0, 7.5, -30.0] {
                let start = fit_crop(&start, tw, th, angle);
                for handle in Handle::RESIZE.into_iter().chain([Handle::Move]) {
                    for delta in [(2.0, 2.0), (-2.0, -2.0), (2.0, -1.0), (-0.5, 3.0)] {
                        for ratio in [None, Some(1.5), Some(0.8)] {
                            let c = drag(start, handle, delta, ratio, tw, th, angle);
                            assert!(
                                inside(&c, tw, th, angle),
                                "{handle:?} {delta:?} {ratio:?} at {angle}°: {c:?}"
                            );
                            assert!(c.w >= MIN - 1e-9 && c.h >= MIN - 1e-9, "{c:?}");
                        }
                    }
                }
            }
        }
        // Shrinking past the minimum stops at it.
        let c = drag(start, Handle::SE, (-1.0, -1.0), None, TW, TH, 0.0);
        assert!(c.w >= MIN - 1e-9 && c.w < MIN + 1e-3, "{c:?}");
    }

    #[test]
    fn move_inside_clamps() {
        let start = Crop {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        };
        let c = drag(start, Handle::Move, (1.0, -1.0), None, TW, TH, 0.0);
        assert!(
            close(c.x, 0.5) && close(c.y, 0.0),
            "slides into the corner: {c:?}"
        );
        assert!(close(c.w, 0.5) && close(c.h, 0.5), "keeps its size: {c:?}");
        // A small move goes where it was asked.
        let c = drag(start, Handle::Move, (0.05, 0.1), None, TW, TH, 0.0);
        assert!(close(c.x, 0.15) && close(c.y, 0.2), "{c:?}");
        // Straightened, it stops at the photo's turned edge, the same size.
        let start = shrunk(largest_crop(TW, TH, 10.0, None), 0.5);
        let c = drag(start, Handle::Move, (1.0, 0.0), None, TW, TH, 10.0);
        assert!(inside(&c, TW, TH, 10.0), "{c:?}");
        assert!(c.x > start.x, "it moved: {c:?}");
        assert!(close(c.w, start.w) && close(c.h, start.h), "{c:?}");
    }

    #[test]
    fn handles_are_found_corners_first_and_inside_moves() {
        let c = Crop {
            x: 0.2,
            y: 0.2,
            w: 0.4,
            h: 0.4,
        };
        let tol = (0.01, 0.01);
        assert_eq!(handle_at(&c, (0.205, 0.195), tol), Some(Handle::NW));
        assert_eq!(handle_at(&c, (0.6, 0.6), tol), Some(Handle::SE));
        assert_eq!(handle_at(&c, (0.3, 0.2), tol), Some(Handle::N));
        assert_eq!(handle_at(&c, (0.6, 0.45), tol), Some(Handle::E));
        assert_eq!(handle_at(&c, (0.4, 0.4), tol), Some(Handle::Move));
        assert_eq!(handle_at(&c, (0.9, 0.9), tol), None);
    }

    #[test]
    fn turning_turns_a_locked_shape_and_aspects_switch() {
        let mut s = EditSettings::default_settings();
        set_aspect(&mut s, "2:3", TW, TH);
        assert!(
            close(s.crop.w * TW / (s.crop.h * TH), 2.0 / 3.0),
            "{:?}",
            s.crop
        );
        assert!(flippable("2:3") && !flippable("1:1") && !flippable("free"));
        turn(&mut s, TurnDir::Clockwise);
        assert_eq!(s.aspect, "3:2");
        assert_eq!(s.rotate, lf_settings::Rotate::R90);
        straighten(&mut s, 20.0, TH, TW);
        assert!(crop_inside(&s.crop, TH, TW, 20.0));
        reset(&mut s, TH, TW);
        assert_eq!((s.angle, s.aspect.as_str()), (0.0, "original"));
        assert_eq!(s.crop, WHOLE);
    }

    fn catalogue() -> (tempfile::TempDir, Catalog, i64) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let folder = c.add_folder("shoot").unwrap();
        let id = c
            .add_photo(
                folder,
                &NewPhoto {
                    path: "shoot/a.jpg".into(),
                    kind: "jpeg".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        (dir, c, id)
    }

    /// What Develop does with a tool's output.
    fn absorb(out: ToolOutput, edit: &mut EditState, c: &Catalog) {
        if out.commit {
            edit.commit(c).unwrap();
        }
    }

    /// Three frame drags and an aspect change in one visit.
    fn crop_session(tool: &mut CropTool, edit: &mut EditState, c: &Catalog) {
        for (handle, (dx, dy)) in [
            (Handle::NW, (0.1, 0.1)),
            (Handle::Move, (-0.05, 0.0)),
            (Handle::SE, (-0.1, -0.05)),
        ] {
            let c0 = edit.settings.crop;
            let (ax, ay) = handle.anchor();
            let p = (c0.x + ax * c0.w, c0.y + ay * c0.h);
            let to = (p.0 + dx, p.1 + dy);
            assert!(tool.press(&edit.settings, p, (0.02, 0.02)), "{p:?}");
            assert_eq!(tool.handle(), Some(handle));
            edit.set(|s| {
                assert!(tool.drag_to(s, to, TW, TH), "{p:?} -> {to:?}");
            });
            let out = tool.release();
            absorb(out, edit, c);
        }
        edit.set(|s| set_aspect(s, "1:1", TW, TH));
    }

    #[test]
    fn a_crop_session_is_one_history_step() {
        let (_dir, c, id) = catalogue();
        let mut edit = EditState::new(id, EditSettings::default_settings());
        let mut tool = CropTool::enter(&edit.settings);
        crop_session(&mut tool, &mut edit, &c);
        assert_eq!(
            edits::history_len(&c, id).unwrap(),
            0,
            "nothing until Enter"
        );
        absorb(tool.leave(), &mut edit, &c);
        // The photo before, then the whole session.
        assert_eq!(edits::history_len(&c, id).unwrap(), 2);
        assert_eq!(edit.committed.aspect, "1:1");
        assert!(!edit.dirty);
    }

    #[test]
    fn esc_restores_the_entry_settings() {
        let (_dir, c, id) = catalogue();
        let mut edit = EditState::new(id, EditSettings::default_settings());
        let mut tool = CropTool::enter(&edit.settings);
        crop_session(&mut tool, &mut edit, &c);
        assert_ne!(edit.settings, *tool.entry());
        let out = tool.cancel(&mut edit);
        assert!(out.changed && !out.commit, "nothing to commit: {out:?}");
        assert_eq!(edit.settings, *tool.entry());
        assert!(!edit.dirty);
        absorb(out, &mut edit, &c);
        assert_eq!(edits::history_len(&c, id).unwrap(), 0, "no history step");

        // A turn committed during the visit is put back as one more step.
        let tool = CropTool::enter(&edit.settings);
        edit.set(|s| turn(s, TurnDir::Clockwise));
        edit.commit(&c).unwrap();
        let mut tool = tool;
        crop_session(&mut tool, &mut edit, &c);
        let out = tool.cancel(&mut edit);
        assert!(out.commit, "{out:?}");
        absorb(out, &mut edit, &c);
        assert_eq!(edit.committed, *tool.entry());
        assert_eq!(edits::history_len(&c, id).unwrap(), 3);
    }

    #[test]
    fn esc_keeps_a_light_change_made_while_cropping() {
        let (_dir, c, id) = catalogue();
        let mut edit = EditState::new(id, EditSettings::default_settings());
        let mut tool = CropTool::enter(&edit.settings);
        crop_session(&mut tool, &mut edit, &c);
        // An Exposure drag in the adjust panels while the crop tool is open, released.
        edit.set(|s| s.exposure = 0.7);
        edit.commit(&c).unwrap();
        edit.set(|s| s.flip_h = true);
        let out = tool.cancel(&mut edit);
        assert_eq!(edit.settings.exposure, 0.7, "the light change survives Esc");
        let entry = tool.entry();
        let s = &edit.settings;
        assert_eq!(
            (
                s.rotate,
                s.flip_h,
                s.flip_v,
                s.angle,
                s.crop,
                s.aspect.as_str()
            ),
            (
                entry.rotate,
                entry.flip_h,
                entry.flip_v,
                entry.angle,
                entry.crop,
                entry.aspect.as_str()
            ),
            "the geometry is put back"
        );
        // The exposure commit carried the session's crop with it: putting it back is a step.
        assert!(out.changed && out.commit, "{out:?}");
        absorb(out, &mut edit, &c);
        assert_eq!(edit.committed.exposure, 0.7);
        assert_eq!(edit.committed.crop, entry.crop);
    }

    #[test]
    fn the_aspect_list_is_the_webs() {
        let labels: Vec<&str> = ASPECTS.iter().map(|(_, l)| *l).collect();
        assert_eq!(
            labels,
            [
                "Original", "Free", "1:1", "4:5", "5:4", "2:3", "3:2", "3:4", "4:3", "9:16",
                "16:9", "5:7", "8×10", "12×18"
            ]
        );
        assert!(ASPECTS.contains(&("8:10", "8×10")));
        // A photo edited on the web as 8:10 lights its own button, and only that one.
        let lit: Vec<&str> = ASPECTS
            .iter()
            .filter(|(v, _)| aspect_lit(v, "8:10"))
            .map(|(v, _)| *v)
            .collect();
        assert_eq!(lit, ["8:10"]);
        // 5:4 is listed now: it lights itself, not 4:5.
        assert!(aspect_lit("5:4", "5:4") && !aspect_lit("4:5", "5:4"));
        // A switched shape that is not listed (7:5) lights its listed twin.
        assert!(aspect_lit("5:7", "7:5"));
        assert!(aspect_lit("12:18", "18:12"));
    }
}
