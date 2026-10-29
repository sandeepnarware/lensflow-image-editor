//! Current settings, the undo history and the DB/sidecar conflict rule.
//!
//! History is a ring of 50 rows per photo. The undo/redo cursor is persisted in the
//! `history_cursor` table (not held in a handle) so it survives reopening the catalogue and every
//! caller sees one truth. A new edit made after undoing discards the redo branch.

use chrono::{DateTime, Utc};
use lf_settings::EditSettings;
use rusqlite::{OptionalExtension, Transaction};
use serde_json::Map;

use crate::db::{Catalog, CatalogError, Result};
use crate::model::{Reconciled, Source, Version, now, parse_ts, ts};
use crate::sidecar::{self, SIDECAR_VERSION, Sidecar};
use std::path::Path;

pub const HISTORY_CAP: usize = 50;

fn settings_text(s: &EditSettings) -> String {
    s.to_value().to_string()
}

fn load_edits(tx: &Transaction, photo_id: i64) -> Result<Option<(EditSettings, DateTime<Utc>)>> {
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT settings, updated_at FROM edits WHERE photo_id = ?1",
            [photo_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(s, t)| {
        (
            EditSettings::from_json_str(&s),
            parse_ts(&t).unwrap_or(DateTime::UNIX_EPOCH),
        )
    }))
}

pub(crate) fn check_photo(tx: &Transaction, photo_id: i64) -> Result<()> {
    let n: i64 = tx.query_row(
        "SELECT COUNT(*) FROM photos WHERE id = ?1",
        [photo_id],
        |r| r.get(0),
    )?;
    if n == 0 {
        return Err(CatalogError::NotFound(format!("photo {photo_id}")));
    }
    Ok(())
}

pub(crate) fn load_versions(tx: &Transaction, photo_id: i64) -> Result<Vec<Version>> {
    let mut st = tx.prepare(
        "SELECT id, label, settings, created_at FROM versions WHERE photo_id = ?1
         ORDER BY created_at, rowid",
    )?;
    let rows = st.query_map([photo_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, label, settings, created) = row?;
        out.push(Version {
            id,
            label,
            settings: EditSettings::from_json_str(&settings),
            created_at: parse_ts(&created).unwrap_or(DateTime::UNIX_EPOCH),
        });
    }
    Ok(out)
}

