//! The Map view (View → Map): where the folder's or album's photos were taken, on
//! OpenStreetMap. Each photo with a place (EXIF GPS, [`lf_catalog::places`]) is a pin; pins
//! closer than a few points are drawn as one with a count. A click on a pin selects its
//! photos; the wheel zooms about the pointer and a drag pans.
//!
//! Tiles come from `tile.openstreetmap.org` (its usage policy: an identifying User-Agent, a
//! visible credit, and tiles cached), each on its own short-lived thread, at most
//! [`MAX_FETCHES`] at a time, and are kept on disk in `<cache>/tiles` so a place seen once
//! needs no network again. Photos catalogued before the map existed are read for their
//! place the first time the map shows them.

use super::{Env, Library};
use crate::jobs::{Lane, Msg};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const TILE: f64 = 256.0;
pub const MIN_ZOOM: u8 = 2;
pub const MAX_ZOOM: u8 = 18;
/// The most tiles fetched at once.
pub const MAX_FETCHES: usize = 6;
pub const CREDIT: &str = "© OpenStreetMap contributors";
pub const NO_PLACES: &str = "None of these photos says where it was taken (no GPS in its EXIF).";
pub const BACK: &str = "Back to grid";
const USER_AGENT: &str = concat!(
    "lensflow-ImageEditor/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/sandeepnarware/lensflow-image-editor)"
);

/// A tile's address: zoom, column, row.
pub type TileKey = (u8, u32, u32);

/// A downloaded tile's bytes (a PNG).
pub struct TileBytes(pub Vec<u8>);

impl std::fmt::Debug for TileBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TileBytes({} bytes)", self.0.len())
    }
}

/// `(lat, lon)` in world pixels at `zoom` (Web Mercator).
pub fn project(lat: f64, lon: f64, zoom: f64) -> (f64, f64) {
    let size = TILE * 2f64.powf(zoom);
    let lat = lat.clamp(-85.051_128, 85.051_128).to_radians();
    let x = (lon + 180.0) / 360.0 * size;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * size;
    (x, y)
}

/// The zoom and centre (world pixels at that zoom) that show every place in `view`.
pub fn fit(places: &[(f64, f64)], view: Vec2) -> (u8, (f64, f64)) {
    if places.is_empty() {
        return (MIN_ZOOM, project(20.0, 0.0, f64::from(MIN_ZOOM)));
    }
    for z in (MIN_ZOOM..=15).rev() {
        let pts: Vec<(f64, f64)> = places
            .iter()
            .map(|&(a, b)| project(a, b, f64::from(z)))
            .collect();
        let (x0, x1) = pts
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = pts
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        if x1 - x0 <= f64::from(view.x) * 0.8 && y1 - y0 <= f64::from(view.y) * 0.8 {
            return (z, ((x0 + x1) / 2.0, (y0 + y1) / 2.0));
        }
    }
    let p = project(places[0].0, places[0].1, f64::from(MIN_ZOOM));
    (MIN_ZOOM, p)
}

/// Pins within `reach` points of each other, grouped: each group's centre and photos.
pub fn cluster(points: &[(i64, Pos2)], reach: f32) -> Vec<(Pos2, Vec<i64>)> {
    let mut out: Vec<(Pos2, Vec<i64>)> = Vec::new();
    for &(id, p) in points {
        match out.iter_mut().find(|(c, _)| c.distance(p) <= reach) {
            Some((c, ids)) => {
                let n = ids.len() as f32;
                *c = Pos2::new((c.x * n + p.x) / (n + 1.0), (c.y * n + p.y) / (n + 1.0));
                ids.push(id);
            }
            None => out.push((p, vec![id])),
        }
    }
    out
}

/// The map's state: where it looks, its tiles, and which are on their way.
pub struct MapView {
    pub zoom: u8,
    /// The view's centre in world pixels at `zoom`.
    pub centre: (f64, f64),
    fitted: bool,
    tiles: HashMap<TileKey, egui::TextureHandle>,
    pending: HashMap<TileKey, TileBytes>,
    asked: HashSet<TileKey>,
    failed: HashSet<TileKey>,
    dir: PathBuf,
}

impl MapView {
    pub fn new(cache: &Path) -> MapView {
        MapView {
            zoom: MIN_ZOOM,
            centre: project(20.0, 0.0, f64::from(MIN_ZOOM)),
            fitted: false,
            tiles: HashMap::new(),
            pending: HashMap::new(),
            asked: HashSet::new(),
            failed: HashSet::new(),
            dir: cache.join("tiles"),
        }
    }

    pub fn tile_arrived(&mut self, key: TileKey, result: Result<TileBytes, String>) {
        self.asked.remove(&key);
        match result {
            Ok(bytes) => {
                self.pending.insert(key, bytes);
            }
            Err(e) => {
                log::warn!("map tile {key:?}: {e}");
                self.failed.insert(key);
            }
        }
    }

