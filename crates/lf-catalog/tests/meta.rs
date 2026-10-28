//! Descriptive metadata: titles, captions, creator, copyright and keywords, set on many
//! photos at once and read back with the folder's rows, without touching a thumbnail's key.

use lf_catalog::meta::{Field, Meta};
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

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn text_fields_are_set_trimmed_and_cleared() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let a = photo(&c, f, "/shoot/a.jpg");
    let b = photo(&c, f, "/shoot/b.jpg");
    assert_eq!(c.photo(a).unwrap().unwrap().meta, Meta::default());
    let stamp = c.photo(a).unwrap().unwrap().updated_at;

    assert_eq!(c.set_meta(&[a, b], Field::Title, "  Sunset  ").unwrap(), 2);
    assert_eq!(c.set_meta(&[a], Field::Copyright, "© Studio").unwrap(), 1);
    let row = c.photo(a).unwrap().unwrap();
    assert_eq!(row.meta.title.as_deref(), Some("Sunset"));
    assert_eq!(row.meta.copyright.as_deref(), Some("© Studio"));
    assert_eq!(row.updated_at, stamp, "a thumbnail's key is untouched");
    // Empty clears.
    c.set_meta(&[a], Field::Title, "   ").unwrap();
    assert_eq!(c.photo(a).unwrap().unwrap().meta.title, None);
    assert_eq!(
        c.photo(b).unwrap().unwrap().meta.title.as_deref(),
        Some("Sunset")
    );
}

#[test]
fn keywords_are_added_once_in_any_case_and_counted() {
    let d = tempfile::tempdir().unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("/shoot").unwrap();
    let a = photo(&c, f, "/shoot/a.jpg");
    let g = c.add_folder("/other").unwrap();
    let b = photo(&c, g, "/other/b.jpg");

    assert_eq!(
        c.add_keywords(&[a, b], &words(&["Wedding", "beach"]))
            .unwrap(),
        4
    );
    // The same word in another case is already there.
    assert_eq!(
        c.add_keywords(&[a], &words(&["wedding", "Zebra"])).unwrap(),
        1
    );
    // A photo that is not catalogued gets nothing.
    assert_eq!(c.add_keywords(&[999], &words(&["ghost"])).unwrap(), 0);
    assert_eq!(
        c.photos_in(f).unwrap()[0].meta.keywords,
        ["beach", "Wedding", "Zebra"]
    );
    assert_eq!(
        c.photos_in(g).unwrap()[0].meta.keywords,
        ["beach", "Wedding"]
    );
    assert_eq!(
        c.keywords().unwrap(),
        [
            ("beach".to_string(), 2),
            ("Wedding".to_string(), 2),
            ("Zebra".to_string(), 1)
        ]
    );

    assert_eq!(c.remove_keywords(&[a, b], &words(&["WEDDING"])).unwrap(), 2);
    assert_eq!(
        c.photo(a).unwrap().unwrap().meta.keywords,
        ["beach", "Zebra"]
    );
    // A folder removed from the catalogue takes its photos' keywords with it.
    c.remove_folder(g).unwrap();
    assert_eq!(
        c.keywords().unwrap(),
        [("beach".to_string(), 1), ("Zebra".to_string(), 1)]
    );
}
