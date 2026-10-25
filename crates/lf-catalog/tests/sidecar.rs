use lf_catalog::model::now;
use lf_catalog::{CatalogError, Sidecar, sidecar};
use lf_settings::EditSettings;
use serde_json::{Map, json};

fn sample() -> Sidecar {
    let mut s = EditSettings::default_settings();
    s.exposure = 0.5;
    let mut extra = Map::new();
    extra.insert("future".into(), json!({"a": [1, 2]}));
    Sidecar {
        v: 2,
        settings: s,
        versions: vec![],
        updated_at: now(),
        extra,
    }
}

#[test]
fn path_is_beside_the_photo() {
    let p = sidecar::path_for(std::path::Path::new("/a/b/IMG_1.NEF"));
    assert_eq!(p.file_name().unwrap(), "IMG_1.NEF.lensflow.json");
    assert_eq!(p.parent().unwrap(), std::path::Path::new("/a/b"));
}

#[test]
fn missing_is_none_and_corrupt_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.lensflow.json");
    assert!(sidecar::read(&p).unwrap().is_none());
    std::fs::write(&p, "{ not json").unwrap();
    assert!(matches!(sidecar::read(&p), Err(CatalogError::Corrupt(_))));
    std::fs::write(&p, "[1]").unwrap();
    assert!(matches!(sidecar::read(&p), Err(CatalogError::Corrupt(_))));
}

#[test]
fn round_trip_preserves_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.nef.lensflow.json");
    let s = sample();
    sidecar::write(&p, &s).unwrap();
    let back = sidecar::read(&p).unwrap().unwrap();
    assert_eq!(back, s);
    assert_eq!(back.extra["future"], json!({"a": [1, 2]}));
    // And it survives a second save.
    sidecar::write(&p, &back).unwrap();
    assert_eq!(
        sidecar::read(&p).unwrap().unwrap().extra["future"],
        json!({"a": [1, 2]})
    );
}

#[test]
fn a_v3_file_loads_its_known_settings() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.lensflow.json");
    let mut settings = EditSettings::default_settings().to_value();
    settings["exposure"] = json!(1.25);
    settings["brandNewKnob"] = json!(7);
    let text = json!({
        "v": 3, "settings": settings, "versions": [], "updated_at": "2030-01-01T00:00:00.000Z",
        "newer": true
    });
    std::fs::write(&p, text.to_string()).unwrap();
    let s = sidecar::read(&p).unwrap().unwrap();
    assert_eq!(s.v, 3);
    assert_eq!(s.settings.exposure, 1.25);
    assert_eq!(s.extra["newer"], json!(true));
    sidecar::write(&p, &s).unwrap();
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(again["v"], 3);
    assert_eq!(again["newer"], true);
}

#[test]
fn atomic_write_leaves_no_temp_file_and_replaces_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.nef.lensflow.json");
    sidecar::write(&p, &sample()).unwrap();
    let mut second = sample();
    second.settings.exposure = -1.0;
    sidecar::write(&p, &second).unwrap();
    assert_eq!(sidecar::read(&p).unwrap().unwrap().settings.exposure, -1.0);
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["a.nef.lensflow.json".to_string()]);
}

#[test]
fn versions_without_an_id_or_date_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.lensflow.json");
    let text = json!({
        "v": 2, "settings": {}, "updated_at": "2030-01-01T00:00:00.000Z",
        "versions": [
            {"label": "no id", "settings": {"exposure": 0.5}, "created_at": "2029-01-01T00:00:00.000Z"},
            {"id": "x1", "label": "no date", "settings": {}},
            {"id": "x2", "label": "bad date", "settings": {}, "created_at": "yesterday"}
        ]
    });
    std::fs::write(&p, text.to_string()).unwrap();
    let s = sidecar::read(&p).unwrap().unwrap();
    assert_eq!(s.versions.len(), 3);
    assert_eq!(s.versions[0].label, "no id");
    assert!(uuid_like(&s.versions[0].id), "{}", s.versions[0].id);
    assert_eq!(s.versions[0].settings.exposure, 0.5);
    assert_eq!(s.versions[1].id, "x1");
    assert_eq!(s.versions[1].created_at, chrono::DateTime::UNIX_EPOCH);
    assert_eq!(s.versions[2].created_at, chrono::DateTime::UNIX_EPOCH);
}

fn uuid_like(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4
}

#[test]
fn a_corrupt_sidecar_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.nef.lensflow.json");
    std::fs::write(&p, "{ half a file").unwrap();
    assert!(matches!(
        sidecar::write(&p, &sample()),
        Err(CatalogError::Corrupt(_))
    ));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ half a file");
}
