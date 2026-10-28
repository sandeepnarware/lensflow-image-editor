//! Importing from a memory card (or any folder): the photos are *copied* into a folder of
//! the studio's choosing, optionally renamed and copied a second time to a backup folder,
//! and the copies are then catalogued like any added folder. The source is only read.
//!
//! [`plan`] reads each file's capture time and camera (headers only, as [`crate::import`]
//! does) and marks suspected duplicates, as Lightroom's "Don't import suspected duplicates"
//! does: a catalogued photo of the same size taken at the same second (so a photo imported
//! before under another name is still found), or, for a file with no capture time, of the
//! same name and size. [`run`] copies: each copy
//! is written under a temporary name in the destination and renamed when complete, so a
//! pulled card or a full disk never leaves a half-copied photo under a real name; a name
//! already taken in the destination gets `-2`, `-3`, … (a file there with the same name and
//! size is the same photo, and is skipped as a duplicate when duplicates are skipped).

use std::collections::HashSet;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::db::{Catalog, Result};
use crate::export::{file_name, safe_stem};
use crate::import::Candidate;

/// A file on the card, read and judged.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub from: PathBuf,
    pub size: u64,
    /// The capture date (`YYYY-MM-DD`), else the file's modified date.
    pub date: String,
    pub camera: Option<String>,
    /// A catalogued photo has the same size and capture time (or, with no capture time,
    /// the same file name and size).
    pub duplicate: bool,
}

/// What to do with the planned files.
#[derive(Clone, Debug, PartialEq)]
pub struct IngestSpec {
    /// The folder the copies go into (made if needed).
    pub destination: PathBuf,
    /// A rename pattern (the export's tokens: `{name}`, `{seq}`, `{date}`, `{camera}`);
    /// `None` keeps the card's names.
    pub rename: Option<String>,
    /// A second copy of each photo goes into this folder too.
    pub backup: Option<PathBuf>,
    /// Suspected duplicates are not copied.
    pub skip_duplicates: bool,
}

/// What a finished (or stopped) import from a card did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IngestOutcome {
    pub copied: usize,
    pub duplicates: usize,
    /// File name and why.
    pub failed: Vec<(String, String)>,
    /// Photos copied whose backup copy could not be made.
    pub backup_failed: usize,
    pub cancelled: bool,
}

/// The date part (`YYYY-MM-DD`) of a capture time, if it is one.
fn date_part(captured: &str) -> Option<String> {
    let d = captured.get(..10)?;
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|_| d.to_owned())
}

/// The local date of a modified time (milliseconds since the epoch).
fn mtime_date(mtime: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(mtime)
        .single()
        .map_or_else(
            || "undated".to_owned(),
            |t| t.format("%Y-%m-%d").to_string(),
        )
}

/// Reads each candidate's capture time and camera and marks those the catalogue already has
/// (see the module's notes).
pub fn plan(catalog: &Catalog, candidates: &[Candidate]) -> Result<Vec<Planned>> {
    let mut by_name = HashSet::new();
    let mut by_time = HashSet::new();
    {
        let mut st = catalog
            .conn
            .prepare("SELECT path, file_size, captured_at FROM photos")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (path, size, captured) = row?;
            if let Some(name) = Path::new(&path).file_name() {
                by_name.insert((name.to_string_lossy().to_lowercase(), size as u64));
            }
            if let Some(t) = captured {
                by_time.insert((t, size as u64));
            }
        }
    }
    Ok(candidates
        .iter()
        .map(|c| {
            let (captured, camera) = match lf_raw::open::summary(&c.path) {
                Ok((_, exif, _)) => (exif.captured_at, exif.model.or(exif.make)),
                Err(_) => (None, None),
            };
            let date = captured.as_deref().and_then(date_part);
            let name = c
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            Planned {
                from: c.path.clone(),
                size: c.size,
                date: date.unwrap_or_else(|| mtime_date(c.mtime)),
                camera,
                duplicate: match &captured {
                    Some(t) => by_time.contains(&(t.clone(), c.size)),
                    None => by_name.contains(&(name, c.size)),
                },
            }
        })
        .collect())
}

