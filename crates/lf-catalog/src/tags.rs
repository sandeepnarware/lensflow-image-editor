//! AI keywords (schema v13): the words `lf_ai::keywords` suggested for each photo, kept apart
//! from the user's own keywords ([`crate::meta`]) so they never reach a sidecar or an export's
//! XMP. Search finds photos by them (`PhotoRow::ai_keywords`); the Metadata panel offers them,
//! and taking one makes it an ordinary keyword. Suggestions are for the file as it was looked
//! at (its size and modification time): a changed file reads as not looked at.

use crate::db::{Catalog, Result};
use std::path::PathBuf;

impl Catalog {
    /// Of `ids`, the photos not keyworded as their files are now, with their paths (missing
    /// files left out).
    pub fn untagged(&self, ids: &[i64]) -> Result<Vec<(i64, PathBuf)>> {
        let mut st = self.conn.prepare(
            "SELECT p.path FROM photos p WHERE p.id = ?1 AND p.missing = 0
             AND NOT EXISTS (SELECT 1 FROM tag_scans s WHERE s.photo_id = p.id
                 AND s.file_size = p.file_size AND s.mtime = p.mtime)",
        )?;
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

    /// Records `words` (with how sure, 0..1) as photo `id`'s suggestions, for its file as it
    /// is now (replacing older ones).
    pub fn set_ai_keywords(&self, id: i64, words: &[(String, f32)]) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute("DELETE FROM ai_keywords WHERE photo_id = ?1", [id])?;
        {
            let mut st = tx.prepare(
                "INSERT OR REPLACE INTO ai_keywords (photo_id, keyword, score)
                 VALUES (?1, ?2, ?3)",
            )?;
            for (word, score) in words {
                st.execute(rusqlite::params![id, word, f64::from(*score)])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO tag_scans (photo_id, file_size, mtime)
             SELECT id, file_size, mtime FROM photos WHERE id = ?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every suggested word and how many photos have it, most first.
    pub fn ai_keyword_counts(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self.conn.prepare(
            "SELECT keyword, COUNT(*) FROM ai_keywords GROUP BY keyword
             ORDER BY COUNT(*) DESC, keyword",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewPhoto;

    #[test]
    fn suggestions_are_kept_apart_searched_and_tied_to_the_file() {
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
        assert_eq!(c.untagged(&[id]).unwrap().len(), 1);
        c.set_ai_keywords(id, &[("beach".into(), 0.6), ("sea".into(), 0.2)])
            .unwrap();
        assert!(c.untagged(&[id]).unwrap().is_empty());
        let row = c.photo(id).unwrap().unwrap();
        assert_eq!(row.ai_keywords, vec!["beach".to_owned(), "sea".to_owned()]);
        assert!(row.meta.keywords.is_empty(), "not the user's keywords");
        assert!(crate::meta::row_matches(&row, "Beach"));
        assert_eq!(
            c.ai_keyword_counts().unwrap(),
            vec![("beach".to_owned(), 1), ("sea".to_owned(), 1)]
        );
        // Looked at again: replaced.
        c.set_ai_keywords(id, &[("sunset".into(), 0.5)]).unwrap();
        assert_eq!(c.photo(id).unwrap().unwrap().ai_keywords, vec!["sunset"]);
        c.conn
            .execute("UPDATE photos SET mtime = 6 WHERE id = ?1", [id])
            .unwrap();
        assert_eq!(c.untagged(&[id]).unwrap().len(), 1);
    }
}
