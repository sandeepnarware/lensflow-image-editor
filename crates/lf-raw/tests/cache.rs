use half::f16;
use lf_pipeline::FloatFrame;
use lf_raw::cache::{CacheKey, FrameCache};
use lf_raw::demosaic::Quality;
use lf_raw::develop::Developed;
use lf_settings::{DevelopSettings, Highlights};
use std::fs;
use std::path::{Path, PathBuf};

fn developed(seed: u16, w: u32, h: u32) -> Developed {
    let data = (0..w * h * 4)
        .map(|i| f16::from_f32(((i as u16).wrapping_mul(seed) % 997) as f32 / 100.0 - 1.0))
        .collect();
    Developed {
        frame: FloatFrame {
            width: w,
            height: h,
            data,
        },
        full_width: w * 2,
        full_height: h * 2,
        reduced: true,
        orientation_applied: seed % 2 == 1,
    }
}

fn raw_file(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

fn key(p: &Path) -> CacheKey {
    CacheKey::for_file(p, &DevelopSettings::default(), Quality::Best, 2048).unwrap()
}

fn entry_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "lff"))
        .collect()
}

#[test]
fn round_trip_is_exact() {
    let t = tempfile::tempdir().unwrap();
    let f = raw_file(t.path(), "a.raw", b"hello raw");
    let cache = FrameCache::open(&t.path().join("cache"), 1 << 30).unwrap();
    let k = key(&f);
    assert!(cache.get(&k).is_none());
    let d = developed(7, 13, 9);
    cache.put(&k, &d);
    let got = cache.get(&k).expect("hit");
    assert_eq!(got.frame.width, 13);
    assert_eq!(got.frame.height, 9);
    assert_eq!(got.frame.data, d.frame.data);
    assert_eq!((got.full_width, got.full_height), (26, 18));
    assert!(got.reduced);
    assert!(got.orientation_applied);
}

#[test]
fn changed_components_miss() {
    let t = tempfile::tempdir().unwrap();
    let f = raw_file(t.path(), "a.raw", b"hello raw");
    let base = key(&f);
    let s = DevelopSettings {
        highlights: Highlights::Clip,
        ..DevelopSettings::default()
    };
    assert_ne!(
        base,
        CacheKey::for_file(&f, &s, Quality::Best, 2048).unwrap()
    );
    assert_ne!(
        base,
        CacheKey::for_file(&f, &DevelopSettings::default(), Quality::Fast, 2048).unwrap()
    );
    assert_ne!(
        base,
        CacheKey::for_file(&f, &DevelopSettings::default(), Quality::Best, 1024).unwrap()
    );
    let cache = FrameCache::open(&t.path().join("cache"), 1 << 30).unwrap();
    cache.put(&base, &developed(3, 4, 4));
    let k2 = CacheKey::for_file(&f, &s, Quality::Best, 2048).unwrap();
    assert!(cache.get(&k2).is_none());
    assert!(cache.get(&base).is_some());
}

#[test]
fn touched_or_changed_file_misses() {
    let t = tempfile::tempdir().unwrap();
    let f = raw_file(t.path(), "a.raw", b"hello raw");
    let k = key(&f);
    filetime::set_file_mtime(&f, filetime::FileTime::from_unix_time(2_000_000_000, 0)).unwrap();
    assert_ne!(k, key(&f));
    let k = key(&f);
    fs::write(&f, b"hello rax").unwrap();
    filetime::set_file_mtime(&f, filetime::FileTime::from_unix_time(2_000_000_000, 0)).unwrap();
    assert_ne!(k, key(&f));
    assert!(
        CacheKey::for_file(
            &t.path().join("nope"),
            &DevelopSettings::default(),
            Quality::Best,
            1
        )
        .is_err()
    );
}

#[test]
fn trim_evicts_least_recently_used() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("cache");
    let keys: Vec<CacheKey> = (0..3)
        .map(|i| key(&raw_file(t.path(), &format!("{i}.raw"), &[i as u8; 8])))
        .collect();
    let big = FrameCache::open(&dir, 1 << 30).unwrap();
    for (i, k) in keys.iter().enumerate() {
        big.put(k, &developed(i as u16 + 1, 64, 64));
    }
    let size: u64 = entry_files(&dir)
        .iter()
        .map(|p| fs::metadata(p).unwrap().len())
        .sum();
    // Access order: 0 oldest after we touch 1 and 2 then 0 is least recent.
    assert!(big.get(&keys[0]).is_some());
    assert!(big.get(&keys[2]).is_some());
    assert!(big.get(&keys[1]).is_some()); // 1 most recent, 0 least... order: 0,2,1
    let small = FrameCache::open(&dir, size * 2 / 3 + 1).unwrap();
    small.trim();
    assert!(
        small.get(&keys[0]).is_none(),
        "least recently used goes first"
    );
    assert!(small.get(&keys[1]).is_some());
}

