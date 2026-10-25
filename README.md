# LensFlow Image Editor

A native desktop RAW editor for LensFlow photo studios, written in Rust. It
brings the LensFlow web editor's develop pipeline to the desktop so that
studios can edit full-resolution RAW files locally and get the same look the
web editor produces.

## Status

Part 2a (core pipeline parity: the settings model and a headless GPU renderer
that reproduces the web editor's output), Part 2b (RAW decoding and
development, so `lf-render` opens camera files) and Part 2c-i (the grade,
account and catalogue libraries) are built. The app itself (2c-ii) and
packaging (2d) follow. 2c-i is libraries only: there is no UI yet.

## Crates

- `lf-settings`: the develop settings model and `normalise`, plus curves,
  shader values, geometry, looks and settings groups, ported from the web
  editor.
- `lf-pipeline`: the headless wgpu renderer (`Gpu`, textures with mip chains,
  and the copy, gaussian, spots and develop passes) behind `Renderer`, which
  renders, draws a preview, and returns pixels and a histogram. It also holds
  `Renderer::new` (8-bit source) and `Renderer::from_texture` (half-float
  source with headroom above 1.0).
- `lf-raw`: opens any supported photo (`open::open`, kind sniffed from magic
  bytes, never the extension): decodes RAW with `rawler`, demosaics (bilinear
  or AHD; X-Trans is simple; or a binned `Preview` at half size for thumbnails),
  white-balances, converts to linear sRGB, applies
  the develop profile and highlight handling, and returns an `Opened` picture.
  `render::renderer_for` turns it into a `Renderer`. It also holds the
  `lf-render` example (see below).
