use lf_fixtures::{Manifest, case_json, compare, load_png, root};
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
    assert_within(&r);
    assert!(
        r.iter().all(|(_, c)| c.max == 0.0),
        "identity must be bit-exact"
    );
}
#[test]
fn blur_dependent_cases() {
    assert_within(&render_all(|c| {
        ["detail-max", "light-max", "light-min", "look-moody"].contains(&c)
    }));
}
#[test]
fn every_case_except_spots() {
    assert_within(&render_all(|c| !c.starts_with("spots-")));
}
