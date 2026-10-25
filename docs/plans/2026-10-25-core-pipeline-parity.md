# lensflow-ImageEditor, Part 2a: Core Settings and Render Pipeline Parity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust workspace whose GPU render pipeline reproduces the lensflow.in web editor pixel for pixel on the 228 golden fixtures, with the shared settings model, looks, curves and geometry it needs.

**Architecture:** Two library crates: `lf-settings` (the `EditSettings` JSON contract, normalisation, built-in looks, curve tables, shader values, geometry maths, all pure CPU) and `lf-pipeline` (a headless wgpu renderer with WGSL ports of the four WebGL shaders: copy, gaussian, spots, develop). A third crate, `lf-fixtures`, loads the golden fixtures and compares renders. Everything renders without a window, so the parity suite runs in CI on a software Vulkan driver. RAW decoding, the catalogue, the account and the app UI are later parts.

**Tech Stack:** Rust stable (edition 2024), `wgpu` (Vulkan, Metal, DX12), `naga`-validated WGSL, `serde`/`serde_json`, `image` (PNG decode/encode), `sha2`, `anyhow`/`thiserror`. No GPL dependencies. Apache-2.0.

**Spec:** `D:\pet-projects\photo-studio-management\docs\superpowers\specs\2026-10-25-lensflow-image-editor-design.md` (sections Constraints, Decisions → RAW headroom, Architecture → The pipeline, Parity tests). The web editor it ports lives at `D:\pet-projects\photo-studio-management\frontend\src\modules\galleries\editor\` (`settings.ts`, `curve.ts`, `geometry.ts`, `renderer.ts`, `shaders.ts`); the golden fixtures and their README at `D:\pet-projects\photo-studio-management\frontend\parity-fixtures\` and `.../editor/parity/README.md` (on branch `feature/desktop-editor-server`, checked out in the worktree `D:\pet-projects\photo-studio-management\.claude\worktrees\desktop-editor-server`).

Parts 2b (RAW develop stage), 2c (catalogue, account, grade, app) and 2d (packaging and release pipeline) follow.

## Global Constraints

- Repository: `D:\pet-projects\lensflow-image-editor` (public, `sandeepnarware/lensflow-image-editor`, branch `main`). Commits go to `main` directly in this part (no users yet); every task ends with a push.
- Toolchain: `rust-toolchain.toml` pins `channel = "stable"`; edition 2024; `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` green before every commit.
- Licence: `LICENSE` is Apache-2.0; `NOTICE` credits every bundled third-party component; no GPL crate anywhere in `cargo tree`.
- Fixtures: `tests/fixtures/` is a verbatim copy of the monorepo's `frontend/parity-fixtures/` (6 sources, 38 cases, 228 renders, `manifest.json`), tracked with `tests/fixtures/** -text` in `.gitattributes`. Never regenerated here.
- Parity thresholds, per channel on a 0..1 scale, for every one of the 228 renders: mean absolute error **< 1/255** and maximum absolute error **< 4/255**; output size must match the golden PNG exactly.
- Settings JSON field names are exactly the web's (camelCase: `sharpenRadius`, `vignetteMidpoint`, `colorNoise`, `grainSize`, `flipH`, `flipV`); numbers serialise as JSON numbers (integers without a decimal point where the web wrote integers is not required: compare as values, not text).
- Rendering is headless: `wgpu::Instance` → adapter (`force_fallback_adapter` allowed when no hardware adapter exists) → device; no window, no winit, in this part.
- CI: `.github/workflows/ci.yml` on `ubuntu-latest` installs `mesa-vulkan-drivers` (lavapipe) and runs fmt, clippy and the full test suite including parity.
- Commit messages end with: `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`

## Review Focus

1. A settings JSON with unknown keys, missing keys, wrong types or out-of-range values must normalise exactly as the web does (drop, default, clamp), never error. Pinned in Task 2 (the web's own normalise cases plus every fixture case round-trips unchanged).
2. The wide-blur input is a 384 px copy sampled through a mip chain; a wrong mip filter or LOD shows up only on blur-dependent cases. Pinned in Task 6 (parity on `detail-max`, `light-max`, `light-min`, `look-moody` is part of the full-suite gate, and the task's step says to report those four first).
3. A machine with no usable GPU adapter must get a clear error, not a panic. Pinned in Task 5 (`Gpu::new` returns `PipelineError::NoAdapter` when forced to fail; the error message is tested).
4. A source larger than the device's texture limit must be refused with a message naming the limit, not crash. Pinned in Task 5.
5. 8-bit output rounding and the curve LUT sampled at texel centres decide whether `identity` matches at all. Pinned in Task 6 (`identity` on all six sources is the first parity gate and must pass with maximum error 0).

---

### Task 1: Workspace scaffold, fixtures and CI

**Files:**
- Create: `Cargo.toml` (workspace), `rust-toolchain.toml`, `.gitignore`, `.gitattributes`, `LICENSE`, `NOTICE`, `README.md`
- Create: `crates/lf-settings/Cargo.toml`, `crates/lf-settings/src/lib.rs` (empty module list)
- Create: `crates/lf-pipeline/Cargo.toml`, `crates/lf-pipeline/src/lib.rs` (empty)
- Create: `crates/lf-fixtures/Cargo.toml`, `crates/lf-fixtures/src/lib.rs`
- Create: `tests/fixtures/` (copied), `.github/workflows/ci.yml`
- Test: `crates/lf-fixtures/tests/manifest.rs`

**Interfaces:**
- Produces: `lf_fixtures::root() -> PathBuf` (the `tests/fixtures` folder, found from `CARGO_MANIFEST_DIR`); `lf_fixtures::Manifest { sources: BTreeMap<String, (u32, u32)>, cases: Vec<String>, files: BTreeMap<String, String> /* path → sha256 */, thresholds: Thresholds { mean, max } }` with `Manifest::load() -> anyhow::Result<Manifest>`; `lf_fixtures::load_png(path) -> anyhow::Result<image::RgbaImage>`; `lf_fixtures::case_json(id) -> anyhow::Result<serde_json::Value>`; `lf_fixtures::compare(got: &RgbaImage, want: &RgbaImage) -> Comparison { mean: f64, max: f64, size_ok: bool }` where errors are per-channel absolute differences over R, G, B on a 0..1 scale (alpha ignored).
- Workspace members: `crates/lf-settings`, `crates/lf-pipeline`, `crates/lf-fixtures`. Shared `[workspace.dependencies]`: `serde` (derive), `serde_json`, `anyhow`, `thiserror`, `image` (png feature only), `sha2`, `hex`.

- [ ] **Step 1: Write the failing test** (`crates/lf-fixtures/tests/manifest.rs`)

```rust
use lf_fixtures::{Manifest, load_png, root};
use sha2::{Digest, Sha256};

