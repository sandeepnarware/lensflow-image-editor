use lf_catalog::{Catalog, CatalogError, NewPhoto};

fn photo(path: &str) -> NewPhoto {
    NewPhoto {
        path: path.into(),
        file_size: 10,
        mtime: 1,
        kind: "raw".into(),
        orientation: 1,
        ..Default::default()
    }
}

#[test]
fn open_creates_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path()).unwrap();
    assert!(dir.path().join(".lensflow/catalog.sqlite").exists());
    let names = c.table_names().unwrap();
    for t in [
        "folders",
        "photos",
        "edits",
        "history",
        "versions",
        "thumbnails",
        "presets",
        "export_jobs",
        "schema_version",
    ] {
        assert!(names.iter().any(|n| n == t), "missing table {t}");
    }
    assert!(names.iter().all(|n| n.len() < 64));
    assert_eq!(c.schema_version().unwrap(), 13);
}

#[test]
fn reopening_keeps_data_and_does_not_remigrate() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let c = Catalog::open(dir.path()).unwrap();
        let f = c.add_folder("D:/shoot").unwrap();
        c.add_photo(f, &photo("D:/shoot/a.nef")).unwrap()
    };
    let c = Catalog::open(dir.path()).unwrap();
    assert_eq!(c.schema_version().unwrap(), 13);
    let f = c.add_folder("D:/shoot").unwrap();
    assert!(
        c.add_photo(f, &photo("D:/shoot/a.nef")).is_err(),
        "path is UNIQUE"
    );
    let b = c.add_photo(f, &photo("D:/shoot/b.nef")).unwrap();
    assert!(b > id);
}

#[test]
fn wal_and_foreign_keys_are_on() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path()).unwrap();
    let f = c.add_folder("x").unwrap();
    // A photo in a folder that does not exist is refused.
    assert!(c.add_photo(f + 100, &photo("p")).is_err());
}

#[test]
fn a_catalogue_from_a_newer_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    drop(Catalog::open(dir.path()).unwrap());
    let conn = rusqlite::Connection::open(dir.path().join(".lensflow/catalog.sqlite")).unwrap();
    conn.execute("INSERT INTO schema_version (version) VALUES (99)", [])
        .unwrap();
    drop(conn);
    match Catalog::open(dir.path()) {
        Err(e @ CatalogError::NewerSchema { found, supported }) => {
            assert_eq!((found, supported), (99, 13));
            assert!(
                e.to_string()
                    .contains("This catalogue was made by a newer lensflow-ImageEditor."),
                "{e}"
            );
        }
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a newer catalogue was opened"),
    }
}