/// The sidecar the database implies; `v` and unknown keys come from the remembered sidecar.
pub(crate) fn build_sidecar(tx: &Transaction, photo_id: i64) -> Result<Sidecar> {
    let (settings, updated_at) = load_edits(tx, photo_id)?
        .unwrap_or_else(|| (EditSettings::default_settings(), DateTime::UNIX_EPOCH));
    let meta: Option<(i64, String)> = tx
        .query_row(
            "SELECT v, extra FROM sidecar_meta WHERE photo_id = ?1",
            [photo_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (v, extra) = match meta {
        Some((v, extra)) => (
            (v.max(0) as u32).max(SIDECAR_VERSION),
            serde_json::from_str::<Map<String, serde_json::Value>>(&extra).unwrap_or_default(),
        ),
        None => (SIDECAR_VERSION, Map::new()),
    };
    Ok(Sidecar {
        v,
        settings,
        versions: load_versions(tx, photo_id)?,
        updated_at,
        extra,
    })
}

fn put_meta(tx: &Transaction, photo_id: i64, sc: &Sidecar) -> Result<()> {
    tx.execute(
        "INSERT INTO sidecar_meta (photo_id, v, extra) VALUES (?1, ?2, ?3)
         ON CONFLICT(photo_id) DO UPDATE SET v = excluded.v, extra = excluded.extra",
        (
            photo_id,
            sc.v as i64,
            serde_json::Value::Object(sc.extra.clone()).to_string(),
        ),
    )?;
    Ok(())
}

/// Remember a sidecar's version number and unknown keys so every sidecar built for this photo
/// carries them (for callers that read a sidecar outside [`reconcile`]).
pub fn remember_sidecar(catalog: &Catalog, photo_id: i64, sc: &Sidecar) -> Result<()> {
    let tx = catalog.write_tx()?;
    check_photo(&tx, photo_id)?;
    put_meta(&tx, photo_id, sc)?;
    tx.commit()?;
    Ok(())
}

/// Write `edits` (stamped `at`) without touching history.
fn put_edits(tx: &Transaction, photo_id: i64, s: &EditSettings, at: DateTime<Utc>) -> Result<()> {
    tx.execute(
        "INSERT INTO edits (photo_id, settings, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(photo_id) DO UPDATE SET settings = excluded.settings,
         updated_at = excluded.updated_at",
        (photo_id, settings_text(s), ts(at)),
    )?;
    Ok(())
}

/// Bump the edits stamp (creating default edits if none): used by version changes.
pub(crate) fn touch(tx: &Transaction, photo_id: i64) -> Result<()> {
    let s = load_edits(tx, photo_id)?
        .map(|e| e.0)
        .unwrap_or_else(EditSettings::default_settings);
    put_edits(tx, photo_id, &s, now())
}

fn cursor_of(tx: &Transaction, photo_id: i64) -> Result<Option<i64>> {
    Ok(tx
        .query_row(
            "SELECT history_id FROM history_cursor WHERE photo_id = ?1",
            [photo_id],
            |r| r.get(0),
        )
        .optional()?)
}

fn push_history(
    tx: &Transaction,
    photo_id: i64,
    s: &EditSettings,
    at: DateTime<Utc>,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO history (photo_id, settings, created_at) VALUES (?1, ?2, ?3)",
        (photo_id, settings_text(s), ts(at)),
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO history_cursor (photo_id, history_id) VALUES (?1, ?2)
         ON CONFLICT(photo_id) DO UPDATE SET history_id = excluded.history_id",
        (photo_id, id),
    )?;
    Ok(id)
}

/// Make `s` the photo's current settings, record it in history and return the sidecar to write.
pub fn set_settings(catalog: &Catalog, photo_id: i64, s: &EditSettings) -> Result<Sidecar> {
    let tx = catalog.write_tx()?;
    let sidecar = set_settings_in(&tx, photo_id, s)?;
    tx.commit()?;
    Ok(sidecar)
}

pub(crate) fn set_settings_in(
    tx: &Transaction,
    photo_id: i64,
    s: &EditSettings,
) -> Result<Sidecar> {
    check_photo(tx, photo_id)?;
    let at = now();
    match cursor_of(tx, photo_id)? {
        // Editing after an undo drops the redo branch.
        Some(c) => {
            tx.execute(
                "DELETE FROM history WHERE photo_id = ?1 AND id > ?2",
                (photo_id, c),
            )?;
        }
        // First edit: seed history with the state before it so it can be undone.
        None => {
            let before = load_edits(tx, photo_id)?
                .map(|e| e.0)
                .unwrap_or_else(EditSettings::default_settings);
            push_history(tx, photo_id, &before, at)?;
        }
    }
    push_history(tx, photo_id, s, at)?;
    tx.execute(
        "DELETE FROM history WHERE photo_id = ?1 AND id NOT IN
         (SELECT id FROM history WHERE photo_id = ?1 ORDER BY id DESC LIMIT ?2)",
        (photo_id, HISTORY_CAP as i64),
    )?;
    put_edits(tx, photo_id, s, at)?;
    build_sidecar(tx, photo_id)
}

/// Current settings, if the photo has been edited.
pub fn get_settings(catalog: &Catalog, photo_id: i64) -> Result<Option<EditSettings>> {
    let tx = catalog.conn.unchecked_transaction()?;
    Ok(load_edits(&tx, photo_id)?.map(|e| e.0))
}

/// The sidecar the database currently implies (for the caller to write).
pub fn sidecar_for(catalog: &Catalog, photo_id: i64) -> Result<Sidecar> {
    let tx = catalog.conn.unchecked_transaction()?;
    check_photo(&tx, photo_id)?;
    build_sidecar(&tx, photo_id)
}

pub fn history_len(catalog: &Catalog, photo_id: i64) -> Result<i64> {
    Ok(catalog.conn.query_row(
        "SELECT COUNT(*) FROM history WHERE photo_id = ?1",
        [photo_id],
        |r| r.get(0),
    )?)
}

/// How many steps [`undo`] and [`redo`] can take from where the photo's history stands:
/// `(back, forward)`; `(0, 0)` for a photo never edited.
pub fn history_steps(catalog: &Catalog, photo_id: i64) -> Result<(i64, i64)> {
    let cursor: Option<i64> = catalog
        .conn
        .query_row(
            "SELECT history_id FROM history_cursor WHERE photo_id = ?1",
            [photo_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(cursor) = cursor else {
        return Ok((0, 0));
    };
    Ok(catalog.conn.query_row(
        "SELECT COALESCE(SUM(id < ?2), 0), COALESCE(SUM(id > ?2), 0) FROM history
         WHERE photo_id = ?1",
        (photo_id, cursor),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn step(catalog: &Catalog, photo_id: i64, back: bool) -> Result<Option<Sidecar>> {
    let tx = catalog.write_tx()?;
    check_photo(&tx, photo_id)?;
    let Some(cursor) = cursor_of(&tx, photo_id)? else {
        return Ok(None);
    };
    let sql = if back {
        "SELECT id, settings FROM history WHERE photo_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT 1"
    } else {
        "SELECT id, settings FROM history WHERE photo_id = ?1 AND id > ?2 ORDER BY id LIMIT 1"
    };
    let target: Option<(i64, String)> = tx
        .query_row(sql, (photo_id, cursor), |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((id, text)) = target else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE history_cursor SET history_id = ?2 WHERE photo_id = ?1",
        (photo_id, id),
    )?;
    put_edits(&tx, photo_id, &EditSettings::from_json_str(&text), now())?;
    let sidecar = build_sidecar(&tx, photo_id)?;
    tx.commit()?;
    Ok(Some(sidecar))
}

/// One step of a photo's history.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub settings: EditSettings,
    pub created_at: String,
}

/// A photo's history, oldest first, and the step it stands at (none: never edited).
pub fn history(catalog: &Catalog, photo_id: i64) -> Result<(Vec<HistoryEntry>, Option<i64>)> {
    let mut st = catalog
        .conn
        .prepare("SELECT id, settings, created_at FROM history WHERE photo_id = ?1 ORDER BY id")?;
    let rows = st
        .query_map([photo_id], |r| {
            Ok(HistoryEntry {
                id: r.get(0)?,
                settings: EditSettings::from_json_str(&r.get::<_, String>(1)?),
                created_at: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let cursor: Option<i64> = catalog
        .conn
        .query_row(
            "SELECT history_id FROM history_cursor WHERE photo_id = ?1",
            [photo_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok((rows, cursor))
}

/// Moves the photo's history to step `id` (one of its own), as undo and redo do several steps
/// at once; `None` when there is no such step or it is already there.
pub fn jump(catalog: &Catalog, photo_id: i64, id: i64) -> Result<Option<Sidecar>> {
    let tx = catalog.write_tx()?;
    check_photo(&tx, photo_id)?;
    if cursor_of(&tx, photo_id)? == Some(id) {
        return Ok(None);
    }
    let text: Option<String> = tx
        .query_row(
            "SELECT settings FROM history WHERE photo_id = ?1 AND id = ?2",
            (photo_id, id),
            |r| r.get(0),
        )
        .optional()?;
    let Some(text) = text else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE history_cursor SET history_id = ?2 WHERE photo_id = ?1",
        (photo_id, id),
    )?;
    put_edits(&tx, photo_id, &EditSettings::from_json_str(&text), now())?;
    let sidecar = build_sidecar(&tx, photo_id)?;
    tx.commit()?;
    Ok(Some(sidecar))
}

/// Step back one history entry; `None` at the oldest.
pub fn undo(catalog: &Catalog, photo_id: i64) -> Result<Option<Sidecar>> {
    step(catalog, photo_id, true)
}

/// Step forward one history entry; `None` at the newest.
pub fn redo(catalog: &Catalog, photo_id: i64) -> Result<Option<Sidecar>> {
    step(catalog, photo_id, false)
}

/// Apply the conflict rule between the database and a sidecar found on reopen (and on import):
/// a strictly newer sidecar wins and the database takes its settings and versions (a stamp in
/// the future is adopted as now, and one finer than milliseconds is adopted truncated to the
/// stored milliseconds; either way the sidecar is rewritten with it); otherwise the database
/// wins. The sidecar is rewritten only when the database is strictly newer or the content
/// (`v`, settings, versions) differs; an equal sidecar is left alone. With no sidecar, one is
/// written when the photo has edits. Being given a readable sidecar (or none) clears the
/// photo's `sidecar_error`.
pub fn reconcile(
    catalog: &Catalog,
    photo_id: i64,
    sidecar: Option<&Sidecar>,
) -> Result<Reconciled> {
    let tx = catalog.write_tx()?;
    let result = reconcile_in(&tx, photo_id, sidecar)?;
    tx.commit()?;
    Ok(result)
}

/// What [`reconcile_on_open`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnOpen {
    /// The sidecar (or its absence) was reconciled; this side won.
    Reconciled(Source),
    /// The sidecar could not be read: the message stored in `photos.sidecar_error`. Nothing
    /// was reconciled, the catalogue's edits stand and the file was left untouched.
    Damaged(String),
}

/// Reopening a photo (Develop): reads the sidecar beside `photo` and applies [`reconcile`],
/// so an edit made on another machine shows; a sidecar the rule says to rewrite is written
/// (a failed write is logged). A sidecar that cannot be read is recorded in
/// `photos.sidecar_error` and skips the reconcile: it is never treated as absent
/// (`reconcile(None)` would write the catalogue's edits over it).
pub fn reconcile_on_open(catalog: &Catalog, photo_id: i64, photo: &Path) -> Result<OnOpen> {
    let path = sidecar::path_for(photo);
    match sidecar::read(&path) {
        Ok(found) => {
            let r = reconcile(catalog, photo_id, found.as_ref())?;
            // Written outside the app's SidecarSaver lock: acceptable, since writes are atomic
            // (temp file + rename) and the newer catalogue wins again on the next open.
            if let Some(out) = &r.sidecar_to_write
                && let Err(e) = sidecar::write(&path, out)
            {
                log::warn!("could not update the sidecar {}: {e}", path.display());
            }
            Ok(OnOpen::Reconciled(r.source))
        }
        Err(e) => {
            log::warn!("the sidecar of {} could not be read: {e}", photo.display());
            let message = sidecar::damaged_message(photo, &e);
            let tx = catalog.write_tx()?;
            check_photo(&tx, photo_id)?;
            tx.execute(
                "UPDATE photos SET sidecar_error = ?2 WHERE id = ?1",
                rusqlite::params![photo_id, message],
            )?;
            tx.commit()?;
            Ok(OnOpen::Damaged(message))
        }
    }
}

/// [`reconcile`] inside the caller's transaction.
pub(crate) fn reconcile_in(
    tx: &Transaction,
    photo_id: i64,
    sidecar: Option<&Sidecar>,
) -> Result<Reconciled> {
    check_photo(tx, photo_id)?;
    let db = load_edits(tx, photo_id)?;
    if let Some(sc) = sidecar {
        put_meta(tx, photo_id, sc)?;
    }
    // The sidecar was read (or there is none): an earlier "damaged sidecar" no longer holds.
    tx.execute(
        "UPDATE photos SET sidecar_error = NULL WHERE id = ?1 AND sidecar_error IS NOT NULL",
        [photo_id],
    )?;
    let result = match (db, sidecar) {
        (None, None) => Reconciled {
            source: Source::Db,
            sidecar_to_write: None,
        },
        (Some(_), None) => Reconciled {
            source: Source::Db,
            sidecar_to_write: Some(build_sidecar(tx, photo_id)?),
        },
        (db, Some(sc)) if db.as_ref().is_none_or(|(_, t)| sc.updated_at > *t) => {
            // A stamp from a clock set ahead would win every later comparison: adopt it as now.
            // A stamp finer than the stored milliseconds would stay newer than the database
            // after the round trip and win every reopen: adopt it truncated, and rewrite it.
            let now = now();
            let clamped = sc.updated_at > now;
            let at = if clamped { now } else { sc.updated_at };
            let stored = parse_ts(&ts(at)).unwrap_or(at);
            let rewrite = clamped || stored != sc.updated_at;
            let at = stored;
            put_edits(tx, photo_id, &sc.settings, at)?;
            tx.execute("DELETE FROM versions WHERE photo_id = ?1", [photo_id])?;
            for v in &sc.versions {
                // A sidecar copied from another photo may carry ids that photo owns.
                let taken: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM versions WHERE id = ?1",
                    [&v.id],
                    |r| r.get(0),
                )?;
                let id = if taken > 0 {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    v.id.clone()
                };
                tx.execute(
                    "INSERT INTO versions (id, photo_id, label, settings, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    (
                        id,
                        photo_id,
                        &v.label,
                        settings_text(&v.settings),
                        ts(v.created_at),
                    ),
                )?;
            }
            // History restarts from the adopted state.
            tx.execute("DELETE FROM history WHERE photo_id = ?1", [photo_id])?;
            tx.execute("DELETE FROM history_cursor WHERE photo_id = ?1", [photo_id])?;
            push_history(tx, photo_id, &sc.settings, at)?;
            Reconciled {
                source: Source::Sidecar,
                sidecar_to_write: if rewrite {
                    Some(build_sidecar(tx, photo_id)?)
                } else {
                    None
                },
            }
        }
        (Some((_, t)), Some(sc)) => {
            let built = build_sidecar(tx, photo_id)?;
            let same = t == sc.updated_at
                && built.v == sc.v
                && built.settings == sc.settings
                && built.versions == sc.versions;
            Reconciled {
                source: Source::Db,
                sidecar_to_write: if same { None } else { Some(built) },
            }
        }
        (None, Some(_)) => unreachable!("a sidecar with no edits in the database is adopted"),
    };
    Ok(result)
}