#[test]
fn manifest_lists_the_whole_fixture_set_and_hashes_match() {
    let m = Manifest::load().unwrap();
    assert_eq!(m.sources.len(), 6);
    assert_eq!(m.cases.len(), 38);
    assert_eq!(m.files.values().count(), 6 + 38 + 228 + 0);
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
```

Read `tests/fixtures/manifest.json` first and adjust the `files` count assertion to what the manifest actually lists (it may list the manifest itself or omit sources); the assertion must equal the real count, not a guess.

- [ ] **Step 2: Scaffold the workspace and copy the fixtures**

Copy `D:\pet-projects\photo-studio-management\.claude\worktrees\desktop-editor-server\frontend\parity-fixtures\` to `tests/fixtures/` verbatim (sources, cases, renders, manifest.json). `.gitattributes`: `tests/fixtures/** -text`. `.gitignore`: `/target`. `NOTICE`: name the fixtures' origin (LensFlow web editor) and leave a section header for later bundled components. `README.md`: what the project is (one paragraph from the spec's Intent), how to build and test, the parity rule.

- [ ] **Step 3: Run the test to verify it fails, then implement `lf-fixtures`, then verify it passes**

Run: `cargo test -p lf-fixtures`
Expected: PASS (2 tests).

- [ ] **Step 4: CI workflow**

`.github/workflows/ci.yml`: on push and pull_request; `ubuntu-latest`; `dtolnay/rust-toolchain@stable` with `rustfmt, clippy`; `sudo apt-get install -y mesa-vulkan-drivers libvulkan1`; `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` with env `WGPU_BACKEND=vulkan` and `LF_ALLOW_FALLBACK_ADAPTER=1`. Cache `~/.cargo` and `target` with `Swatinem/rust-cache@v2`.

- [ ] **Step 5: Commit and push**

```bash
git add -A
git commit -m "Workspace, golden fixtures and CI"
git push -u origin main
```

Then `gh run watch` the CI run; it must pass (fmt, clippy, the two fixture tests) before the task is done.

---

### Task 2: `lf-settings`: the EditSettings model and normalisation

**Files:**
- Create: `crates/lf-settings/src/settings.rs`, `crates/lf-settings/src/normalise.rs`
- Modify: `crates/lf-settings/src/lib.rs`
- Test: `crates/lf-settings/tests/normalise.rs`

**Interfaces:**
- Produces (all `pub`, `serde::{Serialize, Deserialize}`, `Clone`, `PartialEq`, `Debug`):
  - `HslBand` enum `{Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta}` with `HSL_BANDS: [HslBand; 8]` in that order, `HSL_HUES: [f64; 8] = [0, 30, 60, 120, 180, 225, 270, 315]`, serialised as lowercase names.
  - `HslAdjust { h: f64, s: f64, l: f64 }`; `CurvePoint = [f64; 2]`; `Curves { rgb, r, g, b: Vec<CurvePoint> }`; `GradeWheel { hue: f64, sat: f64 }`; `Grading { shadows, midtones, highlights: GradeWheel, balance: f64 }`; `Spot { x, y, sx, sy, r: f64, mode: SpotMode }` with `SpotMode { Heal, Clone }` serialised `"heal"`/`"clone"`; `Crop { x, y, w, h: f64 }`; `Rotate { R0 = 0, R90 = 90, R180 = 180, R270 = 270 }` serialised as the integers.
  - `EditSettings` with every field of the web's interface in the same order and names (`#[serde(rename = "sharpenRadius")]` etc. or `rename_all = "camelCase"` on the struct), `hsl: BTreeMap<HslBand, HslAdjust>` or a fixed `[HslAdjust; 8]` with a custom map (de)serialiser keyed by band name, `desktop: Option<serde_json::Map<String, serde_json::Value>>` with `skip_serializing_if = "Option::is_none"`.
  - `SETTINGS_VERSION: u32 = 2`; `MAX_SPOTS: usize = 48`; `MAX_DESKTOP_JSON: usize = 65536`.
  - `EditSettings::default_settings() -> EditSettings` (the web's `defaultSettings()` values: grading hues 220/30/45, `sharpenRadius` 1, `vignetteMidpoint` 50, `grainSize` 25, `aspect` `"original"`, identity curves `[[0,0],[1,1]]`).
  - `normalise(input: &serde_json::Value) -> EditSettings` (the web's `normaliseSettings`: every clamp range and fallback as in `settings.ts` lines 171–250; curves: points filtered to finite pairs, clamped 0..1, sorted by x, at most 16, fewer than 2 → identity; spots at most 48, `r` 0.002..0.25 default 0.02, coordinates default 0.5; crop `x,y` 0..1 and `w,h` 0.01..1−x/y defaulting to 1−x/y; `aspect` a string of at most 12 chars else `"original"`; `rotate` one of the four else 0; `desktop` kept only when it is a non-empty JSON object whose compact UTF-8 JSON is ≤ 65536 bytes).
  - `EditSettings::to_value(&self) -> serde_json::Value`; `is_default(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**

```rust
use lf_fixtures::{Manifest, case_json};
use lf_settings::{EditSettings, normalise};
use serde_json::json;

#[test]
fn every_fixture_case_is_already_normal_and_round_trips_unchanged() {
    for id in Manifest::load().unwrap().cases {
        let v = case_json(&id).unwrap();
        assert_eq!(normalise(&v).to_value(), v, "{id}");
    }
}

#[test]
fn identity_case_equals_the_defaults() {
    assert_eq!(normalise(&case_json("identity").unwrap()), EditSettings::default_settings());
}

#[test]
fn clamps_and_drops_like_the_web() {
    let s = normalise(&json!({ "exposure": 12, "contrast": -500, "hsl": { "red": { "h": 300 } }, "rotate": 45, "spots": [{ "x": 2, "r": 9 }], "junk": 1 }));
    assert_eq!(s.exposure, 5.0);
    assert_eq!(s.contrast, -100.0);
    assert_eq!(s.hsl[&HslBand::Red], HslAdjust { h: 100.0, s: 0.0, l: 0.0 });
    assert_eq!(s.rotate, Rotate::R0);
    assert_eq!((s.spots[0].x, s.spots[0].r, s.spots[0].mode), (1.0, 0.25, SpotMode::Heal));
    assert!(s.to_value().get("junk").is_none());
}

#[test]
fn missing_or_broken_input_is_an_unedited_photo() {
    assert_eq!(normalise(&json!(null)), EditSettings::default_settings());
    assert_eq!(normalise(&json!("x")), EditSettings::default_settings());
    assert!(normalise(&json!({})).is_default());
}

#[test]
fn desktop_object_rules() {
    let d = json!({ "develop": { "profile": "camera" }, "masks": [{ "kind": "sky" }] });
    assert_eq!(normalise(&json!({ "desktop": d })).to_value()["desktop"], d);
    assert!(normalise(&json!({ "desktop": {} })).desktop.is_none());
    assert!(normalise(&json!({ "desktop": [1, 2] })).desktop.is_none());
    assert!(normalise(&json!({ "desktop": "x" })).desktop.is_none());
    assert!(normalise(&json!({ "desktop": { "blob": "x".repeat(65536) } })).desktop.is_none());
    assert!(normalise(&json!({ "desktop": { "blob": "€".repeat(21900) } })).desktop.is_none()); // 65 700 bytes, 21 900 chars
}
```

(Adjust `s.hsl[&HslBand::Red]` to whatever indexing the chosen `hsl` container offers; the asserted values are fixed.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lf-settings --test normalise`
Expected: compile failure (types missing).

- [ ] **Step 3: Implement**

Port `settings.ts` lines 1–250 faithfully. The curve `normalisePoints` filter keeps only points that are arrays of exactly two finite numbers. Serialise `hsl` in band order so the round-trip test compares equal `Value`s (object key order does not matter to `serde_json::Value` equality; number formatting does not either).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p lf-settings`
Expected: PASS (5 tests, 38 cases round-tripped).

- [ ] **Step 5: Commit and push**

```bash
git add crates/lf-settings
git commit -m "lf-settings: the EditSettings model and normalisation"
git push
```

---

### Task 3: `lf-settings`: curves, shader values and geometry

**Files:**
- Create: `crates/lf-settings/src/curve.rs`, `crates/lf-settings/src/shader_values.rs`, `crates/lf-settings/src/geometry.rs`
- Test: `crates/lf-settings/tests/curve.rs`, `crates/lf-settings/tests/shader_values.rs`, `crates/lf-settings/tests/geometry.rs`

**Interfaces:**
- `curve::curve_table(points: &[CurvePoint], size: usize) -> Vec<f32>` (Fritsch–Carlson monotone cubic exactly as `curve.ts`: sort, drop points within 1e-4 in x of the previous, fewer than 2 → linear ramp; clamp outputs 0..1).
- `curve::pack_curves(c: &Curves) -> [u8; 1024]` (256 RGBA texels: R/G/B = each channel's table evaluated at `round(master[i] * 255)`, scaled `round(v * 255)`, A = 255).
- `shader_values::hue_color(hue: f64) -> [f32; 3]`; `shader_values::grade_offset(hue: f64, sat: f64) -> [f32; 3]`; `shader_values::ShaderValues { wb: [f32; 3], hsl: [f32; 24], band_hue: [f32; 8], band_width: [f32; 8], grade_sh: [f32; 3], grade_mid: [f32; 3], grade_hi: [f32; 3] }` and `shader_values(s: &EditSettings) -> ShaderValues` (the web's `shaderValues`, including the luma normalisation of `wb`).
- `geometry::Affine = [f64; 6]` (row-major `[a, b, c, d, e, f]`), `IDENTITY`, `multiply`, `invert`, `apply`, `turned_size(w, h, rotate) -> (u32, u32)`, `output_to_source(w, h, &EditSettings-like Geometry, crop: &Crop) -> Affine`, `output_size(w, h, g) -> (u32, u32)` (`max(1, round(crop.w * tw))`), `crop_inside`, `largest_crop`, `fit_crop`, `turn_settings` (ports of `geometry.ts`). Define `Geometry { rotate, flip_h, flip_v, angle, crop }` and `impl From<&EditSettings> for Geometry`.

- [ ] **Step 1: Write the failing tests**

```rust
// curve.rs
#[test] fn identity_curve_is_a_ramp() { let t = curve_table(&[[0.0, 0.0], [1.0, 1.0]], 256); for i in 0..256 { assert!((t[i] - i as f32 / 255.0).abs() < 1e-6); } }
#[test] fn fewer_than_two_points_is_a_ramp() { assert_eq!(curve_table(&[[0.3, 0.9]], 256), curve_table(&[], 256)); }
#[test] fn curve_passes_through_its_points_and_never_overshoots() {
    let pts = [[0.0, 0.0], [0.25, 0.1], [0.5, 0.6], [0.75, 0.7], [1.0, 1.0]];
    let t = curve_table(&pts, 256);
    for [x, y] in pts { assert!((t[(x * 255.0).round() as usize] - y as f32).abs() < 0.01); }
    assert!(t.windows(2).all(|w| w[1] >= w[0] - 1e-6));
}
#[test] fn pack_curves_identity_is_the_diagonal() { let p = pack_curves(&Curves::identity()); for i in 0..256 { assert_eq!(&p[i*4..i*4+4], &[i as u8, i as u8, i as u8, 255]); } }

// shader_values.rs
#[test] fn defaults_are_neutral() { let v = shader_values(&EditSettings::default_settings()); assert_eq!(v.wb, [1.0, 1.0, 1.0]); assert_eq!(v.grade_sh, [0.0; 3]); assert_eq!(v.hsl, [0.0; 24]); }
#[test] fn band_widths_are_the_larger_gap_to_a_neighbour() { assert_eq!(shader_values(&EditSettings::default_settings()).band_width, [45.0, 30.0, 60.0, 60.0, 60.0, 45.0, 45.0, 45.0]); }
#[test] fn warm_white_balance_keeps_white_as_bright() { let mut s = EditSettings::default_settings(); s.temperature = 100.0; let wb = shader_values(&s).wb; let y = 0.2126 * wb[0] + 0.7152 * wb[1] + 0.0722 * wb[2]; assert!((y - 1.0).abs() < 1e-5); assert!(wb[0] > wb[2]); }
#[test] fn grade_offset_has_zero_luma() { let o = grade_offset(190.0, 100.0); assert!((0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2]).abs() < 1e-6); }

// geometry.rs
#[test] fn quarter_turn_maps_output_corners_to_source() {
    let g = Geometry { rotate: Rotate::R90, flip_h: false, flip_v: false, angle: 0.0, crop: Crop { x: 0.0, y: 0.0, w: 1.0, h: 1.0 } };
    let m = output_to_source(400, 300, &g, &g.crop);
    assert_close(apply(&m, 0.0, 0.0), (0.0, 1.0));
    assert_close(apply(&m, 1.0, 0.0), (0.0, 0.0));
    assert_close(apply(&m, 1.0, 1.0), (1.0, 0.0));
    assert_eq!(turned_size(400, 300, Rotate::R90), (300, 400));
    assert_eq!(output_size(400, 300, &g), (300, 400));
}
#[test] fn identity_geometry_is_the_identity_map() { let g = Geometry::from(&EditSettings::default_settings()); let m = output_to_source(640, 480, &g, &g.crop); assert_close(apply(&m, 0.25, 0.75), (0.25, 0.75)); }
#[test] fn crop_fits_after_straightening() { assert!(cropInside_equivalent(...)); } // port the web's cropInside/largestCrop expectations: largest_crop(1000, 1000, 45.0, Some(1.0)) is centred and inside; fit_crop of a full crop at 10° shrinks it
```

Write `crop_fits_after_straightening` as two concrete assertions: `largest_crop(1000, 1000, 45.0, Some(1.0))` returns a crop with `w == h`, centred (`x == (1 - w) / 2`), and `crop_inside(&crop, 1000.0, 1000.0, 45.0)`; `fit_crop(&full, 1000.0, 1000.0, 10.0)` returns a crop smaller than 1 and inside.

- [ ] **Step 2: Run the tests to verify they fail**, implement the three modules as straight ports, then run `cargo test -p lf-settings` and verify PASS.

- [ ] **Step 3: Commit and push**

```bash
git add crates/lf-settings
git commit -m "lf-settings: curves, shader values and geometry"
git push
```

---

### Task 4: `lf-settings`: looks, groups, apply and pick

**Files:**
- Create: `crates/lf-settings/src/looks.rs`, `crates/lf-settings/src/groups.rs`
- Test: `crates/lf-settings/tests/looks.rs`

**Interfaces:**
- `groups::GroupKey` enum `{Light, Color, Hsl, Curve, Grading, Detail, Sharpen, Effects, Bw, Geometry, Spots}`; `GROUPS: [(GroupKey, &'static str /* label */, &'static [&'static str] /* JSON keys */); 11]` with the web's labels and key lists; `DEFAULT_COPY_GROUPS: [GroupKey; 9]`.
- `groups::apply_groups(onto: &EditSettings, from: &serde_json::Value, groups: &[GroupKey]) -> EditSettings`; `pick_groups(s: &EditSettings, groups: &[GroupKey]) -> serde_json::Value` (includes `"v"`); `groups_in(partial: &serde_json::Value) -> Vec<GroupKey>`.
- `looks::Look { id: &'static str, name: &'static str, settings: serde_json::Value }`; `LOOKS: [Look; 12]` with the exact values of `settings.ts` `LOOKS` (ids `natural, vivid, warm, cool, golden, matte, fade, moody, teal-orange, bw, bw-high, bw-soft`); `looks::apply_look(onto: &EditSettings, look: &serde_json::Value) -> EditSettings` (reset the copy groups to defaults, then lay the look's groups over).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn every_built_in_look_matches_its_golden_case() {
    for look in LOOKS.iter() {
        let want = case_json(&format!("look-{}", look.id)).unwrap();
        let got = apply_look(&EditSettings::default_settings(), &look.settings).to_value();
        assert_eq!(got, want, "look-{}", look.id);
    }
}
#[test]
fn a_look_keeps_the_photo_s_crop_and_spots() {
    let mut s = EditSettings::default_settings();
    s.crop = Crop { x: 0.1, y: 0.1, w: 0.5, h: 0.5 };
    s.spots.push(Spot { x: 0.5, y: 0.5, sx: 0.6, sy: 0.6, r: 0.02, mode: SpotMode::Heal });
    let out = apply_look(&s, &LOOKS[0].settings);
    assert_eq!(out.crop, s.crop);
    assert_eq!(out.spots, s.spots);
    assert_eq!(out.contrast, 8.0);
}
#[test]
fn looks_do_not_pile_up() {
    let a = apply_look(&EditSettings::default_settings(), &LOOKS[4].settings); // golden: temperature 26
    let b = apply_look(&a, &LOOKS[9].settings); // bw classic: no temperature
    assert_eq!(b.temperature, 0.0);
    assert!(b.bw);
}
#[test]
fn pick_and_apply_groups_round_trip_and_default_groups_skip_geometry() {
    let mut s = EditSettings::default_settings();
    s.exposure = 1.5; s.angle = 10.0;
    let picked = pick_groups(&s, &DEFAULT_COPY_GROUPS);
    assert!(picked.get("angle").is_none());
    assert_eq!(picked["exposure"], 1.5);
    let onto = EditSettings::default_settings();
    assert_eq!(apply_groups(&onto, &picked, &DEFAULT_COPY_GROUPS).exposure, 1.5);
    assert_eq!(groups_in(&picked), vec![GroupKey::Light]);
}
```

