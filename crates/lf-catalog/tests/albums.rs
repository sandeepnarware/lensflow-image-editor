use lf_catalog::Catalog;
use lf_catalog::albums::{Rule, RuleFlag};
use lf_catalog::cull::{Flag, Mark};
use lf_catalog::db::CatalogError;
use lf_catalog::model::NewPhoto;

fn photo(c: &Catalog, folder: i64, name: &str, taken: &str) -> i64 {
    c.add_photo(
        folder,
        &NewPhoto {
            path: format!("D:/shoots/{folder}/{name}"),
            kind: "jpeg".into(),
            captured_at: Some(taken.into()),
            ..NewPhoto::default()
        },
    )
    .unwrap()
}

fn ids(rows: &[lf_catalog::PhotoRow]) -> Vec<i64> {
    rows.iter().map(|r| r.id).collect()
}

#[test]
fn a_plain_album_gathers_photos_from_any_folder_in_the_order_added() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path()).unwrap();
    let a = c.add_folder("D:/shoots/a").unwrap();
    let b = c.add_folder("D:/shoots/b").unwrap();
    let a1 = photo(&c, a, "1.jpg", "2026-10-01T10:00:00");
    let b1 = photo(&c, b, "1.jpg", "2026-09-01T10:00:00");
    let b2 = photo(&c, b, "2.jpg", "2026-09-02T10:00:00");

    let picks = c.create_album("  Client picks ", None).unwrap();
    assert_eq!(c.add_to_album(picks, &[a1, b2]).unwrap(), 2);
    // Already in it: skipped. Unknown ids: skipped.
    assert_eq!(c.add_to_album(picks, &[b2, b1, 999]).unwrap(), 1);
    assert_eq!(ids(&c.album_photos(picks).unwrap()), [a1, b2, b1]);

    let albums = c.albums().unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(
        (albums[0].name.as_str(), albums[0].count),
        ("Client picks", 3)
    );
    assert!(matches!(
        c.create_album("client PICKS", None),
        Err(CatalogError::AlbumNameTaken(_))
    ));

    assert_eq!(c.remove_from_album(picks, &[b2]).unwrap(), 1);
    assert_eq!(ids(&c.album_photos(picks).unwrap()), [a1, b1]);
    // Forgetting a folder takes its photos out of every album.
    c.remove_folder(b).unwrap();
    assert_eq!(ids(&c.album_photos(picks).unwrap()), [a1]);

    c.update_album(picks, "Album layout", None).unwrap();
    assert_eq!(c.albums().unwrap()[0].name, "Album layout");
    c.delete_album(picks).unwrap();
    assert!(c.albums().unwrap().is_empty());
    assert!(c.photo(a1).unwrap().is_some());
}

#[test]
fn a_smart_album_holds_what_matches_its_rule_now() {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path()).unwrap();
    let f = c.add_folder("D:/shoots/a").unwrap();
    let one = photo(&c, f, "1.jpg", "2026-10-01T10:00:00");
    let two = photo(&c, f, "2.jpg", "2026-10-02T10:00:00");
    let rule = Rule {
        min_rating: 4,
        flag: RuleFlag::NotRejected,
        ..Rule::default()
    };
    let best = c.create_album("Best", Some(&rule)).unwrap();
    assert!(c.album_photos(best).unwrap().is_empty());
    c.mark(&[one, two], Mark::Rating(5)).unwrap();
    c.mark(&[two], Mark::Flag(Flag::Rejected)).unwrap();
    assert_eq!(ids(&c.album_photos(best).unwrap()), [one]);
    // A smart album takes no hand-picked photos, and keeps its rule when renamed.
    assert_eq!(c.add_to_album(best, &[two]).unwrap(), 0);
    let looser = Rule {
        min_rating: 1,
        ..Rule::default()
    };
    c.update_album(best, "Starred", Some(&looser)).unwrap();
    let album = &c.albums().unwrap()[0];
    assert_eq!(album.rule.as_ref(), Some(&looser));
    assert_eq!(ids(&c.album_photos(best).unwrap()), [one, two]);
}
