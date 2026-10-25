use chrono::Duration;
use lf_catalog::{Catalog, NewPhoto, Source, edits, versions};
use lf_settings::EditSettings;

fn setup() -> (tempfile::TempDir, Catalog, i64) {
    let dir = tempfile::tempdir().unwrap();
    let c = Catalog::open(dir.path()).unwrap();
    let f = c.add_folder("shoot").unwrap();
    let id = c
        .add_photo(
            f,
            &NewPhoto {
                path: "shoot/a.nef".into(),
                kind: "raw".into(),
                ..Default::default()
            },
        )
        .unwrap();
    (dir, c, id)
}

fn with_exposure(e: f64) -> EditSettings {
    let mut s = EditSettings::default_settings();
    s.exposure = e;
    s
}

fn exposure(c: &Catalog, id: i64) -> f64 {
    edits::get_settings(c, id).unwrap().unwrap().exposure
}

#[test]
fn set_settings_returns_the_sidecar() {
    let (_d, c, id) = setup();
    let sc = edits::set_settings(&c, id, &with_exposure(0.5)).unwrap();
    assert_eq!(sc.settings.exposure, 0.5);
    assert_eq!(sc.v, 2);
    assert_eq!(exposure(&c, id), 0.5);
    assert!(edits::set_settings(&c, id + 99, &with_exposure(1.0)).is_err());
}

#[test]
fn history_ring_caps_at_50() {
    let (_d, c, id) = setup();
    for i in 0..60 {
        edits::set_settings(&c, id, &with_exposure(i as f64 / 100.0)).unwrap();
    }
    assert_eq!(edits::history_len(&c, id).unwrap(), 50);
    // Undo as far as it goes: 49 steps, ending at the oldest kept entry.
    let mut n = 0;
    while edits::undo(&c, id).unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 49);
    assert_eq!(exposure(&c, id), 0.10);
}

#[test]
fn undo_and_redo_walk_and_new_edit_drops_redo() {
    let (_d, c, id) = setup();
    for e in [0.1, 0.2, 0.3] {
        edits::set_settings(&c, id, &with_exposure(e)).unwrap();
    }
    assert_eq!(edits::undo(&c, id).unwrap().unwrap().settings.exposure, 0.2);
    assert_eq!(edits::undo(&c, id).unwrap().unwrap().settings.exposure, 0.1);
    assert_eq!(
        edits::undo(&c, id).unwrap().unwrap().settings.exposure,
        0.0,
        "back to before the first edit"
    );
    assert!(edits::undo(&c, id).unwrap().is_none());
    assert_eq!(edits::redo(&c, id).unwrap().unwrap().settings.exposure, 0.1);
    assert_eq!(edits::redo(&c, id).unwrap().unwrap().settings.exposure, 0.2);
    edits::set_settings(&c, id, &with_exposure(0.9)).unwrap();
    assert!(
        edits::redo(&c, id).unwrap().is_none(),
        "redo branch dropped"
    );
    assert_eq!(edits::undo(&c, id).unwrap().unwrap().settings.exposure, 0.2);
}

#[test]
fn cursor_survives_reopen() {
    let (d, c, id) = setup();
    edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    edits::set_settings(&c, id, &with_exposure(0.2)).unwrap();
    edits::undo(&c, id).unwrap();
    drop(c);
    let c = Catalog::open(d.path()).unwrap();
    assert_eq!(edits::redo(&c, id).unwrap().unwrap().settings.exposure, 0.2);
}

