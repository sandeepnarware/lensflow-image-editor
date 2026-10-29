//! The Library's side of "Tag from GPX track…": the dialog, its file picker, and the tagging
//! on the `Io` lane ([`lf_catalog::gpx`]); a toast says how many photos were placed.

use super::import_ui::{Chosen, Purpose, on_own_thread};
use super::{Env, Library};
use crate::dialogs::geotag::{GeotagAction, GeotagDialog};
use crate::jobs::{Lane, Msg};
use crate::toast::Level;
use eframe::egui;
use std::path::PathBuf;

pub const NO_TRACK: &str = "The file holds no track points with times.";

/// "5 of 8 photos placed from the track."
pub fn placed_text(placed: usize, of: usize) -> String {
    format!(
        "{} of {} placed from the track.",
        crate::catalog_ext::count(placed),
        crate::catalog_ext::count(of)
    )
}

impl Library {
    /// The selection (several photos), else every photo of the folder.
    pub(super) fn open_geotag(&mut self) {
        let ids: Vec<i64> = if self.selection.current.len() > 1 {
            self.selected_in_order()
        } else {
            self.photos.iter().map(|p| p.id).collect()
        };
        if !ids.is_empty() {
            self.geotag = Some(GeotagDialog::new(ids));
        }
    }

    pub(super) fn gpx_chosen(&mut self, ticket: u64, path: Option<PathBuf>) {
        if let (Some(d), Some(path)) = (&mut self.geotag, path)
            && d.ticket == ticket
        {
            d.file = Some(path);
        }
    }

    pub(super) fn geotag_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.geotag else {
            return;
        };
        match dialog.show(ctx) {
            GeotagAction::None => {}
            GeotagAction::Close => self.geotag = None,
            GeotagAction::Choose => {
                dialog.ticket += 1;
                on_own_thread(env.jobs.reporter(), Purpose::GpxFile, dialog.ticket, || {
                    let picked = rfd::FileDialog::new()
                        .set_title("Choose a GPX track")
                        .add_filter("GPX tracks", &["gpx"])
                        .pick_file();
                    Ok(picked.map(|path| Chosen {
                        path,
                        network: false,
                    }))
                });
            }
            GeotagAction::Tag {
                file,
                offset_minutes,
                only_missing,
            } => {
                let ids = dialog.ids.clone();
                let of = ids.len();
                self.geotag = None;
                let work: Vec<(i64, String)> = ids
                    .iter()
                    .filter_map(|id| self.photo(*id))
                    .filter(|p| !only_missing || p.gps.is_none())
                    .filter_map(|p| p.captured_at.clone().map(|c| (p.id, c)))
                    .collect();
                let Some(root) = self.root.clone() else {
                    return;
                };
                env.jobs.spawn(Lane::Io, "geotag", move || {
                    let result = (|| {
                        let text = std::fs::read_to_string(&file)
                            .map_err(|e| format!("The track could not be read. {e}"))?;
                        let track = lf_catalog::gpx::parse(&text);
                        if track.is_empty() {
                            return Err(NO_TRACK.to_owned());
                        }
                        let found: Vec<(i64, Option<(f64, f64)>)> = work
                            .iter()
                            .filter_map(|(id, at)| {
                                let t = lf_catalog::gpx::photo_time(at, offset_minutes)?;
                                lf_catalog::gpx::locate(&track, t).map(|p| (*id, Some(p)))
                            })
                            .collect();
                        let c = lf_catalog::Catalog::open(&root).map_err(|e| e.to_string())?;
                        c.set_gps(&found).map_err(|e| e.to_string())?;
                        Ok((found.len(), of))
                    })();
                    Msg::Geotagged { root, result }
                });
            }
        }
    }

    pub(super) fn geotagged(
        &mut self,
        root: PathBuf,
        result: Result<(usize, usize), String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok((placed, of)) => {
                env.toasts.push(Level::Success, placed_text(placed, of));
                self.reload_photos(env);
            }
            Err(e) => env.toasts.push(Level::Warning, e),
        }
    }
}
