use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::cull::{Flag, Label, MAX_RATING};
use crate::meta::Meta;
use crate::model::{NewPhoto, now, ts};
use crate::scores::Scores;

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
    #[error("A rating is 0 to 5 stars, not {0}.")]
    BadRating(u8),
    #[error("corrupt sidecar: {0}")]
    Corrupt(String),
    #[error("export: {0}")]
    Export(String),
    #[error("You already have a preset called {0}.")]
    PresetNameTaken(String),
    #[error("You already have an album called {0}.")]
    AlbumNameTaken(String),
    #[error("already exists: {}", .0.display())]
    Exists(std::path::PathBuf),
    #[error(
        "photos {} and {} would both be exported to {}; add {{seq}} to the file name pattern",
        .photos.0, .photos.1, .path.display()
    )]
    NameCollision {
        path: std::path::PathBuf,
        photos: (i64, i64),
    },
    #[error(
        "Exporting at this size would make a {width}×{height} picture; the largest this \
         graphics device allows is {limit} pixels on a side. Choose a smaller size."
    )]
    TooLarge { width: u32, height: u32, limit: u32 },
    /// The graphics device was lost (a driver reset, sleep): an export stops at once.
    #[error(
        "The graphics device was reset, so the export stopped. Export the remaining photos \
         again."
    )]
    DeviceLost,
    /// An export would write over a catalogued photo (or a sidecar): refused, whatever
    /// `overwrite` says, before anything is written.
    #[error(
        "{} is one of your photos. Choose another folder or file name pattern.",
        .0.file_name().map_or_else(|| .0.display().to_string(), |n| n.to_string_lossy().into_owned())
    )]
    WouldOverwriteOriginal(std::path::PathBuf),
    #[error(
        "This catalogue was made by a newer lensflow-ImageEditor. Update the app to open it \
         (catalogue schema {found}, this version understands up to {supported})."
    )]
    NewerSchema { found: i64, supported: i64 },
}

impl CatalogError {
    /// Another connection held the catalogue's write lock past the busy timeout
    /// (`SQLITE_BUSY` or `SQLITE_LOCKED`): worth trying again shortly.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            CatalogError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if matches!(
                    e.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        )
    }
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
    // Set by `import::mark_missing` when a file is gone from its folder; cleared when
    // `import::run` sees the file again.
    "ALTER TABLE photos ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;",
    // A damaged sidecar, kept apart from `error` (the file and its thumbnail): import and
    // `edits::reconcile` write it, thumbnails never touch it.
    "ALTER TABLE photos ADD COLUMN sidecar_error TEXT;",
    // Studio presets are kept per studio (names are unique within one), and a delete leaves
    // a tombstone (`deleted_at`) for the sync. Rows from before belong to no studio (0).
    r#"
CREATE TABLE presets_new (
    id INTEGER PRIMARY KEY,
    studio_id INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL,
    settings TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    remote_id INTEGER,
    synced_at TEXT,
    deleted_at TEXT,
    UNIQUE (studio_id, name)
);
INSERT INTO presets_new (id, studio_id, name, settings, updated_at, remote_id, synced_at)
    SELECT id, 0, name, settings, updated_at, remote_id, synced_at FROM presets;
DROP TABLE presets;
ALTER TABLE presets_new RENAME TO presets;
"#,
    // Descriptive metadata (`crate::meta`): text fields on the photo, keywords apart (one
    // row per photo and word; a word is the same word in any case).
    r#"
ALTER TABLE photos ADD COLUMN title TEXT;
ALTER TABLE photos ADD COLUMN caption TEXT;
ALTER TABLE photos ADD COLUMN creator TEXT;
ALTER TABLE photos ADD COLUMN copyright TEXT;
CREATE TABLE keywords (
    photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    keyword TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (photo_id, keyword)
);
CREATE INDEX keywords_word ON keywords(keyword);
"#,
    // Albums (`crate::albums`): a smart album keeps its rule as JSON and no rows in
    // `album_photos`; a plain one keeps its photos in the order they were added.
    r#"
