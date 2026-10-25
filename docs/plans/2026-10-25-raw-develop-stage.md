# lensflow-ImageEditor, Part 2b: RAW Develop Stage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Open RAW files (and JPEG, PNG, TIFF, WebP) into a display-referred float frame with real highlight headroom, feed it into the Part 2a render chain on float textures, and keep the 228 golden renders within the parity gate.

**Architecture:** A new `lf-raw` crate decodes sensor data with `rawler`, demosaics on the CPU with rayon, applies camera white balance, the camera colour matrix to linear sRGB, highlight handling and a camera-like base tone curve, and gamma-encodes **without clamping** into an `Rgba16Float` frame. `lf-pipeline` gains format-parametrised targets and pipelines (`Rgba16Float` intermediates, `Rgba8Unorm` output), `Renderer::from_texture`, and format-aware readback, so the shared chain runs on float with values above 1.0 surviving until the chain's own clamp. A small disk cache stores developed base frames. The `lf-render` example opens any supported file.

**Tech Stack:** Rust stable; `rawler` 0.8 (LGPL-2.1, statically linked, allowed because the source is public), `image` 0.25 (JPEG, PNG, TIFF, WebP), `kamadak-exif` 0.6, `half` 2 (f16), `rayon`, `lz4_flex` (pure-Rust cache compression), `sha2`, `wgpu` 30. No libraw in this part (see Rulings). No GPL.

**Spec:** `D:\pet-projects\photo-studio-management\docs\superpowers\specs\2026-10-25-lensflow-image-editor-design.md` (Decisions → RAW headroom without breaking parity; Architecture → RAW decoding; Constraints → Licensing). Part 2a's code and ledger: `D:\pet-projects\lensflow-image-editor` and `.superpowers/sdd/2026-10-25-core-pipeline-parity/progress.md` (final-review readiness notes).

## Global Constraints