- [ ] **Step 2: Run to verify failure, implement, run to verify PASS** (`cargo test -p lf-settings`).

- [ ] **Step 3: Commit and push**

```bash
git add crates/lf-settings
git commit -m "lf-settings: built-in looks and setting groups"
git push
```

---

### Task 5: `lf-pipeline`: headless GPU, textures with mip chains, copy and gaussian passes

**Files:**
- Create: `crates/lf-pipeline/src/gpu.rs`, `crates/lf-pipeline/src/texture.rs`, `crates/lf-pipeline/src/passes/mod.rs`, `crates/lf-pipeline/src/passes/copy.rs`, `crates/lf-pipeline/src/passes/gaussian.rs`, `crates/lf-pipeline/src/shaders/fullscreen.wgsl`, `crates/lf-pipeline/src/shaders/copy.wgsl`, `crates/lf-pipeline/src/shaders/gaussian.wgsl`, `crates/lf-pipeline/src/error.rs`
- Modify: `crates/lf-pipeline/Cargo.toml` (`wgpu`, `pollster`, `bytemuck`, `image`, `thiserror`), `crates/lf-pipeline/src/lib.rs`
- Test: `crates/lf-pipeline/tests/gpu.rs`

**Interfaces:**
- `error::PipelineError` (`thiserror`): `NoAdapter`, `DeviceRequest(String)`, `TooLarge { width, height, limit }`, `OutOfMemory`, `Readback(String)`.
- `gpu::Gpu { instance, adapter, device, queue, limits }` with `Gpu::new() -> Result<Gpu, PipelineError>`: tries a hardware adapter with `PowerPreference::HighPerformance`; when none, tries `force_fallback_adapter: true`; honours env `LF_ALLOW_FALLBACK_ADAPTER` (`0` forbids the fallback; CI sets `1`); `Gpu::new_with(options: GpuOptions { allow_fallback: bool, backend: Option<wgpu::Backends> })`. `Gpu::max_edge(&self) -> u32` (`limits.max_texture_dimension_2d`).
- `texture::Target { texture, view, width, height, mips: bool }` (`Rgba8Unorm`, `RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC`); `Target::new(gpu, w, h, mips) -> Result<Target, PipelineError>`; `texture::Source::upload(gpu, &RgbaImage, mips: bool) -> Result<Target, PipelineError>` (refuses sizes over `max_edge` with `TooLarge`); `texture::generate_mips(gpu, &Target)` (each level = the previous level sampled with the linear sampler at texel centres of the smaller level, which for even sizes is a 2×2 box average and for odd sizes is what WebGL's `generateMipmap` on SwiftShader does: level size `max(1, floor(prev / 2))`, sampling at `((x + 0.5) / w_level)`); `texture::Samplers { linear_clamp: Sampler /* mag linear, min linear, mipmap linear, clamp to edge */, nearest: Sampler }`.
- `passes::FullscreenPass`: one triangle `[-1,-1, 3,-1, -1,3]`, vertex shader `fullscreen.wgsl` producing `uv = pos * 0.5 + 0.5` with the same orientation convention as the web (row 0 = top for intermediate targets; only the final pass flips).
- `passes::copy::run(gpu, src: &Target, dst: &Target)`; `passes::gaussian::blur(gpu, from: &Target, to: &Target, tmp: &Target, sigma: f32)` (two separable runs with `radius = min(ceil(sigma * 3), 64)` and weights `exp(-i² / (2σ²))`, normalised, exactly as `GAUSSIAN`; regenerates `to`'s mips when `to.mips`).
- `texture::read_back(gpu, &Target) -> Result<RgbaImage, PipelineError>` (handles the 256-byte row alignment).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn gpu_or_a_clear_error() { match Gpu::new() { Ok(g) => assert!(g.max_edge() >= 4096), Err(e) => panic!("{e}") } }
#[test] fn forbidding_the_fallback_without_hardware_is_a_named_error() {
    let r = Gpu::new_with(GpuOptions { allow_fallback: false, backend: Some(wgpu::Backends::empty()) });
    assert!(matches!(r, Err(PipelineError::NoAdapter)));
    assert_eq!(PipelineError::NoAdapter.to_string(), "No graphics device found. lensflow-ImageEditor needs a GPU with Vulkan, Metal or DirectX 12.");
}
#[test] fn a_photo_over_the_texture_limit_is_refused() {
    let gpu = Gpu::new().unwrap();
    let limit = gpu.max_edge();
    let err = Source::upload_size_check(&gpu, limit + 1, 10).unwrap_err();
    assert!(matches!(err, PipelineError::TooLarge { .. }));
    assert!(err.to_string().contains(&limit.to_string()));
}
#[test] fn copy_round_trips_a_picture() { let gpu = Gpu::new().unwrap(); let img = checker(37, 23); let src = Source::upload(&gpu, &img, false).unwrap(); let dst = Target::new(&gpu, 37, 23, false).unwrap(); copy::run(&gpu, &src, &dst); assert_eq!(read_back(&gpu, &dst).unwrap(), img); }
#[test] fn mip_level_one_is_the_box_average_of_an_even_picture() {
    let gpu = Gpu::new().unwrap();
    let img = gradient(8, 4); // values chosen so every 2x2 average is exact in 8 bits
    let src = Source::upload(&gpu, &img, true).unwrap();
    let level1 = read_back_level(&gpu, &src, 1).unwrap();
    for y in 0..2 { for x in 0..4 { assert_eq!(level1.get_pixel(x, y)[0], box_avg(&img, x, y)); } }
}
#[test] fn gaussian_matches_the_cpu_kernel() {
    let gpu = Gpu::new().unwrap();
    let img = noise(64, 48, 7);
    let (src, to, tmp) = (Source::upload(&gpu, &img, false).unwrap(), Target::new(&gpu, 64, 48, false).unwrap(), Target::new(&gpu, 64, 48, false).unwrap());
    gaussian::blur(&gpu, &src, &to, &tmp, 2.5);
    let got = read_back(&gpu, &to).unwrap();
    let want = cpu_gaussian(&img, 2.5); // same radius/weights, clamp-to-edge, bilinear-free (integer taps)
    assert!(max_abs_diff(&got, &want) <= 1); // 8-bit rounding only
}
```

Helpers `checker`, `gradient`, `noise`, `box_avg`, `cpu_gaussian`, `max_abs_diff`, `read_back_level` live in the test file. `Source::upload_size_check` is a `pub fn` that performs only the limit check so the test needs no giant allocation.

- [ ] **Step 2: Run to verify failure (compile errors), implement, run `cargo test -p lf-pipeline` to verify PASS.**

Use `pollster::block_on` for async wgpu calls. WGSL: `textureSample` is only allowed in fragment shaders with uniform control flow; use `textureSampleLevel` wherever the web used `textureLod` and inside loops. Validate every WGSL file through `naga` at build time (`wgpu` does this on `create_shader_module`; a `#[test]` that creates every module on the device is enough).

