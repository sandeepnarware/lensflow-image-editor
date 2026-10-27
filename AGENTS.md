# Project reference: lensflow-image-editor

Working notes for agents and contributors. Human-facing docs: `README.md` (features,
crates, parity rules) and `docs/app.md` (threads, session, preview, file locations).

## What this is

A native desktop RAW photo editor for LensFlow studios, in Rust. It reproduces the
LensFlow web editor's develop pipeline locally, signs in to a LensFlow account,
catalogues folders, and edits/exports full-resolution photos without touching the
originals. Workspace `main` is at `e2a38d5` (2026-10-27): Parts 2a, 2b, 2c-i, 2c-ii
and 2d tasks 1–2 are built; packaging tasks 3–5 are in flight.

## Workspace map

| Crate | Role | Start here |
| --- | --- | --- |
| `lf-settings` | `EditSettings`, `normalise` (only safe JSON entry point), curves, geometry, looks, shader values, groups. `SETTINGS_VERSION` 2, `MAX_SPOTS` 48. | `src/lib.rs`, `src/settings.rs` |
| `lf-pipeline` | Headless wgpu renderer: `Gpu`, `Renderer`, mip-chained textures, copy/gaussian/spots/develop passes, curve texture, histogram, preview draw. | `src/renderer.rs`, `src/passes/develop.rs` |
| `lf-raw` | Magic-byte kind sniffing, rawler decode, demosaic (bilinear/AHD/X-Trans/preview bin), WB + colour + highlight handling, f16 headroom, EXIF, frame cache, `lf-render` example. | `src/open.rs`, `src/develop.rs` |
| `lf-grade` | CPU maths for "Match a reference": measure, correct, calibrate (injected renderer), strength/exposure, contact sheet. | `src/lib.rs` |
| `lf-account` | Blocking reqwest/rustls LensFlow client: sign-in + 2FA, token store, entitlement gate/offline grace, desktop grade, presets sync, download link. | `src/client.rs`, `src/entitlement.rs` |
| `lf-catalog` | SQLite catalogue (bundled, WAL, schema v6), sidecars, edit history (cap 50), versions, import, thumbnails, export, presets. Never touches pixels directly. | `src/db.rs`, `src/edits.rs`, `src/export.rs` |
| `lf-fixtures` | Loads `tests/fixtures` goldens and applies the parity thresholds. | `src/lib.rs` |
| `lf-app` | The `lensflow-image-editor` binary: eframe/egui-wgpu window, lanes, session, Develop/Library UI, dialogs, the console's `grade` (`grade_cli.rs`, before any window), release plumbing. | `src/main.rs` → `src/app.rs` |

Entry points:
- Binary: `crates/lf-app/src/main.rs` → `App::new` in `src/app.rs`.
- CLI: `crates/lf-app/src/cli.rs` (`--version`, `--help`, `grade`).
- Console grade: `crates/lf-app/src/grade_cli.rs` (the dialog's fit, no window, no
  catalogue, no sign-in; sidecars or `--out` JPEG copies).
- Headless render: `cargo run --release -p lf-raw --example lf-render -- <in> <settings.json> <out.png>`.

## Commands

```bash
cargo run --release -p lf-app            # app
cargo run --release -p lf-app -- grade   # grade a folder to match a reference
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                   # includes the 228-render parity suite
```

- Toolchain: stable, pinned by `rust-toolchain.toml`; edition 2024; resolver 3.
- GPU tests need a real or software adapter. Windows uses WARP; Linux CI uses lavapipe.
- GPU tests share one device per test binary via `lf_pipeline::test_support::shared_gpu`
  (`test-support` feature). Do not open extra devices in a test; device-state tests
  live alone in `crates/lf-pipeline/tests/device.rs`.
- `LF_ALLOW_FALLBACK_ADAPTER=0` forbids software adapters. `WGPU_BACKEND` selects wgpu's
  backend (CI sets `vulkan`). The app itself asks for DirectX 12 and OpenGL on Windows
  (Vulkan left out: on an old NVIDIA driver a Vulkan-enabled instance reports no OpenGL
  adapter, leaving only WARP) and for everything eframe enables elsewhere.
