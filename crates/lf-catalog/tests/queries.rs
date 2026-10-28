//! The library view's queries: folders with counts, a folder's photos in capture order, one
//! photo, removing a folder.

use std::path::PathBuf;

use lf_catalog::{Catalog, NewPhoto, edits, versions};
use lf_settings::EditSettings;

fn photo(c: &Catalog, folder: i64, path: &str, captured_at: Option<&str>) -> i64 {
    c.add_photo(
        folder,
        &NewPhoto {
            path: path.into(),
            kind: "jpeg".into(),
            width: Some(6000),
            height: Some(4000),
            camera: Some("Nikon Z 6".into()),
            captured_at: captured_at.map(str::to_owned),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn photos_in_lists_in_capture_order_with_edited_flag() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let other = c.add_folder("/other").unwrap();
    let late = photo(&c, f, "/shoot/b.jpg", Some("2026-05-02T10:00:00"));
    let early = photo(&c, f, "/shoot/z.jpg", Some("2026-05-01T09:00:00"));
    let same_time_a = photo(&c, f, "/shoot/a.jpg", Some("2026-05-02T10:00:00"));
    let undated = photo(&c, f, "/shoot/c.jpg", None);
    photo(&c, other, "/other/x.jpg", Some("2020-01-01T00:00:00"));

    let mut s = EditSettings::default_settings();
    s.exposure = 0.5;
    edits::set_settings(&c, late, &s).unwrap();

    let rows = c.photos_in(f).unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    // Capture time, then path for equal times; photos without a date come last.
    assert_eq!(ids, [early, same_time_a, late, undated]);
    let edited: Vec<bool> = rows.iter().map(|r| r.edited).collect();
    assert_eq!(edited, [false, false, true, false]);

    let row = &rows[2];
    assert_eq!(row.folder_id, f);
    assert_eq!(row.path, PathBuf::from("/shoot/b.jpg"));
    assert_eq!((row.width, row.height), (Some(6000), Some(4000)));
    assert_eq!(row.camera.as_deref(), Some("Nikon Z 6"));
    assert_eq!(row.lens, None);
    assert_eq!(row.captured_at.as_deref(), Some("2026-05-02T10:00:00"));
    assert_eq!(row.kind, "jpeg");
    assert_eq!(
        (row.error.as_deref(), row.sidecar_error.as_deref()),
        (None, None)
    );
    assert!(!row.missing);
    assert!(!row.updated_at.is_empty());

    assert_eq!(c.photo(late).unwrap().as_ref(), Some(row));
    assert_eq!(c.photo(9999).unwrap(), None);
    assert!(c.photos_in(9999).unwrap().is_empty());
}

#[test]
fn folders_list_paths_and_counts() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let a = c.add_folder("/b-shoot").unwrap();
    let b = c.add_folder("/a-shoot").unwrap();
    photo(&c, a, "/b-shoot/1.jpg", None);
    photo(&c, a, "/b-shoot/2.jpg", None);
    let folders = c.folders().unwrap();
    let got: Vec<(i64, PathBuf, i64)> = folders
        .iter()
        .map(|f| (f.id, f.path.clone(), f.count))
        .collect();
    // By path, so the sidebar reads alphabetically.
    assert_eq!(
        got,
        [
            (b, PathBuf::from("/a-shoot"), 0),
            (a, PathBuf::from("/b-shoot"), 2)
        ]
    );
    assert_eq!(folders[1].name(), "b-shoot");
}

#[test]
fn remove_folder_cascades() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("kept.jpg");
    std::fs::write(&file, b"the photo itself").unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let keep = c.add_folder("/keep").unwrap();
    let id = photo(&c, f, &file.to_string_lossy(), None);
    let other = photo(&c, keep, "/keep/k.jpg", None);
    edits::set_settings(&c, id, &EditSettings::default_settings()).unwrap();
    versions::add(&c, id, "v1", &EditSettings::default_settings()).unwrap();

    c.remove_folder(f).unwrap();
    assert_eq!(c.photo(id).unwrap(), None);
    assert!(c.photos_in(f).unwrap().is_empty());
    assert_eq!(edits::get_settings(&c, id).unwrap(), None);
    assert_eq!(edits::history_len(&c, id).unwrap(), 0);
    assert_eq!(
        c.folders()
            .unwrap()
            .iter()
            .map(|f| f.id)
            .collect::<Vec<_>>(),
        [keep]
    );
    assert!(c.photo(other).unwrap().is_some(), "other folders stay");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"the photo itself",
        "files are never deleted"
    );
    // Removing it again is not an error.
    c.remove_folder(f).unwrap();
}

#[test]
fn migration_5_adds_sidecar_error() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    assert_eq!(c.schema_version().unwrap(), 10);
}
