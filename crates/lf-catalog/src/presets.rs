//! The studio presets kept on this computer, per studio, for the sync with LensFlow
//! (`galleries/edit-presets`). `studio_id` is a key, not the server's id as it is: the
//! studio's id for the production server, negative for the development server (the app's
//! `presets_key`), so the two servers' presets never mix. Names are unique within a studio. A delete leaves a
//! tombstone (`deleted_at`) so the next sync can delete the preset on the server too;
//! [`list`] hides tombstones, [`purge`] removes a row for good.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value;

use crate::db::{Catalog, CatalogError, Result};
use crate::model::{now, ts};

/// One preset row.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPresetRow {
    pub id: i64,
    pub studio_id: i64,
    pub name: String,
    /// A partial settings object (the groups the preset keeps).
    pub settings: Value,
    /// ISO-8601 UTC.
    pub updated_at: String,
    /// The preset's id on the server, once synced.
    pub remote_id: Option<i64>,
    /// When it last matched the server.
    pub synced_at: Option<String>,
    /// Set by [`delete`]; the sync deletes it on the server, then purges it.
    pub deleted_at: Option<String>,
}

const COLUMNS: &str = "id, studio_id, name, settings, updated_at, remote_id, synced_at, deleted_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<LocalPresetRow> {
    let settings: String = r.get(3)?;
    Ok(LocalPresetRow {
        id: r.get(0)?,
        studio_id: r.get(1)?,
        name: r.get(2)?,
        settings: serde_json::from_str(&settings).unwrap_or(Value::Null),
        updated_at: r.get(4)?,
        remote_id: r.get(5)?,
        synced_at: r.get(6)?,
        deleted_at: r.get(7)?,
    })
}

fn select(catalog: &Catalog, filter: &str, studio_id: i64) -> Result<Vec<LocalPresetRow>> {
    let mut st = catalog.conn.prepare(&format!(
        "SELECT {COLUMNS} FROM presets WHERE studio_id = ?1 AND {filter}
         ORDER BY name COLLATE NOCASE, name"
    ))?;
    let rows = st.query_map([studio_id], row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The studio's presets (not the deleted ones), by name.
pub fn list(catalog: &Catalog, studio_id: i64) -> Result<Vec<LocalPresetRow>> {
    select(catalog, "deleted_at IS NULL", studio_id)
}

/// The studio's deleted presets still waiting for the sync.
pub fn tombstones(catalog: &Catalog, studio_id: i64) -> Result<Vec<LocalPresetRow>> {
    select(catalog, "deleted_at IS NOT NULL", studio_id)
}

/// The row named `name` (a tombstone too).
pub fn find(catalog: &Catalog, studio_id: i64, name: &str) -> Result<Option<LocalPresetRow>> {
    Ok(catalog
        .conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM presets WHERE studio_id = ?1 AND name = ?2"),
            (studio_id, name),
            row,
        )
        .optional()?)
}

fn id_where(tx: &Transaction, sql: &str, p: impl rusqlite::Params) -> Result<Option<i64>> {
    Ok(tx.query_row(sql, p, |r| r.get(0)).optional()?)
}

/// A row as the sync read it before going to the server: its `updated_at` and `deleted_at`
/// then. A row whose stamps no longer match was saved, renamed or deleted here meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

impl Seen {
    pub fn of(row: &LocalPresetRow) -> Seen {
        Seen {
            updated_at: row.updated_at.clone(),
            deleted_at: row.deleted_at.clone(),
        }
    }
}

/// The rows as the sync read them, by row id.
pub type SeenRows = HashMap<i64, Seen>;

/// Whether row `id` is still as `seen` holds it (a row missing from `seen` appeared since).
fn unchanged(tx: &Transaction, id: i64, seen: &SeenRows) -> Result<bool> {
    let now: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT updated_at, deleted_at FROM presets WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match (now, seen.get(&id)) {
        (Some((updated_at, deleted_at)), Some(s)) => {
            s.updated_at == updated_at && s.deleted_at == deleted_at
        }
        _ => false,
    })
}

/// Writes a preset exactly as given (the sync's pulls and pushes): the row with this
/// `remote_id` when there is one (it follows a rename on the server), else the row with this
/// name, else a new row. A tombstone written this way comes back. Returns the row's id.
pub fn upsert(
    catalog: &Catalog,
    studio_id: i64,
    name: &str,
    settings: &Value,
    updated_at: &str,
    remote_id: Option<i64>,
    synced_at: Option<&str>,
) -> Result<i64> {
    let written = write(
        catalog, studio_id, name, settings, updated_at, remote_id, synced_at, None,
    )?;
    written.ok_or_else(|| CatalogError::NotFound(format!("preset {name}")))
}

/// [`upsert`] for the sync, as a compare-and-set against `seen` (the rows as the sync read
/// them before going to the server): it writes only when every row it would touch (the one
/// with this `remote_id`, the one with this name) is still as read, so a preset saved,
/// renamed or deleted here meanwhile is never overwritten or brought back. Then nothing is
/// written but the link to the server (`remote_id`, `synced_at`) on a row that had none, so
/// the next sync updates or deletes the server's copy as the change here says: `Ok(None)`.
#[allow(clippy::too_many_arguments)]
pub fn upsert_if_unchanged(
    catalog: &Catalog,
    studio_id: i64,
    name: &str,
    settings: &Value,
    updated_at: &str,
    remote_id: Option<i64>,
    synced_at: Option<&str>,
    seen: &SeenRows,
) -> Result<Option<i64>> {
    write(
        catalog,
        studio_id,
        name,
        settings,
        updated_at,
        remote_id,
        synced_at,
        Some(seen),
    )
}

