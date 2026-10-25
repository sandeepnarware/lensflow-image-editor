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
    let mut sc = edits::set_settings(&c, id, &with_exposure(0.1)).unwrap();
    sc.settings = with_exposure(0.8);
    sc.updated_at += Duration::seconds(60);
    let r = edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_eq!(r.source, Source::Sidecar);
    assert!(r.sidecar_to_write.is_none());
    assert_eq!(exposure(&c, id), 0.8);
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
    // Equal stamps: DB wins.
    let r = edits::reconcile(&c, id, Some(&sc)).unwrap();
    assert_eq!(r.source, Source::Db);
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
