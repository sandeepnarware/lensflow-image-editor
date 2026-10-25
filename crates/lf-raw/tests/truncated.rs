//! Truncated copies of the corpus files are refused as incomplete, not opened as a
//! half-decoded frame. Skips files that are absent (see `tests/raw-samples`).

use lf_raw::RawError;
use lf_raw::decode::decode_raw;
use std::path::PathBuf;

fn sample(name: &str) -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/raw-samples/files")
        .join(name);
    let bytes = std::fs::read(&path).ok();
    if bytes.is_none() {
        println!("skipped: no sample {name} (run scripts/fetch-raw-samples.ps1 or .sh)");
    }
    bytes
}

#[test]
fn cut_files_are_corrupt_and_incomplete() {
    // (file, fraction kept for the second cut). The second cut lands inside the sensor
    // data: 99 % where the sensor data runs to the end of the file; the PEF keeps its
    // preview after the sensor data, which ends at about 86 %. RW2 is absent: its
    // sensor data's length is not recorded (see `container.rs`).
    for (name, late) in [
        ("canon-eos-40d.cr2", 99),
        ("canon-eos-20d.cr2", 99),
        ("canon-eos-rp.cr3", 99),
        ("nikon-d40.nef", 99),
        ("nikon-d80.dng", 99),
        ("sony-ilce-7s.arw", 99),
        ("pentax-k10d.pef", 85),
        ("olympus-e-420.orf", 99),
        ("fujifilm-x20.raf", 99),
    ] {
        let Some(bytes) = sample(name) else { continue };
        for (label, keep) in [
            ("50%".to_string(), bytes.len() / 2),
            (format!("{late}%"), bytes.len() * late / 100),
        ] {
            let ext = name.rsplit('.').next().unwrap_or("raw");
            let path = std::env::temp_dir().join(format!(
                "lf-raw-trunc-{}-{label}-{name}.{ext}",
                std::process::id()
            ));
            std::fs::write(&path, &bytes[..keep]).unwrap();
            let r = decode_raw(&path);
            let _ = std::fs::remove_file(&path);
            match r {
                Err(RawError::Corrupt { reason, .. }) => {
                    assert_eq!(reason, "the file is incomplete", "{name} cut to {label}")
                }
                Err(e) => panic!("{name} cut to {label}: {e}"),
                Ok(f) => panic!("{name} cut to {label} decoded as {}x{}", f.width, f.height),
            }
        }
    }
}

/// RW2 is not checked up front (see `container.rs`), but rawler's Panasonic
/// decompressor panics when the data runs out, inside rayon worker threads, and the
/// panic guard turns that into `Corrupt` instead of a crash.
#[test]
fn cut_rw2_is_corrupt_through_the_panic_guard() {
    let Some(bytes) = sample("panasonic-dmc-lx3.rw2") else {
        return;
    };
    for pct in [50, 99] {
        let path =
            std::env::temp_dir().join(format!("lf-raw-trunc-{}-{pct}-lx3.rw2", std::process::id()));
        std::fs::write(&path, &bytes[..bytes.len() * pct / 100]).unwrap();
        let r = decode_raw(&path);
        let _ = std::fs::remove_file(&path);
        match r {
            Err(RawError::Corrupt { reason, .. }) => println!("RW2 cut to {pct}%: {reason}"),
            Err(e) => panic!("RW2 cut to {pct}%: {e}"),
            Ok(f) => panic!("RW2 cut to {pct}% decoded as {}x{}", f.width, f.height),
        }
    }
}
