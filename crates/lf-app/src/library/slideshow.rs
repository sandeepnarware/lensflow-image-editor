//! View → Slideshow: the selected photos (or every photo the grid shows), one at a time over
//! the whole window on black, each rendered with its edits on the `Open` lane (as Compare
//! renders, [`super::compare::render`]); the next one is rendered while this one shows.
//! It moves on every [`INTERVAL`]; Space pauses, the arrow keys step, Esc (or a click) ends.

use super::compare::{Pixels, render};
use super::{Env, Library};
use crate::catalog_ext::count;
use crate::jobs::{Lane, Msg};
use eframe::egui::{self, Color32, Key};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long each photo shows.
pub const INTERVAL: Duration = Duration::from_secs(4);
pub const PAUSED: &str = "Paused";
pub const KEYS: &str = "Space pauses · arrows step · Esc ends";

/// A slide's picture: rendered, then a texture once the egui context is at hand.
enum Slide {
    Waiting,
    Pending(Box<Pixels>),
    Shown(egui::TextureHandle),
    Failed(String),
}

pub struct Slideshow {
    pub ids: Vec<i64>,
    pub index: usize,
    pub playing: bool,
    /// When the current photo started showing (the clock starts once it has a picture).
    since: Option<Instant>,
    slides: HashMap<i64, Slide>,
    generation: u64,
}

impl Slideshow {
    pub fn new(ids: Vec<i64>, generation: u64) -> Slideshow {
        Slideshow {
            ids,
            index: 0,
            playing: true,
            since: None,
            slides: HashMap::new(),
            generation,
        }
    }

    /// Moves `by` photos (wrapping round), restarting the clock.
    pub fn step(&mut self, by: isize) {
        let n = self.ids.len() as isize;
        if n > 0 {
            self.index = (self.index as isize + by).rem_euclid(n) as usize;
        }
        self.since = None;
    }

    /// The photos to have rendered now: this one and the next.
    pub fn wanted(&self) -> Vec<i64> {
        let n = self.ids.len();
        if n == 0 {
            return Vec::new();
        }
        let mut v = vec![self.ids[self.index]];
        if n > 1 {
            v.push(self.ids[(self.index + 1) % n]);
        }
        v
    }

    /// Whether the clock says the next photo is due at `now` (playing, and this one shown for
    /// [`INTERVAL`]).
    pub fn due(&self, now: Instant) -> bool {
        self.playing
            && self
                .since
                .is_some_and(|t| now.duration_since(t) >= INTERVAL)
    }
}

impl Library {
    /// The slideshow's place: photo `index` (0-based) of `count`, while it runs.
    pub fn slideshow_position(&self) -> Option<(usize, usize)> {
        self.slideshow.as_ref().map(|s| (s.index, s.ids.len()))
    }

    pub fn slideshow_open(&self) -> bool {
        self.slideshow.is_some()
    }

    /// View → Slideshow: the selection when several photos are selected, else the grid's.
    pub(super) fn open_slideshow(&mut self, env: &mut Env) {
        let ids = if self.selection.current.len() > 1 {
            self.selected_in_order()
        } else {
            self.order.clone()
        };
        if ids.is_empty() {
            return;
        }
        self.compare_generation += 1;
        self.slideshow = Some(Slideshow::new(ids, self.compare_generation));
        self.request_slides(env);
    }

