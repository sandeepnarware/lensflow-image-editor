//! Renaming catalogued photos in a batch (PhotoLab's "Rename"): each file gets a name from a
//! pattern (the export's tokens: `{name}`, `{seq}`, `{date}`, `{camera}`), its sidecar goes
//! with it, and the catalogue follows. Only names change: a photo's content is never touched,
//! and it stays in its folder.
//!
//! [`plan`] works the new names out and refuses the whole batch when a name would land on a
//! file that is not part of it (or on that file's sidecar), or when two photos of a folder
//! would get the same name (names compared ignoring case, as Windows and macOS do). Photos
//! within the batch may take each other's names: [`run`] moves every file (and sidecar) to a
//! temporary name first and then to its new one, undoing every move if one fails, and only
//! then updates the catalogue (in one transaction; the files are moved back if that fails).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::{Catalog, CatalogError, Result};
use crate::export::{file_name, safe_stem};

/// One photo's move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub id: i64,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// The date part (`YYYY-MM-DD`) of a capture time, else "undated".
fn date_of(captured: Option<&str>) -> String {
    captured
        .and_then(|c| c.get(..10))
        .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
        .map_or_else(|| "undated".to_owned(), str::to_owned)
}

/// Where `row` goes under `pattern` as photo `seq` of `count` (same folder and extension).
pub fn new_name(row: &crate::PhotoRow, seq: usize, count: usize, pattern: &str) -> PathBuf {
    let from = &row.path;
    let stem = from
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "photo".to_owned());
    let ext = from
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let name = safe_stem(&file_name(
        pattern,
        &stem,
        seq,
        count,
        &date_of(row.captured_at.as_deref()),
        row.camera.as_deref().unwrap_or(""),
    ));
    from.with_file_name(format!("{name}{ext}"))
}