- Repository `D:\pet-projects\lensflow-image-editor`, branch `main`; each task works on `wip/2b-task-N`, lands as one squash commit on `main` when green locally and in CI, then deletes the wip branch; no force-pushes.
- `export PATH="$HOME/.cargo/bin:$PATH"` before every cargo command. Local GPU runs use WARP, CI uses lavapipe.
- Fmt, clippy `-D warnings`, `cargo test --workspace` green before every landing. Commit trailer: `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Parity gate (unchanged): per render, per channel on 0..1, mean < 1/255, 99.9th percentile < 4/255, max <= 16/255; `identity` bit-exact; spot regions max < 4/255. After the float-texture change the 228 renders must still pass; report the worst figures before and after.
- Headroom rule: the develop stage output is sRGB-gamma-encoded (`pow(x, 1/2.2)`) and **never clamped**; a sensor value above white after white balance must arrive at the chain above 1.0. The chain's own clamps (after clarity, and at output) are the only clamps.
- Working space: linear sRGB (D65) between the camera matrix and the base curve. Camera matrices come from the file (rawler's `xyz_to_cam` / colour matrix) with Bradford adaptation to D65 when the matrix is for another illuminant; fall back to an identity matrix with a warning when the file has none.
- Licences: everything permissive except `rawler` (LGPL-2.1, allowed). HEIC is not decoded (no permissive decoder; see Rulings).
- Sample RAW files are never committed. A script downloads CC0 samples from raw.pixls.us into a git-ignored folder with pinned SHA-256; tests that need them skip (with a visible "skipped: no sample" message) when the folder is absent; CI downloads and caches them.
- Settings: the develop stage's own controls live under `desktop.develop` in `EditSettings` (`{ "profile": "camera" | "linear", "highlights": "clip" | "reconstruct" }`, defaults `camera`, `reconstruct`). Unknown or missing values fall back to defaults. The web keeps the object opaque.

## Review Focus

1. A RAW whose sensor saturates (white sky): after white balance and the matrix the pixel must be above 1.0 at the chain input, and `highlights: -100` on the web chain must pull it back below 1.0 in the output. Pinned in Task 4 (synthetic over-exposed patch) and Task 8 (end-to-end through the chain).
2. A JPEG with EXIF orientation 6 (rotated phone photo) must open upright with width and height swapped; the same photo opened via the web shows upright too. Pinned in Task 5.
3. A file that is not an image, or a RAW from an unsupported camera, must give a plain error naming the file, never a panic or a half-decoded frame. Pinned in Task 2 and Task 5.
4. A 60-megapixel RAW must not exceed the GPU texture limit silently: the develop stage downsizes to the device's `max_edge` with a `reduced` flag the app can show. Pinned in Task 4.
5. The disk cache must never serve a stale frame after the develop settings or the file change. Pinned in Task 7 (key includes file hash, size, mtime and the develop settings JSON).

---

### Task 1: Float textures in the pipeline

**Files:**
- Modify: `crates/lf-pipeline/src/texture.rs` (`Target.format`, `Source::upload_f16`, format-aware `read_back`), `crates/lf-pipeline/src/passes/mod.rs` (`Pipelines` keyed by colour target format), `crates/lf-pipeline/src/renderer.rs` (`Renderer::from_texture`, intermediates in `Rgba16Float`, output in `Rgba8Unorm`), `crates/lf-pipeline/src/gpu.rs` (required features none; `Rgba16Float` is filterable in core)
- Test: `crates/lf-pipeline/tests/parity.rs` (unchanged gate), `crates/lf-pipeline/tests/float.rs`

**Interfaces:**
- `Target { format: wgpu::TextureFormat, .. }`; `Target::new_with(gpu, w, h, mips, format)`; `Target::new` keeps `Rgba8Unorm`.
- `Source::upload(gpu, &RgbaImage, mips)` keeps 8-bit upload but the renderer converts it to `Rgba16Float` by a copy pass (so JPEG input takes the same float path as RAW).
- `Source::upload_f16(gpu, &FloatFrame, mips) -> Result<Target, PipelineError>` where `FloatFrame { width, height, data: Vec<half::f16> /* RGBA */ }` (new in lf-pipeline, re-exported).
- `Renderer::from_texture(gpu, source: Target, full_width, full_height) -> Result<Renderer, PipelineError>`: the source must be `Rgba16Float` with mips; everything else as `new`.
- All intermediates (`small`, `tmp_small`, `blur_m`, `blur_l`, `tmp`, `sharp`, `texture`, `spots`) are `Rgba16Float`; the develop pass renders to `Rgba8Unorm` for `render`/`pixels`/`histogram`, and to whatever `draw`'s target format is (`Rgba8Unorm` or `Bgra8Unorm`), so `Pipelines` builds the develop pipeline per output format on demand and caches it.
- `read_back(gpu, &Target)` returns `RgbaImage` for 8-bit targets; `read_back_f16(gpu, &Target) -> FloatFrame` for float targets.

- [ ] **Step 1: Write the failing tests** (`tests/float.rs`)

```rust
#[test] fn an_f16_source_above_one_survives_to_the_chain_input() {
    // a 4x4 frame with one pixel at (2.0, 2.0, 2.0); draw with before=true into an f16 target; read back f16: that pixel is still 2.0
}
#[test] fn eight_bit_input_renders_identically_through_the_float_path() {
    // identity case on portrait: render() via Renderer::new equals the golden bit-exactly (identity_is_exact already asserts this; this test asserts Renderer::new and Renderer::from_texture(upload_f16(converted)) give identical bytes)
}
#[test] fn draw_into_bgra_target_works() { /* draw look-golden into a Bgra8Unorm target; read back and swizzle; equals the Rgba8Unorm draw */ }
```

- [ ] **Step 2: Run to verify failure, implement, then run the full parity suite.** All 228 within the gate; identity still exact; record the worst mean / p999 / max before (0.00010 / 0.00392 / 0.0549 on WARP) and after. If a blur-dependent case drifts out of the gate, do not loosen it: report the case and numbers.

- [ ] **Step 3: fmt, clippy, land on main, watch CI. Commit: "lf-pipeline: float intermediates, from_texture and format-aware readback".**

---

### Task 2: `lf-raw`: decoding sensor data with rawler

**Files:**
- Create: `crates/lf-raw/Cargo.toml`, `crates/lf-raw/src/lib.rs`, `crates/lf-raw/src/decode.rs`, `crates/lf-raw/src/error.rs`, `crates/lf-raw/tests/dng.rs`, `crates/lf-raw/tests/support/synthetic_dng.rs`
- Modify: `Cargo.toml` (workspace member), `NOTICE` (rawler, LGPL-2.1)

**Interfaces:**
- `RawError` (`thiserror`): `Unsupported { path, reason }`, `Io { path, source }`, `Corrupt { path, reason }`, `NotAnImage { path }`; `Display` messages in plain words naming the file.
- `RawFrame { width: u32, height: u32, cfa: Cfa /* Bayer { pattern: [[Color; 2]; 2] } | XTrans { pattern: [[Color; 6]; 6] } | Rgb (already demosaiced, e.g. linear DNG) */, data: Vec<u16>, black: [f32; 4], white: f32, wb: [f32; 4] /* as-shot multipliers, G normalised to 1 */, xyz_to_cam: Option<[[f32; 3]; 3]>, crop: Rect, orientation: Orientation /* 1..8 EXIF */, make: String, model: String }`.
- `decode::decode_raw(path: &Path) -> Result<RawFrame, RawError>` using `rawler::decode_file`; maps rawler's `RawImageData`, `cfa`, `blacklevel`, `whitelevel`, `wb_coeffs`, `color_matrix` and `crop_area` onto `RawFrame`.
- Test support: `synthetic_dng::write(path, image: &Rgb32FImage, pattern: RGGB, black: 64, white: 4095, wb: [2.0, 1.0, 1.5], matrix: identity)` writes a minimal valid DNG (TIFF with the DNG tags rawler needs: `DNGVersion`, `CFAPattern`, `CFARepeatPatternDim`, `BlackLevel`, `WhiteLevel`, `AsShotNeutral`, `ColorMatrix1`, `CalibrationIlluminant1`, `PhotometricInterpretation = 32803`, 16-bit uncompressed strips) by mosaicking the image in linear light (undoing WB so that decode+WB restores it).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn decodes_a_synthetic_dng() {
    let img = gradient_rgb(64, 48);
    synthetic_dng::write(&path, &img, ...);
    let raw = decode_raw(&path).unwrap();
    assert_eq!((raw.width, raw.height), (64, 48));
    assert!(matches!(raw.cfa, Cfa::Bayer { .. }));
    assert_eq!(raw.white, 4095.0);
    assert!((raw.wb[0] - 2.0).abs() < 1e-3 && (raw.wb[2] - 1.5).abs() < 1e-3);
    // the raw value at a green site equals the image's green scaled to the sensor range
}
#[test] fn a_non_image_is_refused_with_its_name() { let e = decode_raw(Path::new("Cargo.toml")).unwrap_err(); assert!(e.to_string().contains("Cargo.toml")); }
#[test] fn a_missing_file_is_an_io_error_naming_the_path() { ... }
```