/// Removes row `id` for good, unless it changed since the sync read it (`seen`): true when
/// it was removed.
pub fn purge_if_unchanged(catalog: &Catalog, id: i64, seen: &SeenRows) -> Result<bool> {
    let tx = catalog.write_tx()?;
    if !unchanged(&tx, id, seen)? {
        return Ok(false);
    }
    tx.execute("DELETE FROM presets WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn write(
    catalog: &Catalog,
    studio_id: i64,
    name: &str,
    settings: &Value,
    updated_at: &str,
    remote_id: Option<i64>,
    synced_at: Option<&str>,
    seen: Option<&SeenRows>,
) -> Result<Option<i64>> {
    let tx = catalog.write_tx()?;
    let by_remote = match remote_id {
        Some(rid) => id_where(
            &tx,
            "SELECT id FROM presets WHERE studio_id = ?1 AND remote_id = ?2",
            (studio_id, rid),
        )?,
        None => None,
    };
    let by_name = id_where(
        &tx,
        "SELECT id FROM presets WHERE studio_id = ?1 AND name = ?2",
        (studio_id, name),
    )?;
    if let Some(seen) = seen {
        for id in [by_remote, by_name].into_iter().flatten() {
            if unchanged(&tx, id, seen)? {
                continue;
            }
            // Changed here since the sync read it: keep the change, and only link the row
            // to the server's copy when it has no link yet.
            if let (Some(rid), Some(target)) = (remote_id, by_remote.or(by_name)) {
                tx.execute(
                    "UPDATE presets SET remote_id = ?2, synced_at = ?3
                     WHERE id = ?1 AND remote_id IS NULL",
                    rusqlite::params![target, rid, synced_at],
                )?;
                tx.commit()?;
            }
            return Ok(None);
        }
    }
    let id = match (by_remote, by_name) {
        (Some(id), Some(other)) if other != id => {
            // Renamed on the server onto a name this computer still uses: only a tombstone
            // gives way.
            let gone: bool = tx.query_row(
                "SELECT deleted_at IS NOT NULL FROM presets WHERE id = ?1",
                [other],
                |r| r.get(0),
            )?;
            if !gone {
                return Err(CatalogError::PresetNameTaken(name.to_owned()));
            }
            tx.execute("DELETE FROM presets WHERE id = ?1", [other])?;
            Some(id)
        }
        (Some(id), _) | (None, Some(id)) => Some(id),
        (None, None) => None,
    };
    let text = settings.to_string();
    let id = match id {
        Some(id) => {
            tx.execute(
                "UPDATE presets SET name = ?2, settings = ?3, updated_at = ?4, remote_id = ?5,
                 synced_at = ?6, deleted_at = NULL WHERE id = ?1",
                rusqlite::params![id, name, text, updated_at, remote_id, synced_at],
            )?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO presets (studio_id, name, settings, updated_at, remote_id, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![studio_id, name, text, updated_at, remote_id, synced_at],
            )?;
            tx.last_insert_rowid()
        }
    };
    tx.commit()?;
    Ok(Some(id))
}

/// Saves a preset made on this computer, now. A row of that name, a tombstone too, is
/// replaced and keeps its link to the server (`remote_id`, `synced_at`) with a new
/// `updated_at`, so the next sync reads it as changed here: an unchanged server copy is
/// updated in place (the replace the user meant), one deleted on the web is created again,
/// one changed on the web is a conflict (both kept).
pub fn save(catalog: &Catalog, studio_id: i64, name: &str, settings: &Value) -> Result<i64> {
    let existing = find(catalog, studio_id, name)?;
    let (remote_id, synced_at) = existing.map_or((None, None), |r| (r.remote_id, r.synced_at));
    upsert(
        catalog,
        studio_id,
        name,
        settings,
        &ts(now()),
        remote_id,
        synced_at.as_deref(),
    )
}

fn stamp(catalog: &Catalog, id: i64, sql: &str, params: impl rusqlite::Params) -> Result<()> {
    let tx = catalog.write_tx()?;
    let n = tx.execute(sql, params)?;
    tx.commit()?;
    if n == 0 {
        return Err(CatalogError::NotFound(format!("preset {id}")));
    }
    Ok(())
}

/// Renames a preset here (the sync renames it on the server).
pub fn rename(catalog: &Catalog, id: i64, name: &str) -> Result<()> {
    stamp(
        catalog,
        id,
        "UPDATE presets SET name = ?3, updated_at = ?2 WHERE id = ?1",
        rusqlite::params![id, ts(now()), name],
    )
}

/// Deletes a preset: a tombstone the sync deletes on the server, then purges.
pub fn delete(catalog: &Catalog, id: i64) -> Result<()> {
    stamp(
        catalog,
        id,
        "UPDATE presets SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1",
        rusqlite::params![id, ts(now())],
    )
}

/// Removes a row for good.
pub fn purge(catalog: &Catalog, id: i64) -> Result<()> {
    let tx = catalog.write_tx()?;
    tx.execute("DELETE FROM presets WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod doc_tests {
    #[test]
    fn the_module_doc_says_what_studio_id_is() {
        let doc: String = include_str!("presets.rs")
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//!").trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(doc.contains("`studio_id`"), "{doc}");
        assert!(doc.contains("negative for the development server"), "{doc}");
    }
}
