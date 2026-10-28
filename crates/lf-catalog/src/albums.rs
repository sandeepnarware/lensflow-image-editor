//! Albums: sets of photos drawn from any folders, as Lightroom's collections. A plain album
//! keeps the photos put in it, in the order they were added; a smart album keeps a [`Rule`]
//! and holds whatever photos match it when it is shown ("5 stars from this week").
//!
//! Albums live only in the catalogue (schema v8), like culling marks: they are about
//! choosing and gathering photos, not about how one looks. Removing a photo from an album
//! never touches the photo; removing its folder from the catalogue takes it out of every
//! album.

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::cull::{Flag, Label};
use crate::db::{Catalog, CatalogError, PhotoRow, Result};

/// The longest album name, in characters (longer is cut).
pub const MAX_NAME: usize = 100;

/// Which flags a smart album takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleFlag {
    #[default]
    Any,
    Picked,
    NotRejected,
}

impl RuleFlag {
    pub const ALL: [RuleFlag; 3] = [RuleFlag::Any, RuleFlag::Picked, RuleFlag::NotRejected];

    pub fn name(self) -> &'static str {
        match self {
            RuleFlag::Any => "Any flag",
            RuleFlag::Picked => "Picked",
            RuleFlag::NotRejected => "Not rejected",
        }
    }

    fn keeps(self, flag: Flag) -> bool {
        match self {
            RuleFlag::Any => true,
            RuleFlag::Picked => flag == Flag::Picked,
            RuleFlag::NotRejected => flag != Flag::Rejected,
        }
    }
}

/// What a smart album holds: every photo meeting all of these. Unknown fields in a stored
/// rule are ignored, so a later version's rule still opens (it may hold more photos here).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// At least this many stars (0: any).
    pub min_rating: u8,
    pub flag: RuleFlag,
    /// The colour label's stored name ([`Label::key`]); none takes any label.
    pub label: Option<String>,
    /// Words that must each appear in the file name, camera, lens, title, caption, creator
    /// or a keyword (case ignored).
    pub text: String,
    /// Taken within this many days of today (0: any time). Undated photos never match.
    pub within_days: u32,
}

impl Rule {
    /// Whether `row` belongs in the album on `today`.
    pub fn matches(&self, row: &PhotoRow, today: NaiveDate) -> bool {
        if row.rating < self.min_rating || !self.flag.keeps(row.flag) {
            return false;
        }
        if let Some(key) = &self.label
            && row.label.map(Label::key) != Some(key.as_str())
        {
            return false;
        }
        if self.within_days > 0 {
            let taken = row
                .captured_at
                .as_deref()
                .and_then(|s| s.get(..10))
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            match taken {
                Some(day) if (today - day).num_days() <= i64::from(self.within_days) => {}
                _ => return false,
            }
        }
        crate::meta::row_matches(row, &self.text)
    }
}

/// An album as the sidebar lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Album {
    pub id: i64,
    pub name: String,
    /// Set for a smart album.
    pub rule: Option<Rule>,
    /// How many photos a plain album holds (a smart album's count is known once shown).
    pub count: i64,
}

impl Album {
    pub fn is_smart(&self) -> bool {
        self.rule.is_some()
    }
}

/// A name trimmed and cut to [`MAX_NAME`]; `None` when nothing is left.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    (!name.is_empty()).then_some(name)
}

fn rule_json(rule: Option<&Rule>) -> Result<Option<String>> {
    Ok(rule.map(serde_json::to_string).transpose()?)
}