- [ ] **Step 2: Run to verify failure, implement, verify PASS. Land: "lf-raw: decode sensor data with rawler".**

---

### Task 3: Demosaic

**Files:**
- Create: `crates/lf-raw/src/demosaic/mod.rs`, `bilinear.rs`, `ahd.rs`, `xtrans.rs`
- Test: `crates/lf-raw/tests/demosaic.rs`

**Interfaces:**
- `demosaic::Quality { Fast /* bilinear */, Best /* AHD for Bayer; 3-colour interpolation for X-Trans in this part */ }`.
- `demosaic::run(raw: &RawFrame, quality) -> LinearRgb { width, height, data: Vec<f32> /* RGB, camera space, black-subtracted and normalised 0..1 by white */ }` on rayon; `Rgb` frames pass through.
- AHD implemented from the paper (Hirakawa & Parks 2005), not from dcraw/darktable code (GPL). Document the source in the module header.

- [ ] **Step 1: Write the failing tests**: a smooth synthetic DNG (gradients) demosaiced with `Fast` and `Best` reconstructs the source with PSNR > 40 dB (Fast) and > 45 dB (Best) away from a 4-pixel border; a sharp edge image demosaiced with `Best` has fewer zipper artefacts than `Fast` (measure mean absolute error on the edge columns: Best < Fast); timing sanity (a 24 MP synthetic frame demosaics with `Best` in under 10 s on this machine, printed not asserted).
- [ ] **Step 2: Run to verify failure, implement, verify PASS. Land: "lf-raw: bilinear and AHD demosaic".**