- [ ] **Step 3: Commit and push**

```bash
git add crates/lf-pipeline
git commit -m "lf-pipeline: headless wgpu device, textures with mips, copy and gaussian passes"
git push
```

---

### Task 6: `lf-pipeline`: the develop pass and the parity suite

**Files:**
- Create: `crates/lf-pipeline/src/shaders/develop.wgsl`, `crates/lf-pipeline/src/passes/develop.rs`, `crates/lf-pipeline/src/renderer.rs`, `crates/lf-pipeline/src/curve_texture.rs`
- Modify: `crates/lf-pipeline/src/lib.rs`
- Test: `crates/lf-pipeline/tests/parity.rs`

**Interfaces:**
- `renderer::Renderer::new(gpu: &Gpu, image: &RgbaImage, full_width: u32, full_height: u32) -> Result<Renderer, PipelineError>`: uploads the source with mips; `scale = image.width / full_width`; builds `small` (long edge 384: `k = 384 / max(w, h)`, sizes `max(1, round(w * min(1, k)))`), `tmp_small`, `blur_m` (σ = `max(sw, sh) * 0.015`), `blur_l` (σ = `max(sw, sh) * 0.04`) exactly as `renderer.ts`'s constructor; a 256×1 `Rgba8Unorm` curve texture (sampled with the linear clamp sampler, as the web).
- `Renderer::render(&mut self, s: &EditSettings) -> Result<RgbaImage, PipelineError>`: the export path (`renderJpeg` up to the JPEG step): output size `output_size(full_w, full_h, s)` scaled by `image.width / full_width`, `uView = (0,0,1,1)`, develop on, no flip (intermediate orientation: row 0 = top; the web flips only when drawing to the canvas, and `readPixels` of the canvas gives row 0 = bottom, so the harness flipped rows back: the net effect is that the PNG's row 0 is the top of the picture, which is what `render` must return).
- `Renderer::prepare(&mut self, s)`: the cached detail blurs (`sharp_sigma = max(0.35, sharpenRadius * scale)`, `texture_sigma = max(0.6, 4 * scale)`, computed only when `sharpen > 0` / `texture != 0`) and the curve texture update; the spots pass is Task 7 (until then, `prepare` ignores spots).
- `passes::develop::Uniforms` (`bytemuck::Pod`, std140-compatible layout): every uniform of `DEVELOP` with the same names and the same unit conversions as `developPass` (`/100` where the web divides; `uMatrix` as the web's column-major `mat3x3<f32>` padded for WGSL; `uHsl` as 8 `vec4` padded; `uBandHue`/`uBandWidth` as 8 padded floats, etc.).
- `develop.wgsl`: a line-for-line port of `DEVELOP` from `shaders.ts`, same constants, same order, `textureSampleLevel(.., 0.0)` for the 5×5 noise loop and the curve lookups, `textureSample` elsewhere; the background colour `(0.12, 0.12, 0.13)`; final `clamp` and alpha 1.

- [ ] **Step 1: Write the parity test**

```rust
use lf_fixtures::{Manifest, case_json, compare, load_png, root};
use lf_pipeline::{Gpu, Renderer};
use lf_settings::normalise;

fn render_all(filter: impl Fn(&str) -> bool) -> Vec<(String, lf_fixtures::Comparison)> {
    let gpu = Gpu::new().unwrap();
    let m = Manifest::load().unwrap();
    let mut out = vec![];
    for (name, _) in &m.sources {
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
    let bad: Vec<_> = results.iter().filter(|(_, c)| !c.size_ok || c.mean >= t.mean || c.max >= t.max).collect();
    assert!(bad.is_empty(), "out of tolerance:\n{}", bad.iter().map(|(n, c)| format!("{n}: mean {:.5} max {:.5} size_ok {}", c.mean, c.max, c.size_ok)).collect::<Vec<_>>().join("\n"));
}

#[test] fn identity_is_exact() { let r = render_all(|c| c == "identity"); assert_within(&r); assert!(r.iter().all(|(_, c)| c.max == 0.0), "identity must be bit-exact"); }
#[test] fn blur_dependent_cases() { assert_within(&render_all(|c| ["detail-max", "light-max", "light-min", "look-moody"].contains(&c))); }
#[test] fn every_case_except_spots() { assert_within(&render_all(|c| !c.starts_with("spots-"))); }
```

- [ ] **Step 2: Run `cargo test -p lf-pipeline --test parity identity_is_exact` and verify it fails (no `Renderer`).**

- [ ] **Step 3: Implement the curve texture, the develop pass and `Renderer`, then make `identity_is_exact` pass first.**

Identity exercises decode, upload, the 384 copy, both wide blurs (read but neutral), the curve LUT at texel centres (`c * 255/256 + 0.5/256`), and 8-bit output rounding. If identity is off by one anywhere, fix sampling or rounding before touching anything else: nothing downstream can pass otherwise.

- [ ] **Step 4: Make `blur_dependent_cases` pass, then `every_case_except_spots`.**

Report the four blur-dependent cases' mean/max in the report. If a case is within mean but over max only at the frame edge after a 45° straighten, say so with the pixel coordinates; the controller decides.

- [ ] **Step 5: Run the whole crate's tests, fmt, clippy; commit and push**

```bash
git add crates/lf-pipeline
git commit -m "lf-pipeline: develop pass with parity on 216 golden renders"
git push
```

---

### Task 7: `lf-pipeline`: the spots pass, full parity

**Files:**
- Create: `crates/lf-pipeline/src/shaders/spots.wgsl`, `crates/lf-pipeline/src/passes/spots.rs`
- Modify: `crates/lf-pipeline/src/renderer.rs` (`prepare` runs the spots pass into a mipmapped target when `spots` is non-empty, cached by the spots' JSON), `crates/lf-pipeline/tests/parity.rs`

**Interfaces:**
- `passes::spots::run(gpu, source: &Target, target: &Target, spots: &[Spot], width, height)`: uniforms `uSize`, `uCount`, `uSpot[48]` (target x, y, source x, y), `uSpotB[48]` (radius, mode); a line-for-line port of `SPOTS` with `textureSampleLevel(.., 0.0)` for every sample inside the loop (the web already uses `textureLod` there), regenerating the target's mips afterwards.

- [ ] **Step 1: Add the tests**

```rust
#[test] fn spots_cases() { assert_within(&render_all(|c| c.starts_with("spots-"))); }
#[test] fn all_228_renders_match() { let r = render_all(|_| true); assert_eq!(r.len(), 228); assert_within(&r); }
```

- [ ] **Step 2: Run to verify `spots_cases` fails, implement, run `cargo test -p lf-pipeline --test parity` to verify all pass.**

- [ ] **Step 3: Commit and push**

```bash
git add crates/lf-pipeline
git commit -m "lf-pipeline: spots pass; all 228 golden renders within tolerance"
git push
```

---

### Task 8: `lf-pipeline`: preview, histogram, pixels and a render CLI

**Files:**
- Modify: `crates/lf-pipeline/src/renderer.rs`
- Create: `crates/lf-pipeline/examples/lf-render.rs`
- Test: `crates/lf-pipeline/tests/preview.rs`

**Interfaces:**
- `Renderer::draw(&mut self, s: &EditSettings, opts: &DrawOptions { before: bool, crop: Option<Crop>, view: View { x, y, w, h } }, canvas: &Target)`: the web's `draw` (before = source, no geometry; crop override; zoom/pan view) into a caller-sized target.
- `Renderer::pixels(&mut self, s, edge: u32) -> Result<RgbaImage, PipelineError>` (the whole output at `edge` long edge) and `Renderer::histogram(&mut self, s) -> Histogram { r, g, b, l: [u32; 256] }` (from a 200 px render, luma `round(0.2126 R + 0.7152 G + 0.0722 B)`).
- Example binary: `cargo run -p lf-pipeline --example lf-render -- <in.png> <settings.json> <out.png>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn full_view_draw_equals_render() { /* draw with view (0,0,1,1) into a target of the output size == render(s) exactly */ }
#[test] fn before_ignores_settings_and_geometry() { /* draw(before=true) of a case with rotate 90 and exposure 2 equals the source image */ }
#[test] fn histogram_counts_every_pixel() { /* sum of r == sum of l == width*height of the 200 px render */ }
#[test] fn pixels_scales_to_the_long_edge() { /* pixels(s, 100) on a 512x341 source is 100x67 */ }
```

- [ ] **Step 2: Run to verify failure, implement, verify PASS; try the example on `tests/fixtures/sources/portrait.png` with `cases/look-golden.json` and open the output to eyeball it.**

- [ ] **Step 3: Commit and push**

```bash
git add crates/lf-pipeline
git commit -m "lf-pipeline: preview drawing, histogram, pixels and a render example"
git push
```

---

### Task 9: CI green on lavapipe, README

**Files:**
- Modify: `.github/workflows/ci.yml` if needed, `README.md`, `NOTICE`

- [ ] **Step 1: Push and watch CI** (`gh run watch`). The parity suite must pass on `ubuntu-latest` with the Mesa software Vulkan driver. If lavapipe's rounding differs from SwiftShader's beyond tolerance on specific cases, report the cases and numbers; do not loosen thresholds.
- [ ] **Step 2: README**: crate map, how to run the parity suite locally (`cargo test --workspace`), the fixture copy rule (verbatim from the monorepo, never regenerated here), and the env vars (`LF_ALLOW_FALLBACK_ADAPTER`, `WGPU_BACKEND`). `NOTICE`: list `wgpu`, `naga`, `image`, `serde` and the rest of `cargo tree` with their licences (all permissive).
- [ ] **Step 3: Commit and push**

```bash
git add README.md NOTICE .github
git commit -m "Docs: crate map, parity rule and CI notes"
git push
```