    /// Asks the `Open` lane for the slides wanted and not yet asked for.
    fn request_slides(&mut self, env: &mut Env) {
        let (Some(show), Some(root)) = (&mut self.slideshow, self.root.clone()) else {
            return;
        };
        let quality = crate::develop::open::quality(env.prefs.develop_quality);
        for id in show.wanted() {
            if show.slides.contains_key(&id) {
                continue;
            }
            let Some(path) = self
                .photos
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.path.clone())
            else {
                continue;
            };
            show.slides.insert(id, Slide::Waiting);
            let (gpu, cache, root) = (self.gpu.clone(), self.frames.clone(), root.clone());
            let generation = show.generation;
            env.jobs.spawn(Lane::Open, "slide", move || {
                let result = render(&gpu, &root, id, &path, cache.as_deref(), quality)
                    .map(|p| Box::new(Pixels(p)));
                Msg::SlideRendered {
                    generation,
                    id,
                    result,
                }
            });
        }
        // Slides far behind are let go (memory stays bounded on a long show).
        let keep = show.wanted();
        let current = show.ids.get(show.index).copied();
        show.slides
            .retain(|id, _| keep.contains(id) || Some(*id) == current);
    }

    pub(super) fn slide_rendered(
        &mut self,
        generation: u64,
        id: i64,
        result: Result<Box<Pixels>, String>,
    ) {
        if let Some(show) = self
            .slideshow
            .as_mut()
            .filter(|s| s.generation == generation)
        {
            show.slides.insert(
                id,
                match result {
                    Ok(p) => Slide::Pending(p),
                    Err(e) => Slide::Failed(e),
                },
            );
        }
    }

    /// The slideshow over the whole window.
    pub(super) fn slideshow_ui(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(show) = &mut self.slideshow else {
            return;
        };
        let now = Instant::now();
        let (esc, space, left, right, click) = ctx.input(|i| {
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Space),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.pointer.primary_clicked(),
            )
        });
        if esc || click {
            self.slideshow = None;
            return;
        }
        if space {
            show.playing = !show.playing;
            show.since = Some(now);
        }
        let mut moved = false;
        if left {
            show.step(-1);
            moved = true;
        }
        if right || show.due(now) {
            show.step(1);
            moved = true;
        }
        let id = show.ids[show.index];
        // A rendered slide becomes a texture.
        if let Some(Slide::Pending(_)) = show.slides.get(&id)
            && let Some(Slide::Pending(p)) = show.slides.remove(&id)
        {
            let img = &p.0;
            let pixels = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            let texture =
                ctx.load_texture(format!("slide-{id}"), pixels, egui::TextureOptions::LINEAR);
            show.slides.insert(id, Slide::Shown(texture));
        }
        if show.since.is_none()
            && matches!(
                show.slides.get(&id),
                Some(Slide::Shown(_) | Slide::Failed(_))
            )
        {
            show.since = Some(now);
        }
        let caption = format!("{} / {}", show.index + 1, count(show.ids.len()));
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("slideshow"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.painter().rect_filled(screen, 0.0, Color32::BLACK);
                match show.slides.get(&id) {
                    Some(Slide::Shown(t)) => {
                        let size = t.size_vec2();
                        let k = (screen.width() / size.x).min(screen.height() / size.y);
                        let rect = egui::Rect::from_center_size(screen.center(), size * k);
                        ui.painter().image(
                            t.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    Some(Slide::Failed(e)) => {
                        ui.painter().text(
                            screen.center(),
                            egui::Align2::CENTER_CENTER,
                            e,
                            egui::FontId::proportional(16.0),
                            Color32::GRAY,
                        );
                    }
                    _ => {}
                }
                let note = if show.playing {
                    caption
                } else {
                    format!("{caption}  ·  {PAUSED}")
                };
                ui.painter().text(
                    screen.center_bottom() + egui::vec2(0.0, -16.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("{note}  ·  {KEYS}"),
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(150),
                );
                ui.allocate_rect(screen, egui::Sense::click());
            });
        if let Some(since) = show.since.filter(|_| show.playing) {
            ctx.request_repaint_after(INTERVAL.saturating_sub(now.duration_since(since)));
        }
        if moved {
            self.request_slides(env);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_steps_round_and_wants_this_photo_and_the_next() {
        let mut s = Slideshow::new(vec![10, 20, 30], 1);
        assert_eq!(s.wanted(), vec![10, 20]);
        s.step(-1);
        assert_eq!(s.index, 2);
        assert_eq!(s.wanted(), vec![30, 10]);
        s.step(1);
        assert_eq!(s.index, 0);
        let one = Slideshow::new(vec![5], 1);
        assert_eq!(one.wanted(), vec![5]);
    }

    #[test]
    fn the_clock_runs_only_once_a_photo_shows_and_while_playing() {
        let mut s = Slideshow::new(vec![1, 2], 1);
        let t = Instant::now();
        assert!(!s.due(t + INTERVAL * 2), "nothing shown yet");
        s.since = Some(t);
        assert!(!s.due(t + INTERVAL / 2));
        assert!(s.due(t + INTERVAL));
        s.playing = false;
        assert!(!s.due(t + INTERVAL * 2));
    }
}
