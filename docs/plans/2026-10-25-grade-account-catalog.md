# lensflow-ImageEditor, Part 2c-i: Grade, Account and Catalogue Libraries

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The three libraries the desktop app is built from: `lf-grade` (match a reference, a port of the web's maths), `lf-account` (LensFlow sign-in, entitlement, AI calls, presets, update checks) and `lf-catalog` (SQLite catalogue, sidecars, import, thumbnails, history, versions, export to disk), plus the pipeline hardening the app needs (shared device, error surfacing, banded readback).

**Architecture:** Pure libraries with no UI. `lf-grade` depends on `lf-settings` and takes a render callback, exactly like the web's `calibrate()`. `lf-account` is a blocking `reqwest` client (the app calls it from worker threads) with the token in the OS keychain and a 7-day entitlement grace. `lf-catalog` owns a `rusqlite` database per catalogue folder and writes a sidecar `<file>.lensflow.json` beside every edited photo; thumbnails come from `lf-raw` (fast demosaic) and `lf-pipeline::Renderer::pixels`. Part 2c-ii builds the egui app on these.

**Tech Stack:** `reqwest` 0.13 (blocking, rustls), `keyring` 4, `rusqlite` 0.41 (bundled), `directories` 6, `sha1`/`sha2`, `serde_json`, `image` (JPEG/PNG/TIFF encode), `rayon`, `wgpu` 30.

**Spec:** `D:\pet-projects\photo-studio-management\docs\superpowers\specs\2026-10-25-lensflow-image-editor-design.md` (Architecture → Match a reference, Catalogue and workflow, Account; Decisions → Export to a gallery (not in this part), Permissions and standings; Owner's answers → paused locks the app). Server contracts live in the monorepo on `origin/main`: `backend/app/Http/Controllers/Desktop/{EntitlementController,DownloadController,GradeController}.php`, `backend/app/Http/Requests/GradeRequest.php`, `Auth/SessionController.php`, `Galleries/PhotoEditPresetController.php`, `config/tenancy.php` (`X-Studio` header carries the studio slug).

Depends on Part 2b (`lf-raw`) for Tasks 5–7 only; Tasks 1–4 can start as soon as Part 2a is on `main`.

## Global Constraints

- Repository `D:\pet-projects\lensflow-image-editor`, `main`; each task on `wip/2c-task-N`, landed by squash (locally or via `gh pr merge --squash`), wip branch deleted, no force-pushes. `export PATH="$HOME/.cargo/bin:$PATH"`.
- Fmt, clippy `-D warnings`, `cargo test --workspace` green locally and in CI; commit trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; no GPL crates; NOTICE updated for each new dependency.
- HTTP: base URL configurable (`https://lensflow.in/api` default, `https://dev.lensflow.in/api` when the hidden setting says so); every request carries `Authorization: Bearer <token>`, `X-Studio: <slug>` once a studio is chosen, `X-LensFlow-Client: desktop`, `Accept: application/json`, and `User-Agent: lensflow-ImageEditor/<version> (<os>)`. Timeouts: 20 s connect, 120 s for grade and uploads.
- Server responses the client must understand: `401` (token dead → sign-in), `403` with `code` `studio_suspended` or `desktop_editor_disabled` (lock), `402` with `code` `subscription_paused` (lock, paused message) or `subscription_required` (read-only), `422` with `code` `plan_limit` (AI allowance used up), `429` (slow down). The two-factor step: `POST auth/login` answers `{two_factor_required: true}` until `two_factor_code` is sent.
- Standing semantics (spec): `active` full use; `read_only` local editing allowed, gallery export, preset sync and AI blocked; `paused` and `suspended` and `enabled=false` lock the app. Offline grace: a cached successful entitlement is trusted for 7 days from `checked_at`, then the app locks until a check succeeds.
- Token storage: OS keychain through `keyring` (service `lensflow-ImageEditor`, account = the API base URL); on platforms where no keychain is available fall back to a file under the app config dir with 0600 permissions and say so in logs. Never log the token.
- Sidecar: `<photo path>.lensflow.json` = `{ "v": 2, "settings": <EditSettings JSON>, "versions": [{ "id", "label", "settings", "created_at" }], "updated_at": <ISO-8601 UTC> }`, written atomically (temp file + rename), debounced by the app (the library exposes `write_sidecar` only).
- Catalogue DB: one `catalog.sqlite` per catalogue root (user-chosen folder), WAL mode, schema version table; every write in a transaction; identifiers under 64 characters.
- Match a reference: constants from the web: `MEASURE_EDGE 512`, `AI_EDGE 1024`, `TILE_EDGE 256`, `SHEET_COLUMNS 4`, `SHEET_ROWS 6`, sample cap 24, `PERCENTILES [1,5,10,25,50,75,90,95,99]`, `LUMA_THRESHOLD 0.01`, `CHROMA_THRESHOLD 0.005`, 4 rounds, `MAX_CURVE_MOVE 0.3`, `WB_RATIO [0.85, 1.18]`, `SAT_RATIO [0.5, 2]`, `MAX_WHEEL_SAT 60`, exposure offset clamp ±1.5 EV, JPEG quality 0.85 for pictures sent to the AI, images ≤ 700 KB and ≤ 1024 px.

## Review Focus

1. A token that the server has revoked (401 mid-session) must send the user to sign-in without crashing a running export; `lf-account` returns a typed `Unauthorized` error and clears the cached token. Pinned in Task 3.
2. An entitlement check that fails from a network error must not lock an app that was fine an hour ago; a stale-but-within-grace cache keeps the app usable and the UI shows "last checked". Pinned in Task 3.
3. A sidecar written by a newer desktop version (unknown keys, `v` 3) must still load the settings it understands and must not be destroyed on save (unknown top-level keys preserved). Pinned in Task 5.
4. Importing a folder containing 10,000 files must not block: import is incremental and resumable, thumbnails are generated by a worker pool with priority, and a crash mid-import leaves the DB consistent. Pinned in Task 6 (crash-safety test with a transaction per batch).
5. A JPEG export must carry the original EXIF (as the web's `insertExif`) and the rendered size must equal the pipeline's `output_size`; a TIFF export at 16-bit must come from the float path, not from the 8-bit preview. Pinned in Task 7.

---

### Task 1: Pipeline hardening for the app

**Files:**
- Modify: `crates/lf-pipeline/src/gpu.rs` (`Gpu::from_device`, `Arc` friendliness, error handler, device-lost flag, `is_software()`), `error.rs` (`DeviceLost`, `Gpu(String)`), `texture.rs` (banded readback under `max_buffer_size`), `renderer.rs` (`Renderer` owns `Arc<Gpu>`; `is_lost()`; `trim()` drops large cached targets; `histogram` uses its own `hist` target; persistent `UniformArena` reused across draws), `passes/mod.rs`
- Test: `crates/lf-pipeline/tests/{gpu,preview,geometry_more}.rs`

**Interfaces:**
- `Gpu::from_device(adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Arc<Gpu>` (for sharing egui-wgpu's device); `Gpu::new() -> Result<Arc<Gpu>, _>`; `Gpu::is_software(&self) -> bool` (adapter `DeviceType::Cpu`); `Gpu::is_lost(&self) -> bool` (set by the device-lost callback); uncaptured errors are recorded (last message) and surfaced as `PipelineError::Gpu(String)` by the next `render`/`draw`.
- `Renderer::new(gpu: Arc<Gpu>, ..)` and `from_texture(gpu: Arc<Gpu>, ..)`; `Renderer: Send`.
- `read_back` reads in horizontal bands so no single buffer exceeds `limits.max_buffer_size`; sizes computed in u64.
- `Renderer::trim(&mut self)` drops `output`, `thumb`, `hist` targets.

- [ ] **Step 1: Tests**: `gpu_from_device_shares_handles` (a `Gpu::new()` device passed through `from_device` renders identity exactly); `device_lost_is_reported_not_panicked` (hard to force; test that `is_lost()` is false and that a forced uncaptured error via an invalid operation under an error scope becomes `PipelineError::Gpu`); `banded_readback_matches_single_buffer` (read back a 3000×3000 target with the band size forced small via a test hook and compare to the normal path); `view_quarter_matches_crop` (a draw with view `(0.5, 0.5, 0.5, 0.5)` equals the bottom-right quarter of the full render, within 1/255); `crop_override_sizes_output` (`crop: Some(whole)` on a cropped case gives the full turned frame size); `r180_and_turn_settings_round_trip` (output_to_source for R180 maps (0,0)→(1,1); four clockwise `turn_settings` return to the start; one clockwise then one anticlockwise is identity).
- [ ] **Step 2: Implement; parity suite unchanged; land: "lf-pipeline: shared device, error surfacing, banded readback, geometry tests".**

---

### Task 2: `lf-grade`: measure, correct, calibrate, contact sheet

**Files:**
- Create: `crates/lf-grade/Cargo.toml`, `src/lib.rs`, `src/measure.rs`, `src/correct.rs`, `src/calibrate.rs`, `src/sheet.rs`, `src/strength.rs`
- Test: `crates/lf-grade/tests/{match,calibrate,sheet}.rs` (ports of the web's `match.spec.ts`, `calibrate.spec.ts`, `contactSheet.spec.ts`: same test names, same expectations; the web's `pipeline.testutil.ts` `picture()` and `develop()` helpers are ported into `tests/support.rs` so the tests run without a GPU)

**Interfaces:**
- `measure(pixels: &[u8] /* RGBA */, sample_every: usize) -> Stats` (fields as the web's `Stats`/`BandStats`).
- `white_balance_from_gains([f32;3]) -> (temperature, tint)`; `wheel_from_offset([f32;3]) -> GradeWheel`.
- `correct(current: &EditSettings, rendered: &Stats, reference: &Stats, anchors: &[f64], opts: &CorrectOptions) -> Correction { settings, residual: { luma, chroma } }`.
- `calibrate(opts: CalibrateOptions { start, reference, anchors, render: &mut dyn FnMut(&EditSettings) -> Stats, scene_bands, rounds }) -> Calibration { settings, rounds, residual }` with `preserve()` as the web.
- `apply_strength(grade: &EditSettings, strength: f64) -> EditSettings`; `exposure_offset(reference_median, photo_median) -> f64`.
- `sheet::pick_samples(items: &[T], is_edited: impl Fn(&T) -> bool, cap) -> (Vec<&T>, all_edited: bool)`; `sheet::tile_layout(count, tile_edge, columns) -> Layout { w, h, rows, rects }`; `sheet::tile(bitmaps: &[RgbaImage], tile_edge, columns) -> RgbaImage` (cover-fit each tile, as the web's canvas `drawImage`; use `image`'s `Triangle` filter); `sheet::scaled(img, long_edge) -> RgbaImage`; `sheet::jpeg(img, long_edge, quality 0.85) -> Vec<u8>` (must be ≤ 700 KB; lower quality in steps of 0.05 until it fits, never below 0.6; error otherwise).

- [ ] **Step 1: Port the three spec files as failing tests; run; implement; pass. Land: "lf-grade: match a reference maths, ported from the web".**

---

### Task 3: `lf-account`: client, sign-in, token store, entitlement

**Files:**
- Create: `crates/lf-account/Cargo.toml`, `src/lib.rs`, `src/client.rs`, `src/auth.rs`, `src/token_store.rs`, `src/entitlement.rs`, `src/error.rs`, `src/instance.rs`
- Test: `crates/lf-account/tests/{auth,entitlement}.rs` using a local mock HTTP server (`httpmock` or `wiremock` dev-dependency; pick one permissive)

**Interfaces:**
- `Instance { Prod, Dev, Custom(Url) }` with `base_url()`.
- `Client::new(instance, version: &str) -> Client` (blocking reqwest, rustls, headers from Global Constraints); `Client::with_token(&self, token)`, `with_studio(&self, slug)`.
- `AccountError`: `Network(String)`, `Unauthorized`, `Locked(LockReason /* Suspended | Paused | NotEnabled */ , message)`, `ReadOnly(message)`, `PlanLimit(message)`, `RateLimited`, `Server(status, message)`, `TwoFactorRequired`, `Validation(field → message)`.
- `auth::sign_in(client, email, password, two_factor_code: Option<&str>, device_name) -> Result<SignedIn { token, user: User { id, name, email, is_platform_admin, studios: Vec<StudioSummary { id, name, slug, role, permissions: Vec<String>, plan_features: Vec<String>, subscription_paused: bool }> } }, AccountError>` (`POST auth/login`); `auth::me(client) -> User` (`GET auth/me`); `auth::sign_out(client)` (`POST auth/logout`, ignores failures).
- `TokenStore::open(instance) -> TokenStore`; `load() -> Option<String>`; `save(&str)`; `clear()`; keyring-backed with the file fallback.
- `entitlement::check(client) -> Result<Entitlement { enabled, standing: Standing, ai: AiStatus { available, remaining: Option<u32> }, studio: { id, name, logo_url }, min_version: Option<String>, latest: Option<Release { version, notes, published_at }>, checked_at: SystemTime }, AccountError>` (`GET desktop/entitlement`; a 403 with `code: studio_suspended` becomes `Locked(Suspended, msg)`, `desktop_editor_disabled` → `Locked(NotEnabled, msg)`; 402 `subscription_paused` → `Locked(Paused, msg)`).
- `entitlement::Cache::load(dir) / save(&Entitlement)`; `entitlement::Gate::evaluate(now, cached: Option<&Entitlement>, fresh: Result<Entitlement, AccountError>) -> Decision { Allow(Entitlement), AllowStale(Entitlement, age), Lock(LockReason, message), SignIn }` implementing the 7-day grace and the standing rules; `Decision::ai_allowed()`, `export_allowed()`, `sync_allowed()` (false for `read_only`).
- `download_link(client, platform) -> Result<DownloadLink { url, sha256, version, size, expires_at }, _>` (`GET desktop/download/{platform}`).

- [ ] **Step 1: Tests (mock server)**: sign-in happy path with headers asserted (`X-LensFlow-Client: desktop`, `User-Agent`); two-factor round trip; 401 → `Unauthorized` and `TokenStore::clear` called by a helper `handle_unauthorized`; entitlement mapping for each code; `Gate`: fresh active → Allow; network error + cache 2 days old → AllowStale; network error + cache 8 days old → Lock(?) no: → `SignIn`-like lock with message "Connect to the internet to keep using lensflow-ImageEditor" (define `LockReason::Offline`); fresh paused → Lock(Paused, server message); `read_only` → Allow with `ai_allowed() == false`; token store round trip on this OS (skipped if keyring unavailable, file fallback tested).
- [ ] **Step 2: Implement; land: "lf-account: sign-in, token store, entitlement with offline grace".**

---

### Task 4: `lf-account`: AI grade call, presets, settings sync

**Files:**
- Modify: `crates/lf-account/src/lib.rs`; Create: `src/grade.rs`, `src/presets.rs`
- Test: `crates/lf-account/tests/{grade,presets}.rs` (mock server)

**Interfaces:**
- `grade::extract(client, mode: Mode /* Extract | Review */, reference_jpeg: &[u8], sheet_jpeg: &[u8], examples: &[Look], settings: Option<&EditSettings>) -> Result<Proposal { settings: serde_json::Value, notes, scene_hues: Vec<HslBand>, confidence: Map, requests_used: u32 }, AccountError>` (`POST desktop/grade`, multipart with `reference.jpg`/`sheet.jpg`, `examples` JSON of `{id, name, settings}`, `settings` JSON when reviewing); 422 `plan_limit` → `PlanLimit`; 402 `subscription_paused` → `Locked(Paused)`; `requests_used` taken from the response.
- `presets::list(client) -> Vec<Preset { id, name, settings: Value, updated_at }>`; `create(client, name, settings)`; `update(client, id, name, settings)`; `delete(client, id)` (`galleries/edit-presets`); validation errors mapped to `Validation`.
- `presets::Sync::merge(local: &[LocalPreset], remote: &[Preset]) -> Plan { push: Vec<LocalPreset>, pull: Vec<Preset>, conflicts: Vec<(LocalPreset, Preset)> }` by name, newest `updated_at` wins, conflicts (both changed since last sync) kept as `"<name> (desktop)"`.

- [ ] **Step 1: Tests**: multipart body fields asserted; `plan_limit` mapping; presets CRUD against the mock; merge plan cases (new local → push; new remote → pull; both changed → conflict renamed).
- [ ] **Step 2: Implement; land: "lf-account: desktop grade call and preset sync".**

---

### Task 5: `lf-catalog`: schema, sidecars, edits and versions

**Files:**
- Create: `crates/lf-catalog/Cargo.toml`, `src/lib.rs`, `src/db.rs` (schema + migrations), `src/sidecar.rs`, `src/edits.rs`, `src/model.rs`
- Test: `crates/lf-catalog/tests/{db,sidecar,edits}.rs`

**Interfaces:**
- `Catalog::open(root: &Path) -> Result<Catalog, CatalogError>` (creates `root/.lensflow/catalog.sqlite`, WAL, migrations table).
- Tables: `folders(id, path UNIQUE, added_at)`, `photos(id, folder_id, path UNIQUE, file_size, mtime, sha1 NULL, width, height, camera, lens, captured_at, kind, decoder, orientation, flag, rating, label, imported_at, updated_at)`, `edits(photo_id PK, settings JSON, updated_at)`, `history(id, photo_id, settings JSON, created_at)` (ring of 50 per photo), `versions(id, photo_id, label, settings JSON, created_at)`, `thumbnails(photo_id PK, edge, jpeg BLOB, made_for_updated_at)`, `presets(id, name UNIQUE, settings JSON, updated_at, remote_id NULL, synced_at NULL)`, `export_jobs(id, created_at, status, spec JSON, done, total, error NULL)`.
- `sidecar::path_for(photo) -> PathBuf` (`<file>.lensflow.json`); `sidecar::read(path) -> Option<Sidecar { v, settings: EditSettings (normalised), versions, updated_at, extra: Map /* unknown keys preserved */ }>`; `sidecar::write(path, &Sidecar)` atomic.
- `edits::set_settings(catalog, photo_id, &EditSettings)` writes `edits`, pushes `history` (capped 50), returns the sidecar to write; `edits::undo/redo` cursors over history; `versions::add(photo_id, label, &EditSettings)`, `list`, `use_version` (sets current settings), `delete`.
- Conflict rule on import/reopen: when the sidecar's `updated_at` is newer than the DB's, the sidecar wins and the DB is updated; else the DB wins and the sidecar is rewritten.

- [ ] **Step 1: Tests**: open creates the schema and reopening keeps data; sidecar round trip preserves unknown top-level keys and loads a `v: 3` file's known settings; atomic write leaves no temp file; history ring caps at 50 and undo/redo walk correctly; version add/use/delete; conflict rule both ways.
- [ ] **Step 2: Implement; land: "lf-catalog: schema, sidecars, edit history and versions".**

---

### Task 6: `lf-catalog`: import and thumbnails

**Files:**
- Create: `crates/lf-catalog/src/import.rs`, `src/thumbs.rs`
- Test: `crates/lf-catalog/tests/import.rs`

**Interfaces:**
- `import::scan(root) -> Vec<Candidate { path, size, mtime }>` (recursive, supported extensions only, skips hidden and `.lensflow`).
- `import::run(catalog, candidates, batch: usize = 200, progress: &mut dyn FnMut(Progress)) -> Result<Summary { added, updated, skipped }, _>`: per batch, one transaction; reads `lf_raw::open::sniff` and EXIF summary (not pixels) for width/height/camera/captured_at (falls back to a decode only when EXIF lacks dimensions); existing rows updated when size or mtime changed; resumable (re-running skips unchanged files).
- `thumbs::Thumbnailer::new(gpu: Arc<Gpu>, catalog, workers: usize)`; `request(photo_id, priority: Priority)`; `take_ready() -> Vec<(photo_id, RgbaImage)>`; produces 256 px (grid) JPEGs via `lf_raw::open` (`Quality::Fast`, `max_edge` 1024) + `Renderer::pixels(settings, 256)`; stores in `thumbnails`; a priority queue where on-screen requests jump the queue; cancellation on `drop`.

- [ ] **Step 1: Tests**: scan filters by extension and skips hidden; import of 1,000 generated small PNGs in batches (a fault injected after batch 2 leaves exactly 400 rows committed and a rerun completes without duplicates); unchanged files skipped on rerun; thumbnailer produces a 256 px JPEG for a PNG and respects priority order (high before normal) using a single worker.
- [ ] **Step 2: Implement; land: "lf-catalog: incremental import and prioritised thumbnails".**

---

### Task 7: `lf-catalog`: export to disk

**Files:**
- Create: `crates/lf-catalog/src/export.rs`, `src/exif_embed.rs`
- Test: `crates/lf-catalog/tests/export.rs`

**Interfaces:**
- `ExportSpec { format: Jpeg { quality: u8 } | Png | Tiff16, size: Original | LongEdge(u32) | ShortEdge(u32) | Megapixels(f32) | Percent(f32), destination: Folder(PathBuf), rename: Pattern /* tokens {name} {seq} {date} {camera} */ , metadata: All | CopyrightOnly | None, overwrite: bool }`.
- `export::run(gpu: Arc<Gpu>, catalog, photo_ids, spec, progress) -> Result<Vec<Exported { photo_id, path, width, height }>, _>`: opens the photo via `lf_raw::open` (`Quality::Best`, develop settings from the photo's `desktop.develop`), `Renderer::from_texture`, `render` (8-bit) for JPEG/PNG or the float path read back as f16 → 16-bit for `Tiff16`, resizes per `size` with a high-quality filter, embeds the original JPEG's EXIF (port of the web's `insertExif`: copy APP1 from the source JPEG) when `metadata != None` and the source was JPEG; for RAW sources writes a minimal EXIF (make, model, lens, exposure, ISO, date, orientation 1) from the `ExifSummary`; never overwrites unless `overwrite`.

- [ ] **Step 1: Tests**: JPEG export of a PNG source has the pipeline's `output_size`; a JPEG source's APP1 is carried over byte for byte; `Tiff16` from a synthetic DNG with an above-1 patch keeps 16-bit precision (distinct values above the 8-bit grid) and clamps at 1.0 only at the final encode; rename pattern tokens; `overwrite: false` refuses an existing path with a named error.
- [ ] **Step 2: Implement; land: "lf-catalog: export to disk with EXIF".**

---

### Task 8: Docs and CI

- README: new crates, the HTTP contract summary, the sidecar format, where the catalogue and caches live (`directories` project dirs), the entitlement rules in plain words. NOTICE for new deps. CI unchanged unless a test needs network (none should: mock server only). Land: "Docs: grade, account and catalogue libraries".
