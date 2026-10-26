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

    // Saving the name again brings the row back, as a new preset: no link to the server
    // copy that is being deleted.
    presets::save(&c, STUDIO, "Golden", &json!({"v": 2})).unwrap();
    let back = presets::find(&c, STUDIO, "Golden").unwrap().unwrap();
    assert_eq!((back.remote_id, back.synced_at), (None, None));
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
        // A version-5 catalogue's presets table (the other tables do not matter here).
        let conn = rusqlite::Connection::open(db.join("catalog.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version (version) VALUES (1), (2), (3), (4), (5);
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
    assert_eq!(c.schema_version().unwrap(), 6);
    let rows = presets::list(&c, 0).unwrap();
    assert_eq!(names(&rows), ["Old"]);
    assert_eq!(rows[0].remote_id, Some(4));
    assert_eq!(rows[0].settings["exposure"], 1);
    assert_eq!(rows[0].deleted_at, None);
    // The same name is now free in a real studio.
    presets::save(&c, STUDIO, "Old", &json!({})).unwrap();
    assert_eq!(presets::list(&c, STUDIO).unwrap().len(), 1);
}
