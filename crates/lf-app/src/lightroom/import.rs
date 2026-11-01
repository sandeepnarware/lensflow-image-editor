//! The import job: gathered preset files become the studio's presets.
//!
//! Each file is read ([`read_any`]), mapped to our settings ([`map`]), kept under the size cap
//! ([`fit_budget`]) and saved through [`presets::save`], which marks it changed so the next
//! sync sends it to LensFlow. It runs on the `Long` lane with its own catalogue connection.
//! A preset is named `"<Group> - <Name>"`; a name already taken gets ` (2)`, ` (3)`, so a
//! second import of the same pack keeps both copies instead of replacing the first (a
//! studio preset is shared with the team, and silently replacing one would lose their work).
//! The server keeps at most [`MAX_PRESETS`] per studio: past it the rest are left out and
//! listed, never half-saved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use eframe::egui;
use lf_catalog::{Catalog, presets};
use lf_settings::lightroom::budget::{desktop_fits, fit_budget};
use lf_settings::lightroom::{LrKind, LrPreset, map, read_any};

use super::gather::{Found, gather};
use crate::dialogs::save_preset::MAX_NAME;

/// The account server's limit of presets per studio (`PhotoEditPreset::MAX_PER_STUDIO`).
pub const MAX_PRESETS: usize = 2000;
/// Shown for a Lightroom profile (a "Look"), which is not a develop preset.
pub const PROFILE: &str = "This is a profile, not a preset.";
/// Shown for a preset whose settings stay over the cap even without Lightroom's copy.
pub const TOO_LARGE: &str = "This preset is too large to keep.";
/// Shown for a preset the catalogue could not save, before the catalogue's own words.
pub const NOT_SAVED: &str = "The preset could not be saved.";

/// Adobe's own folders that hold presets: a file sitting straight in one has no group.
const CONTAINER_FOLDERS: &[&str] = &["Settings", "Develop Presets"];

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// The presets saved (their names as saved), each with its ready-to-show notes.
    pub imported: Vec<(String, Vec<String>)>,
    /// `(source, reason)` for each file that could not be imported.
    pub failed: Vec<(String, String)>,
    /// The presets not saved because the studio reached [`MAX_PRESETS`] (their names).
    pub left_out: Vec<String>,
    /// Stop was pressed: how many of the files found were not processed.
    pub stopped: Option<usize>,
}

/// `wanted`, or `wanted (2)`, `wanted (3)`… : the first that is not `taken`. Names stay within
/// the server's [`MAX_NAME`] characters: a long name is cut to leave room for its number.
pub fn unique_name(wanted: &str, taken: &HashSet<String>) -> String {
    let cut = |s: &str, n: usize| -> String { s.chars().take(n).collect() };
    let wanted = cut(wanted.trim(), MAX_NAME);
    if !taken.contains(&wanted) {
        return wanted;
    }
    (2u64..)
        .map(|n| {
            let number = format!(" ({n})");
            format!(
                "{}{number}",
                cut(&wanted, MAX_NAME - number.len()).trim_end()
            )
        })
        .find(|name| !taken.contains(name))
        .unwrap_or(wanted)
}

/// `name` with every character the app's fonts cannot draw replaced by `?`: egui panics on a
/// glyph none of its fonts has, and a preset's name comes from someone else's file. The
/// preset keeps its real name; only what is drawn is changed. Needs the fonts loaded (any
/// frame after the first).
pub fn shown_name(ctx: &egui::Context, name: &str) -> String {
    let font = egui::FontId::proportional(14.0);
    ctx.fonts_mut(|f| {
        name.chars()
            .map(|c| if f.has_glyph(&font, c) { c } else { '?' })
            .collect()
    })
}

/// The name a preset is saved under, before numbering: `"<Group> - <Name>"`. The group is the
/// file's own, else the folder it sat in (not Adobe's container folders); the name is the
/// file's own, else the file name without its extension.
fn wanted_name(preset: &LrPreset, found: &Found) -> String {
    let name = match preset.name.trim() {
        "" => std::path::Path::new(&found.file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| found.file_name.clone()),
        name => name.to_owned(),
    };
    let group = preset
        .group
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .or_else(|| {
            found
                .group_hint
                .as_deref()
                .map(str::trim)
                .filter(|g| !g.is_empty() && !CONTAINER_FOLDERS.contains(g))
        });
    match group {
        Some(group) => format!("{group} - {name}"),
        None => name,
    }
}

/// How far an import is: files done of the files gathered. The job writes it, the dialog
/// reads it each frame (as the other `Long` jobs' counters).
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// Saves each of `found` as a preset of `studio_id` in `catalog`, until `stop` is set.
pub fn import(
    catalog: &Catalog,
    studio_id: i64,
    found: Vec<Found>,
    stop: &AtomicBool,
) -> ImportReport {
    import_counting(catalog, studio_id, found, stop, &Progress::default())
}

/// The studio's presets as they stand while importing.
struct Studio<'a> {
    catalog: &'a Catalog,
    id: i64,
    count: usize,
    taken: HashSet<String>,
}