#[test]
fn corrupt_entry_is_ignored_and_removed() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("cache");
    let f = raw_file(t.path(), "a.raw", b"x");
    let cache = FrameCache::open(&dir, 1 << 30).unwrap();
    let k = key(&f);
    cache.put(&k, &developed(5, 16, 16));
    let e = entry_files(&dir).pop().unwrap();
    let bytes = fs::read(&e).unwrap();
    fs::write(&e, &bytes[..bytes.len() / 2]).unwrap();
    assert!(cache.get(&k).is_none());
    assert!(entry_files(&dir).is_empty());
    // A flipped payload byte fails the CRC.
    cache.put(&k, &developed(5, 16, 16));
    let mut bytes = fs::read(&e).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    fs::write(&e, &bytes).unwrap();
    assert!(cache.get(&k).is_none());
    assert!(entry_files(&dir).is_empty());
}

#[test]
fn version_mismatch_is_ignored_and_removed() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("cache");
    let f = raw_file(t.path(), "a.raw", b"x");
    let cache = FrameCache::open(&dir, 1 << 30).unwrap();
    let k = key(&f);
    cache.put(&k, &developed(5, 16, 16));
    let e = entry_files(&dir).pop().unwrap();
    let mut bytes = fs::read(&e).unwrap();
    bytes[4] = 3; // version u16 LE follows the 4-byte magic
    fs::write(&e, &bytes).unwrap();
    assert!(cache.get(&k).is_none());
    assert!(entry_files(&dir).is_empty());
}

#[test]
fn header_flip_is_ignored_and_removed() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("cache");
    let f = raw_file(t.path(), "a.raw", b"x");
    let cache = FrameCache::open(&dir, 1 << 30).unwrap();
    let k = key(&f);
    cache.put(&k, &developed(5, 16, 16));
    let e = entry_files(&dir).pop().unwrap();
    let mut bytes = fs::read(&e).unwrap();
    bytes[22] ^= 1; // flags
    fs::write(&e, &bytes).unwrap();
    assert!(cache.get(&k).is_none());
    assert!(entry_files(&dir).is_empty());
}

#[test]
fn algo_version_bump_misses() {
    let t = tempfile::tempdir().unwrap();
    let f = raw_file(t.path(), "a.raw", b"x");
    let s = DevelopSettings::default();
    let v = lf_raw::develop::DEVELOP_ALGO_VERSION;
    assert_eq!(
        key(&f),
        CacheKey::for_file_with_version(&f, &s, Quality::Best, 2048, v).unwrap()
    );
    assert_ne!(
        key(&f),
        CacheKey::for_file_with_version(&f, &s, Quality::Best, 2048, v + 1).unwrap()
    );
}

#[test]
fn stale_tmp_files_are_swept_on_open() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("cache");
    fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.tmp");
    let fresh = dir.join("fresh.tmp");
    fs::write(&old, b"x").unwrap();
    fs::write(&fresh, b"x").unwrap();
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(two_hours_ago)).unwrap();
    FrameCache::open(&dir, 1 << 30).unwrap();
    assert!(!old.exists());
    assert!(fresh.exists());
}

#[test]
fn the_key_follows_the_camera_profile_by_its_content() {
    use lf_raw::dcp::testing::{Tag, ascii, srational, write};
    let t = tempfile::tempdir().unwrap();
    let f = raw_file(t.path(), "a.raw", b"hello raw");
    let profile = |name: &str| {
        let fm = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        write(vec![
            Tag(50936, 2, name.len() as u32 + 1, ascii(name)),
            Tag(50964, 10, 9, srational(&fm)),
        ])
    };
    let s = DevelopSettings {
        dcp: Some(lf_settings::dcp_id("Cache Test")),
        ..DevelopSettings::default()
    };
    let missing = CacheKey::for_file(&f, &s, Quality::Best, 2048).unwrap();
    assert_ne!(missing, key(&f));
    lf_raw::dcp::register(
        "Cache Test",
        lf_raw::dcp::Dcp::parse(&profile("one")).unwrap(),
    );
    let one = CacheKey::for_file(&f, &s, Quality::Best, 2048).unwrap();
    lf_raw::dcp::register(
        "Cache Test",
        lf_raw::dcp::Dcp::parse(&profile("two")).unwrap(),
    );
    let two = CacheKey::for_file(&f, &s, Quality::Best, 2048).unwrap();
    assert_ne!(one, missing);
    assert_ne!(one, two, "another file under the same name");
}
