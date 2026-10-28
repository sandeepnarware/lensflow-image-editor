//! Assisted-culling scores (schema v10): how sharp each photo is, its faces, how many have
//! their eyes closed and how sharp the main face's eyes are, as `lf_ai::cull` measured them.
//! A score is for the file as it was scored (its size and modification time): a changed file
//! reads as unscored. Scores only suggest; the flags stay the user's.

use crate::db::{Catalog, Result};
use std::path::PathBuf;

/// Below this sharpness (0..1) a photo reads as blurry.
pub const BLURRY: f32 = 0.35;
/// Below this, the main face's eyes read as out of focus (the photo counts as blurry).
pub const SOFT_EYES: f32 = 0.3;

/// One photo's scores.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scores {
    pub sharpness: f32,
    pub faces: u32,
    pub eyes_closed: u32,
    pub eye_sharpness: Option<f32>,
}

impl Scores {
    /// The eyes of the main face are out of focus.
    pub fn soft_eyes(&self) -> bool {
        self.eye_sharpness.is_some_and(|e| e < SOFT_EYES)
    }

    /// Blurred, or focused off the main face's eyes.
    pub fn blurry(&self) -> bool {
        self.sharpness < BLURRY || self.soft_eyes()
    }

    /// Someone in it has their eyes closed.
    pub fn eyes_closed(&self) -> bool {
        self.eyes_closed > 0
    }

    /// Looks like a reject: blurry or eyes closed.
    pub fn issue(&self) -> bool {
        self.blurry() || self.eyes_closed()
    }

    /// What is wrong, for a cell's hover text ("Blurry, eyes closed"); `None` when nothing.
    pub fn describe(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.sharpness < BLURRY {
            parts.push("Blurry".to_owned());
        } else if self.soft_eyes() {
            parts.push("Eyes out of focus".to_owned());
        }
        match self.eyes_closed {
            0 => {}
            1 if self.faces == 1 => parts.push("Eyes closed".to_owned()),
            1 => parts.push("1 person's eyes closed".to_owned()),
            n => parts.push(format!("{n} people's eyes closed")),
        }
        (!parts.is_empty()).then(|| {
            let mut s = parts.join(", ");
            // Only the first word keeps its capital.
            if parts.len() > 1 {
                let (first, rest) = s.split_at(parts[0].len());
                s = format!("{first}{}", rest.to_lowercase());
            }
            s
        })
    }
}

/// Where a photo's fresh scores are (`p` is the photo; `PHOTO_COLUMNS` says it again).
const FRESH: &str =
    "FROM scores s WHERE s.photo_id = p.id AND s.file_size = p.file_size AND s.mtime = p.mtime";

impl Catalog {
    /// Of `ids`, the photos with no score for their file as it is now, with their paths
    /// (missing files left out).
    pub fn unscored(&self, ids: &[i64]) -> Result<Vec<(i64, PathBuf)>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT p.path FROM photos p WHERE p.id = ?1 AND p.missing = 0
             AND NOT EXISTS (SELECT 1 {FRESH})"
        ))?;
        let mut out = Vec::new();
        for &id in ids {
            if let Some(path) = st
                .query_map([id], |r| r.get::<_, String>(0))?
                .next()
                .transpose()?
            {
                out.push((id, PathBuf::from(path)));
            }
        }
        Ok(out)
    }

    /// Records `scores` for photo `id`'s file as it is now (replacing any older ones).
    pub fn set_scores(&self, id: i64, scores: &Scores) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO scores
                 (photo_id, file_size, mtime, sharpness, faces, eyes_closed, eye_sharpness)
             SELECT id, file_size, mtime, ?2, ?3, ?4, ?5 FROM photos WHERE id = ?1",
            rusqlite::params![
                id,
                f64::from(scores.sharpness),
                scores.faces,
                scores.eyes_closed,
                scores.eye_sharpness.map(f64::from),
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewPhoto;

    #[test]
    fn scores_are_kept_for_the_file_as_scored() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/shoot").unwrap();
        let id = c
            .add_photo(
                f,
                &NewPhoto {
                    path: "D:/shoot/a.jpg".into(),
                    kind: "jpeg".into(),
                    file_size: 10,
                    mtime: 5,
                    ..NewPhoto::default()
                },
            )
            .unwrap();
        assert_eq!(c.unscored(&[id]).unwrap().len(), 1);
        assert_eq!(c.photo(id).unwrap().unwrap().scores, None);
        let s = Scores {
            sharpness: 0.2,
            faces: 2,
            eyes_closed: 1,
            eye_sharpness: Some(0.8),
        };
        c.set_scores(id, &s).unwrap();
        assert!(c.unscored(&[id]).unwrap().is_empty());
        assert_eq!(c.photo(id).unwrap().unwrap().scores, Some(s));
        // The file changed: its scores no longer count.
        c.conn
            .execute("UPDATE photos SET mtime = 6 WHERE id = ?1", [id])
            .unwrap();
        assert_eq!(c.unscored(&[id]).unwrap().len(), 1);
        assert_eq!(c.photo(id).unwrap().unwrap().scores, None);
    }

    #[test]
    fn verdicts_and_words() {
        let mut s = Scores {
            sharpness: 0.9,
            faces: 1,
            ..Scores::default()
        };
        assert!(!s.issue());
        assert_eq!(s.describe(), None);
        s.eye_sharpness = Some(0.1);
        assert!(s.blurry());
        assert_eq!(s.describe().as_deref(), Some("Eyes out of focus"));
        s.eyes_closed = 1;
        assert_eq!(
            s.describe().as_deref(),
            Some("Eyes out of focus, eyes closed")
        );
        s.sharpness = 0.1;
        s.faces = 3;
        s.eyes_closed = 2;
        assert_eq!(
            s.describe().as_deref(),
            Some("Blurry, 2 people's eyes closed")
        );
    }
}
