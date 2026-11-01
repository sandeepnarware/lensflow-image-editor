# Lightroom Presets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Import Lightroom presets (`.xmp`, `.lrtemplate`, folders, `.zip`, Adobe's folders) as studio presets in one step, and refine a preset from one Lightroom example by fitting a colour table, vignette and grain.

**Architecture:** Parsing and mapping are pure functions in a new `lf_settings::lightroom` module; the fit is pure CPU maths in a new `lf_grade::example` module with the render injected; the app adds an import job, a summary dialog, a "Match a Lightroom example" job and dialog, a Lightroom-style apply for imported presets, and embedded tables materialised into the catalogue's LUT folder. No shader change.

**Tech Stack:** Rust 2024, `roxmltree` 0.21 (XMP), `zip` 9 (packs), existing `image`, `lf_merge::align`, `lf_pipeline::lut`, `lf_catalog::presets`, egui.

**Spec:** `docs/superpowers/specs/2026-11-01-lightroom-presets-design.md`

## Global Constraints

- Settings JSON enters only through `lf_settings::normalise`; `desktop` must stay ≤ `MAX_DESKTOP_JSON` (65536 bytes) or it is dropped whole.
- No change to `develop.wgsl` maths, `tests/fixtures`, or the 8-bit parity path; `cargo test --workspace` parity suite stays green.
- Never write to the user's preset files or Adobe folders; import only reads.
- Slow work (reading a collection, rendering, fitting) runs on `Lane::Long`, with progress and cancel.
- UI never says "exact". Wording: "imported", "matched (difference: ΔE …)".
- User-facing strings are `pub const`s; any non-ASCII literal must pass `crates/lf-app/tests/glyphs.rs`.
- Imported presets are named `"<Group> - <Name>"` (no group: `"<Name>"`); a taken name gets ` (2)`, ` (3)`, ….
- Studio preset limit: 2,000 per studio (server `PhotoEditPreset::MAX_PER_STUDIO`); import stops at the limit and reports the rest.
- Embedded table: 17³ RGB, 16-bit, base64, in `desktop.lut.table` with `desktop.lut.hash`; materialised as `<root>/.lensflow/luts/lr-<hash>.cube`.
- Fit target on the example at working size: mean ΔE2000 < 2, p95 < 5 (reported, not enforced).
- zip caps: 50 MB total uncompressed, 5,000 entries; each preset file ≤ 1 MB.
- New deps Apache-2.0 compatible; add to `NOTICE` if it lists crates.
- Commit straight to `main` after each task, tests green (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`).

## Review Focus

1. Preset names in scripts egui's fonts lack (Japanese, Cyrillic packs) must not panic the app: names are shown through a glyph-safe filter (Task 6 test).
2. XMP files with a UTF-8 BOM, UTF-16 encoding, or `crs:` written as elements instead of attributes must read the same (Task 2 tests).
3. A Lightroom export that was cropped, rotated or lens-corrected must give a large, honestly reported difference, never a garbage table presented as good (Task 9 test: mismatched pair → `aligned == false` or ΔE mean > 10).
4. An export in Adobe RGB / ProPhoto (ICC tagged) must be converted to sRGB or refused with a message, never fitted as if sRGB (Task 10 test).
5. Importing the same pack twice must not overwrite the first import's presets silently: names get ` (2)` (Task 7 test).

---

### Task 1: Lightroom-style apply for imported presets

**Files:**
- Modify: `crates/lf-settings/src/looks.rs` (`apply_look`)
- Test: `crates/lf-settings/tests/desktop_groups.rs` (append)

**Interfaces:**
- Produces: `pub const LIGHTROOM_KEY: &str = "lightroom";` in `lf_settings::lightroom` (module created here as `crates/lf-settings/src/lightroom/mod.rs`, `pub mod lightroom;` in `lib.rs`) and `pub fn is_lightroom(preset: &serde_json::Value) -> bool` (true when `preset.desktop.lightroom` exists).

- [ ] **Step 1: Write the failing test** `an_imported_preset_changes_only_what_it_carries`: onto has exposure 1.0, contrast 30, temperature 10, crop.w 0.5; preset `{"v":2,"contrast":20,"desktop":{"lightroom":{"name":"x"}}}`. Assert `apply_look` gives exposure 1.0, contrast 20, temperature 10, crop.w 0.5, and that `desktop.lightroom` is NOT copied onto the photo (`next.desktop` has no `lightroom`). Also assert a preset carrying `crop`/`spots` with `desktop.lightroom` leaves the photo's crop and spots.
- [ ] **Step 2:** `cargo test -p lf-settings --test desktop_groups` → FAIL (exposure reset to 0).
- [ ] **Step 3:** In `apply_look`, when `is_lightroom(look)`: `apply_groups(onto, look, &groups_in(look) minus Geometry and Spots)`; otherwise today's path. `lightroom` is not in any group, so it never reaches the photo.
- [ ] **Step 4:** Test passes; `cargo test -p lf-settings -p lf-app` green.
- [ ] **Step 5:** Commit "Presets: imported Lightroom presets change only what they carry".

### Task 2: Read `.xmp` and `.lrtemplate`

**Files:**
- Create: `crates/lf-settings/src/lightroom/read.rs`
- Modify: `crates/lf-settings/Cargo.toml` (add `roxmltree = "0.21"`; move to `[workspace.dependencies]` and use `.workspace = true` in both `lf-app` and `lf-settings`)
- Test: `crates/lf-settings/tests/lightroom_read.rs`, fixtures in `crates/lf-settings/tests/lightroom/` (hand-written: `attrs.xmp`, `elements.xmp`, `bom-utf16.xmp`, `profile.xmp`, `legacy.lrtemplate`, `broken.xmp`)

**Interfaces:**
- Produces:
  ```rust
  pub enum LrKind { Preset, Profile }
  pub struct LrPreset { pub name: String, pub group: Option<String>, pub kind: LrKind,
                        pub process_version: Option<String>,
                        pub settings: serde_json::Map<String, serde_json::Value> }
  #[derive(thiserror::Error)] pub enum ReadError { #[error("...")] NotXml, #[error("...")] NotAPreset, #[error("...")] TooLarge }
  pub fn read_xmp(bytes: &[u8]) -> Result<LrPreset, ReadError>;
  pub fn read_lrtemplate(text: &str) -> Result<LrPreset, ReadError>;
  pub fn read_any(file_name: &str, bytes: &[u8]) -> Result<LrPreset, ReadError>; // by extension
  ```
  `settings` keys are the `crs:` local names without prefix (`"Exposure2012"`). Numbers → JSON numbers (`"+0.50"` → 0.5), `"True"/"False"` → bool, `rdf:Seq` → array of strings, `rdf:Alt` → its `x-default` string, nested structures (`MaskGroupBasedCorrections`, `Look`) → nested JSON objects/arrays mirroring the RDF (`rdf:li` → array items, child elements and attributes → object keys).

- [ ] **Step 1: Write failing tests:** `attrs_and_elements_read_the_same` (both fixtures give `Exposure2012 == 0.5`, `Contrast2012 == 20`, `ToneCurvePV2012 == ["0, 0","128, 140","255, 255"]`, name "Warm Fade", group "Studio"); `bom_and_utf16_files_read` ; `a_profile_is_a_profile` (`crs:PresetType="Look"` or `crs:ProfileName` → `LrKind::Profile`); `lrtemplate_reads_its_settings` (`title = ZSTR "$$$/x=Moody"` → name "Moody", `Exposure2012 = -0.3`, `ConvertToGrayscale = true`, nested `ToneCurvePV2012 = { 0, 0, 255, 255 }` → `["0, 0","255, 255"]`); `broken_and_huge_files_are_refused` (> 1 MB → `TooLarge`, junk → `NotXml`, XMP without `crs:` → `NotAPreset`).
- [ ] **Step 2:** Run → FAIL (module missing).
- [ ] **Step 3:** Implement. XMP: decode BOM/UTF-16 to `String`, `roxmltree::Document::parse`, find the `rdf:Description` carrying the `http://ns.adobe.com/camera-raw-settings/1.0/` namespace. `.lrtemplate`: a small recursive-descent reader over `{ key = value, ... }` with strings (`"..."`, `ZSTR "..."` keeping the text after `=` in `$$$/...=`), numbers, `true/false`, nested tables, array tables; take `s.value.settings`, name from `s.title`, group from the parent folder name is the caller's job (not here).
- [ ] **Step 4:** Tests pass.
- [ ] **Step 5:** Commit "Lightroom presets: read .xmp and .lrtemplate".

### Task 3: Map Lightroom settings to ours

**Files:**
- Create: `crates/lf-settings/src/lightroom/map.rs`
- Test: `crates/lf-settings/tests/lightroom_map.rs`

**Interfaces:**
- Consumes: `LrPreset` (Task 2).
- Produces:
  ```rust
  pub struct Mapped { pub preset: serde_json::Value, pub notes: Vec<String> }
  pub fn map(p: &LrPreset) -> Mapped;
  ```
  `preset` is partial settings JSON (`"v": 2`), carrying only groups Lightroom set, plus `desktop.lightroom = {"name","group","processVersion","settings":{...all crs keys...},"auto":{"wb":bool,"tone":bool}}`. `notes` are user-facing sentences (constants or `format!` of constants), one per skipped/approximated key family.

Mapping (spec table is authoritative; values copied where ranges agree): `Exposure2012`→exposure; `Contrast2012/Highlights2012/Shadows2012/Whites2012/Blacks2012`→same; `IncrementalTemperature/IncrementalTint`→temperature/tint ×1.0 (tint clamps to ±100); `Temperature/Tint` with `WhiteBalance="Custom"`→ relative estimate: `temperature = clamp((K − 5500) / 50, ±100)`, `tint = clamp(Tint × 100/150, ±100)`, note "white balance in Kelvin approximated"; `WhiteBalance="As Shot"`→ no Color WB keys; `WhiteBalance="Auto"`→ `auto.wb = true`; `AutoTone=true` (or `AutoExposure` etc.) → `auto.tone = true`; `Texture/Clarity2012/Dehaze`, `Vibrance/Saturation` same; `{Hue,Saturation,Luminance}Adjustment{Red,Orange,Yellow,Green,Aqua,Blue,Purple,Magenta}`→`hsl.<band>.{h,s,l}`; `ToneCurvePV2012[Red|Green|Blue]` (`"x, y"` 0–255)→`curve.{rgb,r,g,b}` as `[x/255, y/255]`; parametric (`ParametricShadows/Darks/Lights/Highlights`, `ParametricShadowSplit/MidtoneSplit/HighlightSplit`) → sampled into the rgb curve at 9 points by Lightroom's documented region shape (each region raises/lowers by value/100 × 0.25 within its split range, smoothstep weights), composed with the point curve; `ColorGrade{Shadow,Midtone,Highlight}{Hue,Sat}`→`grading.{shadows,midtones,highlights}.{hue,sat}`, `ColorGradeBalance`→`balance`, `*Lum`/`Global*`/`Blending` → note; legacy `SplitToning{Shadow,Highlight}{Hue,Saturation}`,`SplitToningBalance`→ same grading fields when no `ColorGrade*`; `Sharpness/SharpenRadius`→`sharpen/sharpenRadius`; `LuminanceSmoothing/ColorNoiseReduction`→`noise/colorNoise`; `PostCropVignetteAmount/Midpoint`→`vignette/vignetteMidpoint`; `GrainAmount/GrainSize`→`grain/grainSize`; `ConvertToGrayscale`→`bw`; `LensProfileEnable`,`AutoLateralCA`,`VignetteAmount`,`Defringe*`,`Perspective{Vertical,Horizontal,Rotate,Scale}`,`UprightVersion/UprightTransformN` → `desktop.lens`/`desktop.defringe` using the field names those modules read (`lf_settings::lens::Lens`, `lf_settings::defringe`; check their serde names); `RedHue…BlueSaturation`,`ShadowTint`→`desktop.calibration` (`lf_settings::calibration::Calibration` fields); `PointColors` → `desktop.pointColour` (first four). Crop/`CropAngle`/`RetouchInfo`/positioned masks → not carried, note. `CameraProfile`/`Look` name → note "uses profile X". Every remaining unmapped key → one note listing them.

- [ ] **Step 1: Failing tests:** `light_and_colour_map_value_for_value`; `absent_groups_are_not_carried` (only `Exposure2012` set → `groups_in == [Light]`, contrast present as 0 per spec "unset keys of a carried group take Lightroom's defaults"); `as_shot_wb_is_left_out_and_auto_is_flagged`; `tone_curve_points_scale_to_0_1`; `parametric_curve_lifts_shadows` (ParametricShadows 50 → curve at x=0.15 above 0.15); `split_toning_becomes_grading`; `calibration_and_lens_land_in_desktop` (round-trip through `normalise` and the modules' `from_settings`); `every_crs_key_is_kept_and_unmapped_ones_are_noted`; `a_crop_is_never_carried`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `map`.
- [ ] **Step 4:** Pass.
- [ ] **Step 5:** Commit "Lightroom presets: map settings".

### Task 4: AI masks and the size budget

**Files:**
- Modify: `crates/lf-settings/src/lightroom/map.rs`
- Create: `crates/lf-settings/src/lightroom/budget.rs`
- Test: `crates/lf-settings/tests/lightroom_map.rs` (append)

**Interfaces:**
- Consumes: `lf_settings::adaptive::{AdaptiveMask, write_into_preset}`, `lf_settings::masks::Local`.
- Produces: `pub fn fit_budget(preset: &mut serde_json::Value) -> Option<&'static str>` — returns `Some(BUDGET_NOTE)` when it had to drop `desktop.lightroom.settings`; `pub const BUDGET_NOTE: &str`.

`MaskGroupBasedCorrections` entries whose `CorrectionMasks` are AI masks (`MaskSubType` subject=1, sky=2, background = inverted subject, people = `MaskSubCategoryID` person) become `AdaptiveMask { what: "subject"|"sky"|"people", invert, opacity: 100, local }` with `LocalExposure2012`, `LocalContrast2012`, `LocalHighlights2012`, `LocalShadows2012`, `LocalWhites2012`, `LocalBlacks2012`, `LocalClarity2012`, `LocalTexture`, `LocalDehaze`, `LocalTemperature`, `LocalTint`, `LocalSaturation` → `Local` fields of the same meaning (check `masks::Local`'s field names and ranges; Lightroom's local values are −1..1 → ×100, exposure −4..4 kept). Other mask types → note. `desktop.lightroom.settings.MaskGroupBasedCorrections` is replaced by `"(omitted)"` (no geometry kept).

- [ ] **Step 1: Failing tests:** `a_sky_mask_becomes_an_adaptive_sky_mask` (LocalExposure2012 −0.5 → local exposure −0.5); `a_brushed_mask_is_noted_not_carried`; `the_budget_keeps_desktop_under_the_cap` (a preset with a 40 KB fake `desktop.lut.table` and a 40 KB `desktop.lightroom.settings` → after `fit_budget`, `serde_json::to_vec(desktop).len() <= MAX_DESKTOP_JSON`, table kept, note returned, and `normalise` keeps `desktop`).
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass.
- [ ] **Step 5:** Commit "Lightroom presets: AI masks and the size budget".

### Task 5: Gather preset files

**Files:**
- Create: `crates/lf-app/src/lightroom/gather.rs` (module `crates/lf-app/src/lightroom/mod.rs`, `mod lightroom;` in `lib.rs`)
- Modify: `crates/lf-app/Cargo.toml` (add `zip` — same version/features as `lf-ai`, moved to workspace deps)
- Test: unit tests in `gather.rs` using temp dirs (`tempfile` if already a dev-dep, else `std::env::temp_dir()` + unique names as other tests do)

**Interfaces:**
- Produces:
  ```rust
  pub struct Found { pub source: String /* shown: "pack.zip / Moody.xmp" */, pub group_hint: Option<String> /* parent folder name */, pub bytes: Vec<u8>, pub file_name: String }
  pub struct Gathered { pub found: Vec<Found>, pub failed: Vec<(String, String)> }
  pub fn gather(paths: &[std::path::PathBuf], stop: &std::sync::atomic::AtomicBool) -> Gathered;
  pub fn adobe_folders() -> Vec<std::path::PathBuf>; // existing ones only
  ```
  Accepts `.xmp`, `.lrtemplate` (case-insensitive), folders recursively, `.zip` (in memory; nested folders; caps from Global Constraints; never written to disk). Skips files > 1 MB with a failure line. `adobe_folders`: Windows `%APPDATA%\Adobe\CameraRaw\Settings`, `%APPDATA%\Adobe\Lightroom\Develop Presets`; macOS `~/Library/Application Support/Adobe/CameraRaw/Settings`, `…/Adobe/Lightroom/Develop Presets`; Linux empty. Behind `cfg`.

- [ ] **Step 1: Failing tests:** `folders_are_read_recursively_with_group_hints`; `zip_packs_are_read_in_memory`; `a_zip_bomb_is_refused` (entry claiming > 50 MB → one failure line, nothing found); `other_files_are_ignored`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass.
- [ ] **Step 5:** Commit "Lightroom presets: gather files, folders and zip packs".

### Task 6: Import job, dialog and summary

**Files:**
- Create: `crates/lf-app/src/lightroom/import.rs` (job), `crates/lf-app/src/dialogs/lr_import.rs` (dialog + summary)
- Modify: `crates/lf-app/src/shortcuts.rs` (`Action::ImportLrPresets`, label "Import Lightroom presets…", no default key), `crates/lf-app/src/menu.rs` (Develop tab, Presets group, next to Save preset; icon must pass `ribbon_icons_are_in_the_fonts`), `crates/lf-app/src/develop/panels/presets.rs` (a link "Import from Lightroom…" under the list), `crates/lf-app/src/library/mod.rs` (run the job on `Lane::Long`, `Msg::LrImported`), `crates/lf-app/src/dialogs/mod.rs`
- Test: unit tests in `import.rs`; a UI test in `crates/lf-app/tests/` following the existing dialog tests (kittest, named widgets)

**Interfaces:**
- Consumes: `gather`, `read_any`, `map`, `fit_budget`, `lf_catalog::presets::{list, save}`.
- Produces:
  ```rust
  pub const MAX_PRESETS: usize = 2000;
  pub struct ImportReport { pub imported: Vec<(String /*name*/, Vec<String> /*notes*/)>, pub failed: Vec<(String, String)>, pub left_out: Vec<String> /* over the limit */ }
  pub fn import(catalog: &lf_catalog::Catalog, studio_id: i64, found: Vec<Found>, stop: &AtomicBool) -> ImportReport;
  pub fn unique_name(wanted: &str, taken: &std::collections::HashSet<String>) -> String;
  pub fn shown_name(name: &str) -> String; // glyph-safe for egui
  ```
  `LrKind::Profile` files are counted as failed with "This is a profile, not a preset." unless Task 11 lands. Group: `crs:Group`, else `group_hint`. After import, `Develop::presets_stale()` and the existing "presets changed → sync" path.

- [ ] **Step 1: Failing tests:** `names_get_numbered_when_taken` (`"Studio - Moody"` taken → `"Studio - Moody (2)"`, both taken → `(3)`); `importing_twice_keeps_both` (Review Focus 5); `the_limit_stops_the_import` (1,999 existing + 3 found → 1 imported, 2 in `left_out`); `profiles_are_reported`; `shown_name_replaces_glyphs_egui_lacks` (`"夕焼け"` → contains no char outside the app's fonts; check with the same helper `tests/glyphs.rs` uses or egui's `Fonts::has_glyph` through a test context) (Review Focus 1); UI test: dialog opens from the action, "Find Lightroom's presets" disabled on Linux / when `adobe_folders()` is empty, summary lists notes per preset.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass; run the app (`cargo run --release -p lf-app`) and import a folder of the hand-written fixtures; presets appear in Develop.
- [ ] **Step 5:** Commit "Develop: Import Lightroom presets".

### Task 7: Auto WB and Auto tone on apply

**Files:**
- Modify: `crates/lf-app/src/develop/extras.rs` (preset apply), `crates/lf-app/src/dialogs/batch.rs` / `batch::apply_one` path
- Test: `crates/lf-app/src/develop/extras.rs` unit tests or `crates/lf-app/tests/panels.rs`

**Interfaces:**
- Consumes: `lf_settings::lightroom::auto_flags(preset: &Value) -> (bool /*wb*/, bool /*tone*/)` (add in `lightroom/mod.rs`), `develop::auto::{Sample, auto_wb, auto_tone}`, `ai_job::render` for photos not open.
- Produces: `fn apply_auto(settings: &mut EditSettings, sample: &Sample, wb: bool, tone: bool)` in `develop/auto.rs`.

In Develop, after `Recipe::Look` apply, build the `Sample` from the open picture (as the Auto buttons do) and apply. In Apply to selected (batch), for a preset with auto flags render each photo at 512 px long edge with its new settings minus tone keys, make a `Sample` from it, apply. Paste of already-applied settings carries the resulting values (no auto again), as Lightroom.

- [ ] **Step 1: Failing test:** `a_preset_with_auto_tone_runs_auto_tone` (a dark synthetic sample → exposure > 0 after apply); `a_preset_without_auto_flags_does_not`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass.
- [ ] **Step 5:** Commit "Presets: Lightroom Auto WB and Auto tone run per photo".

### Task 8: Server preset limit 2,000

**Files (web monorepo `D:/pet-projects/photo-studio-management`):**
- Modify: `backend/app/Models/PhotoEditPreset.php` (`MAX_PER_STUDIO = 2000`)
- Test: the existing backend test covering the limit (find with `git grep MAX_PER_STUDIO`), updated to 2000

- [ ] **Step 1:** Update the test's expected limit; run the backend test suite as its README says → FAIL.
- [ ] **Step 2:** Change the constant → PASS.
- [ ] **Step 3:** Commit to the monorepo's `main` and push (per the user's standing rule for monorepo edits).

### Task 9: Fit a table, vignette and grain from an example

**Files:**
- Create: `crates/lf-grade/src/example.rs` (`pub mod example;` in `lib.rs`)
- Test: `crates/lf-grade/tests/example.rs`

**Interfaces:**
- Consumes: `lf_merge::align(reference: &Rgb, frame: &Rgb, levels: u32) -> (i32, i32)` (add `lf-merge` as a dependency of `lf-grade`, both CPU-only).
- Produces:
  ```rust
  pub struct Image { pub w: usize, pub h: usize, pub data: Vec<f32> /* sRGB 0..1, RGB */ }
  pub struct Fit { pub table: lf_settings::lut::Lut /* size 17 */, pub vignette: f64, pub vignette_midpoint: f64,
                   pub grain: f64, pub grain_size: f64, pub delta_e_mean: f64, pub delta_e_p95: f64, pub aligned: bool }
  pub fn fit(ours: &Image, theirs: &Image, render_effects: &mut dyn FnMut(&Fit) -> Image) -> Fit;
  pub fn delta_e2000(a: [f32; 3], b: [f32; 3]) -> f64; // sRGB in
  ```
  `theirs` is resized to `ours`' size first (bilinear) by the caller-independent helper `resize(&Image, w, h)`. Alignment: translation by `lf_merge::align` on 4 levels; `aligned = false` when the post-shift mean absolute luma difference > 0.15. Mask: 5 % border and pixels ≥ 0.995 or ≤ 0.005 in either. Table: bin pairs into 17³ cells by `ours` colour (trilinear weights), robust mean (drop the 10 % furthest from the cell median), empty cells filled by 30 Jacobi iterations of Laplacian smoothing with seen cells pinned, then 2 iterations of light smoothing (weight 0.25) over all. Vignette: alternate 3 rounds — divide `theirs` by the table-predicted colour, fit luma ratio vs normalised radius to our vignette model (use the shader's vignette formula; read it from `develop.wgsl` and port it in Rust here, documented). Grain: std-dev of the high-pass (`x − box3(x)`) in the 20 % flattest 16 px blocks, `theirs` minus `ours` in quadrature → grain amount via a linear factor measured once with `render_effects` at grain 50. ΔE on the full result (`render_effects` applies table + vignette + grain with our pipeline maths) vs `theirs`.