CREATE TABLE albums (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    rule TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE album_photos (
    album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (album_id, photo_id)
);
CREATE INDEX album_photos_photo ON album_photos(photo_id);
"#,
    // Where each photo was taken (EXIF GPS, degrees), and whether its file has been read for
    // it (photos catalogued before this version are read when the map first wants them).
    r#"
ALTER TABLE photos ADD COLUMN lat REAL;
ALTER TABLE photos ADD COLUMN lon REAL;
ALTER TABLE photos ADD COLUMN gps_read INTEGER NOT NULL DEFAULT 0;
"#,
    // Assisted-culling scores (`crate::scores`), for the file as it was scored.
    r#"
CREATE TABLE scores (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    file_size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    sharpness REAL NOT NULL,
    faces INTEGER NOT NULL,
    eyes_closed INTEGER NOT NULL,
    eye_sharpness REAL
);
"#,
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

    /// How long a write waits for another connection's write lock before it fails as busy
    /// ([`CatalogError::is_busy`]); 10 s by default.
    pub fn set_busy_timeout(&self, timeout: std::time::Duration) -> Result<()> {
        self.conn.busy_timeout(timeout)?;
        Ok(())
    }

    fn from_connection(mut conn: Connection) -> Result<Catalog> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        let supported = MIGRATIONS.len() as i64;
        let read_version = |c: &Connection| -> Result<i64> {
            let exists: bool = c.query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
                [],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(0);
            }
            Ok(c.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )?)
        };
        // A quick look first, so an up-to-date catalogue opens without the write lock.
        let seen = read_version(&conn)?;
        if seen > supported {
            return Err(CatalogError::NewerSchema {
                found: seen,
                supported,
            });
        }
        if seen < supported {
            // Re-read under the write lock: another process may have migrated meanwhile.
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
            )?;
            let current = read_version(&tx)?;
            if current > supported {
                return Err(CatalogError::NewerSchema {
                    found: current,
                    supported,
                });
            }
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

    /// Whether the photo at `path` was flagged missing by [`crate::import::mark_missing`].
    pub fn photo_missing(&self, path: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT missing FROM photos WHERE path = ?1", [path], |r| {
                r.get(0)
            })?)
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

/// A catalogued folder and how many photos it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub path: PathBuf,
    pub count: i64,
}

impl Folder {
    /// The folder's own name (the last part of its path), for the sidebar.
    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

/// One photo as the library shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoRow {
    pub id: i64,
    pub folder_id: i64,
    pub path: PathBuf,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub captured_at: Option<String>,
    pub kind: String,
    /// Why the file or its thumbnail could not be read: a message to show, never a reason
    /// not to ask for a thumbnail again.
    pub error: Option<String>,
    /// Why the sidecar beside the photo could not be read.
    pub sidecar_error: Option<String>,
    pub missing: bool,
    /// The photo has a row in `edits`.
    pub edited: bool,
    pub updated_at: String,
    /// Pick or reject ([`crate::cull`]).
    pub flag: Flag,
    /// 0 (no stars) to 5.
    pub rating: u8,
    /// `None` also when the stored label is one this version does not know.
    pub label: Option<Label>,
    /// Title, caption, creator, copyright and keywords ([`crate::meta`]).
    pub meta: Meta,
    /// Where it was taken (latitude, longitude in degrees), when its EXIF says.
    pub gps: Option<(f64, f64)>,
    /// Assisted-culling scores for the file as it is now ([`crate::scores`]).
    pub scores: Option<Scores>,
}