- If the driver crashes locally: `cargo test --workspace -- --test-threads=1`.
- RAW corpus (`tests/raw-samples/files/`, ~88 MB, git-ignored): fetch with
  `scripts/fetch-raw-samples.ps1` (or `.sh`). Missing files make corpus tests skip.
- Windows release links the CRT statically (`.cargo/config.toml`).
- `panic = "unwind"` is required in every profile: `lf-raw` catches rawler panics and
  `lf-catalog` catches worker panics.

## Invariants to preserve

1. **Parity gate.** 6 sources × 38 cases in `tests/fixtures`; each render must match per
   channel: mean < 1/255, p99.9 < 4/255, max ≤ 16/255. `identity` must be bit-exact.
   `manifest.json` is a verbatim web-repo copy; thresholds are constants in `lf-fixtures`.
   Output sizes must match exactly. 8-bit inputs keep the 8-bit path; RAW has no golden.
2. **Headroom rule.** RAW stays unclamped f16 (above 1.0) until the develop pass; default
   profile `camera`, highlights `reconstruct`. `linear`/`clip` are opt-in. Never clamp
   early on the float path.
3. **Never modify a source photo.** Edits go to the catalogue and `<photo>.lensflow.json`
   sidecars; exports go to new files. Exports refuse paths that collide with catalogued
   photos or sidecars, even with overwrite.
4. **Catalogue/sidecar conflict:** newer `updated_at` wins. A sidecar must be strictly
   newer to replace the catalogue; adopting one restarts undo history. Unknown sidecar
   keys are preserved. Writes are atomic; invalid sidecars are never overwritten.
5. **One GPU device.** The app wraps eframe's device once (`Gpu::from_device`); preview,
   thumbnails, export and matching share it. Per-call wgpu error scopes make errors
   attributable; a lost device shows a banner and offers restart, never panics.
6. **No slow work on the UI thread.** `Jobs` has one serial worker per lane:
   `Account`, `Open`, `Io`, `Query`, `Long`. Late messages carry a generation/ticket and
   stale answers are dropped. Each catalogue job opens its own connection.
7. **Account/entitlement.** Gate checks at launch, every 4 h, before AI, and on demand.
   Offline trust lasts 7 days from `checked_at`; clock skew > 5 min future is expired.
   Definite answers clear the cache. Read-only allows local editing and disk export but
   not AI or preset sync. Unknown standings are read-only, never locked.
8. **Forward compatibility.** Higher catalogue schema → `NewerSchema`, refuse untouched.
   Unknown prefs fields are ignored; unknown sidecar keys round-trip.
9. **User-facing strings are constants** (`session.rs`, `app.rs`, `library/mod.rs`, ...),
   and errors carry ready-to-show messages.

## App architecture (see `docs/app.md` for detail)

- `session.rs` is a pure, window-free state machine: `SigningIn → ChoosingStudio →
  Active | UpdateRequired | Locked | NoPermission`; a 401 anywhere returns to sign-in.
  `session_flow.rs` runs calls on the `Account` lane and owns generations.
- `library/` owns catalogue root, folders, grid, filmstrip, selection; `develop/` owns the
  open photo, panels, crop and spots tools, preview texture, history/sidecar debounce.
- The grid's marquee box lives in `library/grid.rs` (`Marquee`, `cells_in`, `follow_marquee`)
  with its state in `Library::marquee`; it is followed before the cells paint so the
  highlight is in step with the pointer, and a drag that starts in the empty space draws one
  too (the backdrop is made before the cells, so the cells stay on top).
- `grade_cli.rs` is the console's "Match a reference…": it runs from `cli::answer` before any
  window, opens its own device (`gpu::headless`), fits with `match_reference`'s own steps and
  writes sidecars (or `--out` JPEG copies). It never touches a photo and never replaces a
  damaged sidecar.
- `develop::preview` renders straight into an `Rgba8Unorm` canvas registered with
  egui-wgpu; no readback while editing. Histogram is throttled to 200 ms.
- The ribbon (`menu.rs`: tabs, groups, large buttons) and keys are the same
  `shortcuts::Action`s; `MAP` is read once per frame.
- `on_exit` order: commit gesture → stop export/batch/import/match → lanes drain (≤10 s)
  → clean export temps → write sidecars → save prefs → release instance lock → hand off
  installer/restart.

## File locations

`ProjectDirs::from("in", "lensflow", "lensflow-ImageEditor")`:

- Config: `prefs.json`, `instance.lock`, `window.ron`, `entitlement.json`
  (`entitlement-dev.json` for dev).
- Data: `logs/app.log` (5 MB, 3 rotated).
- Cache: `frames/` (LFF1, 4 GB default), `updates/`.
- Catalogue: `<root>/.lensflow/catalog.sqlite`; sidecars beside photos.
- Token: OS keychain, else an encrypted `token-<hash>` + `token-salt` in config.

## Current state and roadmap

- Shipped in 0.1.1: the DirectX 12 + OpenGL backend pair on Windows, the preview gutter.
- Since 0.1.1 (unreleased): the console's `grade` (`crates/lf-app/src/grade_cli.rs`) and the
  grid's marquee multi-select (`library/grid.rs`, `library/selection.rs`), and culling:
  flags, stars and colour labels (`lf_catalog::cull`, `library/cull.rs`, `library/cull_ui.rs`).
  Lens corrections: distortion, lens vignetting, CA, keystone, desktop-only in `desktop.lens`
  (`lf_settings::lens`, `lens_to_source` in `develop.wgsl`, `SpotMap::through_lens`).
  Masks: linear and radial local adjustments, desktop-only in `desktop.masks`
  (`lf_settings::masks`, `develop/tools/masks.rs`, `mask_weight` in `develop.wgsl`).
  Export finishing: sharpening, watermark, sRGB profile, WebP, EXIF in PNG/WebP
  (`lf_catalog::finish`). White balance eyedropper, Auto WB and Auto tone (`develop/auto.rs`, `ToolKind::WhiteBalance`,
  `lf_settings::shader_values::{wb_gains, neutral_wb}`).
- Part 2d plan: `docs/plans/2026-10-26-packaging-and-release.md`; progress ledger in
  `.superpowers/sdd/2026-10-26-packaging-and-release/progress.md`.
- Merged: Task 1 app release plumbing (PR #59), Task 2 packaging inputs (PR #58).
- In flight: Task 3 `.github/workflows/build.yml` + release scripts. Not yet present:
  `build.yml`, `scripts/release/{set-version,check-runtime-libs,smoke}.*`, `docs/release.md`.
- Tasks 4 (publish job) and 5 (runbook) pending; CI today is `.github/workflows/ci.yml`.
- `.superpowers/` holds SDD briefs/reports/diffs; it is process documentation, not app code.
- `target/`, `tests/raw-samples/files/` are git-ignored.

## Conventions

- Extensive `//!` module docs and `///` item docs explaining *why*; follow that style.
- `thiserror` with human-readable `#[error(...)]` messages; avoid `unwrap` on I/O paths.
- Keep platform-specific code behind `cfg`; Linux folder pickers use the XDG portal.
- Tests live in each crate's `tests/` and as `#[cfg(test)]` unit tests; use fakes/injected
  renderers (see `lf-grade`) rather than real network or GPU where possible.
- Do not commit secrets; signing material and tokens are env/CI-only.
- License is Apache-2.0; new dependencies must be compatible (see `NOTICE`).

## Known gaps / gotchas

- HEIC/AVIF unsupported; rawler 0.8 gaps: Nikon HE/HE* NEF, DNG 1.7 JPEG XL, some
  Sony ARW / Phase One IIQ.
- TIFF exports carry no EXIF (the image crate's TIFF encoder cannot add it); WebP export is
  lossless only. Every export is sRGB with an embedded profile; no other output space yet.
- Float (RAW) intermediate renders are not bit-identical to 8-bit; five sharpen/detail
  cases exceed the parity gate — intentional, the gate covers the 8-bit path.
- The OpenGL backend is the app's hardware path on Windows, and it is not bit-identical to
  DirectX 12 (measured on an NVIDIA Kepler driver, `WGPU_BACKEND=dx12,gl`): nine
  sharpen/detail/blur cases drift past the p99.9 gate — `landscape__sharpen-min` reaches
  max 14/255 — while `identity` and the spots cases stay exact. The gate is enforced with
  the default adapter (`lf-pipeline/tests/parity.rs`), not on every backend.
- `tests/fixtures` must be replaced wholesale, never merged.
- Windows software adapter (WARP) crashes if a process opens several GPU devices.
