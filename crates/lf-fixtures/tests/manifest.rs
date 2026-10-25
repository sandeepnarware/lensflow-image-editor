use lf_fixtures::{Manifest, load_png, root};
use sha2::{Digest, Sha256};

#[test]
fn manifest_lists_the_whole_fixture_set_and_hashes_match() {
    let m = Manifest::load().unwrap();
    assert_eq!(m.sources.len(), 6);
    assert_eq!(m.cases.len(), 38);
    assert_eq!(m.files.values().count(), 6 + 38 + 228);
    assert!((m.thresholds.mean - 1.0 / 255.0).abs() < 1e-12);
    assert!((m.thresholds.max - 4.0 / 255.0).abs() < 1e-12);
    for (path, sha) in &m.files {
        let bytes = std::fs::read(root().join(path)).unwrap();
        assert_eq!(hex::encode(Sha256::digest(&bytes)), *sha, "{path}");
    }
}

#[test]
fn sources_decode_to_the_sizes_the_manifest_states() {
    let m = Manifest::load().unwrap();
    for (name, (w, h)) in &m.sources {
        let img = load_png(root().join(format!("sources/{name}.png"))).unwrap();
        assert_eq!((img.width(), img.height()), (*w, *h), "{name}");
    }
}