#[test]
fn versions_add_use_delete() {
    let (_d, c, id) = setup();
    edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    let a = versions::add(&c, id, "warm", &with_exposure(0.7)).unwrap();
    let b = versions::add(&c, id, "flat", &with_exposure(-0.7)).unwrap();
    let list = versions::list(&c, id).unwrap();
    assert_eq!(
        list.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(),
        ["warm", "flat"]
    );
    assert_eq!(edits::sidecar_for(&c, id).unwrap().versions.len(), 2);

    let sc = versions::use_version(&c, id, &a.id).unwrap();
    assert_eq!(sc.settings.exposure, 0.7);
    assert_eq!(exposure(&c, id), 0.7);
    // Recorded in history: undo goes back to 0.1.
    assert_eq!(edits::undo(&c, id).unwrap().unwrap().settings.exposure, 0.1);

    assert!(versions::delete(&c, id, &b.id).unwrap());
    assert!(!versions::delete(&c, id, &b.id).unwrap());
    assert_eq!(versions::list(&c, id).unwrap().len(), 1);
    assert!(versions::use_version(&c, id, &b.id).is_err());
}

#[test]
fn newer_sidecar_wins_and_updates_the_db() {
    let (_d, c, id) = setup();
    edits::set_settings(&c, id, &with_exposure(0.05)).unwrap();
    versions::add(&c, id, "old", &with_exposure(0.2)).unwrap();
    let mut sc = edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    assert!(edits::history_len(&c, id).unwrap() > 1);
    sc.settings = with_exposure(0.8);
    // Newer than the catalogue but not in the future (a future stamp is clamped and rewritten).
    std::thread::sleep(std::time::Duration::from_millis(30));
    sc.updated_at += Duration::milliseconds(10);
    sc.versions = vec![lf_catalog::Version {
        id: "from-the-sidecar".into(),
        label: "theirs".into(),
        settings: with_exposure(-0.4),
        created_at: lf_catalog::model::now(),
    }];
    let r = edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_eq!(r.source, Source::Sidecar);
    assert!(r.sidecar_to_write.is_none());
    assert_eq!(exposure(&c, id), 0.8);
    // The sidecar's versions replace the catalogue's.
    let list = versions::list(&c, id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0].id.as_str(), list[0].label.as_str()),
        ("from-the-sidecar", "theirs")
    );
    // History restarts at the adopted state: nothing to undo.
    assert_eq!(edits::history_len(&c, id).unwrap(), 1);
    assert!(edits::undo(&c, id).unwrap().is_none());
}

#[test]
fn equal_stamps_with_different_content_rewrite_the_sidecar() {
    let (_d, c, id) = setup();
    let mut sc = edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    sc.settings = with_exposure(0.6);
    let r = edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_eq!(r.source, Source::Db);
    assert_eq!(r.sidecar_to_write.unwrap().settings.exposure, 0.1);
}

#[test]
fn a_sidecar_dated_in_the_future_is_adopted_at_now_and_rewritten() {
    let (_d, c, id) = setup();
    let mut sc = edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    sc.settings = with_exposure(0.7);
    sc.updated_at = lf_catalog::model::now() + Duration::days(365);
    let r = edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_eq!(r.source, Source::Sidecar);
    assert_eq!(exposure(&c, id), 0.7);
    let out = r
        .sidecar_to_write
        .expect("rewritten with the clamped stamp");
    assert!(out.updated_at <= lf_catalog::model::now());
    // Next reopen: the rewritten sidecar matches, nothing to do.
    let again = edits::reconcile(&c, id, Some(&out)).unwrap();
    assert_eq!(again.source, Source::Db);
    assert!(again.sidecar_to_write.is_none());
}

