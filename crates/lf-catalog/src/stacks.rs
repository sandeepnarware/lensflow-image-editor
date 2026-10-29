//! Stacks (schema v11): photos gathered under one "top" photo, as Lightroom stacks them.
//! A collapsed stack shows only its top in the grid, with how many it holds; an expanded
//! one shows them all. A photo is in at most one stack, and a stack always has at least two
//! photos (taking photos out removes a stack left with one).
//!
//! Auto-stacking ([`group`]) finds bursts (photos taken moments apart that look alike) and,
//! when asked, duplicates anywhere in the folder (photos that look the same). "Look alike"
//! is a 64-bit difference hash of the photo as shot ([`dhash`]), kept in `fingerprints` for
//! the file as it was read (its size and modification time), so it is made once.

use crate::db::{Catalog, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// A photo's stack, as a row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stack {
    pub id: i64,
    /// The photo a collapsed stack shows.
    pub top: i64,
    /// How many photos it holds.
    pub size: u32,
    pub collapsed: bool,
}

/// The difference hash of a picture's luma (`w × h`, 0..1): it is shrunk to 9 × 8 and each
/// bit says whether a pixel is brighter than its right-hand neighbour. Pictures that look
/// alike differ in few bits, whatever their size or exposure.
pub fn dhash(luma: &[f32], w: usize, h: usize) -> u64 {
    if w == 0 || h == 0 {
        return 0;
    }
    // Box-average each of the 9 × 8 cells.
    let mut cells = [[0.0f32; 9]; 8];
    for (cy, row) in cells.iter_mut().enumerate() {
        let (y0, y1) = (cy * h / 8, ((cy + 1) * h / 8).max(cy * h / 8 + 1).min(h));
        for (cx, cell) in row.iter_mut().enumerate() {
            let (x0, x1) = (cx * w / 9, ((cx + 1) * w / 9).max(cx * w / 9 + 1).min(w));
            let mut sum = 0.0;
            for y in y0..y1 {
                sum += luma[y * w + x0..y * w + x1].iter().sum::<f32>();
            }
            *cell = sum / ((y1 - y0) * (x1 - x0)).max(1) as f32;
        }
    }
    let mut bits = 0u64;
    for row in &cells {
        for x in 0..8 {
            bits = (bits << 1) | u64::from(row[x] > row[x + 1]);
        }
    }
    bits
}

/// How many bits two hashes differ in (0 the same, 64 opposite).
pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// A photo as auto-stacking sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: i64,
    /// Capture time, seconds (none: it never joins a burst).
    pub taken: Option<f64>,
    pub hash: u64,
    /// Assisted culling's sharpness, when scored (picks the top).
    pub sharpness: Option<f32>,
}

/// How auto-stacking groups.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grouping {
    /// The longest gap between two frames of a burst, seconds.
    pub burst_gap: f64,
    /// Also stack photos that look the same wherever they are in the folder.
    pub duplicates: bool,
}

impl Default for Grouping {
    fn default() -> Grouping {
        Grouping {
            burst_gap: 2.0,
            duplicates: true,
        }
    }
}

/// The most bits two frames of a burst may differ in (the subject moves between frames).
pub const BURST_DISTANCE: u32 = 16;
/// The most bits duplicates may differ in (re-saved, resized or lightly edited copies).
pub const DUPLICATE_DISTANCE: u32 = 4;
/// "Find similar": photos at most this far from the one asked about.
pub const SIMILAR_DISTANCE: u32 = 18;

/// Of `hashes`, the photos like `of` (within [`SIMILAR_DISTANCE`]), nearest first, `of` itself
/// first of all.
pub fn similar(of: i64, hashes: &HashMap<i64, u64>) -> Vec<i64> {
    let Some(&h) = hashes.get(&of) else {
        return Vec::new();
    };
    let mut near: Vec<(u32, i64)> = hashes
        .iter()
        .map(|(&id, &x)| (if id == of { 0 } else { distance(h, x).max(1) }, id))
        .filter(|(d, _)| *d <= SIMILAR_DISTANCE)
        .collect();
    near.sort();
    near.into_iter().map(|(_, id)| id).collect()
}