- [ ] **Step 1: Failing tests:** `a_known_table_is_recovered` (apply a hand-made warm/S-curve function to a synthetic colourful image → fitted table maps 200 random in-gamut colours within ΔE 1.0); `unseen_colours_map_smoothly` (fit on a reds-only image; mapping of blue stays within ΔE 10 of identity and is continuous); `a_vignette_is_recovered` (−30 applied → fitted −30 ± 5); `a_few_pixels_of_shift_are_tolerated` (3 px shift → `aligned`, ΔE mean < 2); `a_mismatched_pair_says_so` (theirs = different image or cropped by 20 % → `!aligned || delta_e_mean > 10`) (Review Focus 3); `delta_e2000_matches_published_pairs` (Sharma 2005 test data, 3 pairs, ±0.0001).
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass (`cargo test -p lf-grade --release`, under 10 s).
- [ ] **Step 5:** Commit "Grade: fit a table, vignette and grain from an example pair".

### Task 10: Embedded tables in presets

**Files:**
- Modify: `crates/lf-settings/src/lut.rs` (embedded table encode/decode, `to_cube`), `crates/lf-app/src/luts.rs` (materialise), `crates/lf-app/src/clipboard.rs` (`Recipe::Look` rewrites an embedded table to a `LutChoice` by name), `crates/lf-app/src/develop/extras.rs` + `crates/lf-app/src/library/mod.rs` (call `ensure_embedded` before applying a preset, single or batch, and for hover preview)
- Test: `crates/lf-settings/src/lut.rs` unit tests; `crates/lf-app/src/luts.rs` unit tests

