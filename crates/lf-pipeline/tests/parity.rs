use lf_fixtures::{Manifest, Rect, case_json, compare, compare_region, load_png, root};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_pipeline::texture::{Source, read_back_level};
use lf_settings::normalise;

fn render_all(filter: impl Fn(&str) -> bool) -> Vec<(String, lf_fixtures::Comparison)> {
    let gpu = shared_gpu();
    let m = Manifest::load().unwrap();
    let mut out = vec![];
    for name in m.sources.keys() {
        let src = load_png(root().join(format!("sources/{name}.png"))).unwrap();
        for id in m.cases.iter().filter(|c| filter(c)) {
            let s = normalise(&case_json(id).unwrap());
            let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
            let got = r.render(&s).unwrap();
            let want = load_png(root().join(format!("renders/{name}__{id}.png"))).unwrap();
            out.push((format!("{name}__{id}"), compare(&got, &want)));
        }
    }
    out
}

/// The worst mean, p999 and max over `results`, each with the render that produced it.
fn worst(results: &[(String, lf_fixtures::Comparison)]) -> String {
    let pick = |f: fn(&lf_fixtures::Comparison) -> f64| {
        let (n, c) = results
            .iter()
            .max_by(|a, b| f(&a.1).total_cmp(&f(&b.1)))
            .expect("at least one render");
        format!("{:.3}/255 ({n})", f(c) * 255.0)
    };
    format!(
        "worst mean {}, p999 {}, max {}",
        pick(|c| c.mean),
        pick(|c| c.p999),
        pick(|c| c.max)
    )
}

/// Fails on any render outside the gate; on success prints the worst figures (shown in CI,
/// which runs the tests with `--nocapture`), so drift is visible before it fails.
fn assert_within(label: &str, results: &[(String, lf_fixtures::Comparison)]) {
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
    println!(
        "parity {label}: {} renders, {}",
        results.len(),
        worst(results)
    );
}

#[test]
fn identity_is_exact() {
    let r = render_all(|c| c == "identity");
    assert_eq!(r.len(), 6);
    assert_within("identity", &r);
    assert!(
        r.iter().all(|(_, c)| c.max == 0.0),
        "identity must be bit-exact"
    );
}
#[test]
fn blur_dependent_cases() {
    let r = render_all(|c| ["detail-max", "light-max", "light-min", "look-moody"].contains(&c));
    assert_eq!(r.len(), 24);
    assert_within("blur-dependent", &r);
}
#[test]
fn every_case_except_spots() {
    let r = render_all(|c| !c.starts_with("spots-"));
    assert_eq!(r.len(), 216);
    assert_within("every case except spots", &r);
}
#[test]
fn spots_cases() {
    let r = render_all(|c| c.starts_with("spots-"));
    assert_eq!(r.len(), 12);
    assert_within("spots", &r);
}
/// Inside each spot (its centre `(x·w, y·h)`, radius `r·w` pixels, plus a 2 px margin) the
/// largest error is under 4/255, strictly: a spot is a small part of the picture, so the
/// whole-image mean and p999 would hide a wrong spot, and its p999 is meaningless.
#[test]
fn spots_regions_match() {
    let gpu = shared_gpu();
    let m = Manifest::load().unwrap();
    let bound = 4.0 / 255.0;
    let mut lines = vec![];
    let mut bad = vec![];
    for name in m.sources.keys() {
        let src = load_png(root().join(format!("sources/{name}.png"))).unwrap();
        let (w, h) = src.dimensions();
        for id in m.cases.iter().filter(|c| c.starts_with("spots-")) {
            let json = case_json(id).unwrap();
            let mut r = Renderer::new(gpu.clone(), &src, w, h).unwrap();
            let s = normalise(&json);
            let got = r.render(&s).unwrap();
            // The same render without the spots: the regions must see the spots.
            let mut unspotted = s.clone();
            unspotted.spots.clear();
            let plain = r.render(&unspotted).unwrap();
            let mut plain_worst: f64 = 0.0;
            let want = load_png(root().join(format!("renders/{name}__{id}.png"))).unwrap();
            let spots = json["spots"].as_array().expect("a spots case has spots");
            assert!(!spots.is_empty(), "{id} has no spots");
            let mut worst: f64 = 0.0;
            for spot in spots {
                let at = |k: &str| spot[k].as_f64().expect("spot field");
                let (cx, cy) = (at("x") * f64::from(w), at("y") * f64::from(h));
                let rect = Rect::around(cx, cy, at("r") * f64::from(w) + 2.0, w, h);
                let c = compare_region(&got, &want, rect);
                if !c.size_ok || c.max >= bound {
                    bad.push(format!(
                        "{name}__{id} spot at ({cx}, {cy}): max {:.3}/255",
                        c.max * 255.0
                    ));
                }
                worst = worst.max(c.max);
                plain_worst = plain_worst.max(compare_region(&plain, &want, rect).max);
            }
            // spots-min's one 1 px heal can leave smooth content within the bound; spots-max's
            // three large spots cannot.
            assert!(
                id != "spots-max" || plain_worst >= bound,
                "{name}__{id}: the regions miss the spots"
            );
            lines.push(format!(
                "{name}__{id} {:.0}/255 (without spots {:.0}/255)",
                worst * 255.0,
                plain_worst * 255.0
            ));
        }
    }
    assert_eq!(lines.len(), 12);
    assert!(
        bad.is_empty(),
        "spots out of tolerance:
{}",
        bad.join(
            "
"
        )
    );
    println!("parity spot regions: max {}", lines.join(", "));
}

#[test]
fn all_228_renders_match() {
    let r = render_all(|_| true);
    assert_eq!(r.len(), 228);
    assert_within("all 228", &r);
}

/// The caches follow the spots as the web's `spotsKey` and `detailKey` do: one renderer taken
/// through spots on, off, on again and changed draws what a fresh renderer draws each time
/// (so the detail blurs are rebuilt from the spotted image), and the spotted target's mips
/// are rebuilt after the spots pass.
#[test]
fn spots_and_detail_caches_follow_the_spots() {
    let gpu = shared_gpu();
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

    let mut reused = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    for (step, s) in [&with, &without, &with, &changed].into_iter().enumerate() {
        let got = reused.render(s).unwrap();
        let mut fresh = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
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
