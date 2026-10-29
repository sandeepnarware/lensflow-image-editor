//! People (schema v12): the faces found in each photo (`lf_ai::identity`), each with its
//! description (128 numbers, unit length) and a small JPEG of it, and the people they are.
//!
//! Faces become people in two ways. The user names a group of faces, which makes those faces
//! the person's by hand (`confirmed`). Then [`Catalog::regroup_faces`] runs after every scan
//! and every naming: each face not named by hand goes to the named person it is most like,
//! when it is like them enough ([`MATCH`]); the faces left are gathered into suggested
//! groups ([`SAME`]) for the user to name. A face the user took away from a person
//! (`not_person_id`) is never given back to them.
//!
//! Faces are for the file as it was scanned (its size and modification time): a changed file
//! reads as unscanned, and scanning it again replaces its faces.

use crate::db::{Catalog, CatalogError, PhotoRow, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// How alike (cosine) a face must be to a named person's faces (their mean) to be matched
/// to them. A little stricter than OpenCV's same-person threshold for SFace (0.363), since a
/// wrong match puts someone's photos under another name.
pub const MATCH: f32 = 0.42;
/// How alike a face must be to a suggested group (its mean) to join it.
pub const SAME: f32 = 0.42;
/// The most suggested groups the People list offers (the largest).
pub const MAX_GROUPS: usize = 60;
const MAX_NAME: usize = 80;

/// A face to record: where it is (fractions of the photo as shot, `[x, y, w, h]`), its
/// description and its picture (JPEG).
#[derive(Clone, Debug, PartialEq)]
pub struct NewFace {
    pub rect: [f32; 4],
    pub description: Vec<f32>,
    pub thumb: Vec<u8>,
}

/// A named person, or a suggested group of faces not yet named (known by its first face's id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Person(i64),
    Unnamed(i64),
}

/// One entry of the People list.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonEntry {
    pub group: Group,
    /// `None` for a suggested group.
    pub name: Option<String>,
    /// Photos with this person in them.
    pub photos: u32,
    /// A picture of one of their faces (JPEG): the largest one named by hand, else the largest.
    pub thumb: Vec<u8>,
}

fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    (!name.is_empty()).then_some(name)
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `sum` scaled to unit length.
fn unit(sum: &[f32]) -> Vec<f32> {
    let n = dot(sum, sum).sqrt().max(1e-12);
    sum.iter().map(|x| x / n).collect()
}