/// [`import`], counting each file done in `progress`.
pub fn import_counting(
    catalog: &Catalog,
    studio_id: i64,
    found: Vec<Found>,
    stop: &AtomicBool,
    progress: &Progress,
) -> ImportReport {
    let mut report = ImportReport::default();
    let total = found.len();
    progress.total.store(total, Ordering::Relaxed);
    let existing = match presets::list(catalog, studio_id) {
        Ok(rows) => rows,
        Err(e) => {
            let reason = format!("{NOT_SAVED} {e}");
            report.failed = found
                .into_iter()
                .map(|f| (f.source, reason.clone()))
                .collect();
            progress.done.store(total, Ordering::Relaxed);
            return report;
        }
    };
    let mut studio = Studio {
        catalog,
        id: studio_id,
        count: existing.len(),
        taken: existing.into_iter().map(|r| r.name).collect(),
    };
    for (i, f) in found.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            report.stopped = Some(total - i);
            break;
        }
        import_one(&mut studio, f, &mut report);
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    report
}

/// Reads, maps and saves one file, noting the outcome in `report`.
fn import_one(studio: &mut Studio, f: Found, report: &mut ImportReport) {
    let preset = match read_any(&f.file_name, &f.bytes) {
        Ok(p) => p,
        Err(e) => {
            report.failed.push((f.source, e.to_string()));
            return;
        }
    };
    if preset.kind == LrKind::Profile {
        report.failed.push((f.source, PROFILE.to_owned()));
        return;
    }
    let wanted = wanted_name(&preset, &f);
    if studio.count >= MAX_PRESETS {
        report.left_out.push(wanted);
        return;
    }
    let mut mapped = map(&preset);
    if let Some(note) = fit_budget(&mut mapped.preset) {
        mapped.notes.push(note.to_owned());
    }
    if !desktop_fits(&mapped.preset) {
        report.failed.push((f.source, TOO_LARGE.to_owned()));
        return;
    }
    let name = unique_name(&wanted, &studio.taken);
    match presets::save(studio.catalog, studio.id, &name, &mapped.preset) {
        Ok(_) => {
            studio.count += 1;
            studio.taken.insert(name.clone());
            report.imported.push((name, mapped.notes));
        }
        Err(e) => report.failed.push((f.source, format!("{NOT_SAVED} {e}"))),
    }
}

