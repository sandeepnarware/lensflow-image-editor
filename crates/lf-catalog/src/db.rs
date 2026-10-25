use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::model::{NewPhoto, now, ts};

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("corrupt sidecar: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

/// Each entry is one migration; the schema version is the number applied.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE folders (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    added_at TEXT NOT NULL
);
CREATE TABLE photos (
    id INTEGER PRIMARY KEY,
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    file_size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    sha1 TEXT,
    width INTEGER,
    height INTEGER,
    camera TEXT,
    lens TEXT,
    captured_at TEXT,
    kind TEXT NOT NULL DEFAULT '',
    decoder TEXT,
    orientation INTEGER NOT NULL DEFAULT 1,
    flag INTEGER NOT NULL DEFAULT 0,
    rating INTEGER NOT NULL DEFAULT 0,
    label TEXT,
    imported_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX photos_folder ON photos(folder_id);
CREATE TABLE edits (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    settings TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE history (
    id INTEGER PRIMARY KEY,
    photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    settings TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX history_photo ON history(photo_id, id);
CREATE TABLE history_cursor (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    history_id INTEGER NOT NULL
);
CREATE TABLE versions (
    id TEXT PRIMARY KEY,
    photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    settings TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX versions_photo ON versions(photo_id);
CREATE TABLE thumbnails (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    edge INTEGER NOT NULL,
    jpeg BLOB NOT NULL,
    made_for_updated_at TEXT NOT NULL
);
CREATE TABLE presets (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    settings TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    remote_id INTEGER,
    synced_at TEXT
);
CREATE TABLE export_jobs (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL,
    status TEXT NOT NULL,
    spec TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    error TEXT
);
"#,
    r#"
CREATE TABLE sidecar_meta (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    v INTEGER NOT NULL,
    extra TEXT NOT NULL
);
"#,
    "ALTER TABLE photos ADD COLUMN error TEXT;",
];

pub struct Catalog {
    pub(crate) conn: Connection,
}

impl Catalog {
    /// Open (creating if needed) `root/.lensflow/catalog.sqlite`, in WAL mode, migrated.
    pub fn open(root: &Path) -> Result<Catalog> {
        let dir = root.join(".lensflow");
        fs::create_dir_all(&dir)?;
        Self::from_connection(Connection::open(dir.join("catalog.sqlite"))?)
    }

    fn from_connection(mut conn: Connection) -> Result<Catalog> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;
        let current: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?;
        if current < MIGRATIONS.len() as i64 {
            let tx = conn.transaction()?;
            for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
                tx.execute_batch(sql)?;
                tx.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [(i + 1) as i64],
                )?;
            }
            tx.commit()?;
        }
        Ok(Catalog { conn })
    }

    /// A write transaction that takes the write lock up front (no deferred read-then-write
    /// upgrade, which fails with SQLITE_BUSY under WAL when another connection writes).
    pub(crate) fn write_tx(&self) -> Result<Transaction<'_>> {
        Ok(Transaction::new_unchecked(
            &self.conn,
            TransactionBehavior::Immediate,
        )?)
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?)
    }

    /// The folder's id, adding it when new.
    pub fn add_folder(&self, path: &str) -> Result<i64> {
        let tx = self.write_tx()?;
        tx.execute(
            "INSERT OR IGNORE INTO folders (path, added_at) VALUES (?1, ?2)",
            (path, ts(now())),
        )?;
        let id = tx.query_row("SELECT id FROM folders WHERE path = ?1", [path], |r| {
            r.get(0)
        })?;
        tx.commit()?;
        Ok(id)
    }

    /// Insert a photo row and return its id.
    pub fn add_photo(&self, folder_id: i64, p: &NewPhoto) -> Result<i64> {
        let tx = self.write_tx()?;
        let t = ts(now());
        tx.execute(
            "INSERT INTO photos (folder_id, path, file_size, mtime, width, height, camera, lens,
             captured_at, kind, decoder, orientation, imported_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            rusqlite::params![
                folder_id,
                p.path,
                p.file_size,
                p.mtime,
                p.width,
                p.height,
                p.camera,
                p.lens,
                p.captured_at,
                p.kind,
                p.decoder,
                p.orientation.max(1),
                t
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn photo_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))?)
    }

    pub fn photo_ids(&self) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare("SELECT id FROM photos ORDER BY id")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// `(kind, error)` of the photo at `path`.
    pub fn photo_kind_error(&self, path: &str) -> Result<(String, Option<String>)> {
        Ok(self.conn.query_row(
            "SELECT kind, error FROM photos WHERE path = ?1",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// `(width, height)` of the photo at `path`.
    pub fn photo_size(&self, path: &str) -> Result<(Option<u32>, Option<u32>)> {
        Ok(self.conn.query_row(
            "SELECT width, height FROM photos WHERE path = ?1",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// The stored thumbnail `(edge, jpeg)`.
    pub fn thumbnail(&self, photo_id: i64) -> Result<Option<(u32, Vec<u8>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT edge, jpeg FROM thumbnails WHERE photo_id = ?1",
                [photo_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn table_names(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