/// Groups `photos` (in capture order) into stacks: each group of two or more, its top first
/// (the sharpest when scored, else the first taken), the rest in capture order.
pub fn group(photos: &[Candidate], how: Grouping) -> Vec<Vec<i64>> {
    let n = photos.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let join = |p: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            p[ra.max(rb)] = ra.min(rb);
        }
    };
    for i in 1..n {
        let (a, b) = (&photos[i - 1], &photos[i]);
        if let (Some(ta), Some(tb)) = (a.taken, b.taken)
            && (tb - ta).abs() <= how.burst_gap
            && distance(a.hash, b.hash) <= BURST_DISTANCE
        {
            join(&mut parent, i - 1, i);
        }
    }
    if how.duplicates {
        for i in 0..n {
            for j in i + 1..n {
                if distance(photos[i].hash, photos[j].hash) <= DUPLICATE_DISTANCE {
                    join(&mut parent, i, j);
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    let mut out: Vec<(usize, Vec<i64>)> = groups
        .into_iter()
        .filter(|(_, members)| members.len() > 1)
        .map(|(root, members)| {
            // The sharpest scored photo; ties and unscored keep capture order.
            let top = members
                .iter()
                .copied()
                .filter(|&i| photos[i].sharpness.is_some())
                .max_by(|&a, &b| {
                    photos[a]
                        .sharpness
                        .unwrap_or(0.0)
                        .total_cmp(&photos[b].sharpness.unwrap_or(0.0))
                        .then(b.cmp(&a))
                })
                .unwrap_or(members[0]);
            let mut ids = vec![photos[top].id];
            ids.extend(members.iter().filter(|&&i| i != top).map(|&i| photos[i].id));
            (root, ids)
        })
        .collect();
    out.sort_by_key(|(root, _)| *root);
    out.into_iter().map(|(_, ids)| ids).collect()
}

/// Raw files' extensions (lower case), for pairing a raw file with the JPEG shot beside it.
pub const RAW_EXTENSIONS: [&str; 24] = [
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "iiq", "k25", "kdc", "mef", "mos",
    "mrw", "nef", "nrw", "orf", "pef", "raf", "rw2", "rwl", "sr2", "srf", "srw",
];

/// RAW+JPEG pairs among `photos` (id, path, stack id): a raw file and a JPEG with the same
/// name in the same folder (the camera's two copies of one shot), raw first, in `photos`'
/// order. Pairs already stacked together are left out.
pub fn raw_jpeg_pairs(photos: &[(i64, &std::path::Path, Option<i64>)]) -> Vec<(i64, i64)> {
    let key = |p: &std::path::Path| {
        (
            p.parent().map(|d| d.to_path_buf()),
            p.file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        )
    };
    let ext = |p: &std::path::Path| {
        p.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let mut jpegs: HashMap<_, (i64, Option<i64>)> = HashMap::new();
    for (id, path, stack) in photos {
        if matches!(ext(path).as_str(), "jpg" | "jpeg") {
            jpegs.entry(key(path)).or_insert((*id, *stack));
        }
    }
    let mut out = Vec::new();
    for (id, path, stack) in photos {
        if !RAW_EXTENSIONS.contains(&ext(path).as_str()) {
            continue;
        }
        if let Some((jpeg, jstack)) = jpegs.get(&key(path))
            && !(stack.is_some() && stack == jstack)
        {
            out.push((*id, *jpeg));
        }
    }
    out
}

impl Catalog {
    /// Puts `ids` (two or more) in a new collapsed stack topped by `top` (one of them; else
    /// the first). They leave any stack they were in. Returns the new stack's id.
    pub fn stack(&self, ids: &[i64], top: i64) -> Result<Option<i64>> {
        if ids.len() < 2 {
            return Ok(None);
        }
        let top = if ids.contains(&top) { top } else { ids[0] };
        let tx = self.write_tx()?;
        tx.execute(
            "INSERT INTO stacks (top_photo_id, collapsed) VALUES (?1, 1)",
            [top],
        )?;
        let stack = tx.last_insert_rowid();
        {
            let mut st = tx.prepare("UPDATE photos SET stack_id = ?1 WHERE id = ?2")?;
            for id in ids {
                st.execute([stack, *id])?;
            }
        }
        prune(&tx)?;
        tx.commit()?;
        Ok(Some(stack))
    }

    /// Takes `ids` out of their stacks (a stack left with one photo is removed); returns how
    /// many were in one.
    pub fn unstack(&self, ids: &[i64]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut n = 0;
        {
            let mut st = tx.prepare(
                "UPDATE photos SET stack_id = NULL WHERE id = ?1 AND stack_id IS NOT NULL",
            )?;
            for id in ids {
                n += st.execute([*id])?;
            }
        }
        prune(&tx)?;
        tx.commit()?;
        Ok(n)
    }

    /// Removes whole stacks: every photo of the stacks `ids` are in leaves it (the photos'
    /// `stack_id` is cleared by its `ON DELETE SET NULL`).
    pub fn dissolve_stacks_of(&self, ids: &[i64]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut n = 0;
        {
            let mut st = tx.prepare(
                "DELETE FROM stacks WHERE id = (SELECT stack_id FROM photos WHERE id = ?1)",
            )?;
            for id in ids {
                n += st.execute([*id])?;
            }
        }
        prune(&tx)?;
        tx.commit()?;
        Ok(n)
    }

    /// Collapses or expands stack `stack`.
    pub fn set_collapsed(&self, stack: i64, collapsed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE stacks SET collapsed = ?2 WHERE id = ?1",
            rusqlite::params![stack, collapsed],
        )?;
        Ok(())
    }

    /// Makes `photo` the top of its stack.
    pub fn set_top(&self, photo: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE stacks SET top_photo_id = ?1
             WHERE id = (SELECT stack_id FROM photos WHERE id = ?1)",
            [photo],
        )?;
        Ok(())
    }

    /// Of `ids`, the photos with no fingerprint for their file as it is now, with paths.
    pub fn unhashed(&self, ids: &[i64]) -> Result<Vec<(i64, PathBuf)>> {
        let mut st = self.conn.prepare(
            "SELECT p.path FROM photos p WHERE p.id = ?1 AND p.missing = 0
             AND NOT EXISTS (SELECT 1 FROM fingerprints f WHERE f.photo_id = p.id
                 AND f.file_size = p.file_size AND f.mtime = p.mtime)",
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

    /// Records photo `id`'s fingerprint for its file as it is now.
    pub fn set_hash(&self, id: i64, hash: u64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO fingerprints (photo_id, file_size, mtime, dhash)
             SELECT id, file_size, mtime, ?2 FROM photos WHERE id = ?1",
            // SQLite integers are signed: the bits are kept as they are.
            rusqlite::params![id, hash as i64],
        )?;
        Ok(())
    }

    /// The fresh fingerprints of `ids` (photos without one left out).
    pub fn hashes(&self, ids: &[i64]) -> Result<HashMap<i64, u64>> {
        let mut st = self.conn.prepare(
            "SELECT f.dhash FROM fingerprints f JOIN photos p ON p.id = f.photo_id
             WHERE f.photo_id = ?1 AND f.file_size = p.file_size AND f.mtime = p.mtime",
        )?;
        let mut out = HashMap::new();
        for &id in ids {
            if let Some(h) = st
                .query_map([id], |r| r.get::<_, i64>(0))?
                .next()
                .transpose()?
            {
                out.insert(id, h as u64);
            }
        }
        Ok(out)
    }
}

/// Removes stacks with fewer than two photos, and moves a top that left its stack to the
/// first photo still in it.
fn prune(tx: &rusqlite::Transaction) -> Result<()> {
    tx.execute(
        "UPDATE photos SET stack_id = NULL WHERE stack_id IN
             (SELECT stack_id FROM photos WHERE stack_id IS NOT NULL
              GROUP BY stack_id HAVING COUNT(*) < 2)",
        [],
    )?;
    tx.execute(
        "DELETE FROM stacks WHERE id NOT IN
             (SELECT stack_id FROM photos WHERE stack_id IS NOT NULL)",
        [],
    )?;
    tx.execute(
        "UPDATE stacks SET top_photo_id =
             (SELECT MIN(id) FROM photos WHERE stack_id = stacks.id)
         WHERE top_photo_id NOT IN (SELECT id FROM photos WHERE stack_id = stacks.id)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewPhoto;

    fn catalog(n: usize) -> (tempfile::TempDir, Catalog, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/burst").unwrap();
        let ids = (0..n)
            .map(|i| {
                c.add_photo(
                    f,
                    &NewPhoto {
                        path: format!("D:/burst/{i}.jpg"),
                        kind: "jpeg".into(),
                        file_size: 100,
                        mtime: 1,
                        ..NewPhoto::default()
                    },
                )
                .unwrap()
            })
            .collect();
        (dir, c, ids)
    }

    #[test]
    fn stacking_unstacking_and_tops() {
        let (_d, c, ids) = catalog(4);
        let s = c.stack(&ids[..3], ids[1]).unwrap().unwrap();
        let row = |id| c.photo(id).unwrap().unwrap().stack;
        assert_eq!(
            row(ids[0]),
            Some(Stack {
                id: s,
                top: ids[1],
                size: 3,
                collapsed: true
            })
        );
        assert_eq!(row(ids[3]), None);
        c.set_collapsed(s, false).unwrap();
        c.set_top(ids[2]).unwrap();
        assert_eq!(row(ids[0]).unwrap().top, ids[2]);
        assert!(!row(ids[0]).unwrap().collapsed);
        // Taking the top out moves the top; leaving one photo removes the stack.
        assert_eq!(c.unstack(&[ids[2]]).unwrap(), 1);
        assert_eq!(row(ids[0]).unwrap().top, ids[0]);
        assert_eq!(row(ids[0]).unwrap().size, 2);
        c.unstack(&[ids[0]]).unwrap();
        assert_eq!(row(ids[1]), None);
        // A new stack takes photos from an old one.
        c.stack(&ids[..2], ids[0]).unwrap();
        let t = c.stack(&ids[1..], ids[3]).unwrap().unwrap();
        assert_eq!(row(ids[0]), None);
        assert_eq!(row(ids[1]).unwrap().id, t);
        assert_eq!(c.dissolve_stacks_of(&[ids[2]]).unwrap(), 1);
        assert!(ids.iter().all(|&id| row(id).is_none()));
        assert_eq!(c.stack(&ids[..1], ids[0]).unwrap(), None);
    }

    #[test]
    fn fingerprints_are_kept_for_the_file_as_read() {
        let (_d, c, ids) = catalog(2);
        assert_eq!(c.unhashed(&ids).unwrap().len(), 2);
        c.set_hash(ids[0], u64::MAX).unwrap();
        assert_eq!(c.unhashed(&ids).unwrap().len(), 1);
        assert_eq!(c.hashes(&ids).unwrap(), HashMap::from([(ids[0], u64::MAX)]));
        c.conn
            .execute("UPDATE photos SET file_size = 7 WHERE id = ?1", [ids[0]])
            .unwrap();
        assert!(c.hashes(&ids).unwrap().is_empty());
    }

    #[test]
    fn dhash_sees_likeness_not_exposure() {
        let (w, h) = (90, 60);
        let pic: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                0.5 + 0.3 * (x * 0.11).sin() * (y * 0.07).cos()
            })
            .collect();
        let brighter: Vec<f32> = pic.iter().map(|v| v * 1.3 + 0.05).collect();
        let other: Vec<f32> = pic
            .iter()
            .enumerate()
            .map(|(i, _)| 0.5 + 0.3 * ((i % w) as f32 * 0.31).cos())
            .collect();
        assert_eq!(distance(dhash(&pic, w, h), dhash(&brighter, w, h)), 0);
        assert!(distance(dhash(&pic, w, h), dhash(&other, w, h)) > 16);
        assert_eq!(dhash(&[], 0, 0), 0);
    }

    #[test]
    fn bursts_and_duplicates_group_with_the_sharpest_on_top() {
        let c = |id, taken: Option<f64>, hash, sharpness| Candidate {
            id,
            taken,
            hash,
            sharpness,
        };
        let photos = [
            // A burst of three, the middle one sharpest.
            c(1, Some(0.0), 0b1111, Some(0.5)),
            c(2, Some(0.4), 0b1110, Some(0.9)),
            c(3, Some(0.9), 0b1100, None),
            // Too late for the burst.
            c(4, Some(10.0), 0b1111_0000_0000, None),
            // A copy of 4 much later; and an unrelated photo.
            c(5, Some(500.0), 0b1111_0000_0001, None),
            c(6, None, u64::MAX, None),
        ];
        let g = group(&photos, Grouping::default());
        assert_eq!(g, vec![vec![2, 1, 3], vec![4, 5]]);
        let bursts_only = group(
            &photos,
            Grouping {
                duplicates: false,
                ..Grouping::default()
            },
        );
        assert_eq!(bursts_only, vec![vec![2, 1, 3]]);
        let tight = group(
            &photos,
            Grouping {
                burst_gap: 0.3,
                duplicates: false,
            },
        );
        assert!(tight.is_empty());
    }
}

#[cfg(test)]
mod pair_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_raw_and_its_jpeg_in_one_folder_pair_up() {
        let photos = [
            (1, Path::new("/s/DSC_1.NEF"), None),
            (2, Path::new("/s/DSC_1.JPG"), None),
            (3, Path::new("/s/DSC_2.nef"), None),
            (4, Path::new("/other/DSC_2.jpg"), None),
            (5, Path::new("/s/DSC_3.CR3"), Some(9)),
            (6, Path::new("/s/dsc_3.jpeg"), Some(9)),
            (7, Path::new("/s/DSC_4.png"), None),
            (8, Path::new("/s/DSC_4.dng"), None),
        ];
        assert_eq!(
            raw_jpeg_pairs(&photos),
            vec![(1, 2)],
            "another folder, already stacked together, and a PNG do not pair"
        );
    }
}

#[cfg(test)]
mod similar_tests {
    use super::*;

    #[test]
    fn similar_photos_come_nearest_first_and_far_ones_are_left_out() {
        let hashes: HashMap<i64, u64> = [
            (1, 0b0000u64),
            (2, 0b0111u64),
            (3, 0b0001u64),
            (4, u64::MAX),
        ]
        .into_iter()
        .collect();
        assert_eq!(similar(1, &hashes), vec![1, 3, 2]);
        assert!(similar(9, &hashes).is_empty(), "not fingerprinted");
    }
}
