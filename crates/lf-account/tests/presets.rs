use httpmock::prelude::*;
use lf_account::presets::{self, LocalPreset, Preset, Sync};
use lf_account::{AccountError, Client, Instance};
use serde_json::json;

fn client(server: &MockServer) -> Client {
    let url = url::Url::parse(&server.url("/api")).unwrap();
    Client::new(Instance::custom(url).unwrap(), "1.0.0")
        .with_token("t")
        .with_studio("frames")
}

#[test]
fn crud_round_trip() {
    let server = MockServer::start();
    let item = json!({"id": 7, "name": "Warm", "settings": {"exposure": 1}, "created_at": "2026-10-01T00:00:00+00:00"});
    server.mock(|w, t| {
        w.method(GET).path("/api/galleries/edit-presets");
        t.status(200).json_body(json!({"data": [item.clone()]}));
    });
    let post = server.mock(|w, t| {
        w.method(POST)
            .path("/api/galleries/edit-presets")
            .json_body(json!({"name": "Warm", "settings": {"exposure": 1}}));
        t.status(201).json_body(json!({"data": item.clone()}));
    });
    let patch = server.mock(|w, t| {
        w.method(PATCH).path("/api/galleries/edit-presets/7");
        t.status(200).json_body(json!({"data": {"id": 7, "name": "Warmer", "settings": {}, "created_at": "x", "updated_at": "2026-10-02T00:00:00+00:00"}}));
    });
    let del = server.mock(|w, t| {
        w.method(DELETE).path("/api/galleries/edit-presets/7");
        t.status(204);
    });
    let c = client(&server);
    let l = presets::list(&c).unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].updated_at, "2026-10-01T00:00:00+00:00");
    let p = presets::create(&c, "Warm", &json!({"exposure": 1})).unwrap();
    assert_eq!(p.id, 7);
    let p = presets::update(&c, 7, "Warmer", &json!({})).unwrap();
    assert_eq!(
        (p.name.as_str(), p.updated_at.as_str()),
        ("Warmer", "2026-10-02T00:00:00+00:00")
    );
    presets::delete(&c, 7).unwrap();
    post.assert();
    patch.assert();
    del.assert();
}

#[test]
fn duplicate_name_is_a_validation_error() {
    let server = MockServer::start();
    server.mock(|w, t| {
        w.method(POST).path("/api/galleries/edit-presets");
        t.status(422).json_body(json!({"message": "x", "errors": {"name": ["You already have a preset with this name."]}}));
    });
    match presets::create(&client(&server), "Warm", &json!({})) {
        Err(AccountError::Validation(m)) => assert!(m["name"].contains("already")),
        other => panic!("{other:?}"),
    }
}

fn local(name: &str, s: i64, at: &str, id: Option<i64>, synced: Option<&str>) -> LocalPreset {
    LocalPreset {
        name: name.into(),
        settings: json!({"v": s}),
        updated_at: at.into(),
        remote_id: id,
        synced_at: synced.map(Into::into),
    }
}

fn remote(name: &str, s: i64, at: &str) -> Preset {
    Preset {
        id: 9,
        name: name.into(),
        settings: json!({"v": s}),
        updated_at: at.into(),
    }
}

const SYNCED: &str = "2026-10-10T00:00:00Z";

#[test]
fn new_local_pushes_and_new_remote_pulls() {
    let plan = Sync::merge(
        &[local("A", 1, "2026-10-11T00:00:00Z", None, None)],
        &[],
        &[remote("B", 2, "2026-10-11T00:00:00+00:00")],
    );
    assert_eq!(plan.push.len(), 1);
    assert_eq!(plan.push[0].name, "A");
    assert_eq!(plan.pull.len(), 1);
    assert_eq!(plan.pull[0].name, "B");
    assert!(plan.conflicts.is_empty());
}

