use lf_fixtures::{Manifest, case_json, compare, load_png, root};
use lf_pipeline::texture::{Source, read_back_level};
use lf_pipeline::{Gpu, Renderer};
use lf_settings::normalise;

fn render_all(filter: impl Fn(&str) -> bool) -> Vec<(String, lf_fixtures::Comparison)> {
    let gpu = Gpu::new().unwrap();
    let m = Manifest::load().unwrap();
    let mut out = vec![];
    for name in m.sources.keys() {
        let src = load_png(root().join(format!("sources/{name}.png"))).unwrap();
        for id in m.cases.iter().filter(|c| filter(c)) {
            let s = normalise(&case_json(id).unwrap());
            let mut r = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
            let got = r.render(&s).unwrap();
            let want = load_png(root().join(format!("renders/{name}__{id}.png"))).unwrap();
            out.push((format!("{name}__{id}"), compare(&got, &want)));
        }
    }
    out
}

fn assert_within(results: &[(String, lf_fixtures::Comparison)]) {
    let t = Manifest::load().unwrap().thresholds;
    let bad: Vec<_> = results
        .iter()
        .filter(|(_, c)| !c.size_ok || c.mean >= t.mean || c.p999 >= t.p999 || c.max > t.max_sanity)
        .collect();
    assert!(
        bad.is_empty(),
        "out of tolerance:\n{}",
        bad.iter()
            .map(|(n, c)| format!(
                "{n}: mean {:.5} p999 {:.5} max {:.5} size_ok {}",
                c.mean, c.p999, c.max, c.size_ok
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn identity_is_exact() {
    let r = render_all(|c| c == "identity");
    assert_eq!(r.len(), 6);
    assert_within(&r);
    assert!(
        r.iter().all(|(_, c)| c.max == 0.0),
        "identity must be bit-exact"
    );
}
#[test]
fn blur_dependent_cases() {
    let r = render_all(|c| ["detail-max", "light-max", "light-min", "look-moody"].contains(&c));
    assert_eq!(r.len(), 24);
    assert_within(&r);
}
#[test]
fn every_case_except_spots() {
    let r = render_all(|c| !c.starts_with("spots-"));
    assert_eq!(r.len(), 216);
    assert_within(&r);
}
#[test]
fn spots_cases() {
    let r = render_all(|c| c.starts_with("spots-"));
    assert_eq!(r.len(), 12);
    assert_within(&r);
}
#[test]
fn all_228_renders_match() {
    let r = render_all(|_| true);
    assert_eq!(r.len(), 228);
    assert_within(&r);
}

/// The caches follow the spots as the web's `spotsKey` and `detailKey` do: one renderer taken
/// through spots on, off, on again and changed draws what a fresh renderer draws each time
/// (so the detail blurs are rebuilt from the spotted image), and the spotted target's mips
/// are rebuilt after the spots pass.
#[test]
fn spots_and_detail_caches_follow_the_spots() {
    let gpu = Gpu::new().unwrap();
    let src = load_png(root().join("sources/portrait.png")).unwrap();
    let detail = |id: &str| {
        let mut s = normalise(&case_json(id).unwrap());
        s.sharpen = 60.0;
        s.texture = 40.0;
        s
    };
    let with = detail("spots-max");
    let mut without = with.clone();
    without.spots.clear();
    let changed = detail("spots-min");

    let mut reused = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
    for (step, s) in [&with, &without, &with, &changed].into_iter().enumerate() {
        let got = reused.render(s).unwrap();
        let mut fresh = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
        assert!(
            got == fresh.render(s).unwrap(),
            "step {step} differs from a fresh render"
        );
    }

    let spotted = reused.prepare(&with).unwrap();
    let level0 = read_back_level(&gpu, spotted, 0).unwrap();
    let level1 = read_back_level(&gpu, spotted, 1).unwrap();
    let rebuilt = Source::upload(&gpu, &level0, true).unwrap();
    assert!(level0 != src, "prepare returns the spotted image");
    assert!(
        level1 == read_back_level(&gpu, &rebuilt, 1).unwrap(),
        "mips rebuilt after the spots pass"
    );
}
