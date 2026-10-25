use chrono::{DateTime, SecondsFormat, Utc};
use lf_settings::EditSettings;

use crate::sidecar::Sidecar;

/// A named snapshot of settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub id: String,
    pub label: String,
    pub settings: EditSettings,
    pub created_at: DateTime<Utc>,
}

/// The columns an importer must supply for a photo row (the rest are filled later).
#[derive(Clone, Debug, Default)]
pub struct NewPhoto {
    pub path: String,
    pub file_size: i64,
    pub mtime: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub captured_at: Option<String>,
    pub kind: String,
    pub decoder: Option<String>,
    pub orientation: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Db,
    Sidecar,
}

/// Outcome of comparing the database with a sidecar on reopen.
#[derive(Clone, Debug)]
pub struct Reconciled {
    pub source: Source,
    pub sidecar_to_write: Option<Sidecar>,
}

/// ISO-8601 UTC with millisecond precision, the form stored in the database and sidecars.
pub fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Now, truncated to the stored precision so a value compares equal after a round trip.
pub fn now() -> DateTime<Utc> {
    parse_ts(&ts(Utc::now())).unwrap_or_else(Utc::now)
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}