---

### Task 4: Colour, highlights, base curve: the developed frame

**Files:**
- Create: `crates/lf-raw/src/develop.rs`, `crates/lf-raw/src/color.rs` (matrices, Bradford), `crates/lf-raw/src/curve.rs` (base curve)
- Modify: `crates/lf-settings/src/settings.rs` (`DevelopSettings` typed accessor over `desktop.develop`)
- Test: `crates/lf-raw/tests/develop.rs`, `crates/lf-settings/tests/develop_settings.rs`

**Interfaces:**
- `lf_settings::DevelopSettings { profile: Profile /* Camera | Linear */, highlights: Highlights /* Clip | Reconstruct */ }` with `DevelopSettings::from_settings(&EditSettings) -> DevelopSettings` (reads `desktop.develop`, defaults on anything missing or unknown) and `DevelopSettings::write_into(&self, &mut EditSettings)` (sets `desktop.develop`, creating the object; removes it again when both are defaults so a default export carries no `desktop` key).
- `develop::develop(raw: &RawFrame, quality, settings: &DevelopSettings, max_edge: u32) -> Result<Developed, RawError>` → `Developed { frame: lf_pipeline::FloatFrame /* RGBA f16, gamma-encoded, unclamped */, full_width, full_height, reduced: bool, orientation_applied: bool }`. Steps, in order: demosaic → white balance (multiply by `wb`, G = 1) → `cam_to_srgb` (inverse of `xyz_to_cam` composed with XYZ→linear sRGB D65, Bradford-adapted from the matrix's illuminant; identity when absent) → highlights (`Clip`: clamp each channel at 1.0; `Reconstruct`: for pixels with any channel > 1.0, keep luminance and pull chroma toward neutral in proportion to how far the max channel exceeds 1.0, so clipped skies go white not magenta; values above 1.0 in luminance **remain** above 1.0) → base curve (`Camera`: a fixed S-curve with 0.18 → 0.46 after gamma, toe below 0.02, shoulder from 0.8 that reaches 1.0 at linear 1.0 and continues linearly with its end slope above 1.0; `Linear`: identity) → gamma encode `pow(max(x, 0), 1/2.2)` with no clamp → orientation applied (rotate/flip the buffer per EXIF 1..8) → downscale by an integer box factor when the long edge exceeds `max_edge` (`reduced = true`) → f16 RGBA.
- `develop::develop_rgb8(img: &RgbaImage, orientation) -> Developed` for JPEG/PNG/TIFF/WebP: orientation applied, 8-bit → f16 of the same gamma values (no curve, no matrix), so an 8-bit file renders identically to Part 2a.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_known_srgb_picture_round_trips_through_a_synthetic_dng() {
    // Linear profile, Clip highlights, identity matrix: develop(decode(synthetic_dng(img))) ≈ img within 1.5/255 mean, away from the border
}
#[test] fn an_overexposed_patch_stays_above_one() {
    // a patch at linear 3.0 (sensor clipped at white) with wb G=1: after develop (Reconstruct), the f16 value at the patch is > 1.0 in all channels and the patch is neutral (|r-g|,|g-b| < 0.02)
}
#[test] fn clip_mode_clamps_each_channel() { /* same patch with Clip: all channels <= 1.0 */ }
#[test] fn the_camera_curve_maps_mid_grey_to_sRGB_mid() { /* linear 0.18 → 0.46 ± 0.01 after gamma; monotone; continuous at 1.0 */ }
#[test] fn large_frames_are_reduced_to_max_edge() { /* a 9000x6000 synthetic (generated cheaply) with max_edge 4096 → reduced = true, long edge <= 4096, full_width still 9000 */ }
#[test] fn develop_settings_round_trip_and_default_to_no_desktop_key() { ... }
```

- [ ] **Step 2: Run to verify failure, implement, verify PASS. Land: "lf-raw: white balance, colour matrix, highlights and base curve".**

---

### Task 5: Opening any supported file; EXIF orientation; HEIC refused clearly

**Files:**
- Create: `crates/lf-raw/src/open.rs`, `crates/lf-raw/src/exif.rs`
- Test: `crates/lf-raw/tests/open.rs`

**Interfaces:**
- `open::Kind { Raw, Jpeg, Png, Tiff, WebP, Heic, Unknown }` from magic bytes (not extension) via `open::sniff(path) -> Result<Kind, RawError>`.
- `open::open(path, quality, settings: &DevelopSettings, max_edge) -> Result<Opened, RawError>` → `Opened { developed: Developed, kind, exif: ExifSummary { make, model, lens, iso, exposure_time, f_number, focal_length, captured_at, orientation } , jpeg_exif_bytes: Option<Vec<u8>> /* for re-embedding on export, as the web does */ }`.
- RAW extensions handled by rawler: `.dng .cr2 .cr3 .nef .nrw .arw .srf .sr2 .raf .orf .rw2 .pef .srw .3fr .iiq .x3f` (whatever rawler 0.8 supports; unsupported → `Unsupported` with the camera name when known).
- HEIC: `Kind::Heic` → `RawError::Unsupported { reason: "HEIC files aren’t supported yet." }`.

- [ ] **Step 1: Write the failing tests**: a JPEG written with EXIF orientation 6 opens with swapped dimensions and the expected pixel at the top-left; a PNG opens with `kind == Png` and `developed.frame` equal to the 8-bit values; a text file is `NotAnImage`; a file with HEIC magic (`ftypheic`) is `Unsupported` with the exact message; `sniff` on a synthetic DNG is `Raw`.
- [ ] **Step 2: Run to verify failure, implement, verify PASS. Land: "lf-raw: open any supported file with EXIF orientation".**

---

### Task 6: Real-camera sample corpus and smoke tests

**Files:**
- Create: `scripts/fetch-raw-samples.sh` (and `.ps1`), `tests/raw-samples/README.md`, `tests/raw-samples/manifest.json` (url, sha256, camera, licence CC0, bytes), `crates/lf-raw/tests/samples.rs`
- Modify: `.gitignore` (`tests/raw-samples/files/`), `.github/workflows/ci.yml` (download step with `actions/cache` keyed on the manifest hash)

**Interfaces:**
- Samples (one each, smallest available CC0 files on raw.pixls.us): Canon CR2, Canon CR3, Nikon NEF, Sony ARW, Fujifilm RAF (X-Trans), Olympus ORF, Panasonic RW2, Pentax PEF, a DNG. The manifest pins the SHA-256 of each; the script verifies before use. The builder picks the specific files and records camera names.
- Tests: for each sample present, `open()` succeeds, dimensions are plausible (> 1000 px), the developed frame's mean luminance is between 0.05 and 0.95, and no channel is NaN; `Quality::Fast` and `Best` both run. Skipped (with a printed note) when the file is absent.

- [ ] **Step 1: Write the tests (they skip locally until the script has run), run the script, run the tests, fix decode gaps found (report any camera rawler cannot decode).**
- [ ] **Step 2: Land: "lf-raw: CC0 sample corpus and smoke tests"; CI downloads and caches the corpus.**

---

### Task 7: Developed-frame disk cache

**Files:**
- Create: `crates/lf-raw/src/cache.rs`
- Test: `crates/lf-raw/tests/cache.rs`

**Interfaces:**
- `cache::FrameCache::open(dir: &Path, cap_bytes: u64) -> FrameCache`; `get(&self, key: &CacheKey) -> Option<Developed>`; `put(&self, key, &Developed)`; `trim(&self)` (LRU by last access, under `cap_bytes`).
- `CacheKey::for_file(path, settings: &DevelopSettings, quality, max_edge) -> Result<CacheKey, RawError>` = SHA-256 of the file bytes plus size, mtime, the develop settings JSON, quality and `max_edge`.
- Storage: one file per entry, `lz4_flex`-compressed f16 RGBA plus a small header (magic, version, dims, flags); corrupt or version-mismatched entries are ignored and deleted.

- [ ] **Step 1: Write the failing tests**: put/get round trip is exact; a changed develop setting misses; a touched file (new mtime) misses; trimming to a small cap evicts the least recently used; a corrupt entry is ignored and removed.
- [ ] **Step 2: Run to verify failure, implement, verify PASS. Land: "lf-raw: developed-frame disk cache".**

---

### Task 8: End to end: `lf-render` opens RAW, headroom through the chain, docs

**Files:**
- Modify: `crates/lf-pipeline/examples/lf-render.rs` (use `lf_raw::open`, `Renderer::from_texture`), `README.md`, `NOTICE`
- Test: `crates/lf-pipeline/tests/raw_chain.rs`

**Interfaces:**
- `lf-render <in.(raw|jpg|png|tif|webp)> <settings.json> <out.png> [--develop camera|linear] [--highlights clip|reconstruct]`.

- [ ] **Step 1: Write the failing test** `headroom_is_recoverable_by_the_chain`: a synthetic DNG with a patch at linear 3.0 → `open` → `Renderer::from_texture` → render with defaults: the patch is 255 (clipped at output); render with `highlights: -100`: the patch is below 255 and still neutral; render a JPEG of the same scene (patch at 255) with `highlights: -100`: the patch stays at or near 255 (the JPEG has no headroom), which is the whole point of the RAW path.
- [ ] **Step 2: Implement, verify PASS, run the example on a corpus file and a JPEG, update README (new crate, supported formats, HEIC not yet, cache location note, the headroom rule) and NOTICE (rawler LGPL-2.1, kamadak-exif, half, lz4_flex, rayon). Land: "lf-raw: RAW through the chain, lf-render opens any file, docs". Watch CI.**

## Rulings made in planning

- **No libraw in 2b.** Statically building libraw on three OSes is a packaging task; `rawler` 0.8 covers the mainstream bodies. The corpus smoke test (Task 6) shows the gaps; libraw returns in Part 2d if a mainstream format is missing.
- **HEIC refused, not previewed.** Apple HEIC thumbnails are HEVC too, so "open from the embedded preview" cannot work without an HEVC decoder (LGPL-3 + patents). Clear error for now; revisit with licensing.
- **AHD from the paper, X-Trans simple.** AMaZE and RCD are GPL/LGPL implementations; AHD implemented from the paper is clean. X-Trans gets a correct but simple interpolation in this part; Markesteijn-class quality is a later improvement.
- **Float intermediates may drift sub-1/255 on blur-dependent cases**; the gate, not bit-identity, is the contract (spec PR "Docs: desktop parity gate").