#[test]
fn newer_or_equal_db_wins_and_rewrites_the_sidecar() {
    let (_d, c, id) = setup();
    let mut sc = edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    sc.extra.insert("keep".into(), serde_json::json!(1));
    sc.v = 3;
    let mut stale = sc.clone();
    stale.settings = with_exposure(0.8);
    stale.updated_at -= Duration::seconds(60);
    let r = edits::reconcile(&c, id, Some(&stale)).unwrap();
    assert_eq!(r.source, Source::Db);
    let out = r.sidecar_to_write.unwrap();
    assert_eq!(out.settings.exposure, 0.1);
    assert_eq!(out.v, 3);
    assert_eq!(out.extra["keep"], serde_json::json!(1));
    assert_eq!(exposure(&c, id), 0.1);
    // Equal stamps and the same content: DB wins and nothing is rewritten.
    let r = edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_eq!(r.source, Source::Db);
    assert!(
        r.sidecar_to_write.is_none(),
        "an equal sidecar is not rewritten"
    );
    // No sidecar: DB wins and writes one; no edits and no sidecar: nothing to do.
    assert!(
        edits::reconcile(&c, id, None)
            .unwrap()
            .sidecar_to_write
            .is_some()
    );
    let f = c.add_folder("shoot").unwrap();
    let other = c
        .add_photo(
            f,
            &NewPhoto {
                path: "shoot/b.nef".into(),
                kind: "raw".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        edits::reconcile(&c, other, None)
            .unwrap()
            .sidecar_to_write
            .is_none()
    );
}

fn v3_sidecar_file(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("a.nef.lensflow.json");
    let text = serde_json::json!({
        "v": 3, "settings": EditSettings::default_settings().to_value(), "versions": [],
        "updated_at": "2099-01-01T00:00:00.000Z", "newer": {"k": 1}
    });
    std::fs::write(&p, text.to_string()).unwrap();
    p
}

fn assert_kept(sc: &lf_catalog::Sidecar, dir: &std::path::Path) {
    let p = dir.join("out.lensflow.json");
    lf_catalog::sidecar::write(&p, sc).unwrap();
    let back = lf_catalog::sidecar::read(&p).unwrap().unwrap();
    assert_eq!(back.v, 3);
    assert_eq!(back.extra["newer"], serde_json::json!({"k": 1}));
}

#[test]
fn unknown_keys_survive_edits_undo_and_versions_after_reconcile() {
    let (d, c, id) = setup();
    let sc = lf_catalog::sidecar::read(&v3_sidecar_file(d.path()))
        .unwrap()
        .unwrap();
    edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_kept(
        &edits::set_settings(&c, id, &with_exposure(0.3)).unwrap(),
        d.path(),
    );
    edits::set_settings(&c, id, &with_exposure(0.4)).unwrap();
    assert_kept(&edits::undo(&c, id).unwrap().unwrap(), d.path());
    assert_kept(&edits::redo(&c, id).unwrap().unwrap(), d.path());
    versions::add(&c, id, "v", &with_exposure(0.9)).unwrap();
    assert_kept(&edits::sidecar_for(&c, id).unwrap(), d.path());
    // Survives a reopen too.
    drop(c);
    let c = Catalog::open(d.path()).unwrap();
    assert_kept(&edits::sidecar_for(&c, id).unwrap(), d.path());
}

#[test]
fn remember_sidecar_keeps_keys_without_reconcile() {
    let (d, c, id) = setup();
    let sc = lf_catalog::sidecar::read(&v3_sidecar_file(d.path()))
        .unwrap()
        .unwrap();
    edits::remember_sidecar(&c, id, &sc).unwrap();
    assert_kept(
        &edits::set_settings(&c, id, &with_exposure(0.3)).unwrap(),
        d.path(),
    );
}

#[test]
fn copied_sidecar_does_not_steal_another_photos_version() {
    let (_d, c, a) = setup();
    let f = c.add_folder("shoot").unwrap();
    let b = c
        .add_photo(
            f,
            &NewPhoto {
                path: "shoot/b.nef".into(),
                kind: "raw".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let v = versions::add(&c, a, "mine", &with_exposure(0.5)).unwrap();
    let mut copy = edits::sidecar_for(&c, a).unwrap();
    copy.updated_at += Duration::seconds(60);
    assert_eq!(
        edits::reconcile(&c, b, Some(&copy)).unwrap().source,
        Source::Sidecar
    );
    assert_eq!(versions::list(&c, a).unwrap().len(), 1);
    assert_eq!(versions::list(&c, a).unwrap()[0].id, v.id);
    let bl = versions::list(&c, b).unwrap();
    assert_eq!(bl.len(), 1);
    assert_ne!(bl[0].id, v.id);
}

#[test]
fn use_version_of_an_unknown_id_is_not_found() {
    let (_d, c, id) = setup();
    assert!(versions::use_version(&c, id, "nope").is_err());
}