impl Catalog {
    /// Every album, by name (case ignored).
    pub fn albums(&self) -> Result<Vec<Album>> {
        let mut st = self.conn.prepare(
            "SELECT a.id, a.name, a.rule,
                    (SELECT COUNT(*) FROM album_photos ap WHERE ap.album_id = a.id)
             FROM albums a ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, rule, count) = row?;
            out.push(Album {
                id,
                name,
                // A rule that no longer reads holds nothing rather than everything.
                rule: rule.map(|r| {
                    serde_json::from_str(&r).unwrap_or(Rule {
                        min_rating: 6,
                        ..Rule::default()
                    })
                }),
                count,
            });
        }
        Ok(out)
    }

    /// Makes an album (a smart one with `rule`); returns its id. Names are unique, case
    /// ignored.
    pub fn create_album(&self, name: &str, rule: Option<&Rule>) -> Result<i64> {
        let name = clean_name(name)
            .ok_or_else(|| CatalogError::NotFound("An album needs a name.".to_owned()))?;
        let tx = self.write_tx()?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM albums WHERE name = ?1 COLLATE NOCASE)",
            [&name],
            |r| r.get(0),
        )?;
        if taken {
            return Err(CatalogError::AlbumNameTaken(name));
        }
        tx.execute(
            "INSERT INTO albums (name, rule, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                name,
                rule_json(rule)?,
                crate::model::ts(crate::model::now())
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// Renames an album and, for a smart one, replaces its rule (a plain album stays plain).
    pub fn update_album(&self, id: i64, name: &str, rule: Option<&Rule>) -> Result<()> {
        let name = clean_name(name)
            .ok_or_else(|| CatalogError::NotFound("An album needs a name.".to_owned()))?;
        let tx = self.write_tx()?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM albums WHERE name = ?1 COLLATE NOCASE AND id <> ?2)",
            rusqlite::params![name, id],
            |r| r.get(0),
        )?;
        if taken {
            return Err(CatalogError::AlbumNameTaken(name));
        }
        tx.execute(
            "UPDATE albums SET name = ?1,
                 rule = CASE WHEN rule IS NULL THEN NULL ELSE ?2 END
             WHERE id = ?3",
            rusqlite::params![name, rule_json(rule)?, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes an album; its photos are untouched.
    pub fn delete_album(&self, id: i64) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute("DELETE FROM albums WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Puts `ids` at the end of a plain album, skipping photos already in it; returns how
    /// many were added. A smart album takes none.
    pub fn add_to_album(&self, album: i64, ids: &[i64]) -> Result<usize> {
        let tx = self.write_tx()?;
        let smart: Option<bool> = tx
            .query_row(
                "SELECT rule IS NOT NULL FROM albums WHERE id = ?1",
                [album],
                |r| r.get(0),
            )
            .ok();
        match smart {
            None => return Err(CatalogError::NotFound("That album was deleted.".to_owned())),
            Some(true) => return Ok(0),
            Some(false) => {}
        }
        let mut next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM album_photos WHERE album_id = ?1",
            [album],
            |r| r.get(0),
        )?;
        let mut added = 0;
        {
            let mut st = tx.prepare(
                "INSERT OR IGNORE INTO album_photos (album_id, photo_id, position)
                 SELECT ?1, id, ?3 FROM photos WHERE id = ?2",
            )?;
            for id in ids {
                let n = st.execute(rusqlite::params![album, id, next])?;
                added += n;
                next += n as i64;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Takes `ids` out of a plain album; returns how many were in it.
    pub fn remove_from_album(&self, album: i64, ids: &[i64]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut removed = 0;
        {
            let mut st =
                tx.prepare("DELETE FROM album_photos WHERE album_id = ?1 AND photo_id = ?2")?;
            for id in ids {
                removed += st.execute([album, *id])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// The album's photos: a plain album's in the order they were added, a smart album's
    /// (those matching its rule today, missing files left out) in capture order.
    pub fn album_photos(&self, album: i64) -> Result<Vec<PhotoRow>> {
        let rule: Option<Option<String>> = self
            .conn
            .query_row("SELECT rule FROM albums WHERE id = ?1", [album], |r| {
                r.get(0)
            })
            .ok();
        let Some(rule) = rule else {
            return Err(CatalogError::NotFound("That album was deleted.".to_owned()));
        };
        match rule {
            None => {
                let mut st = self.conn.prepare(&format!(
                    "SELECT {} FROM album_photos ap JOIN photos p ON p.id = ap.photo_id
                     WHERE ap.album_id = ?1 ORDER BY ap.position",
                    crate::db::PHOTO_COLUMNS
                ))?;
                let rows = st.query_map([album], crate::db::photo_row)?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            }
            Some(json) => {
                let rule: Rule = serde_json::from_str(&json).unwrap_or(Rule {
                    min_rating: 6,
                    ..Rule::default()
                });
                let today = Local::now().date_naive();
                let mut st = self.conn.prepare(&format!(
                    "SELECT {} FROM photos p WHERE p.missing = 0
                     ORDER BY p.captured_at IS NULL, p.captured_at, p.path",
                    crate::db::PHOTO_COLUMNS
                ))?;
                let rows = st.query_map([], crate::db::photo_row)?;
                let mut out = Vec::new();
                for row in rows {
                    let row = row?;
                    if rule.matches(&row, today) {
                        out.push(row);
                    }
                }
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(rating: u8, flag: Flag, taken: Option<&str>) -> PhotoRow {
        PhotoRow {
            rating,
            flag,
            captured_at: taken.map(str::to_owned),
            path: "D:/shoot/DSC_0001.NEF".into(),
            ..PhotoRow::default()
        }
    }

    #[test]
    fn a_rule_keeps_stars_flags_labels_words_and_recent_days() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let rule = Rule {
            min_rating: 4,
            flag: RuleFlag::NotRejected,
            within_days: 7,
            ..Rule::default()
        };
        assert!(rule.matches(&row(5, Flag::Picked, Some("2026-10-25T10:00:00")), today));
        assert!(!rule.matches(&row(3, Flag::Picked, Some("2026-10-25T10:00:00")), today));
        assert!(!rule.matches(&row(5, Flag::Rejected, Some("2026-10-25T10:00:00")), today));
        assert!(!rule.matches(&row(5, Flag::Picked, Some("2026-10-01T10:00:00")), today));
        assert!(!rule.matches(&row(5, Flag::Picked, None), today));
        let words = Rule {
            text: "dsc_0001".into(),
            label: Some("red".into()),
            ..Rule::default()
        };
        let mut red = row(0, Flag::Unflagged, None);
        assert!(!words.matches(&red, today));
        red.label = Some(Label::Red);
        assert!(words.matches(&red, today));
    }

    #[test]
    fn a_stored_rule_with_unknown_fields_still_reads() {
        let rule: Rule = serde_json::from_str(r#"{"min_rating":3,"camera":"X100"}"#).unwrap();
        assert_eq!(rule.min_rating, 3);
        assert_eq!(rule.flag, RuleFlag::Any);
    }
}