**Interfaces:**
- Produces:
  ```rust
  // lf_settings::lut
  pub fn encode_table(lut: &Lut) -> serde_json::Value; // {"size":17,"table":"<base64 u16 LE RGB>","hash":"<16 hex of sha256>"}
  pub fn decode_table(v: &serde_json::Value) -> Option<Lut>;
  pub fn embedded_name(hash: &str) -> String; // "lr-<hash>"
  impl Lut { pub fn to_cube(&self) -> String; }
  // lf_app::luts
  pub fn ensure_embedded(root: &Path, preset: &serde_json::Value) -> Result<Option<String>, String>; // writes lr-<hash>.cube if missing, registers, returns name
  ```
  Base64: move `masks.rs`' private `base64_encode/decode` into `crates/lf-settings/src/b64.rs` (`pub(crate)`), used by both. `Recipe::Look` apply turns `desktop.lut = {table…, amount}` into `{"name": embedded_name(hash), "amount": amount}` on the photo (pure; no I/O).

- [ ] **Step 1: Failing tests:** `an_embedded_table_round_trips` (decode(encode(lut)) within 1/65535 per channel; encoded JSON < 40,000 bytes for size 17); `a_corrupt_table_is_none`; `applying_a_preset_with_a_table_stores_only_its_name`; `ensure_embedded_writes_once_and_registers` (temp root; second call doesn't rewrite; `lf_pipeline::lut::get(name)` is some).
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass.
- [ ] **Step 5:** Commit "Presets: carry a colour table inside the preset".

### Task 11: "Match a Lightroom example…"

**Files:**
- Create: `crates/lf-app/src/lightroom/example_job.rs`, `crates/lf-app/src/dialogs/lr_example.rs`
- Modify: `crates/lf-app/src/develop/panels/presets.rs` (context menu "Match a Lightroom example…" on a preset; also a button "New preset from a Lightroom example…"), `crates/lf-app/src/shortcuts.rs` (`Action::MatchLrExample`), `crates/lf-app/src/library/mod.rs` (`Msg::LrExampleFitted`), `crates/lf-app/src/menu.rs`
- Test: unit tests in `example_job.rs` with an injected renderer (as `lf-grade`'s calibrate tests); UI test for the dialog

**Interfaces:**
- Consumes: `lf_grade::example::{fit, Image, Fit}`, `ai_job::render(gpu, path, settings, max_edge) -> Result<Rgb, String>`, `lf_catalog::colour` (ICC → sRGB), `encode_table`, `fit_budget`, `presets::save`.
- Produces: `pub fn run(gpu, original: &Path, export: &Path, preset: Option<&Value>, stop) -> Result<(Value /*new preset*/, Fit, Previews), String>` where `Previews` holds four 512 px RGBA images (ours before, Lightroom, ours after, difference ×4).

Flow: render `original` at 1024 px with the preset (or defaults) minus `desktop.lut`, vignette 0, grain 0; decode `export` with `image`; if it carries an ICC profile that is not sRGB, convert through `lf_catalog::colour` (or refuse with "Export from Lightroom in sRGB, or with an embedded profile."); fit; build the preset: original preset + `desktop.lut = encode_table(table) + amount 100` + vignette/grain fields + `desktop.lightroom.example = {deltaEMean, deltaEP95}`; `fit_budget`. Dialog shows the four previews, "Difference: ΔE 1.6 average, 3.9 at most for 95 % of pixels", a plain-language verdict (< 2 "very close", < 5 "close", else "not a good match: was the export cropped or lens-corrected?"), and Save (replace preset) / Save as new (`"<Name> (matched)"`) / Cancel. The glyph `Δ` must pass `tests/glyphs.rs`, else write "dE".

- [ ] **Step 1: Failing tests:** `the_preset_gets_its_table_and_effects` (injected render + synthetic export); `an_adobe_rgb_export_is_converted` (tagged PNG fixture → converted, fit ΔE as if sRGB source) (Review Focus 4); `a_cropped_export_is_reported_not_saved_silently` (verdict text "not a good match"); UI test: dialog shows verdict and both save buttons.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** Pass; run the app, match a fixture pair, apply the matched preset to two photos via Apply to selected.
- [ ] **Step 5:** Commit "Develop: Match a Lightroom example".

### Task 12: Opt-in real-sample test and docs

**Files:**
- Create: `crates/lf-app/tests/lightroom_samples.rs` (skips unless `LF_LR_SAMPLES` is set; each subfolder holds `original.*`, `export.*`, optional `preset.xmp`; prints import notes and fit ΔE; asserts mean ΔE < 5)
- Modify: `AGENTS.md` (workspace map line for `lf_settings::lightroom`, `lf_grade::example`, the roadmap entry, the `LF_LR_SAMPLES` note), `README.md` features list, `docs/competitor-analysis.md` (Lightroom preset import done)

- [ ] **Step 1:** Write the test; run without the env var → skipped (passes).
- [ ] **Step 2:** Update docs.
- [ ] **Step 3:** Full verification: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` → all green, parity unchanged.
- [ ] **Step 4:** Commit "Lightroom presets: real-sample test and docs".

## Deferred (spec allows)

- Decoding embedded profile tables (`crs:RGBTable`/`LookTable`, base85 + zlib): profiles are reported as such in Task 6. A follow-up can decode them into the same embedded-table format from Task 10.
