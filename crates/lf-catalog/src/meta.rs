//! Descriptive metadata: a title, a caption, the creator and copyright, and keywords, as
//! Lightroom's Metadata and Keywording panels keep them. The text fields live in the `photos`
//! row and keywords in their own table (schema v7); like culling marks they stay in the
//! catalogue (the web editor's sidecar is about how a photo looks), and exports carry them
//! as XMP ([`crate::exif_embed::xmp_segment`]).
//!
//! Setting metadata never touches `photos.updated_at` (a thumbnail's key, see
//! [`crate::cull`]). Keywords are matched without regard to case, and the first spelling
//! given is the one kept.

use crate::db::{Catalog, Result};

/// The longest title, caption or name kept, in characters (longer is cut).
pub const MAX_TEXT: usize = 2000;
/// The longest keyword, in characters.
pub const MAX_KEYWORD: usize = 100;

/// A photo's descriptive metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    /// Sorted without regard to case.
    pub keywords: Vec<String>,
}

impl Meta {
    /// Whether every term of `query` (split on spaces, case ignored) is in the title, the
    /// caption, the creator or a keyword.
    pub fn matches(&self, query: &str) -> bool {
        let fields: Vec<String> = [&self.title, &self.caption, &self.creator]
            .into_iter()
            .flatten()
            .chain(&self.keywords)
            .map(|s| s.to_lowercase())
            .collect();
        query
            .split_whitespace()
            .map(str::to_lowercase)
            .all(|term| fields.iter().any(|f| f.contains(&term)))
    }
}

/// One text field of [`Meta`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Caption,
    Creator,
    Copyright,
}

impl Field {
    pub const ALL: [Field; 4] = [
        Field::Title,
        Field::Caption,
        Field::Creator,
        Field::Copyright,
    ];

    fn column(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Caption => "caption",
            Field::Creator => "creator",
            Field::Copyright => "copyright",
        }
    }

    /// The field's name as the panel shows it.
    pub fn name(self) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Caption => "Caption",
            Field::Creator => "Creator",
            Field::Copyright => "Copyright",
        }
    }

    /// This field of `meta`.
    pub fn of(self, meta: &Meta) -> Option<&str> {
        match self {
            Field::Title => meta.title.as_deref(),
            Field::Caption => meta.caption.as_deref(),
            Field::Creator => meta.creator.as_deref(),
            Field::Copyright => meta.copyright.as_deref(),
        }
    }
}

/// `text` cut to `max` characters and trimmed; `None` when nothing is left.
fn tidy(text: &str, max: usize) -> Option<String> {
    let cut: String = text.trim().chars().take(max).collect();
    let cut = cut.trim();
    (!cut.is_empty()).then(|| cut.to_owned())
}

/// The keywords typed in `text`: separated by commas or semicolons, trimmed, each at most
/// [`MAX_KEYWORD`] characters, without repeats (case ignored, the first spelling kept).
pub fn parse_keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split([',', ';']).filter_map(|w| tidy(w, MAX_KEYWORD)) {
        if !out.iter().any(|k| k.to_lowercase() == word.to_lowercase()) {
            out.push(word);
        }
    }
    out
}

/// One change to the metadata of some photos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A text field set (empty clears it).
    Set(Field, String),
    AddKeywords(Vec<String>),
    RemoveKeywords(Vec<String>),
}

impl Change {
    /// What the catalogue will hold after this change, made to `meta` in memory (so a view
    /// shows it before the save is done).
    pub fn apply(&self, meta: &mut Meta) {
        match self {
            Change::Set(field, text) => {
                let value = tidy(text, MAX_TEXT);
                match field {
                    Field::Title => meta.title = value,
                    Field::Caption => meta.caption = value,
                    Field::Creator => meta.creator = value,
                    Field::Copyright => meta.copyright = value,
                }
            }
            Change::AddKeywords(words) => {
                for word in words.iter().filter_map(|w| tidy(w, MAX_KEYWORD)) {
                    if !meta
                        .keywords
                        .iter()
                        .any(|k| k.to_lowercase() == word.to_lowercase())
                    {
                        meta.keywords.push(word);
                    }
                }
                meta.keywords.sort_by_key(|k| k.to_lowercase());
            }
            Change::RemoveKeywords(words) => meta.keywords.retain(|k| {
                !words
                    .iter()
                    .any(|w| w.trim().to_lowercase() == k.to_lowercase())
            }),
        }
    }
}