fn lower(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// The new name of each of `ids` (in the order given, `{seq}` counting from 1) under
/// `pattern`; photos whose name would not change are left out. Refused (with a sentence to
/// show) when a name is taken.
pub fn plan(catalog: &Catalog, ids: &[i64], pattern: &str) -> Result<Vec<Planned>> {
    if pattern.trim().is_empty() {
        return Err(CatalogError::NotFound(
            "Type a pattern for the new names.".to_owned(),
        ));
    }
    let mut rows = Vec::new();
    for &id in ids {
        if let Some(row) = catalog.photo(id)? {
            rows.push(row);
        }
    }
    let count = rows.len();
    let moving: HashSet<String> = rows
        .iter()
        .flat_map(|r| [lower(&r.path), lower(&crate::sidecar::path_for(&r.path))])
        .collect();
    let mut taken: HashMap<String, PathBuf> = HashMap::new();
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let from = &row.path;
        let to = new_name(row, i + 1, count, pattern);
        if let Some(other) = taken.insert(lower(&to), from.clone()) {
            return Err(CatalogError::NotFound(format!(
                "Two photos would both be called {}: {} and {}. Add {{seq}} to the pattern.",
                display_name(&to),
                display_name(&other),
                display_name(from)
            )));
        }
        if lower(&to) == lower(from) {
            if to != *from {
                // Only the case changes: still a move.
                out.push(Planned {
                    id: row.id,
                    from: from.clone(),
                    to,
                });
            }
            continue;
        }
        for target in [to.clone(), crate::sidecar::path_for(&to)] {
            if target.exists() && !moving.contains(&lower(&target)) {
                return Err(CatalogError::NotFound(format!(
                    "{} is already taken in its folder; nothing was renamed.",
                    display_name(&target)
                )));
            }
        }
        out.push(Planned {
            id: row.id,
            from: from.clone(),
            to,
        });
    }
    Ok(out)
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A file's move, done or to undo.
struct Move {
    from: PathBuf,
    to: PathBuf,
}

fn undo(done: &[Move]) {
    for m in done.iter().rev() {
        if let Err(e) = fs::rename(&m.to, &m.from) {
            log::error!(
                "could not move {} back to {}: {e}",
                m.to.display(),
                m.from.display()
            );
        }
    }
}

/// Does `plan` ([`plan`]'s answer): every photo and its sidecar renamed, and the catalogue's
/// paths updated. Nothing is changed when any step fails.
pub fn run(catalog: &Catalog, plan: &[Planned]) -> Result<usize> {
    let tag = std::process::id();
    let mut done: Vec<Move> = Vec::new();
    // Each file and each sidecar there is: (from, temporary, to).
    let mut steps: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        let dir = p.from.parent().unwrap_or(Path::new("."));
        steps.push((
            p.from.clone(),
            dir.join(format!(".lfrename-{tag}-{i}.tmp")),
            p.to.clone(),
        ));
        let side = crate::sidecar::path_for(&p.from);
        if side.exists() {
            steps.push((
                side,
                dir.join(format!(".lfrename-{tag}-{i}.side.tmp")),
                crate::sidecar::path_for(&p.to),
            ));
        }
    }
    let fail = |done: &[Move], what: &Path, e: std::io::Error| {
        undo(done);
        CatalogError::NotFound(format!(
            "{} could not be renamed ({e}); nothing was renamed.",
            display_name(what)
        ))
    };
    for (from, tmp, _) in &steps {
        fs::rename(from, tmp).map_err(|e| fail(&done, from, e))?;
        done.push(Move {
            from: from.clone(),
            to: tmp.clone(),
        });
    }
    for (_, tmp, to) in &steps {
        if to.exists() {
            let e = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "the name is taken");
            return Err(fail(&done, to, e));
        }
        fs::rename(tmp, to).map_err(|e| fail(&done, to, e))?;
        done.push(Move {
            from: tmp.clone(),
            to: to.clone(),
        });
    }
    let updated = (|| -> Result<usize> {
        let tx = catalog.write_tx()?;
        {
            // Through placeholder paths first, as the files went: photos of the batch may take
            // each other's names, and a path is unique.
            let mut st = tx.prepare("UPDATE photos SET path = ?2 WHERE id = ?1")?;
            for p in plan {
                st.execute(rusqlite::params![p.id, format!("\u{1}rename:{}", p.id)])?;
            }
            for p in plan {
                st.execute(rusqlite::params![p.id, p.to.to_string_lossy()])?;
            }
        }
        tx.commit()?;
        Ok(plan.len())
    })();
    if updated.is_err() {
        undo(&done);
    }
    updated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewPhoto;

    fn catalog_with(dir: &Path, names: &[&str]) -> (Catalog, Vec<i64>) {
        let c = Catalog::open(&dir.join("cat")).unwrap();
        let shoot = dir.join("shoot");
        fs::create_dir_all(&shoot).unwrap();
        let f = c.add_folder(&shoot.to_string_lossy()).unwrap();
        let ids = names
            .iter()
            .map(|n| {
                let path = shoot.join(n);
                fs::write(&path, n.as_bytes()).unwrap();
                c.add_photo(
                    f,
                    &NewPhoto {
                        path: path.to_string_lossy().into_owned(),
                        kind: "jpeg".into(),
                        captured_at: Some("2026-10-20T10:00:00".into()),
                        camera: Some("Test Cam".into()),
                        ..NewPhoto::default()
                    },
                )
                .unwrap()
            })
            .collect();
        (c, ids)
    }

    #[test]
    fn photos_and_sidecars_are_renamed_and_the_catalogue_follows() {
        let dir = tempfile::tempdir().unwrap();
        let (c, ids) = catalog_with(dir.path(), &["IMG_1.jpg", "IMG_2.JPG"]);
        let shoot = dir.path().join("shoot");
        fs::write(crate::sidecar::path_for(&shoot.join("IMG_1.jpg")), b"{}").unwrap();
        let plan = plan(&c, &ids, "Wedding_{date}_{seq}").unwrap();
        assert_eq!(plan[0].to, shoot.join("Wedding_2026-10-20_1.jpg"));
        assert_eq!(
            plan[1].to,
            shoot.join("Wedding_2026-10-20_2.JPG"),
            "the extension stays"
        );
        assert_eq!(run(&c, &plan).unwrap(), 2);
        assert_eq!(
            fs::read(shoot.join("Wedding_2026-10-20_1.jpg")).unwrap(),
            b"IMG_1.jpg"
        );
        assert!(crate::sidecar::path_for(&shoot.join("Wedding_2026-10-20_1.jpg")).exists());
        assert!(!shoot.join("IMG_1.jpg").exists());
        assert_eq!(
            c.photo(ids[1]).unwrap().unwrap().path,
            shoot.join("Wedding_2026-10-20_2.JPG")
        );
        // No temporary file is left.
        let left: Vec<_> = fs::read_dir(&shoot)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("lfrename"))
            .collect();
        assert!(left.is_empty());
    }

    #[test]
    fn photos_of_the_batch_may_swap_names() {
        let dir = tempfile::tempdir().unwrap();
        let (c, ids) = catalog_with(dir.path(), &["b.jpg", "a.jpg"]);
        let shoot = dir.path().join("shoot");
        // In this order: b becomes a, a becomes b.
        let p = plan(&c, &[ids[0], ids[1]], "{seq}").unwrap();
        assert_eq!(p[0].to, shoot.join("1.jpg"));
        let swap = vec![
            Planned {
                id: ids[0],
                from: shoot.join("b.jpg"),
                to: shoot.join("a.jpg"),
            },
            Planned {
                id: ids[1],
                from: shoot.join("a.jpg"),
                to: shoot.join("b.jpg"),
            },
        ];
        run(&c, &swap).unwrap();
        assert_eq!(fs::read(shoot.join("a.jpg")).unwrap(), b"b.jpg");
        assert_eq!(fs::read(shoot.join("b.jpg")).unwrap(), b"a.jpg");
        assert_eq!(c.photo(ids[0]).unwrap().unwrap().path, shoot.join("a.jpg"));
        assert_eq!(c.photo(ids[1]).unwrap().unwrap().path, shoot.join("b.jpg"));
    }

    #[test]
    fn a_name_taken_outside_the_batch_or_twice_refuses_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (c, ids) = catalog_with(dir.path(), &["x.jpg", "y.jpg"]);
        let shoot = dir.path().join("shoot");
        fs::write(shoot.join("keep.jpg"), b"other").unwrap();
        let e = plan(&c, &ids[..1], "keep").unwrap_err().to_string();
        assert!(e.contains("keep.jpg is already taken"), "{e}");
        let e = plan(&c, &ids, "same").unwrap_err().to_string();
        assert!(e.contains("Add {seq}"), "{e}");
        assert!(plan(&c, &ids, "  ").is_err());
        // Nothing moved.
        assert!(shoot.join("x.jpg").exists() && shoot.join("y.jpg").exists());
    }

    #[test]
    fn an_unchanged_name_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let (c, ids) = catalog_with(dir.path(), &["x.jpg"]);
        assert!(plan(&c, &ids, "{name}").unwrap().is_empty());
    }
}