/// The name each planned file gets in `spec.destination` (`None`: skipped as a duplicate),
/// with names already in the destination, or given earlier in the list, avoided.
pub fn target_names(spec: &IngestSpec, plan: &[Planned]) -> Vec<Option<String>> {
    let kept = plan
        .iter()
        .filter(|p| !(spec.skip_duplicates && p.duplicate))
        .count();
    let mut taken: HashSet<String> = HashSet::new();
    let mut seq = 0;
    plan.iter()
        .map(|p| {
            if spec.skip_duplicates && p.duplicate {
                return None;
            }
            seq += 1;
            let stem = p
                .from
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "photo".to_owned());
            let ext = p
                .from
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let base = match &spec.rename {
                Some(pattern) => safe_stem(&file_name(
                    pattern,
                    &stem,
                    seq,
                    kept,
                    &p.date,
                    p.camera.as_deref().unwrap_or(""),
                )),
                None => stem,
            };
            let mut n = 1;
            loop {
                let candidate = if n == 1 {
                    format!("{base}{ext}")
                } else {
                    format!("{base}-{n}{ext}")
                };
                let at = spec.destination.join(&candidate);
                if !taken.contains(&candidate.to_lowercase()) {
                    match fs::metadata(&at) {
                        // The same photo copied before: skipped when duplicates are.
                        Ok(m) if spec.skip_duplicates && m.len() == p.size => return None,
                        Ok(_) => {}
                        Err(_) => {
                            taken.insert(candidate.to_lowercase());
                            return Some(candidate);
                        }
                    }
                }
                n += 1;
            }
        })
        .collect()
}

/// Copies `from` to `dir/name` through a temporary name, checking the size; never replaces
/// a file.
fn copy_into(from: &Path, dir: &Path, name: &str, size: u64) -> std::result::Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("The folder could not be made ({e})."))?;
    let to = dir.join(name);
    if to.exists() {
        return Err("A file with that name is already there.".to_owned());
    }
    let tmp = dir.join(format!(".lfingest-{name}.{}.tmp", std::process::id()));
    let result = fs::copy(from, &tmp)
        .map_err(|e| format!("It could not be copied ({e})."))
        .and_then(|n| {
            if n == size {
                Ok(())
            } else {
                Err("The copy came out the wrong size; the card may have been removed.".into())
            }
        })
        .and_then(|()| {
            fs::rename(&tmp, &to).map_err(|e| format!("The copy could not be named ({e})."))
        });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Copies the planned files (with `names` from [`target_names`]) into the destination and the