/// Gathers unit `descriptions` into groups of one person each: each joins the group whose
/// mean it is most like when that is at least `same`, else starts one; then each is moved to
/// the group it is now most like (one pass, so early faces are not stuck where they started).
/// Answers each description's group, numbered from 0 by size (largest first).
pub fn gather(descriptions: &[&[f32]], same: f32) -> Vec<usize> {
    let mut sums: Vec<Vec<f32>> = Vec::new();
    let mut means: Vec<Vec<f32>> = Vec::new();
    let mut of = Vec::with_capacity(descriptions.len());
    let best = |means: &[Vec<f32>], d: &[f32]| {
        means
            .iter()
            .enumerate()
            .map(|(g, m)| (g, dot(m, d)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    };
    for d in descriptions {
        match best(&means, d) {
            Some((g, s)) if s >= same => {
                sums[g].iter_mut().zip(*d).for_each(|(a, b)| *a += b);
                means[g] = unit(&sums[g]);
                of.push(g);
            }
            _ => {
                sums.push(d.to_vec());
                means.push(d.to_vec());
                of.push(means.len() - 1);
            }
        }
    }
    // One pass of moving each face to the group it is most like now.
    for (i, d) in descriptions.iter().enumerate() {
        if let Some((g, s)) = best(&means, d)
            && s >= same
        {
            of[i] = g;
        }
    }
    // Numbered by size, largest first (ties by first face, so the answer is stable).
    let mut sizes: HashMap<usize, (usize, usize)> = HashMap::new();
    for (i, &g) in of.iter().enumerate() {
        let e = sizes.entry(g).or_insert((0, i));
        e.0 += 1;
    }
    let mut order: Vec<(usize, (usize, usize))> = sizes.into_iter().collect();
    order.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    let renumber: HashMap<usize, usize> = order
        .iter()
        .enumerate()
        .map(|(n, (g, _))| (*g, n))
        .collect();
    of.iter().map(|g| renumber[g]).collect()
}

/// One face as regrouping reads it.
struct Stored {
    id: i64,
    photo: i64,
    person: Option<i64>,
    confirmed: bool,
    not_person: Option<i64>,
    description: Vec<f32>,
}

impl Catalog {
    /// Of `ids`, the photos not scanned for faces as their files are now, with their paths
    /// (missing files left out).
    pub fn unscanned_for_faces(&self, ids: &[i64]) -> Result<Vec<(i64, PathBuf)>> {
        let mut st = self.conn.prepare(
            "SELECT p.path FROM photos p WHERE p.id = ?1 AND p.missing = 0
             AND NOT EXISTS (SELECT 1 FROM face_scans s WHERE s.photo_id = p.id
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

    /// Records `faces` as photo `id`'s, for its file as it is now (replacing what an older
    /// scan found). Call [`Catalog::regroup_faces`] once the scan is done.
    pub fn set_faces(&self, id: i64, faces: &[NewFace]) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute("DELETE FROM faces WHERE photo_id = ?1", [id])?;
        {
            let mut st = tx.prepare(
                "INSERT INTO faces (photo_id, x, y, w, h, description, thumb)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for f in faces {
                let [x, y, w, h] = f.rect.map(f64::from);
                st.execute(rusqlite::params![
                    id,
                    x,
                    y,
                    w,
                    h,
                    to_blob(&f.description),
                    f.thumb
                ])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO face_scans (photo_id, file_size, mtime)
             SELECT id, file_size, mtime FROM photos WHERE id = ?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Matches every face not named by hand to a named person, or gathers it into a
    /// suggested group (the module's doc says how).
    pub fn regroup_faces(&self) -> Result<()> {
        let tx = self.write_tx()?;
        let faces: Vec<Stored> = {
            let mut st = tx.prepare(
                "SELECT id, photo_id, person_id, confirmed, not_person_id, description
                 FROM faces ORDER BY photo_id, id",
            )?;
            st.query_map([], |r| {
                Ok(Stored {
                    id: r.get(0)?,
                    photo: r.get(1)?,
                    person: r.get(2)?,
                    confirmed: r.get(3)?,
                    not_person: r.get(4)?,
                    description: from_blob(&r.get::<_, Vec<u8>>(5)?),
                })
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        // Each named person's mean, from the faces named by hand.
        let mut sums: HashMap<i64, Vec<f32>> = HashMap::new();
        for f in faces.iter().filter(|f| f.confirmed) {
            if let Some(p) = f.person {
                let sum = sums
                    .entry(p)
                    .or_insert_with(|| vec![0.0; f.description.len()]);
                sum.iter_mut()
                    .zip(&f.description)
                    .for_each(|(a, b)| *a += b);
            }
        }
        let means: Vec<(i64, Vec<f32>)> = sums.into_iter().map(|(p, s)| (p, unit(&s))).collect();
        let mut matched: Vec<(i64, Option<i64>)> = Vec::new();
        let mut left: Vec<&Stored> = Vec::new();
        for f in faces.iter().filter(|f| !f.confirmed) {
            let best = means
                .iter()
                .filter(|(p, _)| Some(*p) != f.not_person)
                .map(|(p, m)| (*p, dot(m, &f.description)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((p, s)) if s >= MATCH => matched.push((f.id, Some(p))),
                _ => left.push(f),
            }
        }
        let groups = gather(
            &left
                .iter()
                .map(|f| f.description.as_slice())
                .collect::<Vec<_>>(),
            SAME,
        );
        // A group in one photo only is not offered (one face, or a crowd of look-alikes).
        // A group is known by its first face, so naming one group leaves the others' keys
        // as the People list last read them.
        let mut photos_in: HashMap<usize, std::collections::HashSet<i64>> = HashMap::new();
        let mut key: HashMap<usize, i64> = HashMap::new();
        for (f, &g) in left.iter().zip(&groups) {
            photos_in.entry(g).or_default().insert(f.photo);
            key.entry(g)
                .and_modify(|k| *k = (*k).min(f.id))
                .or_insert(f.id);
        }
        {
            let mut person = tx.prepare(
                "UPDATE faces SET person_id = ?2, grp = NULL WHERE id = ?1 AND confirmed = 0",
            )?;
            for (id, p) in &matched {
                person.execute(rusqlite::params![id, p])?;
            }
            let mut group = tx.prepare(
                "UPDATE faces SET person_id = NULL, grp = ?2 WHERE id = ?1 AND confirmed = 0",
            )?;
            for (f, &g) in left.iter().zip(&groups) {
                let offered = photos_in[&g].len() >= 2;
                group.execute(rusqlite::params![f.id, offered.then_some(key[&g])])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The People list: named people by name, then the suggested groups, largest first.
    pub fn people(&self) -> Result<Vec<PersonEntry>> {
        let mut out = Vec::new();
        let mut st = self.conn.prepare(
            "SELECT n.id, n.name,
                 (SELECT COUNT(DISTINCT f.photo_id) FROM faces f WHERE f.person_id = n.id),
                 (SELECT f.thumb FROM faces f WHERE f.person_id = n.id
                     ORDER BY f.confirmed DESC, f.w * f.h DESC LIMIT 1)
             FROM people n ORDER BY n.name",
        )?;
        let named = st.query_map([], |r| {
            Ok(PersonEntry {
                group: Group::Person(r.get(0)?),
                name: Some(r.get(1)?),
                photos: r.get(2)?,
                thumb: r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
            })
        })?;
        for p in named {
            out.push(p?);
        }
        let mut st = self.conn.prepare(
            "SELECT f.grp, COUNT(DISTINCT f.photo_id),
                 (SELECT g.thumb FROM faces g WHERE g.grp = f.grp AND g.person_id IS NULL
                     ORDER BY g.w * g.h DESC LIMIT 1)
             FROM faces f WHERE f.person_id IS NULL AND f.grp IS NOT NULL
             GROUP BY f.grp ORDER BY COUNT(DISTINCT f.photo_id) DESC, f.grp LIMIT ?1",
        )?;
        let unnamed = st.query_map([MAX_GROUPS as i64], |r| {
            Ok(PersonEntry {
                group: Group::Unnamed(r.get(0)?),
                name: None,
                photos: r.get(1)?,
                thumb: r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
            })
        })?;
        for p in unnamed {
            out.push(p?);
        }
        Ok(out)
    }

    /// The photos `group` is in, by capture time.
    pub fn group_photos(&self, group: Group) -> Result<Vec<PhotoRow>> {
        let (column, id) = match group {
            Group::Person(id) => ("person_id", id),
            Group::Unnamed(g) => ("grp", g),
        };
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM photos p WHERE EXISTS (SELECT 1 FROM faces f
                 WHERE f.photo_id = p.id AND f.{column} = ?1
                 AND (?2 OR f.person_id IS NULL))
             ORDER BY p.captured_at IS NULL, p.captured_at, p.path",
            crate::db::PHOTO_COLUMNS
        ))?;
        let rows = st.query_map(
            rusqlite::params![id, matches!(group, Group::Person(_))],
            crate::db::photo_row,
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Names `group`'s faces `name` (by hand): a new person, or one already called that (case
    /// ignored), whom they join. Renaming a person to another's name merges the two. Answers
    /// the person, and regroups.
    pub fn name_group(&self, group: Group, name: &str) -> Result<i64> {
        let name = clean_name(name)
            .ok_or_else(|| CatalogError::NotFound("A person needs a name.".to_owned()))?;
        let tx = self.write_tx()?;
        let existing: Option<i64> = tx
            .query_row("SELECT id FROM people WHERE name = ?1", [&name], |r| {
                r.get(0)
            })
            .ok();
        let person = match (group, existing) {
            (Group::Person(p), Some(e)) if e != p => e,
            (Group::Unnamed(_), Some(e)) => e,
            // Renaming a person keeps them (and their id); the same name in another case is
            // a rename too.
            (Group::Person(p), _) => {
                tx.execute(
                    "UPDATE people SET name = ?2 WHERE id = ?1",
                    rusqlite::params![p, name],
                )?;
                p
            }
            (Group::Unnamed(_), None) => {
                tx.execute(
                    "INSERT INTO people (name, created_at) VALUES (?1, ?2)",
                    rusqlite::params![name, crate::model::ts(crate::model::now())],
                )?;
                tx.last_insert_rowid()
            }
        };
        match group {
            Group::Person(p) if p != person => {
                tx.execute(
                    "UPDATE faces SET person_id = ?2 WHERE person_id = ?1",
                    [p, person],
                )?;
                tx.execute(
                    "UPDATE faces SET not_person_id = ?2 WHERE not_person_id = ?1",
                    [p, person],
                )?;
                tx.execute("DELETE FROM people WHERE id = ?1", [p])?;
            }
            Group::Person(_) => {}
            Group::Unnamed(g) => {
                tx.execute(
                    "UPDATE faces SET person_id = ?2, confirmed = 1, grp = NULL
                     WHERE grp = ?1 AND person_id IS NULL",
                    [g, person],
                )?;
            }
        }
        // Every face the person has is now theirs by hand: naming says the group is right.
        tx.execute(
            "UPDATE faces SET confirmed = 1, grp = NULL WHERE person_id = ?1",
            [person],
        )?;
        tx.commit()?;
        self.regroup_faces()?;
        Ok(person)
    }

    /// Forgets the person `id`: their faces go back to being unnamed, and regroup.
    pub fn delete_person(&self, id: i64) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE faces SET person_id = NULL, confirmed = 0 WHERE person_id = ?1",
            [id],
        )?;
        tx.execute(
            "UPDATE faces SET not_person_id = NULL WHERE not_person_id = ?1",
            [id],
        )?;
        tx.execute("DELETE FROM people WHERE id = ?1", [id])?;
        tx.commit()?;
        self.regroup_faces()
    }

    /// Takes the person `id`'s faces out of the photos `ids` (they are someone else), so they
    /// are never matched to them again. Answers how many photos changed, and regroups.
    pub fn not_this_person(&self, id: i64, ids: &[i64]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut changed = 0;
        {
            let mut st = tx.prepare(
                "UPDATE faces SET person_id = NULL, confirmed = 0, not_person_id = ?1, grp = NULL
                 WHERE person_id = ?1 AND photo_id = ?2",
            )?;
            for &photo in ids {
                changed += usize::from(st.execute([id, photo])? > 0);
            }
        }
        tx.commit()?;
        self.regroup_faces()?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewPhoto;

    /// A unit description pointing mostly along axis `axis`, nudged by `nudge` along the next.
    fn desc(axis: usize, nudge: f32) -> Vec<f32> {
        let mut v = vec![0.0; 8];
        v[axis] = 1.0;
        v[(axis + 1) % 8] = nudge;
        unit(&v)
    }

    fn face(axis: usize, nudge: f32) -> NewFace {
        NewFace {
            rect: [0.1, 0.1, 0.2, 0.2],
            description: desc(axis, nudge),
            thumb: vec![axis as u8],
        }
    }

    fn catalog_with(n: usize) -> (tempfile::TempDir, Catalog, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/shoot").unwrap();
        let ids = (0..n)
            .map(|i| {
                c.add_photo(
                    f,
                    &NewPhoto {
                        path: format!("D:/shoot/{i}.jpg"),
                        kind: "jpeg".into(),
                        file_size: 10,
                        mtime: 5,
                        captured_at: Some(format!("2026-10-0{}T10:00:00", i + 1)),
                        ..NewPhoto::default()
                    },
                )
                .unwrap()
            })
            .collect();
        (dir, c, ids)
    }

    #[test]
    fn gather_keeps_people_apart_and_numbers_groups_by_size() {
        let (a1, a2, a3) = (desc(0, 0.1), desc(0, 0.2), desc(0, -0.1));
        let (b1, b2) = (desc(3, 0.1), desc(3, 0.0));
        let all: Vec<&[f32]> = vec![&b1, &a1, &b2, &a2, &a3];
        assert_eq!(gather(&all, SAME), vec![1, 0, 1, 0, 0]);
        assert!(gather(&[], SAME).is_empty());
    }

    #[test]
    fn scans_are_kept_for_the_file_as_scanned() {
        let (_d, c, ids) = catalog_with(1);
        assert_eq!(c.unscanned_for_faces(&ids).unwrap().len(), 1);
        c.set_faces(ids[0], &[face(0, 0.0)]).unwrap();
        assert!(c.unscanned_for_faces(&ids).unwrap().is_empty());
        c.conn
            .execute("UPDATE photos SET mtime = 6 WHERE id = ?1", [ids[0]])
            .unwrap();
        assert_eq!(c.unscanned_for_faces(&ids).unwrap().len(), 1);
        // Scanning again replaces the faces.
        c.set_faces(ids[0], &[face(1, 0.0), face(2, 0.0)]).unwrap();
        let n: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM faces", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn groups_are_suggested_named_and_matched() {
        let (_d, c, ids) = catalog_with(4);
        // Anna (axis 0) in photos 0, 1, 2; Ben (axis 4) in 1 and 3; a stranger in 3 only.
        c.set_faces(ids[0], &[face(0, 0.1)]).unwrap();
        c.set_faces(ids[1], &[face(0, 0.2), face(4, 0.1)]).unwrap();
        c.set_faces(ids[2], &[face(0, -0.1)]).unwrap();
        c.set_faces(ids[3], &[face(4, 0.0), face(6, 0.0)]).unwrap();
        c.regroup_faces().unwrap();
        let list = c.people().unwrap();
        assert_eq!(list.len(), 2, "the stranger is in one photo only: {list:?}");
        assert_eq!(list[0].group, Group::Unnamed(1));
        assert_eq!(list[0].photos, 3);
        assert_eq!(list[0].thumb, vec![0]);
        assert_eq!(list[1].photos, 2);
        let anna_photos: Vec<i64> = c
            .group_photos(list[0].group)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(anna_photos, vec![ids[0], ids[1], ids[2]]);

        let anna = c.name_group(list[0].group, " Anna ").unwrap();
        let list = c.people().unwrap();
        assert_eq!(list[0].name.as_deref(), Some("Anna"));
        assert_eq!(list[0].group, Group::Person(anna));
        assert_eq!(list[0].photos, 3);
        // Search finds her by name.
        let row = c.photo(ids[1]).unwrap().unwrap();
        assert_eq!(row.people, vec!["Anna".to_owned()]);
        assert!(crate::meta::row_matches(&row, "anna"));

        // A new photo of Anna is matched to her when scanned.
        let f = c.add_folder("D:/more").unwrap();
        let new = c
            .add_photo(
                f,
                &NewPhoto {
                    path: "D:/more/x.jpg".into(),
                    kind: "jpeg".into(),
                    ..NewPhoto::default()
                },
            )
            .unwrap();
        c.set_faces(new, &[face(0, 0.15)]).unwrap();
        c.regroup_faces().unwrap();
        assert_eq!(c.group_photos(Group::Person(anna)).unwrap().len(), 4);

        // Not Anna after all: taken out, and never matched to her again.
        assert_eq!(c.not_this_person(anna, &[new]).unwrap(), 1);
        c.regroup_faces().unwrap();
        assert_eq!(c.group_photos(Group::Person(anna)).unwrap().len(), 3);
    }

    #[test]
    fn naming_a_person_after_another_merges_them_and_deleting_forgets() {
        let (_d, c, ids) = catalog_with(4);
        c.set_faces(ids[0], &[face(0, 0.0)]).unwrap();
        c.set_faces(ids[1], &[face(0, 0.1)]).unwrap();
        c.set_faces(ids[2], &[face(4, 0.0)]).unwrap();
        c.set_faces(ids[3], &[face(4, 0.1)]).unwrap();
        c.regroup_faces().unwrap();
        let groups: Vec<Group> = c.people().unwrap().iter().map(|p| p.group).collect();
        let a = c.name_group(groups[0], "Anna").unwrap();
        let b = c.name_group(groups[1], "Annie").unwrap();
        // Renaming keeps the person.
        assert_eq!(c.name_group(Group::Person(b), "Anne").unwrap(), b);
        // Annie and Anna were the same person all along.
        assert_eq!(c.name_group(Group::Person(b), "anna").unwrap(), a);
        let list = c.people().unwrap();
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].photos, 4);
        assert!(matches!(
            c.name_group(Group::Person(a), "  "),
            Err(CatalogError::NotFound(_))
        ));
        c.delete_person(a).unwrap();
        let list = c.people().unwrap();
        assert!(list.iter().all(|p| p.name.is_none()), "{list:?}");
        assert_eq!(list.len(), 2, "back to two suggested groups");
    }
}
