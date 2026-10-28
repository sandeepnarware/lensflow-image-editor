//! Where photos were taken: latitude and longitude from their EXIF GPS, in the `photos` row
//! (schema v9). Import reads it with the rest of a file's facts; photos catalogued before
//! that are read when the map first asks for them ([`Catalog::fill_gps`]), header only.

use crate::db::{Catalog, Result};
use std::path::PathBuf;

impl Catalog {
    /// Of `ids`, the photos whose files have not been read for GPS yet, with their paths.
    pub fn gps_unread(&self, ids: &[i64]) -> Result<Vec<(i64, PathBuf)>> {
        let mut st = self
            .conn
            .prepare("SELECT path FROM photos WHERE id = ?1 AND gps_read = 0 AND missing = 0")?;
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

    /// Records what reading the files said: each photo's place (or none), and that it was
    /// read. Returns how many have a place.
    pub fn set_gps(&self, found: &[(i64, Option<(f64, f64)>)]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut placed = 0;
        {
            let mut st =
                tx.prepare("UPDATE photos SET lat = ?2, lon = ?3, gps_read = 1 WHERE id = ?1")?;
            for (id, gps) in found {
                st.execute(rusqlite::params![id, gps.map(|g| g.0), gps.map(|g| g.1)])?;
                placed += usize::from(gps.is_some());
            }
        }
        tx.commit()?;
        Ok(placed)
    }

    /// Reads the files of `ids` not read for GPS yet (headers only) and records it; returns
    /// how many photos gained a place. A file that cannot be read counts as read, without one.
    pub fn fill_gps(&self, ids: &[i64]) -> Result<usize> {
        let found: Vec<(i64, Option<(f64, f64)>)> = self
            .gps_unread(ids)?
            .into_iter()
            .map(|(id, path)| {
                let gps = std::panic::catch_unwind(|| lf_raw::open::summary(&path))
                    .ok()
                    .and_then(|r| r.ok())
                    .and_then(|(_, exif, _)| exif.gps);
                (id, gps)
            })
            .collect();
        if found.is_empty() {
            return Ok(0);
        }
        self.set_gps(&found)
    }
}

#[cfg(test)]
mod tests {
    use crate::Catalog;
    use crate::model::NewPhoto;

    #[test]
    fn places_are_recorded_and_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/trip").unwrap();
        let id = c
            .add_photo(
                f,
                &NewPhoto {
                    path: "D:/trip/missing.jpg".into(),
                    kind: "jpeg".into(),
                    ..NewPhoto::default()
                },
            )
            .unwrap();
        assert_eq!(c.gps_unread(&[id]).unwrap().len(), 1);
        assert_eq!(c.set_gps(&[(id, Some((26.91, 75.79)))]).unwrap(), 1);
        assert_eq!(c.photo(id).unwrap().unwrap().gps, Some((26.91, 75.79)));
        assert!(c.gps_unread(&[id]).unwrap().is_empty());
        // A file that cannot be read is read once, and has no place.
        let other = c
            .add_photo(
                f,
                &NewPhoto {
                    path: "D:/trip/gone.jpg".into(),
                    kind: "jpeg".into(),
                    ..NewPhoto::default()
                },
            )
            .unwrap();
        assert_eq!(c.fill_gps(&[other]).unwrap(), 0);
        assert!(c.gps_unread(&[other]).unwrap().is_empty());
        assert_eq!(c.photo(other).unwrap().unwrap().gps, None);
    }
}