    fn zoom_about(&mut self, to: u8, anchor: Vec2) {
        let to = to.clamp(MIN_ZOOM, MAX_ZOOM);
        if to == self.zoom {
            return;
        }
        let k = 2f64.powi(i32::from(to) - i32::from(self.zoom));
        // The world point under the anchor stays under it.
        let (ax, ay) = (f64::from(anchor.x), f64::from(anchor.y));
        self.centre = ((self.centre.0 + ax) * k - ax, (self.centre.1 + ay) * k - ay);
        self.zoom = to;
    }
}

/// Fetches tile `key` (from the disk cache, else the network, then kept on disk).
fn fetch(dir: &Path, key: TileKey) -> Result<Vec<u8>, String> {
    let (z, x, y) = key;
    let path = dir.join(format!("{z}/{x}/{y}.png"));
    if let Ok(bytes) = std::fs::read(&path) {
        return Ok(bytes);
    }
    let url = format!("https://tile.openstreetmap.org/{z}/{x}/{y}.png");
    let bytes = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .and_then(|c| c.get(&url).send())
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(|r| r.bytes())
        .map_err(|e| e.to_string())?
        .to_vec();
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    Ok(bytes)
}

impl Library {
    pub fn mapping(&self) -> bool {
        self.map.is_some() && self.mode == super::Mode::Library
    }

    /// View → Map: the map of what is shown; photos not yet read for their place are read
    /// (headers only) on the `Query` lane.
    pub(super) fn open_map(&mut self, env: &mut Env) {
        let cache = self
            .frames_dir
            .parent()
            .map_or_else(|| self.frames_dir.clone(), Path::to_path_buf);
        self.compare = None;
        self.map = Some(MapView::new(&cache));
        let Some(root) = self.root.clone() else {
            return;
        };
        let ids = self.order.clone();
        env.jobs.spawn(Lane::Query, "places", move || {
            let result = lf_catalog::Catalog::open(&root)
                .and_then(|c| c.fill_gps(&ids))
                .map_err(|e| e.to_string());
            Msg::PlacesRead { root, result }
        });
    }

