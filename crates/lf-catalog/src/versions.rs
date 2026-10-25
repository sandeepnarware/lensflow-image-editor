//! Named snapshots of a photo's settings. Add and delete stamp the photo's `edits.updated_at`
//! so the sidecar built afterwards (see `edits::sidecar_for`) is newer than any older copy.

use lf_settings::EditSettings;

use crate::db::{Catalog, CatalogError, Result};
use crate::edits::{self, check_photo, load_versions, touch};
use crate::model::{Version, now, ts};
use crate::sidecar::Sidecar;

pub fn add(catalog: &Catalog, photo_id: i64, label: &str, s: &EditSettings) -> Result<Version> {
    let tx = catalog.conn.unchecked_transaction()?;
    check_photo(&tx, photo_id)?;
    let v = Version {
        id: uuid::Uuid::new_v4().to_string(),
        label: label.to_string(),
        settings: s.clone(),
        created_at: now(),
    };
    tx.execute(
        "INSERT INTO versions (id, photo_id, label, settings, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            &v.id,
            photo_id,
            &v.label,
            s.to_value().to_string(),
            ts(v.created_at),
        ),
    )?;
    touch(&tx, photo_id)?;
    tx.commit()?;
    Ok(v)
}

pub fn list(catalog: &Catalog, photo_id: i64) -> Result<Vec<Version>> {
    let tx = catalog.conn.unchecked_transaction()?;
    load_versions(&tx, photo_id)
}

/// Make a version's settings current (recorded in history); returns the sidecar to write.
pub fn use_version(catalog: &Catalog, photo_id: i64, version_id: &str) -> Result<Sidecar> {
    let found = list(catalog, photo_id)?
        .into_iter()
        .find(|v| v.id == version_id);
    let v = found.ok_or_else(|| CatalogError::NotFound(format!("version {version_id}")))?;
    edits::set_settings(catalog, photo_id, &v.settings)
}

/// True when a version was removed.
pub fn delete(catalog: &Catalog, photo_id: i64, version_id: &str) -> Result<bool> {
    let tx = catalog.conn.unchecked_transaction()?;
    let n = tx.execute(
        "DELETE FROM versions WHERE id = ?1 AND photo_id = ?2",
        (version_id, photo_id),
    )?;
    if n > 0 {
        touch(&tx, photo_id)?;
    }
    tx.commit()?;
    Ok(n > 0)
}