/// backup. `progress(done, total)` after each; a `Break` stops after the current file.
pub fn run(
    spec: &IngestSpec,
    plan: &[Planned],
    names: &[Option<String>],
    progress: &mut dyn FnMut(usize, usize) -> ControlFlow<()>,
) -> IngestOutcome {
    let mut out = IngestOutcome::default();
    let total = plan.len();
    for (i, (p, name)) in plan.iter().zip(names).enumerate() {
        match name {
            None => out.duplicates += 1,
            Some(name) => match copy_into(&p.from, &spec.destination, name, p.size) {
                Ok(()) => {
                    out.copied += 1;
                    if let Some(backup) = &spec.backup
                        && let Err(e) = copy_into(&p.from, backup, name, p.size)
                    {
                        log::warn!("backup of {name} failed: {e}");
                        out.backup_failed += 1;
                    }
                }
                Err(e) => {
                    log::warn!("copying {} failed: {e}", p.from.display());
                    out.failed.push((name.clone(), e));
                }
            },
        }
        if progress(i + 1, total).is_break() && i + 1 < total {
            out.cancelled = true;
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(path: &str, size: u64, duplicate: bool) -> Planned {
        Planned {
            from: PathBuf::from(path),
            size,
            date: "2026-10-28".into(),
            camera: Some("Z 6".into()),
            duplicate,
        }
    }

    #[test]
    fn names_follow_the_pattern_and_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DSC_0001.NEF"), b"other photo").unwrap();
        let plan = vec![
            planned("/card/DSC_0001.NEF", 4, false),
            planned("/card/DSC_0002.NEF", 4, true),
            planned("/card/sub/DSC_0001.NEF", 4, false),
        ];
        let mut spec = IngestSpec {
            destination: dir.path().to_path_buf(),
            rename: None,
            backup: None,
            skip_duplicates: true,
        };
        assert_eq!(
            target_names(&spec, &plan),
            [
                Some("DSC_0001-2.NEF".to_owned()),
                None,
                Some("DSC_0001-3.NEF".to_owned())
            ]
        );
        spec.rename = Some("{date}_{camera}_{seq}".into());
        spec.skip_duplicates = false;
        assert_eq!(
            target_names(&spec, &plan),
            [
                Some("2026-10-28_Z-6_1.NEF".to_owned()),
                Some("2026-10-28_Z-6_2.NEF".to_owned()),
                Some("2026-10-28_Z-6_3.NEF".to_owned())
            ]
        );
    }

    #[test]
    fn a_run_copies_backs_up_and_skips_what_is_already_there() {
        let card = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        std::fs::write(card.path().join("a.jpg"), b"aaaa").unwrap();
        std::fs::write(card.path().join("b.jpg"), b"bbbbbb").unwrap();
        let plan = vec![
            planned(&card.path().join("a.jpg").to_string_lossy(), 4, false),
            planned(&card.path().join("b.jpg").to_string_lossy(), 6, false),
        ];
        let spec = IngestSpec {
            destination: dest.path().join("2026-10-28"),
            rename: None,
            backup: Some(backup.path().join("2026-10-28")),
            skip_duplicates: true,
        };
        let names = target_names(&spec, &plan);
        let out = run(&spec, &plan, &names, &mut |_, _| ControlFlow::Continue(()));
        assert_eq!((out.copied, out.duplicates, out.backup_failed), (2, 0, 0));
        assert_eq!(
            std::fs::read(dest.path().join("2026-10-28/b.jpg")).unwrap(),
            b"bbbbbb"
        );
        assert!(backup.path().join("2026-10-28/a.jpg").exists());
        // Nothing temporary is left behind.
        assert_eq!(
            std::fs::read_dir(dest.path().join("2026-10-28"))
                .unwrap()
                .count(),
            2
        );
        // The same card again: both copies are there already (same name and size).
        let names = target_names(&spec, &plan);
        assert_eq!(names, [None, None]);
        let again = run(&spec, &plan, &names, &mut |_, _| ControlFlow::Continue(()));
        assert_eq!((again.copied, again.duplicates), (0, 2));
    }

    #[test]
    fn a_missing_file_fails_alone_and_a_break_stops() {
        let card = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(card.path().join("b.jpg"), b"bb").unwrap();
        std::fs::write(card.path().join("c.jpg"), b"cc").unwrap();
        let plan = vec![
            planned(&card.path().join("gone.jpg").to_string_lossy(), 2, false),
            planned(&card.path().join("b.jpg").to_string_lossy(), 2, false),
            planned(&card.path().join("c.jpg").to_string_lossy(), 2, false),
        ];
        let spec = IngestSpec {
            destination: dest.path().to_path_buf(),
            rename: None,
            backup: None,
            skip_duplicates: false,
        };
        let names = target_names(&spec, &plan);
        let out = run(&spec, &plan, &names, &mut |done, _| {
            if done == 2 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(out.failed.len(), 1);
        assert_eq!(out.failed[0].0, "gone.jpg");
        assert_eq!(out.copied, 1);
        assert!(out.cancelled);
        assert!(!dest.path().join("c.jpg").exists());
    }

    #[test]
    fn a_file_the_catalogue_has_by_name_and_size_is_a_suspected_duplicate() {
        let d = tempfile::tempdir().unwrap();
        let c = Catalog::open(d.path()).unwrap();
        let f = c.add_folder("/shoot").unwrap();
        c.add_photo(
            f,
            &crate::NewPhoto {
                path: "/shoot/DSC_0001.jpg".into(),
                file_size: 4,
                kind: "jpeg".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let candidate = |name: &str, size: u64| Candidate {
            path: PathBuf::from(format!("/card/{name}")),
            size,
            mtime: 0,
        };
        let plan = plan(
            &c,
            &[candidate("dsc_0001.JPG", 4), candidate("DSC_0001.jpg", 5)],
        )
        .unwrap();
        assert!(plan[0].duplicate, "same name in another case, same size");
        assert!(!plan[1].duplicate, "another size is another photo");
        assert_eq!(
            plan[0].date.len(),
            10,
            "the file's date when it has no capture time"
        );
    }

    #[test]
    fn a_renamed_earlier_import_is_found_by_its_size_and_capture_time() {
        // A JPEG with a capture time in its EXIF.
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&image::RgbImage::new(8, 8))
            .unwrap();
        let exif = lf_raw::exif::ExifSummary {
            captured_at: Some("2024-05-01T12:34:56".into()),
            orientation: 1,
            ..Default::default()
        };
        let app1 = crate::exif_embed::build_exif(&exif, false).unwrap();
        let jpeg = crate::exif_embed::insert_exif(&jpeg, &app1);
        let card = tempfile::tempdir().unwrap();
        std::fs::write(card.path().join("DSC_0009.JPG"), &jpeg).unwrap();

        let d = tempfile::tempdir().unwrap();
        let c = Catalog::open(d.path()).unwrap();
        let f = c.add_folder("/shoot").unwrap();
        c.add_photo(
            f,
            &crate::NewPhoto {
                path: "/shoot/sharma_0001.jpg".into(),
                file_size: jpeg.len() as i64,
                captured_at: Some("2024-05-01T12:34:56".into()),
                kind: "jpeg".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let scan = crate::import::scan(card.path());
        let plan = plan(&c, &scan.candidates).unwrap();
        assert_eq!(plan[0].date, "2024-05-01");
        assert!(plan[0].duplicate, "found under its new name");
    }

    #[test]
    fn dates_come_from_the_capture_time_or_else_the_file() {
        assert_eq!(
            date_part("2024-05-01T12:34:56").as_deref(),
            Some("2024-05-01")
        );
        assert_eq!(date_part("garbage"), None);
        assert_eq!(mtime_date(0).len(), 10);
    }
}
