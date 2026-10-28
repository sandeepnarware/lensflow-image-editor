//! Auto-stacking on the `Long` lane: each photo not fingerprinted yet is rendered small as
//! shot and hashed ([`lf_catalog::stacks::dhash`]), then the photos are grouped into bursts
//! and duplicates ([`lf_catalog::stacks::group`]) and each group becomes a collapsed stack,
//! the sharpest scored photo on top. Photos already in a stack are left where they are.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lf_catalog::Catalog;
use lf_catalog::stacks::{self, Candidate, Grouping};
use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_settings::EditSettings;

/// The edge photos are fingerprinted at.
const HASH_EDGE: u32 = 256;

/// What auto-stacking made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutoStacked {
    pub stacks: usize,
    pub photos: usize,
    /// Photos whose files could not be read (left out).
    pub unreadable: usize,
}

impl AutoStacked {
    /// What to tell the user.
    pub fn text(&self) -> String {
        let made = match (self.stacks, self.photos) {
            (0, _) => "No bursts or duplicates were found.".to_owned(),
            (1, p) => format!("Made 1 stack of {p} photos."),
            (s, p) => format!(
                "Made {} stacks from {} photos.",
                crate::catalog_ext::count(s),
                crate::catalog_ext::count(p)
            ),
        };
        match self.unreadable {
            0 => made,
            1 => format!("{made} 1 photo could not be read."),
            n => format!(
                "{made} {} photos could not be read.",
                crate::catalog_ext::count(n)
            ),
        }
    }
}

/// Seconds since the epoch of a stored capture time (`YYYY-MM-DDTHH:MM:SS`).
pub fn seconds(captured_at: &str) -> Option<f64> {
    chrono::NaiveDateTime::parse_from_str(captured_at.get(..19)?, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|t| t.and_utc().timestamp() as f64)
}

/// Stacks the bursts and duplicates among `ids` (in capture order).
pub fn run(
    gpu: Arc<Gpu>,
    root: &Path,
    ids: &[i64],
    how: Grouping,
    mut progress: impl FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<AutoStacked, String> {
    let catalog = Catalog::open(root).map_err(|e| e.to_string())?;
    let todo = catalog.unhashed(ids).map_err(|e| e.to_string())?;
    let as_shot = EditSettings::default_settings();
    let mut done = AutoStacked::default();
    for (i, (id, path)) in todo.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err(lf_ai::AiError::Stopped.to_string());
        }
        progress(i, todo.len());
        match crate::ai_job::render_at(&gpu, path, &as_shot, HASH_EDGE, Quality::Preview) {
            Ok(img) => {
                let hash = stacks::dhash(&lf_ai::cull::luma(&img), img.w, img.h);
                catalog.set_hash(*id, hash).map_err(|e| e.to_string())?;
            }
            Err(e) => {
                log::warn!("auto-stack could not read {}: {e}", path.display());
                done.unreadable += 1;
            }
        }
    }
    progress(todo.len(), todo.len());
    let hashes = catalog.hashes(ids).map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    for &id in ids {
        let (Some(row), Some(&hash)) = (
            catalog.photo(id).map_err(|e| e.to_string())?,
            hashes.get(&id),
        ) else {
            continue;
        };
        if row.stack.is_some() {
            continue;
        }
        candidates.push(Candidate {
            id,
            taken: row.captured_at.as_deref().and_then(seconds),
            hash,
            sharpness: row.scores.map(|s| s.sharpness),
        });
    }
    for group in stacks::group(&candidates, how) {
        if catalog
            .stack(&group, group[0])
            .map_err(|e| e.to_string())?
            .is_some()
        {
            done.stacks += 1;
            done.photos += group.len();
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_times_read_as_seconds() {
        assert_eq!(
            seconds("2026-10-28T10:00:02").unwrap() - seconds("2026-10-28T10:00:00").unwrap(),
            2.0
        );
        assert_eq!(seconds("2026-10-28"), None);
        assert_eq!(seconds("garbage-garbage-garbage"), None);
    }

    #[test]
    fn the_result_reads_well() {
        assert_eq!(
            AutoStacked::default().text(),
            "No bursts or duplicates were found."
        );
        let one = AutoStacked {
            stacks: 1,
            photos: 3,
            unreadable: 0,
        };
        assert_eq!(one.text(), "Made 1 stack of 3 photos.");
        let many = AutoStacked {
            stacks: 4,
            photos: 11,
            unreadable: 2,
        };
        assert_eq!(
            many.text(),
            "Made 4 stacks from 11 photos. 2 photos could not be read."
        );
    }
}
