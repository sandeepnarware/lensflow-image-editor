# Lightroom presets: import and learn-from-example

Date: 2026-11-01. Status: draft for approval (builder check done).

## Why

In a customer demo, a studio wanted to bring its Lightroom presets into LensFlow and apply them
to many photos, and expected the photos to come out looking the same as in Lightroom. (The
same demo found that paste carried only saturation; that bug is fixed in `99d876e`.)

Lightroom's processing is unpublished (slider maths, local tone mapping, Adobe's own
profiles), so a pixel-identical result from slider values alone is not possible; nobody
outside Adobe achieves it. We get as close as can be measured by combining two things:

1. **Import**: read Lightroom preset files in one step and translate every setting we can into
   ours. Useful on day one, close but not identical.
2. **Learn from an example**: given one photo processed in Lightroom with the preset and the
   original, fit a colour table (3D LUT) that turns our render into Lightroom's, plus vignette
   and grain. This captures Adobe's profile, tone, colour mix, grading and curves without
   knowing how Adobe computes them. Near-identical on the example, close on similar photos.

Both produce ordinary studio presets, so they appear in the preset list, sync to the LensFlow
account, and work with copy/paste, Apply to selected, Amount and auto-sync.

## Goals

- One action imports a whole preset collection: picked `.xmp` / `.lrtemplate` files, a folder
  (recursively), a `.zip` preset pack, or, with nothing picked, Adobe's own preset folders.
- Every Lightroom setting with a counterpart in our pipeline is translated (table below).
- A preset can be refined from one example pair, and the result is measurably close
  (target: mean ΔE2000 < 2 and p95 < 5 on the example photo, at preview size).
- Honest reporting: after an import or a fit, the user sees what was skipped or approximated,
  per preset, and the measured difference after a fit.

## Non-goals (this project)

- Pixel-identical output from slider values alone.
- Shipping Adobe's profiles or camera DCPs (licence). Loading those already on the user's
  computer is a follow-up (see Later).
- Exporting our presets back to Lightroom format.
- The wider "behaves like Lightroom" work (full Copy Settings tree, Paste from previous, Sync
  dialog, panel order). Separate project, after this one.
- Changes to the web editor, other than the preset limit. The web ignores `desktop.*` keys, so synced presets stay safe there.

## Anti-goals (would make this a failure even if it works)

- Claiming "exact" anywhere in the UI. Wording is "imported" and "matched (difference: …)".
- Changing renders of existing photos or breaking the parity gate: no change to the develop
  shader's existing maths; the 8-bit path and `tests/fixtures` stay untouched.
- Silent loss: a setting we drop without saying so.
- A preset whose look depends on a file that does not travel with it (see "Learned table";
  photos reference tables by name as LUT looks already do).

## Constraints

- Settings JSON enters only through `lf_settings::normalise`. `desktop` is capped at 64 KB
  (`MAX_DESKTOP_JSON`).
- Never touch the user's files: import reads only; Adobe's folders are never written.
- No slow work on the UI thread: parsing a collection and fitting run on the `Long` lane with
  progress and cancel, like other batch jobs.
- New dependencies Apache-2.0 compatible. The `.zip` reader and XML parsing need crates (e.g.
  `zip`, `quick-xml`); both MIT/Apache.
- User-facing strings are constants; egui glyph rule (`tests/glyphs.rs`).
- AI/entitlement: neither feature uses `lf-ai`; read-only standing still allows them locally,
  but saving into studio presets follows the existing preset-sync rules.

## Approaches considered

1. **Translate sliders only.** Read the preset, map each value. Cheap, editable, but the look
   is only approximate: different slider maths, no Adobe profile.
2. **Learn only from examples.** Fit a LUT from one Lightroom export. Very close colour, but
   needs a Lightroom export for every preset, and local effects (Clarity, Texture, Highlights,
   Shadows act area by area) are only approximated by a global table.
3. **Both, combined (chosen).** Import gives editable sliders including the local effects; the
   fit then learns only the *remaining* difference between our render with those sliders and
   Lightroom's export. Works without Lightroom (import alone) and gets closest with it.

The user chose 3 ("implement it that way").

## Part 1: Import

### Entry points

- Develop's preset panel and the ribbon: **Import Lightroom presets…** (`Action::ImportLrPresets`).
  A small dialog: "Choose files…", "Choose folder…", "Find Lightroom's presets" (the default
  folders), and the import button.