pub(crate) const PHOTO_COLUMNS: &str =
    "p.id, p.folder_id, p.path, p.width, p.height, p.camera, p.lens,
     p.captured_at, p.kind, p.error, p.sidecar_error, p.missing,
     EXISTS (SELECT 1 FROM edits e WHERE e.photo_id = p.id), p.updated_at,
     p.flag, p.rating, p.label, p.title, p.caption, p.creator, p.copyright,
     (SELECT group_concat(keyword, char(31)) FROM
         (SELECT keyword FROM keywords k WHERE k.photo_id = p.id ORDER BY keyword)),
     p.lat, p.lon,
     (SELECT s.sharpness FROM scores s WHERE s.photo_id = p.id AND s.file_size = p.file_size
         AND s.mtime = p.mtime),
     (SELECT s.faces FROM scores s WHERE s.photo_id = p.id),
     (SELECT s.eyes_closed FROM scores s WHERE s.photo_id = p.id),
     (SELECT s.eye_sharpness FROM scores s WHERE s.photo_id = p.id)";

pub(crate) fn photo_row(r: &rusqlite::Row) -> rusqlite::Result<PhotoRow> {
    Ok(PhotoRow {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        path: PathBuf::from(r.get::<_, String>(2)?),
        width: r.get(3)?,
        height: r.get(4)?,
        camera: r.get(5)?,
        lens: r.get(6)?,
        captured_at: r.get(7)?,
        kind: r.get(8)?,
        error: r.get(9)?,
        sidecar_error: r.get(10)?,
        missing: r.get(11)?,
        edited: r.get(12)?,
        updated_at: r.get(13)?,
        flag: Flag::from_stored(r.get(14)?),
        // Clamped, so a hand-edited row cannot show more than five stars.
        rating: r.get::<_, i64>(15)?.clamp(0, i64::from(MAX_RATING)) as u8,
        label: r
            .get::<_, Option<String>>(16)?
            .as_deref()
            .and_then(Label::from_key),
        meta: Meta {
            title: r.get(17)?,
            caption: r.get(18)?,
            creator: r.get(19)?,
            copyright: r.get(20)?,
            keywords: r
                .get::<_, Option<String>>(21)?
                .map(|k| k.split('\u{1f}').map(str::to_owned).collect())
                .unwrap_or_default(),
        },
        gps: r
            .get::<_, Option<f64>>(22)?
            .zip(r.get::<_, Option<f64>>(23)?),
        // The sharpness is read only for the file as scored, so it alone says whether the
        // scores are fresh.
        scores: r
            .get::<_, Option<f64>>(24)?
            .map(|sharpness| -> rusqlite::Result<Scores> {
                Ok(Scores {
                    sharpness: sharpness as f32,
                    faces: r.get::<_, Option<u32>>(25)?.unwrap_or(0),
                    eyes_closed: r.get::<_, Option<u32>>(26)?.unwrap_or(0),
                    eye_sharpness: r.get::<_, Option<f64>>(27)?.map(|v| v as f32),
                })
            })
            .transpose()?,
    })
}

impl Catalog {
    /// Every folder with its photo count, by path.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut st = self.conn.prepare(
            "SELECT f.id, f.path, (SELECT COUNT(*) FROM photos p WHERE p.folder_id = f.id)
             FROM folders f ORDER BY f.path",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The folder's photos by capture time, then path; photos without a capture time last.
    pub fn photos_in(&self, folder_id: i64) -> Result<Vec<PhotoRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {PHOTO_COLUMNS} FROM photos p WHERE p.folder_id = ?1
             ORDER BY p.captured_at IS NULL, p.captured_at, p.path"
        ))?;
        let rows = st.query_map([folder_id], photo_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// One photo, if it is catalogued.
    pub fn photo(&self, id: i64) -> Result<Option<PhotoRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PHOTO_COLUMNS} FROM photos p WHERE p.id = ?1"),
                [id],
                photo_row,
            )
            .optional()?)
    }

    /// Forgets a folder and everything the catalogue knows about its photos (edits, history,
    /// versions, thumbnails). The files themselves, and their sidecars, are never touched.
    pub fn remove_folder(&self, id: i64) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}
