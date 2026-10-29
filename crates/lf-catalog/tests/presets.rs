use lf_catalog::Catalog;
use lf_catalog::presets::{self, LocalPresetRow};
use serde_json::json;

fn catalogue() -> (tempfile::TempDir, Catalog) {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path()).unwrap();
    (dir, c)
}

const STUDIO: i64 = 7;
const OTHER: i64 = 8;

fn names(rows: &[LocalPresetRow]) -> Vec<&str> {
    rows.iter().map(|r| r.name.as_str()).collect()
}

#[test]
fn presets_crud_round_trip() {
    let (_d, c) = catalogue();
    let warm = presets::upsert(
        &c,
        STUDIO,
        "Warm",
        &json!({"v": 2, "temperature": 20}),
        "2026-10-01T00:00:00.000Z",
        None,
        None,
    )
    .unwrap();
    presets::upsert(
        &c,
        STUDIO,
        "Cool",
        &json!({"v": 2, "temperature": -20}),
        "2026-10-02T00:00:00.000Z",
        Some(41),
        Some("2026-10-02T00:00:00.000Z"),
    )
    .unwrap();
    // Another studio's preset of the same name never mixes in.
    presets::upsert(
        &c,
        OTHER,
        "Warm",
        &json!({}),
        "2026-10-01T00:00:00.000Z",
        None,
        None,
    )
    .unwrap();

    let rows = presets::list(&c, STUDIO).unwrap();
    assert_eq!(names(&rows), ["Cool", "Warm"], "by name");
    assert_eq!(rows[0].remote_id, Some(41));
    assert_eq!(
        rows[0].synced_at.as_deref(),
        Some("2026-10-02T00:00:00.000Z")
    );
    assert_eq!(rows[1].settings["temperature"], 20);
    assert_eq!(names(&presets::list(&c, OTHER).unwrap()), ["Warm"]);

    // Upsert by name replaces the row (same id).
    let again = presets::upsert(
        &c,
        STUDIO,
        "Warm",
        &json!({"v": 2, "temperature": 30}),
        "2026-10-03T00:00:00.000Z",
        Some(42),
        Some("2026-10-03T00:00:00.000Z"),
    )
    .unwrap();
    assert_eq!(again, warm);
    let rows = presets::list(&c, STUDIO).unwrap();
    assert_eq!(rows[1].settings["temperature"], 30);
    assert_eq!(rows[1].remote_id, Some(42));

    // Upsert by remote id follows a rename on the server.
    let renamed = presets::upsert(
        &c,
        STUDIO,
        "Warmer",
        &json!({"v": 2}),
        "2026-10-04T00:00:00.000Z",
        Some(42),
        Some("2026-10-04T00:00:00.000Z"),
    )
    .unwrap();
    assert_eq!(renamed, warm);
    assert_eq!(
        names(&presets::list(&c, STUDIO).unwrap()),
        ["Cool", "Warmer"]
    );

    // Rename here: the name changes and the row counts as changed.
    presets::rename(&c, warm, "Golden").unwrap();
    let row = presets::find(&c, STUDIO, "Golden").unwrap().unwrap();
    assert_eq!(row.id, warm);
    assert!(row.updated_at.as_str() > "2026-10-04T00:00:00.000Z");

    // A local save keeps the link to the server and stamps the time.
    let saved = presets::save(&c, STUDIO, "Golden", &json!({"v": 2, "exposure": 1})).unwrap();
    assert_eq!(saved, warm);
    let row = presets::find(&c, STUDIO, "Golden").unwrap().unwrap();
    assert_eq!(
        (row.remote_id, row.settings["exposure"].as_i64()),
        (Some(42), Some(1))
    );

    // Delete leaves a tombstone: hidden from the list, kept for the sync.
    presets::delete(&c, warm).unwrap();
    assert_eq!(names(&presets::list(&c, STUDIO).unwrap()), ["Cool"]);
    let gone = presets::tombstones(&c, STUDIO).unwrap();
    assert_eq!(names(&gone), ["Golden"]);
    assert!(gone[0].deleted_at.is_some());
    assert!(presets::tombstones(&c, OTHER).unwrap().is_empty());

    // Saving the name again brings the row back, still linked to its server copy, and
    // changed here (a new `updated_at`), so the sync replaces the server copy.
    let gone = presets::tombstones(&c, STUDIO).unwrap().remove(0);
    presets::save(&c, STUDIO, "Golden", &json!({"v": 2})).unwrap();
    let back = presets::find(&c, STUDIO, "Golden").unwrap().unwrap();
    assert_eq!(
        (back.remote_id, &back.synced_at),
        (gone.remote_id, &gone.synced_at)
    );
    assert_eq!(back.remote_id, Some(42));
    assert_eq!(back.deleted_at, None);
    assert_ne!(Some(back.updated_at.as_str()), back.synced_at.as_deref());
    assert_eq!(
        names(&presets::list(&c, STUDIO).unwrap()),
        ["Cool", "Golden"]
    );
    assert!(presets::tombstones(&c, STUDIO).unwrap().is_empty());

    // Purge removes the row for good.
    presets::purge(&c, warm).unwrap();
    assert_eq!(names(&presets::list(&c, STUDIO).unwrap()), ["Cool"]);
    assert!(presets::tombstones(&c, STUDIO).unwrap().is_empty());
}