- Default folders. Windows: `%APPDATA%\Adobe\CameraRaw\Settings` (Lightroom Classic 7.3+ and
  Camera Raw `.xmp`) and `%APPDATA%\Adobe\Lightroom\Develop Presets` (legacy `.lrtemplate`).
  macOS: `~/Library/Application Support/Adobe/CameraRaw/Settings` and
  `~/Library/Application Support/Adobe/Lightroom/Develop Presets`. Linux: none (picker only).
  Adobe's built-in presets live under the install folder; they are skipped (Adobe content).

### Reading

- `.xmp`: an `x:xmpmeta` with `crs:` settings, either as attributes on `rdf:Description` or as
  child elements; sequences (`rdf:Seq`) for curves. Preset name from `crs:Name` (an
  `rdf:Alt` with `x-default`), group from `crs:Group`. Files whose `crs:PresetType` is not
  `Normal` (e.g. profiles, `Look`) are profiles, handled under "Profiles" below.
- `.lrtemplate`: a Lua table `s = { title = "...", value = { settings = { Exposure2012 = 0.5,
  ... } } }`. A small tolerant reader for the subset Lightroom writes (strings, numbers,
  booleans, nested tables, `ZSTR "..."` titles); no Lua interpreter.
- `.zip`: entries read in memory, same rules; nested folders allowed; cap total uncompressed
  size (e.g. 50 MB) and entry count against zip bombs.
- A file that fails to parse is reported by name and skipped; the rest still import.

### Mapping (Lightroom → ours)