impl Catalog {
    /// Sets `field` to `text` (trimmed; empty clears it) on every photo in `ids`, in one
    /// transaction; returns how many rows changed.
    pub fn set_meta(&self, ids: &[i64], field: Field, text: &str) -> Result<usize> {
        let value = tidy(text, MAX_TEXT);
        let tx = self.write_tx()?;
        let mut changed = 0;
        {
            let mut st = tx.prepare(&format!(
                "UPDATE photos SET {} = ?1 WHERE id = ?2",
                field.column()
            ))?;
            for id in ids {
                changed += st.execute(rusqlite::params![value, id])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Adds `words` to every photo in `ids` (one already there, in any case, is kept as it
    /// was); returns how many were added.
    pub fn add_keywords(&self, ids: &[i64], words: &[String]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut added = 0;
        {
            let mut st = tx.prepare(
                "INSERT OR IGNORE INTO keywords (photo_id, keyword)
                 SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM photos WHERE id = ?1)",
            )?;
            for id in ids {
                for word in words.iter().filter_map(|w| tidy(w, MAX_KEYWORD)) {
                    added += st.execute(rusqlite::params![id, word])?;
                }
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Takes `words` (case ignored) off every photo in `ids`; returns how many were removed.
    pub fn remove_keywords(&self, ids: &[i64], words: &[String]) -> Result<usize> {
        let tx = self.write_tx()?;
        let mut removed = 0;
        {
            let mut st = tx.prepare("DELETE FROM keywords WHERE photo_id = ?1 AND keyword = ?2")?;
            for id in ids {
                for word in words {
                    removed += st.execute(rusqlite::params![id, word.trim()])?;
                }
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Makes `change` on every photo in `ids`; returns how many rows or keywords changed.
    pub fn change_meta(&self, ids: &[i64], change: &Change) -> Result<usize> {
        match change {
            Change::Set(field, text) => self.set_meta(ids, *field, text),
            Change::AddKeywords(words) => self.add_keywords(ids, words),
            Change::RemoveKeywords(words) => self.remove_keywords(ids, words),
        }
    }

    /// Every keyword in the catalogue with how many photos have it, by name (case ignored).
    pub fn keywords(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self.conn.prepare(
            "SELECT MIN(keyword), COUNT(*) FROM keywords GROUP BY keyword ORDER BY keyword",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_split_trimmed_and_unique() {
        assert_eq!(
            parse_keywords(" Wedding, beach;  ,wedding ; Golden hour,"),
            ["Wedding", "beach", "Golden hour"]
        );
        assert!(parse_keywords(" , ; ").is_empty());
        let long = "x".repeat(150);
        assert_eq!(parse_keywords(&long)[0].chars().count(), MAX_KEYWORD);
    }

    #[test]
    fn a_search_needs_every_term_somewhere() {
        let meta = Meta {
            title: Some("Sunset at Goa".into()),
            caption: Some("The couple on the beach".into()),
            keywords: vec!["Wedding".into(), "Reception".into()],
            ..Meta::default()
        };
        assert!(meta.matches("goa wedding"));
        assert!(meta.matches("BEACH"));
        assert!(meta.matches(""));
        assert!(!meta.matches("goa mountains"));
    }
}

#[cfg(test)]
mod change_tests {
    use super::*;

    #[test]
    fn a_change_in_memory_is_what_the_catalogue_will_hold() {
        let mut meta = Meta::default();
        Change::Set(Field::Title, "  Haldi  ".into()).apply(&mut meta);
        Change::AddKeywords(vec!["zebra".into(), "Apple".into(), "apple".into()]).apply(&mut meta);
        assert_eq!(meta.title.as_deref(), Some("Haldi"));
        assert_eq!(meta.keywords, ["Apple", "zebra"]);
        Change::RemoveKeywords(vec!["APPLE".into()]).apply(&mut meta);
        assert_eq!(meta.keywords, ["zebra"]);
        Change::Set(Field::Title, String::new()).apply(&mut meta);
        assert_eq!(meta.title, None);
    }
}
