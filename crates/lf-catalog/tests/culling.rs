//! Culling marks: flags, star ratings and colour labels, set on many photos at once and read
//! back with the folder's rows, without touching anything that makes a thumbnail again.

use lf_catalog::cull::{Flag, Label, Mark};
use lf_catalog::{Catalog, NewPhoto};

fn photo(c: &Catalog, folder: i64, path: &str) -> i64 {
    c.add_photo(
        folder,
        &NewPhoto {
            path: path.into(),
            kind: "jpeg".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn a_new_photo_is_unmarked() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let id = photo(&c, f, "/shoot/a.jpg");
    let row = c.photo(id).unwrap().unwrap();
    assert_eq!(
        (row.flag, row.rating, row.label),
        (Flag::Unflagged, 0, None)
    );
}

#[test]
fn marks_apply_to_every_photo_named_and_only_those() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let a = photo(&c, f, "/shoot/a.jpg");
    let b = photo(&c, f, "/shoot/b.jpg");
    let other = photo(&c, f, "/shoot/c.jpg");

    assert_eq!(c.mark(&[a, b], Mark::Flag(Flag::Picked)).unwrap(), 2);
    assert_eq!(c.mark(&[a, b], Mark::Rating(4)).unwrap(), 2);
    assert_eq!(c.mark(&[b], Mark::Label(Some(Label::Green))).unwrap(), 1);
    assert_eq!(c.mark(&[a], Mark::Flag(Flag::Rejected)).unwrap(), 1);

    let row = |id| c.photo(id).unwrap().unwrap();
    assert_eq!(
        (row(a).flag, row(a).rating, row(a).label),
        (Flag::Rejected, 4, None)
    );
    assert_eq!(
        (row(b).flag, row(b).rating, row(b).label),
        (Flag::Picked, 4, Some(Label::Green))
    );
    assert_eq!(
        (row(other).flag, row(other).rating, row(other).label),
        (Flag::Unflagged, 0, None)
    );
    // The folder's list carries the same marks.
    let rows = c.photos_in(f).unwrap();
    assert_eq!(rows.iter().find(|r| r.id == b).unwrap().rating, 4);

    // Clearing: no label, no stars, no flag.
    c.mark(&[b], Mark::Label(None)).unwrap();
    c.mark(&[b], Mark::Rating(0)).unwrap();
    c.mark(&[b], Mark::Flag(Flag::Unflagged)).unwrap();
    assert_eq!(
        (row(b).flag, row(b).rating, row(b).label),
        (Flag::Unflagged, 0, None)
    );
    // Nothing named, nothing changed; an id no longer catalogued is skipped.
    assert_eq!(c.mark(&[], Mark::Rating(3)).unwrap(), 0);
    assert_eq!(c.mark(&[9_999], Mark::Rating(3)).unwrap(), 0);
}

#[test]
fn ratings_above_five_are_refused() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let a = photo(&c, f, "/shoot/a.jpg");
    assert!(c.mark(&[a], Mark::Rating(6)).is_err());
    assert_eq!(c.photo(a).unwrap().unwrap().rating, 0);
}

#[test]
fn marking_leaves_the_thumbnail_key_alone() {
    // `photos.updated_at` is part of the thumbnail's key: a rating must not remake it.
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let a = photo(&c, f, "/shoot/a.jpg");
    let before = c.photo(a).unwrap().unwrap().updated_at;
    std::thread::sleep(std::time::Duration::from_millis(5));
    c.mark(&[a], Mark::Rating(5)).unwrap();
    c.mark(&[a], Mark::Flag(Flag::Picked)).unwrap();
    c.mark(&[a], Mark::Label(Some(Label::Red))).unwrap();
    assert_eq!(c.photo(a).unwrap().unwrap().updated_at, before);
}

#[test]
fn a_label_this_version_does_not_know_reads_as_none_and_survives_other_marks() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let a = photo(&c, f, "/shoot/a.jpg");
    drop(c);
    // A newer version wrote a label this one has no name for, and an odd flag value.
    let conn = rusqlite::Connection::open(d.path().join(".lensflow/catalog.sqlite")).unwrap();
    conn.execute(
        "UPDATE photos SET label = 'orange', flag = 7 WHERE id = ?1",
        [a],
    )
    .unwrap();
    drop(conn);
    let c = Catalog::open(d.path()).unwrap();
    let row = c.photo(a).unwrap().unwrap();
    assert_eq!((row.label, row.flag), (None, Flag::Unflagged));
    c.mark(&[a], Mark::Rating(2)).unwrap();
    drop(c);
    let conn = rusqlite::Connection::open(d.path().join(".lensflow/catalog.sqlite")).unwrap();
    let label: String = conn
        .query_row("SELECT label FROM photos WHERE id = ?1", [a], |r| r.get(0))
        .unwrap();
    assert_eq!(label, "orange", "a rating leaves the label it cannot read");
}

#[test]
fn labels_and_flags_have_stable_stored_names() {
    // These are what the catalogue holds; a later version must keep reading them.
    assert_eq!(Label::Red.key(), "red");
    assert_eq!(Label::Yellow.key(), "yellow");
    assert_eq!(Label::Green.key(), "green");
    assert_eq!(Label::Blue.key(), "blue");
    assert_eq!(Label::Purple.key(), "purple");
    for label in Label::ALL {
        assert_eq!(Label::from_key(label.key()), Some(label));
    }
    assert_eq!(Label::from_key("orange"), None);
    assert_eq!(Flag::from_stored(1), Flag::Picked);
    assert_eq!(Flag::from_stored(-1), Flag::Rejected);
    assert_eq!(Flag::from_stored(0), Flag::Unflagged);
}