Process version 2012+ keys (`*2012`) are primary. Ranges are scaled to ours where they differ.
The first cut maps value-for-value where both are −100..100 and the same idea; calibration
against Lightroom renders (Part 2's measurement tool, run on sliders one at a time) refines the
per-slider response curves later without changing the stored format.

| Lightroom (`crs:`) | Ours |
| --- | --- |
| `Exposure2012`, `Contrast2012`, `Highlights2012`, `Shadows2012`, `Whites2012`, `Blacks2012` | Light group |
| `WhiteBalance` = As Shot | white balance left out of the preset |
| `Temperature`/`Tint` (Kelvin, RAW presets with a custom WB) | our temperature/tint are relative, so the first cut stores a relative estimate against a 5500 K as-shot baseline (via `kelvin_wb`), and keeps the Kelvin target in `desktop.lightroom` for a later per-photo conversion |
| `IncrementalTemperature`/`IncrementalTint` (non-RAW) | temperature/tint, scaled |
| `Texture`, `Clarity2012`, `Dehaze` | Detail group |
| `Vibrance`, `Saturation` | Color group |
| `HueAdjustment*`, `SaturationAdjustment*`, `LuminanceAdjustment*` (8 colours) | `hsl` |
| `ToneCurvePV2012` (+`Red`/`Green`/`Blue`), 0–255 points | `curve` |
| `ParametricShadows/Darks/Lights/Highlights` + splits | folded into the `curve` points (sampled) |
| `ColorGrade{Shadow,Midtone,Highlight}{Hue,Sat,Lum}`, `ColorGradeBlending`, `ColorGradeBalance`, `ColorGradeGlobal*`; legacy `SplitToning*` | `grading` (lum and global approximated) |
| `Sharpness`, `SharpenRadius` (`SharpenDetail`, `SharpenEdgeMasking` noted, not mapped) | Sharpen group |
| `LuminanceSmoothing`, `ColorNoiseReduction` | `noise`, `colorNoise` |
| `PostCropVignetteAmount`, `PostCropVignetteMidpoint` (feather/roundness/style noted) | Effects |
| `GrainAmount`, `GrainSize` (`GrainFrequency` noted) | Effects |
| `ConvertToGrayscale`, `GrayMixer*` | `bw` (mixer noted) |
| `LensProfileEnable`, `AutoLateralCA`, `VignetteAmount`/`Midpoint` (lens), `Defringe*` | `desktop.lens` (auto profile via lensfun when applied), `desktop.defringe` |
| `Perspective*`, `UprightVersion`/`UprightTransform*` | `desktop.lens` keystone / Upright mode |
| `RedHue`, `RedSaturation`, `GreenHue`, …, `ShadowTint` | `desktop.calibration` |
| `PointColors` | `desktop.pointColour` (up to four) |
| `MaskGroupBasedCorrections` with AI subject, sky, background, people masks | adaptive masks (`desktop.adaptiveMasks`, generated on apply) with their local sliders |
| Crop, `CropAngle`, spot removal, brushed/radial/linear masks with positions | skipped: they belong to one photo (as Lightroom's own presets usually exclude them) |
| `CameraProfile`, `Look` (profile names) | recorded and reported; see Profiles |

- Every original `crs:` key and value is kept in the preset under `desktop.lightroom`
  (`{ "name", "group", "processVersion", "settings": { ... } }`), so a better mapping later can
  re-translate a preset without the original file, and nothing is silently lost.
- `crs:` keys Lightroom omits mean "not part of this preset": the matching groups are left out
  of the preset JSON, and applying it leaves those settings on the photo as they are. This is
  how Lightroom applies presets. Today's `apply_look` does not do that: it resets every web look
  group (`DEFAULT_COPY_GROUPS`) the preset doesn't carry, as the web's `applyLook` does. So
  **imported presets get a Lightroom-style apply** (recognised by `desktop.lightroom`): only the
  groups they carry are laid on, through the same `Recipe::Look` entry point, so copy/paste,
  Apply to selected, Amount, hover preview and auto-sync all follow. Existing looks and studio
  presets keep their current behaviour (web parity). *(Builder-check answer; my call.)*
- Within a group Lightroom may set only some keys (e.g. Exposure without Contrast). A group is
  carried when any of its keys is set; its unset keys take Lightroom's defaults (0), as
  Lightroom's preset would apply them only if saved, which in practice means the group's
  checkbox was ticked and all its keys were written.
- Lightroom's Auto settings (`WhiteBalance` = Auto, `AutoTone`, `AutoExposure`…) become our Auto
  WB / Auto tone (`develop/auto.rs`), computed per photo when the preset is applied, queued the
  way adaptive masks are (`extras::AdaptiveQueue`). *(Builder-check answer; my call.)*

### Profiles

- `crs:Look` in a preset names a profile and may embed its table (`crs:RGBTable` /
  `crs:LookTable`, base85 + zlib per the DNG SDK's `dng_big_table` encoding). When the table is
  embedded (creative profiles: B&W, Vintage, Artistic, third-party), decode it into a LUT and
  apply it as the preset's learned table (below). When only the name is given (Adobe Color,
  Adobe Portrait…), record it and report "uses Adobe profile X: refine from an example for the
  closest match". Decoding embedded tables is a stretch goal; the builder may defer it if the
  encoding proves costly, reporting such presets instead.

### Saving

- The account server's preset limit rises from 200 to **2,000 per studio**
  (`PhotoEditPreset::MAX_PER_STUDIO` in the web monorepo, `backend/app/Models/PhotoEditPreset.php`;
  the user chose 2,000 over unlimited because the list endpoint returns every preset's settings
  in one response). Settings stay capped at 160 KB each server-side; ours stay under 64 KB.
  Import stops adding at the limit and reports the presets left out. Plain imported presets are
  a few KB; only presets matched from an example carry the ≈ 39 KB table.
- Each preset becomes a studio preset through `lf_catalog::presets::save`, named
  `"<Group> - <Name>"` (no group: just the name). A name already taken gets ` (2)`, ` (3)`.
- A summary dialog: N imported, M with notes (expandable per preset: "skipped: grain
  frequency, sharpen masking; uses Adobe Portrait"), K failed (file and reason).

## Part 2: Learn from an example

### Entry point

- On a preset's context menu, and from the import summary: **Match a Lightroom example…**.
  Also usable with no preset ("new preset from example"). Inputs: the original photo (RAW or
  other; can be picked from the catalogue) and Lightroom's export of it with the preset applied
  (JPEG, TIFF or PNG, sRGB, any size).
- Guidance shown in the dialog: export from Lightroom at full size or at least 2000 px, sRGB,
  without crop or lens-profile geometry changes if possible.

### Fit

1. Render the original with our pipeline: the preset's imported sliders (or defaults) and no
   LUT, at a working size (long edge ~1024).
2. Bring Lightroom's export to the same size; align by translation and scale (search a small
   range; reuse `lf-merge`'s alignment where it fits). Exclude a border (5 %) and pixels clipped
   in either image.
3. Fit vignette: the luminance ratio Lightroom/ours as a function of radius, after removing the
   global colour mapping; alternate with step 4 for two or three rounds.
4. Fit a 17³ RGB table from ours → Lightroom's on display sRGB (where our LUT stage sits),
   by binning pairs per cell with a robust mean, then smoothing and filling empty cells
   (Laplacian smoothing; nearest valid colours for extrapolation), so unseen colours on other
   photos are mapped smoothly rather than to garbage.
5. Grain: compare high-frequency energy in flat regions; set our grain amount/size to match.
6. Measure: ΔE2000 mean and p95 between our full result and Lightroom's export at working size.
   Shown with before / Lightroom / ours / difference views.

All of this is CPU maths in a crate without GPU, like `lf-grade` (rendering is injected), with
unit tests on synthetic pairs: a known LUT and vignette applied to an image are recovered.

### Learned table

- Size budget: `normalise` drops the whole `desktop` object above 64 KB, silently. The import
  therefore checks the size before saving: the table (≈ 39 KB) and mapped settings come first;
  `desktop.lightroom` keeps the original `crs:` values but never mask geometry, and if the
  total is still over the cap, its `settings` are dropped and the summary says so. A save is
  never allowed to pass the cap. *(Builder-check answer; my call.)*
- A preset must carry its table: it syncs to the account and may be applied in another
  catalogue. So the table is embedded in the preset JSON as `desktop.lut.table`: 17³ × RGB as
  16-bit values, base64 (≈ 39 KB, under the 64 KB `desktop` cap), plus its content hash.
- Applying such a preset writes the table into the catalogue's LUT folder
  (`.lensflow/luts/lr-<hash>.cube`) if missing, registers it, and stores the usual
  `LutChoice` (by name) on the photo. Photos keep referencing tables by name, as today, so the
  per-photo settings stay small and the existing LUT path renders them. The name is the content
  hash, so the same table is the same file everywhere. A photo moved to another catalogue
  without the file renders without the table, as LUT looks already do; re-applying the preset
  (which carries the table) restores it. The anti-goal is about presets: a **preset** never
  depends on a file outside it. *(Builder-check answer; my call.)*
- Every path that applies a preset with a table (apply, paste of a preset-applied photo within
  the catalogue, Apply to selected, auto-sync) runs after the table is registered: applying
  the preset registers it first; the others copy a name that is already in the catalogue.
- The fit result is saved back onto the preset (update, or new preset "<Name> (matched)").

### Where it sits in the pipeline

Unchanged shader maths: the existing LUT stage (display-referred, before vignette and grain)
applies the table; vignette and grain use the existing sliders. No new render path, so parity
and existing photos are unaffected.

## Testing

- Parser tests on real-format fixtures written by hand (attribute and element XMP forms,
  `rdf:Seq` curves, `.lrtemplate`, zip), including malformed files and a zip bomb.
- Mapping tests: each mapped key lands in the right setting with the right scale; unknown keys
  survive in `desktop.lightroom`; absent groups are not carried.
- Fit tests on synthetic pairs (known LUT + vignette + grain recovered within tolerance;
  misalignment of a few pixels tolerated; a mismatched pair reports a large difference).
- An opt-in test with real Lightroom pairs (`LF_LR_SAMPLES` folder: original + export + preset),
  printing ΔE, like `LF_DCP_DIR`.

## Left to the builder

- Crate layout (likely `lf-settings::lightroom` for parsing/mapping, a CPU fit module beside
  `lf-grade` or inside it), dialog layout, exact scaling constants for the first cut.
- Working size, table smoothing strength, alignment search range, within the targets above.
- Whether embedded profile tables (`RGBTable`) land in this project or are reported for later.

## Later (not this project)

- Load Adobe's camera profiles (DCP) already installed on the user's computer
  (`%ProgramData%\Adobe\CameraRaw\CameraProfiles`), through the existing `lf_raw::dcp`, so
  imported presets start from Adobe's colour on RAW files.
- Calibrate each slider's response against Lightroom renders (the Part 2 measurement tool,
  one slider at a time) and refine the mapping curves.
- The "behaves like Lightroom" project: Copy Settings tree, Paste from previous, Sync dialog.