#[test]
fn only_one_side_changed_the_newer_wins() {
    let l = local("A", 1, "2026-10-12T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 2, "2026-10-09T00:00:00Z")]);
    assert_eq!((plan.push.len(), plan.pull.len()), (1, 0));
    assert_eq!(plan.push[0].remote_id, Some(9));

    let l = local("A", 1, "2026-10-09T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 2, "2026-10-12T05:30:00+05:30")]);
    assert_eq!((plan.push.len(), plan.pull.len()), (0, 1));
}

#[test]
fn unchanged_or_identical_does_nothing() {
    let l = local("A", 1, "2026-10-09T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 2, "2026-10-09T00:00:00Z")]);
    assert_eq!(plan, Default::default());
}

#[test]
fn both_changed_keeps_remote_and_renames_the_local_copy() {
    let l = local("A", 1, "2026-10-12T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 2, "2026-10-13T00:00:00Z")]);
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.conflicts[0].0.name, "A (desktop)");
    assert_eq!(plan.conflicts[0].0.remote_id, None);
    assert_eq!(plan.push[0].name, "A (desktop)");
    assert_eq!(plan.pull[0].name, "A");
}

#[test]
fn numbers_compare_regardless_of_int_or_float_spelling() {
    use presets::settings_equal;
    assert!(settings_equal(
        &json!({"exposure": 1, "c": [0, {"a": 2}]}),
        &json!({"exposure": 1.0, "c": [0.0, {"a": 2.0}]})
    ));
    assert!(!settings_equal(
        &json!({"exposure": 1}),
        &json!({"exposure": 1.5})
    ));
    let l = local("A", 1, "2026-10-12T00:00:00Z", Some(9), Some(SYNCED));
    let mut r = remote("A", 1, "2026-10-13T00:00:00Z");
    r.settings = json!({"v": 1.0});
    assert_eq!(Sync::merge(&[l], &[], &[r]), Default::default());
}

#[test]
fn conflict_copy_name_avoids_existing_names() {
    let l = local("A", 1, "2026-10-12T00:00:00Z", Some(9), Some(SYNCED));
    let r = vec![
        remote("A", 2, "2026-10-13T00:00:00Z"),
        remote_id("A (desktop)", 10, 3, "2026-10-01T00:00:00Z"),
    ];
    let plan = Sync::merge(&[l], &[], &r);
    assert_eq!(plan.conflicts[0].0.name, "A (desktop 2)");
}

#[test]
fn unreadable_timestamps_are_a_conflict() {
    let l = local("A", 1, "garbage", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 2, "2026-10-09T00:00:00Z")]);
    assert_eq!(plan.conflicts.len(), 1);
    let l = local("A", 1, "2026-10-12T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 2, "")]);
    assert_eq!(plan.conflicts.len(), 1);
}

fn remote_id(name: &str, id: i64, s: i64, at: &str) -> Preset {
    Preset {
        id,
        ..remote(name, s, at)
    }
}

#[test]
fn a_tombstone_still_on_the_server_is_deleted_there() {
    let gone = local("A", 1, SYNCED, Some(9), Some(SYNCED));
    let plan = Sync::merge(&[], &[gone], &[remote("A", 1, SYNCED)]);
    assert_eq!(plan.delete_remote, [9]);
    assert!(
        plan.pull.is_empty(),
        "a tombstoned preset is not pulled back"
    );
    assert!(plan.push.is_empty() && plan.purge.is_empty());
}

#[test]
fn a_tombstone_absent_remotely_is_purged() {
    // Synced once, already gone from the server; and one never synced at all.
    let plan = Sync::merge(
        &[],
        &[
            local("A", 1, SYNCED, Some(9), Some(SYNCED)),
            local("B", 1, SYNCED, None, None),
        ],
        &[remote_id("C", 10, 1, SYNCED)],
    );
    assert_eq!(plan.purge, ["A", "B"]);
    assert!(plan.delete_remote.is_empty());
    assert_eq!(plan.pull.len(), 1, "C is new here");
}

#[test]
fn a_remote_preset_absent_locally_is_pulled_unless_tombstoned() {
    let plan = Sync::merge(&[], &[], &[remote("A", 1, SYNCED)]);
    assert_eq!(plan.pull.len(), 1);
    // Deleted here before it was ever synced: the remote one is someone else's, pulled.
    let plan = Sync::merge(
        &[],
        &[local("A", 1, SYNCED, None, None)],
        &[remote("A", 2, SYNCED)],
    );
    assert_eq!(plan.pull.len(), 1);
    assert_eq!(plan.purge, ["A"]);
    assert!(plan.delete_remote.is_empty());
}

#[test]
fn a_synced_local_preset_gone_from_the_server_is_purged() {
    let l = local("A", 1, SYNCED, Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[]);
    assert_eq!(plan.purge, ["A"]);
    assert!(
        plan.push.is_empty(),
        "not pushed back: it was deleted on the web"
    );
    // Never synced: it is new here and pushed.
    let l = local("B", 1, SYNCED, None, None);
    let plan = Sync::merge(&[l], &[], &[]);
    assert_eq!(plan.push.len(), 1);
    assert!(plan.purge.is_empty());
}

#[test]
fn a_rename_follows_the_remote_id() {
    // Renamed here after the last sync: an update of the same preset, not a new one.
    let l = local("B", 1, "2026-10-12T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("A", 1, "2026-10-09T00:00:00Z")]);
    assert_eq!(plan.push.len(), 1);
    assert_eq!(
        (plan.push[0].name.as_str(), plan.push[0].remote_id),
        ("B", Some(9))
    );
    assert!(plan.pull.is_empty() && plan.purge.is_empty());
    // Renamed on the web: pulled under the new name.
    let l = local("A", 1, "2026-10-09T00:00:00Z", Some(9), Some(SYNCED));
    let plan = Sync::merge(&[l], &[], &[remote("B", 1, "2026-10-12T00:00:00Z")]);
    assert_eq!(plan.pull.len(), 1);
    assert_eq!(plan.pull[0].name, "B");
    assert!(plan.push.is_empty());
}

#[test]
fn an_identical_preset_never_linked_is_linked() {
    let plan = Sync::merge(
        &[local("A", 1, SYNCED, None, None)],
        &[],
        &[remote("A", 1, SYNCED)],
    );
    assert_eq!(
        plan.pull.len(),
        1,
        "pulled so the local row learns its remote id"
    );
    assert!(plan.push.is_empty() && plan.conflicts.is_empty());
}