- `lf-grade`: match a reference image's colour grade. Pure CPU maths ported
  from the web editor: `measure` (pixel statistics), `correct` and `calibrate`
  (rounds that invert the shader's formulas, with an injected renderer),
  `apply_strength`, `exposure_offset`, and `sheet` (picks sample photos and
  tiles the contact sheet).
- `lf-account`: LensFlow sign-in (with two-factor), the token store, the
  entitlement gate with its offline grace, the desktop grade call
  (`desktop/grade`), the download link, and edit-preset sync. A blocking
  `reqwest` client (rustls). See "LensFlow account (lf-account)".
- `lf-catalog`: the SQLite catalogue, `<file>.lensflow.json` sidecars, edit
  history and named versions, incremental import, prioritised background
  thumbnails, and export to disk with EXIF. It never touches pixels itself:
  thumbnails and export render through `lf-raw` and `lf-pipeline`.
- `lf-fixtures`: loads the golden fixtures in `tests/fixtures` and compares
  renders against them (mean, 99.9th percentile and maximum error).

## LensFlow account (lf-account)

Base URLs: production `https://lensflow.in/api`, development
`https://dev.lensflow.in/api` (`Instance::Prod` / `Dev`; `Custom(url)` for
tests). Every request carries `Accept: application/json`,
`X-LensFlow-Client: desktop` and `User-Agent: lensflow-ImageEditor/<version>
(<os>)`; once signed in, `Authorization: Bearer <token>`; once a studio is
chosen, `X-Studio: <slug>`. Timeouts: 20 s to connect, 30 s to read (120 s for
grading).

Endpoints used: `POST auth/login` (email, password, optional
`two_factor_code`; a reply with `two_factor_required` means ask for the code
and call again), `GET auth/me`, `POST auth/logout`, `GET desktop/entitlement`,
`GET desktop/download/{platform}`, `POST desktop/grade` (multipart: mode
`extract` or `review`, a reference JPEG and a contact-sheet JPEG each under
700 KB, up to 16 example looks, and for a review the current settings), and
`galleries/edit-presets` (list, create, PATCH, delete). Preset sync names a
local preset `<name> (desktop)` (or `(desktop 2)`, ...) when the name is taken.

How a response becomes behaviour (`client.rs`, `entitlement.rs`):

| Response | Result |
| --- | --- |
| 401 | Signed out: clear the token and the cached entitlement, sign in again |
| 404 `studio_not_found` | The remembered studio is gone: choose another |
| 403 or 402 `studio_suspended` | Locked (suspended) |
| 403 or 402 `desktop_editor_disabled` | Locked (not enabled) |
| 403 or 402 `subscription_paused` | Locked (paused) |
| 402 `subscription_required` | Read-only (below) |
| 422 `plan_limit` | The AI allowance is used up |
| other 422 | Field validation errors |
| network error, 429, 5xx | Transient: lean on the cached entitlement (below) |
| any other 4xx | Refused: locked, with the server's message |

Entitlement rules. The reply carries `enabled`, `standing`
(`active`, `read_only`, `paused`, `suspended`), AI availability and remaining
count, the studio, `min_version` and the latest release.

- Active and enabled: everything works. AI grading only while the AI
  allowance is available.
- Read-only: edit locally, but no export to the gallery, no AI and no preset
  sync (`export_allowed`, `ai_allowed` and `sync_allowed` are false).
- Paused, suspended, or not enabled for the studio: locked.
- Offline: after a network failure, 429 or 5xx, a cached entitlement is trusted
  for 7 days from its `checked_at` (`AllowStale`), unless it was itself a lock.
  Older than 7 days, dated more than 5 minutes in the future (a clock set
  back), or missing: locked with "Connect to the internet to keep using
  lensflow-ImageEditor." A definite answer (401, a lock, studio not found)
  clears the cache, so an older active entitlement is never replayed.

Where things live. The sign-in token goes in the OS keychain (service
`lensflow-ImageEditor`, account = the API base URL). With no usable keychain it
falls back to a file `token-<hash>` (0600 on Unix) in the `directories`
project config dir (`ProjectDirs::from("in", "lensflow",
"lensflow-ImageEditor")`), with a logged warning. The entitlement cache is
`entitlement.json`, written atomically in a folder the app chooses
(`entitlement::Cache::new(dir)`); the crate does not pick that folder.

## Catalogue (lf-catalog)

`Catalog::open(root)` creates `<catalogue root>/.lensflow/catalog.sqlite` in
WAL mode (bundled SQLite) and migrates it; the schema version is 3 (three migrations). Tables:
folders, photos, edits, history and history_cursor, versions, thumbnails,
presets, export_jobs and sidecar_meta. Each thumbnail worker opens its own
connection.

Sidecars. Edits are also written beside each photo as
`<file>.lensflow.json`: `v` (format version, currently 2), `settings` (the
edit settings), `versions[]` (`id`, `label`, `settings`, `created_at`) and
`updated_at`. Keys this version does not know are kept and written back
unchanged. Writes are atomic (temp file in the same folder, then rename).
Conflict rule on reopen (`edits::reconcile`): the newer `updated_at` wins. A
sidecar must be strictly newer to replace the catalogue, so ties go to the
catalogue, and the sidecar is rewritten from it. Adopting a sidecar restarts
the undo history from that state.

Import. `import::scan` lists supported files under a folder (RAW and
JPEG/PNG/TIFF/WebP by extension; hidden entries, `.lensflow` and sidecars are
skipped), and `import::run` records them in batches, one transaction each. It
reads headers and EXIF only. A file already catalogued with the same size and
mtime is skipped, so a rerun resumes. A file that cannot be read is still
added, with kind `unreadable` and the reason in `photos.error`. The progress
callback can stop the import after the batch it reports.

Thumbnails. `Thumbnailer` runs a small worker pool (default `min(2, cores)`)
over a priority queue (`High` for what is on screen, `Normal` otherwise),
renders 256 px thumbnails through the shared GPU, and stores JPEGs in the
catalogue. `take_ready` returns finished ones; `take_failed` returns
`(photo id, reason)` for those that could not be made (the reason is also
stored in `photos.error` and logged). The demosaic `Quality` is a constructor
parameter. Memory: each in-flight open of a 60 MP RAW can use about 1 GB at
`Quality::Fast`, which is why the pool is small. `lf-raw` has only `Fast` and
`Best` today; there is no cheaper `Preview` yet.

Developed-frame cache. `lf_raw::cache::FrameCache` is a folder the app picks:
one `<sha256>.lff` per file, quality, max edge, develop-settings and `DEVELOP_ALGO_VERSION` key. Format: `LFF1`
magic, a version (currently 2), sizes, flags, an LZ4 block of half-float RGBA
and a CRC32 covering the header and the block. A corrupt or old-version entry is a miss and is deleted. The cache
is trimmed to a byte cap (given to `FrameCache::open`), least recently used
first.

Export. `export::run` writes photos to a folder, one at a time, at full
resolution through the GPU pipeline.

- Formats: JPEG (quality 1..100), PNG (8-bit) and 16-bit TIFF (from the float
  render, clamped to 0..1 only at the encode).
- EXIF is written for JPEG only; PNG and TIFF exports carry none yet. For JPEG,
  metadata `All` carries the source JPEG's EXIF byte for byte (or a minimal EXIF
  built from the file's facts for other sources), `CopyrightOnly` writes Artist
  and Copyright, `None` writes nothing.
- Size: `Original`, `LongEdge`, `ShortEdge`, `Megapixels`, `Percent`. Only
  `Percent` can enlarge; the others never upscale.
- Rename pattern tokens: `{name}` (original stem), `{seq}` (1-based, padded to
  the count's width), `{date}` (`YYYY-MM-DD`, capture date else file date),
  `{camera}` (make and model with `-`, or `unknown`). Path-illegal characters
  become `-`, and Windows reserved names get a `_`.
- Collisions are refused before anything is rendered: two photos on one path
  (compared case-insensitively) is `NameCollision` (add `{seq}`). With
  `overwrite: false` an existing file is `Exists` and nothing is written; with
  `overwrite: true` it is replaced.
- The first failure stops the run (files already written stay) and is recorded
  in `export_jobs`.

## Supported files

- RAW (checked against the sample corpus): Canon CR2 (full resolution and
  sRAW) and CR3, Nikon NEF, Sony ARW, Fujifilm RAF (X-Trans), Olympus ORF,
  Panasonic RW2, Pentax PEF and DNG.
- JPEG, PNG, TIFF and WebP. TIFFs exported by Lightroom, Photoshop or DPP open
  as pictures even when they carry the camera's `Make` (Canon, Hasselblad...):
  a TIFF is treated as RAW only on RAW evidence (a `DNGVersion` tag, the CR2
  signature, or CFA / LinearRaw sensor data in an IFD or SubIFD).
- HEIC and AVIF are not supported yet: opening one gives a plain error.
  (Their embedded previews are HEVC too, so they cannot be shown without an
  HEVC decoder.)
- Known decoder gaps (rawler 0.8), reported as "not supported" rather than
  damaged: Nikon High Efficiency (HE / HE*) NEF, DNG 1.7 with JPEG XL
  compression, and some Sony ARW and Phase One IIQ variants.
- A truncated file is refused as incomplete before decoding (TIFF-based RAW,
  RAF and CR3 are checked against their headers; a cut RW2 is caught when the
  decoder fails on it). A decoder panic is reported as a damaged file, never a
  crash, so release builds must keep `panic = "unwind"`.

## Headroom rule

A RAW file records light above what an 8-bit picture can hold. Development
keeps those values as unclamped half floats (above 1.0) all the way to the
develop pass, so pulling highlights down later brings detail back instead of
flat white. By default the profile is `camera` (a gentle toe and shoulder
curve) and highlights are `reconstruct` (where the sensor clipped, colour is
pulled toward neutral, keeping its brightness above 1.0; bright colours that did
not clip are left alone); `desktop.develop` in the settings can pick `linear` and
`clip` (white-balanced camera values clamped at 1.0 before the colour matrix, so
clipped areas stay white instead of turning magenta). An 8-bit file has no
headroom, so it takes the web-identical 8-bit path and its output is the one
the parity gate covers.

## 8-bit and float paths

`render::renderer_for` picks the path from the picture: `Picture::EightBit`
goes through `Renderer::new` (RGBA8 chain, as the web editor); `Picture::Float`
(RAW) is uploaded as `Rgba16Float` with mips and goes through
`Renderer::from_texture`, whose whole chain stays half float. Float
intermediates are not bit-identical to the 8-bit chain, and not always inside the
parity gate: running the 228 fixtures through the float chain (Part 2b, task 1,
WARP) left most within it, but five sharpen and detail renders fell outside (worst
mean 1.03/255, p99.9 5/255, max 14/255), because sharpening amplifies the web
editor's 8-bit blur rounding about four times. That is why 8-bit files keep the
8-bit chain. RAW renders have no web golden; the gate covers the 8-bit path only.

## RAW sample corpus

`tests/raw-samples/` pins ten small CC0 files from raw.pixls.us (one per
format, plus a full-resolution CR2 next to the Canon sRAW one; about 87 MB). Fetch them with `scripts/fetch-raw-samples.sh` (or
`scripts/fetch-raw-samples.ps1`); they land in the git-ignored
`tests/raw-samples/files/`. Without them the corpus tests print a skip note
and pass. See `tests/raw-samples/README.md`.

`lf_raw::cache::FrameCache` keeps developed RAW frames on disk (one `.lff` file
per file, quality and develop-settings key, LZ4-compressed, trimmed to a byte
cap) so reopening a photo skips the demosaic. The caller picks the directory;
`lf-render` does not use it.

## lf-render

```bash
cargo run --release -p lf-raw --example lf-render -- <in> <settings.json> <out.png> \
  [--develop camera|linear] [--highlights clip|reconstruct] [--quality fast|best|preview]
```

`<in>` is any supported file. The develop flags override `desktop.develop` in
the settings file. It prints the kind, the output size (and the original size
when a very large file was reduced) and the adapter used; any error prints a
plain message and exits 1.

## Build and test

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`cargo test --workspace` runs the parity suite (all 228 renders: 6 sources
by 38 cases) on a real GPU or on software Vulkan. Rust stable is pinned by
`rust-toolchain.toml`.

GPU tests share one device per test binary
(`lf_pipeline::test_support::shared_gpu`, behind the `test-support` feature):
Windows' WARP software adapter crashes when one process opens several devices
at once. A new GPU test should use it too; only tests that change device-wide
state (the error handler, destroying the device) open their own, one at a time,
in a binary of their own (`crates/lf-pipeline/tests/device.rs`). If a local
run still crashes in the driver, `cargo test --workspace -- --test-threads=1`
is the safe fallback.

Environment variables:

- `LF_ALLOW_FALLBACK_ADAPTER`: `0` forbids a software adapter, so a missing
  GPU is an error; anything else, or unset, allows one. CI sets `1` and runs
  on Mesa lavapipe.
- `WGPU_BACKEND`: wgpu's own backend selector (for example `vulkan`). CI sets
  `vulkan`.

On Linux without a GPU, install `mesa-vulkan-drivers libvulkan1` and run the
tests with `WGPU_BACKEND=vulkan LF_ALLOW_FALLBACK_ADAPTER=1`.

## Parity rule

`tests/fixtures/` is a verbatim copy of the web editor's golden renders:
six source images (`sources/`), 38 cases of normalised settings (`cases/`),
the web renderer's output for every source and case (`renders/`), and
`manifest.json` with a SHA-256 for each file.

For each `renders/<source>__<case>.png`, rendering `sources/<source>.png` with
`cases/<case>.json` must match per channel (R, G, B on a 0..1 scale, every
sample counted):

- mean absolute error under 1/255,
- 99.9th percentile of the absolute error under 4/255 (the smallest error `e`
  such that at least 99.9% of samples have error ≤ `e`), and
- maximum absolute error at most 16/255 (inclusive), as a sanity bound.

The `identity` case must be bit-exact (maximum error 0) on every source.

Why a percentile and not a strict 4/255 maximum: the goldens come from
SwiftShader, and every rasteriser rounds a few intermediate 8-bit values
differently. Some stages extrapolate (texture, dehaze, clarity and the
sharpening mask), and they amplify a one-level difference in a blur target
into 4 or 5 levels at a handful of pixels. The goldens also have SwiftShader
edge artefacts: on row 0 of odd-height canvases, where the GLSL's implicit
derivatives are undefined after its early return, some cases differ from
their own next row. A mean bound plus a 99.9th percentile catches systematic
errors (orientation, LOD, gamma, rounding), which move many pixels; it
tolerates the few that only rounding and those artefacts touch. The 16/255
maximum is inclusive. It catches gross breakage, but a 16/255 spike on a few dozen
pixels (seen with saturation +100 on a software rasteriser) is rounding
amplification, and the mean and percentile already bound the picture. The 4/255 and 16/255 bounds are
constants in `lf-fixtures`; `manifest.json` stays a verbatim copy.

Output sizes must match exactly. To update the fixtures, replace the whole
folder with the web repo's `frontend/parity-fixtures/` (do not merge) and
check the manifest hashes.

## Licence

Apache-2.0, see `LICENSE` and `NOTICE`.