#[test]
fn migration_6_keeps_existing_presets_under_no_studio() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join(".lensflow");
    std::fs::create_dir_all(&db).unwrap();
    {
        // A version-5 catalogue's presets table, and a bare photos table (the later
        // migrations add columns to it; its other columns do not matter here).
        let conn = rusqlite::Connection::open(db.join("catalog.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version (version) VALUES (1), (2), (3), (4), (5);
             CREATE TABLE photos (id INTEGER PRIMARY KEY);
             CREATE TABLE presets (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE,
                 settings TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 remote_id INTEGER,
                 synced_at TEXT
             );
             INSERT INTO presets (name, settings, updated_at, remote_id, synced_at)
             VALUES ('Old', '{\"v\":2,\"exposure\":1}', '2026-10-01T00:00:00.000Z', 4,
                     '2026-10-01T00:00:00.000Z');",
        )
        .unwrap();
    }
    let c = Catalog::open(dir.path()).unwrap();
    assert_eq!(c.schema_version().unwrap(), 13);
    let rows = presets::list(&c, 0).unwrap();
    assert_eq!(names(&rows), ["Old"]);
    assert_eq!(rows[0].remote_id, Some(4));
    assert_eq!(rows[0].settings["exposure"], 1);
    assert_eq!(rows[0].deleted_at, None);
    // The same name is now free in a real studio.
    presets::save(&c, STUDIO, "Old", &json!({})).unwrap();
    assert_eq!(presets::list(&c, STUDIO).unwrap().len(), 1);
}

/// The sync's writes are a compare-and-set against the rows as it read them: a preset
/// deleted, saved or made here meanwhile is never brought back or overwritten; only a row
/// without a link to the server gets one.
#[test]
fn the_sync_never_revives_or_overwrites_a_row_changed_after_it_read_it() {
    let (_d, c) = catalogue();
    let t0 = "2026-10-01T00:00:00.000Z";
    let t1 = "2026-10-02T00:00:00.000Z";
    let id = presets::upsert(&c, STUDIO, "Warm", &json!({"v": 2}), t0, None, None).unwrap();
    let read = |c: &Catalog| -> presets::SeenRows {
        presets::list(c, STUDIO)
            .unwrap()
            .iter()
            .chain(&presets::tombstones(c, STUDIO).unwrap())
            .map(|r| (r.id, presets::Seen::of(r)))
            .collect()
    };

    // Unchanged since it was read: written.
    let seen = read(&c);
    let written = presets::upsert_if_unchanged(
        &c,
        STUDIO,
        "Warm",
        &json!({"v": 2, "exposure": 1}),
        t1,
        Some(9),
        Some(t1),
        &seen,
    )
    .unwrap();
    assert_eq!(written, Some(id));

    // Deleted here after the read: it stays deleted.
    let seen = read(&c);
    presets::delete(&c, id).unwrap();
    let written = presets::upsert_if_unchanged(
        &c,
        STUDIO,
        "Warm",
        &json!({"v": 2}),
        t1,
        Some(9),
        Some(t1),
        &seen,
    )
    .unwrap();
    assert_eq!(written, None);
    assert!(presets::list(&c, STUDIO).unwrap().is_empty(), "not revived");
    assert_eq!(names(&presets::tombstones(&c, STUDIO).unwrap()), ["Warm"]);
    assert!(
        !presets::purge_if_unchanged(&c, id, &seen).unwrap(),
        "nor purged"
    );

    // A new row took a name after the read: kept, and linked to the server's copy.
    let seen = read(&c);
    let cool = presets::save(&c, STUDIO, "Cool", &json!({"v": 2, "contrast": 5})).unwrap();
    let written = presets::upsert_if_unchanged(
        &c,
        STUDIO,
        "Cool",
        &json!({"v": 2}),
        t1,
        Some(12),
        Some(t1),
        &seen,
    )
    .unwrap();
    assert_eq!(written, None);
    let row = presets::find(&c, STUDIO, "Cool").unwrap().unwrap();
    assert_eq!(row.id, cool);
    assert_eq!(
        row.settings,
        json!({"v": 2, "contrast": 5}),
        "the change here stays"
    );
    assert_eq!(row.remote_id, Some(12));

    // Unchanged rows purge.
    let seen = read(&c);
    assert!(presets::purge_if_unchanged(&c, id, &seen).unwrap());
    assert!(presets::find(&c, STUDIO, "Warm").unwrap().is_none());
}
