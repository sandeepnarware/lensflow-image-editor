//! The studio presets kept on this computer, per studio, for the sync with LensFlow
//! (`galleries/edit-presets`). Names are unique within a studio. A delete leaves a
//! tombstone (`deleted_at`) so the next sync can delete the preset on the server too;
//! [`list`] hides tombstones, [`purge`] removes a row for good.

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
    Ok(id)
}

/// Saves a preset made on this computer, now: a row of that name (a tombstone too) is
/// replaced and keeps its link to the server, so the sync updates it there.
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

fn stamp(catalog: &Catalog, sql: &str, id: i64, name: Option<&str>) -> Result<()> {
    let tx = catalog.write_tx()?;
    let n = tx.execute(sql, rusqlite::params![id, ts(now()), name])?;
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
        "UPDATE presets SET name = ?3, updated_at = ?2 WHERE id = ?1",
        id,
        Some(name),
    )
}

/// Deletes a preset: a tombstone the sync deletes on the server, then purges.
pub fn delete(catalog: &Catalog, id: i64) -> Result<()> {
    stamp(
        catalog,
        "UPDATE presets SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1 AND ?3 IS NULL",
        id,
        None,
    )
}

/// Removes a row for good.
pub fn purge(catalog: &Catalog, id: i64) -> Result<()> {
    let tx = catalog.write_tx()?;
    tx.execute("DELETE FROM presets WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}
