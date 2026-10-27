//! Culling marks: a pick or reject flag, zero to five stars and a colour label, as Lightroom
//! has them. They live in the `photos` row (columns since schema v1, unused until now) and
//! only in the catalogue: they are about choosing photos, not about how a photo looks, so
//! they stay out of the sidecar the web editor reads.
//!
//! Setting a mark never touches `photos.updated_at`: that stamp is part of a thumbnail's key
//! ([`crate::thumbs`]), and a star must not make every thumbnail again.

use crate::db::{Catalog, CatalogError, Result};

/// A photo's flag. Stored as Lightroom's numbers: 1 picked, -1 rejected, 0 neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Flag {
    #[default]
    Unflagged,
    Picked,
    Rejected,
}

impl Flag {
    /// The stored number for this flag.
    pub fn stored(self) -> i64 {
        match self {
            Flag::Unflagged => 0,
            Flag::Picked => 1,
            Flag::Rejected => -1,
        }
    }

    /// The flag a stored number means; a number this version does not know is no flag.
    pub fn from_stored(n: i64) -> Flag {
        match n {
            1 => Flag::Picked,
            -1 => Flag::Rejected,
            _ => Flag::Unflagged,
        }
    }
}

/// A colour label. Stored by name, so a label a later version adds reads here as none and is
/// left alone (see [`Catalog::mark`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Label {
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl Label {
    /// Every label, in Lightroom's order (the keys `6` to `9` name the first four).
    pub const ALL: [Label; 5] = [
        Label::Red,
        Label::Yellow,
        Label::Green,
        Label::Blue,
        Label::Purple,
    ];

    /// The name stored in `photos.label`.
    pub fn key(self) -> &'static str {
        match self {
            Label::Red => "red",
            Label::Yellow => "yellow",
            Label::Green => "green",
            Label::Blue => "blue",
            Label::Purple => "purple",
        }
    }

    /// The label a stored name means, if this version knows it.
    pub fn from_key(key: &str) -> Option<Label> {
        Label::ALL.into_iter().find(|l| l.key() == key)
    }

    /// The label's name as the menus show it.
    pub fn name(self) -> &'static str {
        match self {
            Label::Red => "Red",
            Label::Yellow => "Yellow",
            Label::Green => "Green",
            Label::Blue => "Blue",
            Label::Purple => "Purple",
        }
    }
}

/// The highest star rating.
pub const MAX_RATING: u8 = 5;

/// One change to the marks of some photos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Flag(Flag),
    /// 0 (no stars) to [`MAX_RATING`].
    Rating(u8),
    /// `None` clears the label.
    Label(Option<Label>),
}

impl Catalog {
    /// Sets `mark` on every photo in `ids` in one transaction; returns how many rows changed
    /// (an id no longer catalogued is skipped). Only the mark's own column is written, so a
    /// label this version cannot read survives a new rating.
    pub fn mark(&self, ids: &[i64], mark: Mark) -> Result<usize> {
        if let Mark::Rating(stars) = mark
            && stars > MAX_RATING
        {
            return Err(CatalogError::BadRating(stars));
        }
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.write_tx()?;
        let mut changed = 0;
        {
            let (sql, value): (&str, rusqlite::types::Value) = match mark {
                Mark::Flag(flag) => (
                    "UPDATE photos SET flag = ?1 WHERE id = ?2",
                    flag.stored().into(),
                ),
                Mark::Rating(stars) => (
                    "UPDATE photos SET rating = ?1 WHERE id = ?2",
                    i64::from(stars).into(),
                ),
                Mark::Label(label) => (
                    "UPDATE photos SET label = ?1 WHERE id = ?2",
                    label.map_or(rusqlite::types::Value::Null, |l| l.key().to_owned().into()),
                ),
            };
            let mut st = tx.prepare(sql)?;
            for id in ids {
                changed += st.execute(rusqlite::params![value, id])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }
}