    pub(super) fn places_read(
        &mut self,
        root: PathBuf,
        result: Result<usize, String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok(0) => {}
            Ok(n) => {
                log::info!("{n} photos gained a place");
                if let Some(map) = &mut self.map {
                    map.fitted = false;
                }
                self.reload_photos(env);
            }
            Err(e) => log::warn!("could not read the photos' places: {e}"),
        }
    }

    pub(super) fn map_ui(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        let reporter = env.jobs.reporter();
        let places: Vec<(i64, (f64, f64))> = self
            .order
            .iter()
            .filter_map(|id| self.photo(*id).and_then(|p| p.gps.map(|g| (*id, g))))
            .collect();
        let selected: HashSet<i64> = self.selection.current.iter().copied().collect();
        let names: HashMap<i64, String> = places
            .iter()
            .filter_map(|(id, _)| {
                self.photo(*id)
                    .map(|p| (*id, crate::develop::open::file_name(&p.path)))
            })
            .collect();
        let Some(map) = &mut self.map else {
            return;
        };
        let mut back = false;
        ui.horizontal(|ui| {
            if ui.button(format!("⏴ {BACK}")).clicked() {
                back = true;
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} of {} photos have a place",
                    places.len(),
                    self.order.len()
                ))
                .color(crate::theme::MUTED)
                .small(),
            );
        });
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        if !map.fitted && !places.is_empty() {
            let pts: Vec<(f64, f64)> = places.iter().map(|(_, g)| *g).collect();
            (map.zoom, map.centre) = fit(&pts, area.size());
            map.fitted = true;
        }
        // Pan and zoom.
        if response.dragged() {
            let d = response.drag_delta();
            map.centre.0 -= f64::from(d.x);
            map.centre.1 -= f64::from(d.y);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 20.0 {
                let anchor = response
                    .hover_pos()
                    .map_or(Vec2::ZERO, |p| p - area.center());
                let to = if scroll > 0.0 {
                    map.zoom.saturating_add(1)
                } else {
                    map.zoom.saturating_sub(1)
                };
                map.zoom_about(to, anchor);
            }
        }
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::from_gray(200));
        // Tiles: the ones in view, drawn when here, asked for when not.
        let z = map.zoom;
        let n = 1u32 << z;
        let left = map.centre.0 - f64::from(area.width()) / 2.0;
        let top = map.centre.1 - f64::from(area.height()) / 2.0;
        let (tx0, ty0) = ((left / TILE).floor() as i64, (top / TILE).floor() as i64);
        let (tx1, ty1) = (
            ((left + f64::from(area.width())) / TILE).floor() as i64,
            ((top + f64::from(area.height())) / TILE).floor() as i64,
        );
        for ty in ty0.max(0)..=ty1.min(i64::from(n) - 1) {
            for tx in tx0..=tx1 {
                let key = (z, tx.rem_euclid(i64::from(n)) as u32, ty as u32);
                if let Some(bytes) = map.pending.remove(&key) {
                    match image::load_from_memory(&bytes.0) {
                        Ok(img) => {
                            let img = img.to_rgba8();
                            let colour = egui::ColorImage::from_rgba_unmultiplied(
                                [img.width() as usize, img.height() as usize],
                                img.as_raw(),
                            );
                            map.tiles.insert(
                                key,
                                ui.ctx().load_texture(
                                    format!("tile-{z}-{}-{}", key.1, key.2),
                                    colour,
                                    egui::TextureOptions::LINEAR,
                                ),
                            );
                        }
                        Err(_) => {
                            map.failed.insert(key);
                        }
                    }
                }
                let min = Pos2::new(
                    area.left() + (tx as f64 * TILE - left) as f32,
                    area.top() + (ty as f64 * TILE - top) as f32,
                );
                let rect = Rect::from_min_size(min, Vec2::splat(TILE as f32));
                match map.tiles.get(&key) {
                    Some(t) => {
                        painter.image(
                            t.id(),
                            rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        if !map.asked.contains(&key)
                            && !map.failed.contains(&key)
                            && map.asked.len() < MAX_FETCHES
                        {
                            map.asked.insert(key);
                            let dir = map.dir.clone();
                            let reporter = reporter.clone();
                            std::thread::spawn(move || {
                                let result = fetch(&dir, key).map(TileBytes);
                                reporter.send(Msg::Tile { key, result });
                            });
                        }
                    }
                }
            }
        }
        // Pins.
        let pins: Vec<(i64, Pos2)> = places
            .iter()
            .map(|&(id, (lat, lon))| {
                let (x, y) = project(lat, lon, f64::from(z));
                (
                    id,
                    Pos2::new(
                        area.left() + (x - left) as f32,
                        area.top() + (y - top) as f32,
                    ),
                )
            })
            .filter(|(_, p)| area.expand(20.0).contains(*p))
            .collect();
        let groups = cluster(&pins, 22.0);
        let mut picked = None;
        for (at, ids) in &groups {
            let r = if ids.len() == 1 { 7.0 } else { 11.0 };
            let on = ids.iter().any(|id| selected.contains(id));
            let fill = if on {
                crate::theme::BRAND
            } else {
                Color32::from_rgb(220, 60, 60)
            };
            painter.circle(*at, r, fill, Stroke::new(2.0, Color32::WHITE));
            if ids.len() > 1 {
                painter.text(
                    *at,
                    egui::Align2::CENTER_CENTER,
                    ids.len().to_string(),
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
            if let Some(p) = response.hover_pos()
                && p.distance(*at) <= r + 3.0
            {
                let text: Vec<&str> = ids
                    .iter()
                    .take(8)
                    .filter_map(|id| names.get(id).map(String::as_str))
                    .collect();
                response.clone().on_hover_text(text.join("\n"));
                if response.clicked() {
                    picked = Some(ids.clone());
                }
            }
        }
        if places.is_empty() {
            painter.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                NO_PLACES,
                egui::FontId::proportional(14.0),
                Color32::from_gray(40),
            );
        }
        // The credit the tiles' licence asks for.
        let credit = painter.text(
            area.right_bottom() - Vec2::new(6.0, 4.0),
            egui::Align2::RIGHT_BOTTOM,
            CREDIT,
            egui::FontId::proportional(11.0),
            Color32::from_gray(30),
        );
        painter.rect_filled(credit.expand(2.0), 2.0, Color32::from_white_alpha(170));
        painter.text(
            area.right_bottom() - Vec2::new(6.0, 4.0),
            egui::Align2::RIGHT_BOTTOM,
            CREDIT,
            egui::FontId::proportional(11.0),
            Color32::from_gray(30),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Map"));
        if let Some(ids) = picked {
            self.selection.select(&ids);
            self.reveal_active = true;
        }
        if back {
            self.map = None;
        }
    }

    pub(super) fn tile_arrived(&mut self, key: TileKey, result: Result<TileBytes, String>) {
        if let Some(map) = &mut self.map {
            map.tile_arrived(key, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_projection_puts_the_world_in_its_tiles() {
        let (x, y) = project(0.0, 0.0, 0.0);
        assert!((x - 128.0).abs() < 1e-9 && (y - 128.0).abs() < 1e-9);
        let (x, _) = project(0.0, 180.0, 1.0);
        assert!((x - 512.0).abs() < 1e-9);
        // North is up: a northern place has a smaller y.
        assert!(project(51.5, 0.0, 3.0).1 < project(-33.9, 0.0, 3.0).1);
    }

    #[test]
    fn the_fit_shows_every_place_and_close_pins_group() {
        let places = [(26.9, 75.8), (27.2, 78.0), (28.6, 77.2)];
        let (z, c) = fit(&places, Vec2::new(800.0, 600.0));
        for &(a, b) in &places {
            let p = project(a, b, f64::from(z));
            assert!((p.0 - c.0).abs() < 400.0 && (p.1 - c.1).abs() < 300.0);
        }
        assert!(z >= 5, "close enough to tell the cities apart: {z}");
        let groups = cluster(
            &[
                (1, Pos2::new(10.0, 10.0)),
                (2, Pos2::new(15.0, 12.0)),
                (3, Pos2::new(200.0, 200.0)),
            ],
            22.0,
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].1, vec![1, 2]);
    }
}