/// The whole job: gathers `paths`, opens the catalogue at `root` (its own connection) and
/// imports into `studio_id`, counting in `progress`. Files that could not be gathered come
/// first among the failures.
pub fn run(
    root: &Path,
    studio_id: i64,
    paths: &[PathBuf],
    stop: &AtomicBool,
    progress: &Progress,
) -> ImportReport {
    let gathered = gather(paths, stop);
    let mut report = match Catalog::open(root) {
        Ok(catalog) => import_counting(&catalog, studio_id, gathered.found, stop, progress),
        Err(e) => {
            let reason = format!("{NOT_SAVED} {e}");
            ImportReport {
                failed: gathered
                    .found
                    .into_iter()
                    .map(|f| (f.source, reason.clone()))
                    .collect(),
                ..ImportReport::default()
            }
        }
    };
    report.failed.splice(0..0, gathered.failed);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn xmp(name: &str, group: Option<&str>, extra: &str) -> Vec<u8> {
        let group = group.map_or(String::new(), |g| {
            format!(
                r#"<crs:Group><rdf:Alt><rdf:li xml:lang="x-default">{g}</rdf:li></rdf:Alt></crs:Group>"#
            )
        });
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal" crs:ProcessVersion="11.0" crs:Exposure2012="+0.50" {extra}>
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name>
   {group}
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
        )
        .into_bytes()
    }

    fn found(file_name: &str, group_hint: Option<&str>, bytes: Vec<u8>) -> Found {
        Found {
            source: format!("pack.zip / {file_name}"),
            group_hint: group_hint.map(str::to_owned),
            bytes,
            file_name: file_name.to_owned(),
        }
    }

    fn catalogue() -> (tempfile::TempDir, Catalog) {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        (dir, catalog)
    }

    fn names(catalog: &Catalog, studio: i64) -> Vec<String> {
        presets::list(catalog, studio)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect()
    }

    #[test]
    fn names_get_numbered_when_taken() {
        let mut taken = HashSet::new();
        assert_eq!(unique_name("Studio - Moody", &taken), "Studio - Moody");
        taken.insert("Studio - Moody".to_owned());
        assert_eq!(unique_name("Studio - Moody", &taken), "Studio - Moody (2)");
        taken.insert("Studio - Moody (2)".to_owned());
        assert_eq!(unique_name("Studio - Moody", &taken), "Studio - Moody (3)");
    }

    #[test]
    fn long_names_stay_within_the_servers_limit() {
        let long = "x".repeat(MAX_NAME + 20);
        let mut taken = HashSet::new();
        let first = unique_name(&long, &taken);
        assert_eq!(first.chars().count(), MAX_NAME);
        taken.insert(first);
        let second = unique_name(&long, &taken);
        assert_eq!(second.chars().count(), MAX_NAME);
        assert!(second.ends_with(" (2)"), "{second}");
    }

    #[test]
    fn importing_twice_keeps_both() {
        let (_dir, catalog) = catalogue();
        let stop = AtomicBool::new(false);
        let pack = || vec![found("Moody.xmp", None, xmp("Moody", Some("Studio"), ""))];
        let first = import(&catalog, 7, pack(), &stop);
        assert_eq!(first.imported.len(), 1);
        let second = import(&catalog, 7, pack(), &stop);
        assert_eq!(second.imported[0].0, "Studio - Moody (2)");
        assert_eq!(names(&catalog, 7), ["Studio - Moody", "Studio - Moody (2)"]);
        assert!(
            names(&catalog, 8).is_empty(),
            "another studio's list is untouched"
        );
        let row = presets::find(&catalog, 7, "Studio - Moody")
            .unwrap()
            .unwrap();
        assert!(lf_settings::lightroom::is_lightroom(&row.settings));
        assert_eq!(row.settings["exposure"], json!(0.5));
    }

    #[test]
    fn groups_names_and_notes() {
        let (_dir, catalog) = catalogue();
        let stop = AtomicBool::new(false);
        let report = import(
            &catalog,
            7,
            vec![
                // The folder names the group when the file does not.
                found("a.xmp", Some("Weddings"), xmp("Soft", None, "")),
                // Adobe's own folder is no group.
                found("b.xmp", Some("Settings"), xmp("Plain", None, "")),
                // No name: the file's.
                found("Faded.xmp", None, xmp("", None, r#"crs:Mystery="1""#)),
                found("broken.xmp", None, b"not xml".to_vec()),
            ],
            &stop,
        );
        let saved: Vec<&str> = report.imported.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(saved, ["Weddings - Soft", "Plain", "Faded"]);
        assert!(report.imported[0].1.is_empty());
        assert!(
            report.imported[2].1.iter().any(|n| n.contains("Mystery")),
            "{:?}",
            report.imported[2].1
        );
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "pack.zip / broken.xmp");
    }

    #[test]
    fn the_limit_stops_the_import() {
        let (_dir, catalog) = catalogue();
        for i in 0..MAX_PRESETS - 1 {
            presets::save(&catalog, 7, &format!("P{i:04}"), &json!({"v": 2})).unwrap();
        }
        let stop = AtomicBool::new(false);
        let report = import(
            &catalog,
            7,
            vec![
                found("a.xmp", None, xmp("A", None, "")),
                found("b.xmp", None, xmp("B", None, "")),
                found("c.xmp", None, xmp("C", None, "")),
            ],
            &stop,
        );
        assert_eq!(report.imported.len(), 1);
        assert_eq!(report.left_out, ["B", "C"]);
        assert_eq!(names(&catalog, 7).len(), MAX_PRESETS);
    }

    #[test]
    fn profiles_are_reported() {
        let (_dir, catalog) = catalogue();
        let stop = AtomicBool::new(false);
        let bytes = String::from_utf8(xmp("Faded Film", None, ""))
            .unwrap()
            .replace(r#"crs:PresetType="Normal""#, r#"crs:PresetType="Look""#)
            .into_bytes();
        let report = import(&catalog, 7, vec![found("look.xmp", None, bytes)], &stop);
        assert!(report.imported.is_empty());
        assert_eq!(
            report.failed,
            [("pack.zip / look.xmp".to_owned(), PROFILE.to_owned())]
        );
    }

    #[test]
    fn stop_ends_the_import() {
        let (_dir, catalog) = catalogue();
        let stop = AtomicBool::new(true);
        let report = import(
            &catalog,
            7,
            vec![found("a.xmp", None, xmp("A", None, ""))],
            &stop,
        );
        assert!(report.imported.is_empty());
        assert_eq!(report.stopped, Some(1), "the one file was not processed");
    }

    #[test]
    fn progress_counts_each_file_and_a_full_run_is_not_stopped() {
        let (_dir, catalog) = catalogue();
        let stop = AtomicBool::new(false);
        let progress = Progress::default();
        let report = import_counting(
            &catalog,
            7,
            vec![
                found("a.xmp", None, xmp("A", None, "")),
                found("broken.xmp", None, b"no".to_vec()),
                found("c.xmp", None, xmp("C", None, "")),
            ],
            &stop,
            &progress,
        );
        assert_eq!(progress.total.load(Ordering::Relaxed), 3);
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);
        assert_eq!(report.stopped, None);
    }

    #[test]
    fn shown_name_replaces_glyphs_egui_lacks() {
        let ctx = egui::Context::default();
        // Fonts are loaded on the first frame.
        let mut frame = ctx.run_ui(egui::RawInput::default(), |_| {});
        frame.textures_delta.clear();
        let font = egui::FontId::proportional(14.0);
        let shown = shown_name(&ctx, "Studio - \u{5915}\u{713c}\u{3051}");
        assert!(shown.starts_with("Studio - "), "{shown}");
        assert!(
            shown
                .chars()
                .all(|c| ctx.fonts_mut(|f| f.has_glyph(&font, c))),
            "{shown}"
        );
        assert_eq!(shown_name(&ctx, "Café Moody"), "Café Moody");
    }
}
