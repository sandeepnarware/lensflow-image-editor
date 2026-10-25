use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use lf_settings::EditSettings;
use serde_json::{Map, Value, json};

use crate::db::{CatalogError, Result};
use crate::model::{Version, parse_ts, ts};

pub const SIDECAR_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Sidecar {
    pub v: u32,
    pub settings: EditSettings,
    pub versions: Vec<Version>,
    pub updated_at: DateTime<Utc>,
    /// Top-level keys this version does not know, written back untouched.
    pub extra: Map<String, Value>,
}

/// `<photo>.lensflow.json`, beside the photo.
pub fn path_for(photo: &Path) -> PathBuf {
    let mut name = photo
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lensflow.json");
    photo.with_file_name(name)
}

/// `None` when the file does not exist; an error when it is not a valid sidecar.
pub fn read(path: &Path) -> Result<Option<Sidecar>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|e| CatalogError::Corrupt(e.to_string()))?;
    let Value::Object(mut map) = value else {
        return Err(CatalogError::Corrupt("not a JSON object".into()));
    };
    let v = map
        .remove("v")
        .and_then(|v| v.as_u64())
        .unwrap_or(SIDECAR_VERSION as u64) as u32;
    let settings = match map.remove("settings") {
        Some(s) => lf_settings::normalise(&s),
        None => EditSettings::default_settings(),
    };
    let updated_at = map
        .remove("updated_at")
        .and_then(|u| u.as_str().and_then(parse_ts))
        .unwrap_or(DateTime::UNIX_EPOCH);
    let mut versions = Vec::new();
    if let Some(Value::Array(list)) = map.remove("versions") {
        for item in list {
            let (Some(id), Some(created)) = (
                item.get("id").and_then(Value::as_str),
                item.get("created_at")
                    .and_then(Value::as_str)
                    .and_then(parse_ts),
            ) else {
                continue;
            };
            versions.push(Version {
                id: id.to_string(),
                label: item
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                settings: item
                    .get("settings")
                    .map(lf_settings::normalise)
                    .unwrap_or_else(EditSettings::default_settings),
                created_at: created,
            });
        }
    }
    Ok(Some(Sidecar {
        v,
        settings,
        versions,
        updated_at,
        extra: map,
    }))
}

impl Sidecar {
    pub fn to_value(&self) -> Value {
        let mut map = self.extra.clone();
        map.insert("v".into(), json!(self.v));
        map.insert("settings".into(), self.settings.to_value());
        map.insert(
            "versions".into(),
            Value::Array(
                self.versions
                    .iter()
                    .map(|x| {
                        json!({
                            "id": x.id,
                            "label": x.label,
                            "settings": x.settings.to_value(),
                            "created_at": ts(x.created_at),
                        })
                    })
                    .collect(),
            ),
        );
        map.insert("updated_at".into(), json!(ts(self.updated_at)));
        Value::Object(map)
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomic: write a temp file in the same directory, flush it, then rename over the target.
pub fn write(path: &Path, sidecar: &Sidecar) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&sidecar.to_value())?.as_bytes())?;
        f.sync_all()?;
        drop(f);
        // std's rename replaces an existing target on Windows (MoveFileEx with REPLACE_EXISTING).
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
