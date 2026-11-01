//! Real Lightroom sample pairs, opt-in: set `LF_LR_SAMPLES` to a folder whose subfolders each
//! hold `original.*` (the photo), `export.*` (Lightroom's export of it with a look applied)
//! and, optionally, `preset.xmp` or `preset.lrtemplate` (the look). For each, the preset is
//! imported (its notes printed) and matched against the export on the shared device; the
//! mean colour difference must stay under 5 (the dialog's "Close"). Nothing ships to run
//! this on, so without the variable the test prints a note and passes.
//!
//! These runs are also how the still-unverified mappings get checked against Lightroom: the
//! horizontal perspective sign, the point-colour scale and the mask sub-type numbering.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use lf_app::lightroom::example_job;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::lightroom::map::map;
use lf_settings::lightroom::read::read_any;

/// The first file in `dir` named `stem.<anything>`.
fn find(dir: &Path, stem: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(stem)) && p.is_file())
}

#[test]
fn real_lightroom_samples_match_closely() {
    let Some(root) = std::env::var_os("LF_LR_SAMPLES") else {
        eprintln!("LF_LR_SAMPLES is not set: skipping the real Lightroom samples");
        return;
    };
    let gpu = shared_gpu();
    let mut seen = 0;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let (Some(original), Some(export)) = (find(&dir, "original"), find(&dir, "export")) else {
            continue;
        };
        let label = dir.file_name().unwrap().to_string_lossy().into_owned();
        let preset_file = find(&dir, "preset");
        let mapped = preset_file.as_ref().map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let lr = read_any(&name, &std::fs::read(p).unwrap())
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            map(&lr)
        });
        if let Some(m) = &mapped {
            for note in &m.notes {
                println!("{label}: import note: {note}");
            }
        }
        let stop = AtomicBool::new(false);
        let (_, fit, _) = example_job::run(
            &gpu,
            &original,
            &export,
            mapped.as_ref().map(|m| &m.preset),
            &label,
            &stop,
        )
        .unwrap_or_else(|e| panic!("{label}: {e}"));
        println!(
            "{label}: dE mean {:.2}, p95 {:.2}, aligned {}, residual {:.4}, grain {:.1}",
            fit.delta_e_mean, fit.delta_e_p95, fit.aligned, fit.residual, fit.grain
        );
        assert!(
            fit.delta_e_mean < 5.0,
            "{label}: mean dE {:.2} is not under 5",
            fit.delta_e_mean
        );
        seen += 1;
    }
    assert!(
        seen > 0,
        "no sample folder with original.* and export.* in LF_LR_SAMPLES"
    );
}
