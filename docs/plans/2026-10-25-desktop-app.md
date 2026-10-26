# lensflow-ImageEditor, Part 2c-ii: The Desktop Application (`lf-app`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Phase 1 application: a native egui window that signs in with LensFlow, enforces the entitlement, catalogues folders, shows a library grid and filmstrip, develops any supported photo (RAW with headroom) with every web control, matches a reference, keeps versions and history, syncs presets, and exports to disk. Nothing in Phase 2 (masks, heal brush beyond the web's spots, lens, upright, denoise models) or Phase 3 (culling, compare, filters, batch rename, export presets, watermark, export to gallery) is built here.

**Architecture:** One binary crate `crates/lf-app` (`lensflow-image-editor`) on `eframe` 0.37 with the `wgpu` backend; at start it wraps egui's device once with `Gpu::from_device` so previews, thumbnails and exports share the device. The UI thread owns the current photo's `Renderer` and draws the preview straight into a wgpu texture that egui shows (no readback on the interactive path). Everything slow runs on named worker threads (`account`, `open`, `io`) plus `lf-catalog`'s thumbnail pool; workers answer through one `mpsc` channel of `Msg` values that `update()` drains every frame and wake the UI with `Context::request_repaint`. Session state is a pure state machine fed by `lf_account::Decision`, so screens follow from data and the machine is unit-tested without a window.

**Tech Stack:** `eframe`/`egui`/`egui-wgpu`/`egui_extras` 0.37 (wgpu ^30, matches the workspace's `wgpu` 30), `egui_kittest` 0.37 (dev, no `wgpu`/`snapshot` features), `rfd` 0.17 (file dialogs; GTK on Linux, licence MIT), the system clipboard through egui's own `ctx.copy_text` (text is all that is copied), `semver` 1, `directories` 6, `env_logger` + `log` to a file, `serde`/`serde_json`, `image` 0.25 (PNG/JPEG decode for the logo and thumbnails), the workspace crates `lf-settings`, `lf-pipeline`, `lf-raw`, `lf-grade`, `lf-account`, `lf-catalog`.

**Spec:** `D:\pet-projects\photo-studio-management\docs\superpowers\specs\2026-10-25-lensflow-image-editor-design.md` (Goals 1, 2, 5; Architecture → Catalogue and workflow, Account, Security; Phases → 1; Decisions → RAW headroom, Permissions and standings; Owner's answers → paused locks the app; What's left to the builder → egui layout, keyboard map following Lightroom). The web editor is the UI reference: `frontend/src/modules/galleries/editor/{PhotoEditor.vue, state.ts, components/AdjustPanel.vue, CropPanel.vue, CurveEditor.vue, MatchReferencePanel.vue, PresetsPanel.vue, GroupPicker.vue}`, `BatchEditDialog.vue`, `VersionsPanel.vue` on the monorepo's `origin/main` (implementers read these with `git show origin/main:<path>` from `D:\pet-projects\photo-studio-management`; never edit the monorepo).

Depends on Part 2c-i and its final fix wave (`wip/2c-fix-wave`: sidecars reconciled on import, per-call GPU error scopes, fresh stored thumbnails, `ExportReport` with cancel, `Standing::Unknown`, `gallery_export_allowed`, `import::mark_missing`, `StudioSummary::can_use_desktop`, `Thumbnailer::cancel_pending`) and on the Part 2b fix wave (landed as 55759b5: `Quality::Preview`, `open` hardening). Tasks 1–2 need neither; Tasks 3–9 need the 2c-i fix wave on `main`.

## Global Constraints

- Repository `D:\pet-projects\lensflow-image-editor`, `main`; each task in a sibling worktree on `wip/app-task-N`, landed with `gh pr merge --squash --delete-branch` after CI is green; no force-pushes; the main clone is not an agent workspace. `export PATH="$HOME/.cargo/bin:$PATH"` before every cargo command.
- Fmt, clippy `-D warnings`, `cargo test --workspace` green locally and in CI; commit trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; PR bodies end with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`; no GPL crates (`cargo deny`-style check by reading each new crate's licence; MPL-2.0 and LGPL static linking are allowed); NOTICE updated for each new dependency.
- The desktop never rewrites a source file. Edits go through `lf_catalog::edits` (database) and sidecars; output goes to new files via `lf_catalog::export`.
- Parity: the preview, thumbnails and exports render only through `lf_pipeline::Renderer` with settings from `lf_settings`. No colour maths in `lf-app`.
- GPU: exactly one `Gpu::from_device(adapter, device, queue)` call, after `eframe` creates its `RenderState`; never a second `Gpu::new`; the `Arc<Gpu>` lives as long as the app. eframe's device descriptor requests `adapter.limits()` (the default 8192 px texture limit would refuse 45–100 MP photos). Each `Renderer` call reports its own errors through per-call error scopes (library); the app logs `gpu.take_error()` each frame only as a tap for uncaptured errors. `gpu.is_lost()` true shows a banner and drops the renderer (no panic). On `gpu.is_software()` a banner says: "No graphics driver found: previews will be slow."
- Account: `lf_account::Client::new(instance, env!("CARGO_PKG_VERSION"))`; the token from `TokenStore::open(&instance)`; the entitlement cache at `<config dir>/entitlement.json` (`Cache::new(config_dir)`); `Gate::refresh` at launch, every 4 hours, and before any AI call or export; `AllowStale(e, age)` shows "Last checked N hours ago" in the status bar. Never log the token or the password.
- Standing and permission screens (exact copy): paused → "Your studio is paused. Resume your LensFlow plan to use lensflow-ImageEditor."; suspended → "Your studio is suspended. Contact LensFlow support to continue."; not enabled → "Your studio does not have lensflow-ImageEditor yet. Ask LensFlow support to enable it."; no `galleries.manage` permission (from `auth/me`'s `permissions` for the chosen studio, and a `Locked(Refused)` answer from `desktop/entitlement` for such a studio) → "Your studio's owner can give you access to lensflow-ImageEditor. Ask for the 'manage galleries' permission."; offline past grace → `lf-account`'s own message; read-only → a banner "Your LensFlow plan has lapsed: editing works, but preset sync and AI are off until you renew." Local editing and export to disk stay available in read-only (spec: only gallery export, preset sync and AI are blocked).
- Files and folders: config dir and data dir from `directories::ProjectDirs::from("in", "lensflow", "lensflow-ImageEditor")`; `prefs.json` in the config dir; `logs/app.log` in the data dir (rotate at 5 MB, keep 3); the frame cache at `<cache dir>/frames` with a 4 GB default cap; the catalogue at `<catalogue root>/.lensflow/catalog.sqlite` (`Catalog::open(root.join(".lensflow"))`), the root chosen by the user on first run and remembered in prefs.
- Colours and type: the web's dark palette: background `#0f0f12`, surface `#18181c`, surface-2 `#222228`, line `#2e2e36`, ink `#f4f4f6`, muted `#a0a0ab`, brand `#6d28d9` (brand-contrast `#ffffff`), danger `#f87171`, success `#4ade80`, warning `#fbbf24`. The logo is `assets/logo.png` rasterised from the monorepo's `frontend/public/logo.svg` (also the window icon). Default egui fonts.
- Copy: plain words, no jargon, sentences end with a full stop; the web app's tone. Error toasts carry the library's message verbatim after the first sentence of context.
- Keyboard map (Lightroom defaults where they exist): `G` library, `D` develop, `Ctrl+Shift+E` export, `R` crop tool, `Q` spot tool, `\` before/after (held or toggled), `Ctrl+Z` undo, `Ctrl+Shift+Z` and `Ctrl+Y` redo, `Ctrl+Shift+C` copy settings (opens the group picker), `Ctrl+Shift+V` paste, `Ctrl+A` select all, `←`/`→` previous/next photo, `Space` toggles fit and 100 %, `Esc` leaves the current tool, `Delete` removes the selected spot, `F1` about. On macOS `Ctrl` reads as `Cmd`.
- Tests for UI code use `egui_kittest::Harness` (layout and accesskit queries, no rendering) and pure functions; nothing in a test opens a window. GPU tests skip without an adapter, as the rest of the workspace does.

## Review Focus

1. Dragging a slider on a 60 MP RAW must stay responsive: the interactive path draws at viewport size through `Renderer::draw` with no readback, the histogram is throttled (at most every 200 ms after the last change), and the sidecar write is debounced. Pinned in Task 4 (throttle test) and Task 5 (debouncer test; no `render`/`pixels` call in the drag path).
2. A token revoked mid-session (`Unauthorized` from any account call) must send the user to sign-in without aborting a running export or losing unsaved edits (edits are already in the database). Pinned in Task 2 (state-machine test) and Task 9 (the export worker does not hold the session).
3. The graphics device being lost (driver reset, sleep) must show "The graphics device was reset. Restart lensflow-ImageEditor to continue." and offer a restart, never panic. Pinned in Task 4 (copy changed in the final fix wave: a lost device does not come back until the app restarts).
4. Quitting with a pending sidecar write or a running import must leave files consistent: the debouncer flushes in `on_exit`, imports commit per batch (library guarantee). Pinned in Task 5 (flush test).
5. Two selected photos with the same file stem exported to one folder must stop before writing anything and name both files in the message (the library returns `NameCollision`; the dialog shows it). Pinned in Task 9.

---

### Task 1: Crate, window, shared GPU, prefs, logging, jobs

**Files:**
- Create: `crates/lf-app/Cargo.toml` (`[[bin]] name = "lensflow-image-editor"`), `src/main.rs`, `src/app.rs` (the `eframe::App`), `src/gpu.rs`, `src/prefs.rs`, `src/logging.rs`, `src/jobs.rs`, `src/theme.rs`, `src/toast.rs`, `src/version.rs`, `assets/logo.png` (512 px, from the monorepo's `frontend/public/logo.svg`; `assets/README.md` says where it came from)
- Modify: `Cargo.toml` (workspace member; `[profile.release] lto = "fat"`, `codegen-units = 1`, `strip = true`; `panic` stays `unwind`, the catalogue's workers rely on it), `NOTICE`
- Test: `crates/lf-app/src/{prefs,jobs,version,theme}.rs` unit tests

**Interfaces:**
- `Prefs { instance: InstanceChoice (Prod|Dev), catalogue_root: Option<PathBuf>, studio_slug: Option<String>, frame_cache_bytes: u64 (default 4 << 30), last_export: Option<serde_json::Value>, develop_quality: Quality-like enum (Best default) }` with `Prefs::load(dir) -> Prefs` (missing or corrupt → defaults, corrupt logged) and `save(&self, dir) -> io::Result<()>` (atomic write).
- `Jobs::new(ctx: egui::Context) -> Jobs`; `jobs.spawn(Lane::{Account, Open, Io}, name: &str, f: impl FnOnce() -> Msg + Send + 'static)`; `jobs.drain() -> Vec<Msg>` (non-blocking); one serial worker thread per lane, named `lf-<lane>`; a panic in a job becomes `Msg::JobFailed { name, message }` instead of killing the lane; each job's closure gets nothing but what it captures.
- `Msg` is one enum in `jobs.rs` extended by later tasks (variants named in each task).
- `gpu::attach(frame: &eframe::Frame) -> Result<Arc<Gpu>, String>`: `Gpu::from_device((*rs.adapter).clone(), (*rs.device).clone(), (*rs.queue).clone())` from `frame.wgpu_render_state()`; called exactly once from `App::new`. `NativeOptions.wgpu_options` uses `WgpuSetup::CreateNew` with a `device_descriptor` closure requesting `required_limits: adapter.limits()`; the start-up log line includes `max_texture_dimension_2d`.
- `version::newer(latest: &str, current: &str) -> bool` (semver; a non-semver string is never newer; a `v` prefix is accepted).
- `theme::apply(ctx)` sets the palette above; `toast::Toasts` (push with a level, auto-hide after 6 s, errors stay until closed; drawn bottom-right).
- `logging::init(data_dir) -> Result<PathBuf, String>` (file + stderr in debug; `RUST_LOG` honoured); logs the version, OS, adapter name and `is_software()` at start.

- [ ] **Step 1: Failing tests**: `prefs_round_trip_and_defaults_on_corrupt_file`; `jobs_delivers_results_in_order_per_lane_and_survives_a_panic` (two jobs on `Io`, the first panics: drain yields `JobFailed` then the second's `Msg`); `newer_compares_semver` (`"1.2.0" > "1.1.9"`, `"1.10.0" > "1.9.0"`, `"v1.0.1" > "1.0.0"`, `"garbage"` never newer, equal not newer); `theme_palette_matches_web` (the six main colours' hex).
- [ ] **Step 2: Implement.** `main.rs`: `eframe::run_native` with `NativeOptions { renderer: Renderer::Wgpu, viewport: with_inner_size(1400×900).with_min_inner_size(1024×640).with_icon(logo), persistence_path: Some(config_dir/"window.ron") (eframe's `persistence` feature keeps only window geometry) }`; `App::new` attaches the GPU (an error → a window with the message and a Quit button), loads prefs, starts jobs, applies the theme; `update()` drains jobs, reads `gpu.take_error()` into the log, shows the software and device-lost banners, and draws a placeholder central panel "Sign in to begin." (Task 2 replaces it).
- [ ] **Step 3: Run `cargo run -p lf-app` once locally and paste the first 10 log lines (adapter name, software flag) into the report. `cargo test -p lf-app` green; land: "lf-app: window, shared GPU, prefs, logging, jobs".**

---

### Task 2: Session: sign-in, studio choice, entitlement gate, locks, updates

**Files:**
- Create: `src/session.rs` (pure state machine), `src/screens/{mod,signin,studio,locked,update}.rs`, `src/account.rs` (the worker-side calls: sign in, me, refresh, download), `src/update.rs`
- Modify: `src/app.rs`, `src/jobs.rs` (`Msg::SignedIn(Result<SignedIn, AccountError>)`, `Msg::Me(Result<User, AccountError>)`, `Msg::Gate(Decision)`, `Msg::Download(DownloadProgress)`), `src/prefs.rs`
- Test: `src/session.rs` unit tests, `tests/session_screens.rs` (kittest)

**Interfaces:**
- `Session` enum: `SigningIn { email, password, code, two_factor: bool, busy: bool, error: Option<String> }`, `ChoosingStudio { user: User }`, `NoPermission { studio: String }`, `Locked { reason: LockReason, message: String }`, `UpdateRequired { latest: Option<Release> }`, `Active { studio: StudioSummary, entitlement: Entitlement, stale: Option<Duration>, read_only: bool, update: Option<Release> }`.
- `Session::on_decision(self, d: Decision, studio: &StudioSummary, now) -> Session` (pure): `Allow` → `Active` (read-only when `standing` is `ReadOnly` or `Unknown`; `update` when `version::newer(latest, current)`; `UpdateRequired` when `min_version` is newer than the current version); `AllowStale(e, age)` → `Active { stale: Some(age), .. }`; `Lock(reason, msg)` → `Locked`, except `Refused` for a studio where `!studio.can_use_desktop()` → `NoPermission`; `SignIn` → `SigningIn` (token cleared); `ChooseStudio` → `ChoosingStudio`. A studio without `can_use_desktop()` goes to `NoPermission` straight from the chooser, before any entitlement call. Feature gates read `decision.ai_allowed()`, `sync_allowed()` and `gallery_export_allowed()` (disk export is never gated).
- `Session::on_unauthorized(self) -> Session` (any later `Unauthorized` → `SigningIn` with the message "You were signed out. Sign in again to continue."); clears the token store and the entitlement cache (`lf_account::handle_unauthorized`).
- Studio choice: one studio → chosen at once; several → the chooser (name, role); remembered in `prefs.studio_slug`; "Switch studio" and "Sign out" in the account menu (sign-out calls `sign_out`, clears the token, the cache and the prefs slug).
- Gate schedule: `GateClock::due(last: Option<Instant>, now) -> bool` (4 h); `App` also calls `refresh_now(reason)` before an AI call or export and uses the `Msg::Gate` answer before proceeding.
- Update: `update::run(client, platform: &'static str, dest_dir) -> Result<PathBuf, String>` on the `Io` lane: `download_link`, stream to `dest_dir/<file name from url>.part` reporting `Msg::Download(Progress { done, total })`, verify SHA-256 against `link.sha256` (mismatch → delete, error "The download did not verify. Try again."), rename, then `update::launch(path)` (Windows: `cmd /C start "" <path>`; macOS: `open <path>`; Linux: `xdg-open <path>` and, for an AppImage, `chmod +x` then execute) and `ctx.send_viewport_cmd(Close)`. `platform` is `windows-x64`, `macos-universal`, `linux-x64` as the server's `DownloadController` names them (read it on `origin/main`).
- Hidden instance switch: `Ctrl+Shift+D` on the sign-in screen toggles Prod/Dev and shows "Using the development server." under the logo.

- [ ] **Step 1: Failing tests** (pure, no network): `allow_active_goes_to_active`; `allow_read_only_sets_banner_and_keeps_editing`; `stale_keeps_age`; `lock_paused_uses_owner_copy` (exact string); `refused_without_permission_is_no_permission`; `sign_in_decision_clears_token`; `min_version_newer_blocks`; `latest_newer_offers_update`; `unauthorized_mid_session_returns_to_sign_in_with_message`; `gate_due_after_four_hours`. Kittest: `sign_in_screen_has_email_password_and_button` (labels "Email", "Password", button "Sign in"); `two_factor_step_shows_code_field` ("Code from your authenticator app"); `locked_paused_screen_shows_message`. Update: `sha_mismatch_is_rejected_and_file_removed` (httpmock serving bytes with a wrong hash).
- [ ] **Step 2: Implement** screens with the logo top-centre, a 360 px card, enter submits, errors in danger colour under the form; the `account` lane runs `sign_in` → `me` → `Gate::refresh`; the status bar (bottom, 24 px) shows studio name, standing, "Last checked …", and the update offer "Version X is ready — Update" when present.
- [ ] **Step 3: Manual check against `https://dev.lensflow.in/api` is the owner's; paste the state-machine test output. Land: "lf-app: sign-in, studio choice, entitlement gate, locks, updates".**

---

### Task 3: Catalogue root, import, library grid, filmstrip, selection

**Files:**
- Create: `src/library/{mod,grid,filmstrip,selection,import_ui}.rs`, `src/catalog_ext.rs` (thin wrappers), `src/thumbs_ui.rs`
- Modify: `crates/lf-catalog/src/db.rs` (`Folder`, `PhotoRow`, `Catalog::folders()`, `Catalog::photos_in(folder_id) -> Vec<PhotoRow>` ordered by `captured_at, path`, `Catalog::photo(id) -> Option<PhotoRow>`, `Catalog::remove_folder(id)`; migration 5: `photos.sidecar_error TEXT` so sidecar problems no longer share `photos.error` with file and thumbnail problems — `import` and `edits::reconcile` write `sidecar_error`, thumbnails write `error`, and a thumbnail success clears only `error`), `crates/lf-catalog/src/thumbs.rs` (the freshness key includes the demosaic quality, `|q:{quality:?}`), `crates/lf-catalog/src/edits.rs` (an adopted sidecar `updated_at` is truncated to milliseconds, `parse_ts(ts(at))`, and the sidecar rewritten when that changed it, so a third-party stamp cannot win every reopen), `crates/lf-catalog/src/lib.rs` (re-exports), `src/app.rs`, `src/jobs.rs` (`Msg::ImportProgress(Progress)`, `Msg::ImportDone(Result<Summary, String>)`, `Msg::Scanned(Vec<Candidate>)`)
- Test: `crates/lf-catalog/tests/queries.rs`, `src/library/{grid,selection}.rs` unit tests

**Interfaces:**
- `PhotoRow { id, folder_id, path: PathBuf, width: Option<u32>, height: Option<u32>, camera: Option<String>, lens: Option<String>, captured_at: Option<String>, kind: String, error: Option<String>, sidecar_error: Option<String>, missing: bool, edited: bool, updated_at: String }` (`edited` = a row in `edits`). `error` is a message to show, never a reason not to request a thumbnail; `sidecar_error` shows a small warning badge ("Edit file damaged") and is explained in Develop (Task 5).
- First run (no `prefs.catalogue_root`): a screen "Choose a folder for your catalogue. Photos stay where they are; the catalogue only remembers them." with a folder picker (`rfd::FileDialog::pick_folder` on the `Io` lane: on Linux GTK dialogs must not run on the UI thread with winit; on Windows/macOS they may, so always use the lane). Then `Catalog::open(root/.lensflow)`, `Thumbnailer::new(gpu, root/.lensflow, Thumbnailer::default_workers(), THUMB_QUALITY)` where `THUMB_QUALITY` is a `const` (`Quality::Preview` once the fix wave lands, `Fast` before), `FrameCache::open(cache_dir/frames, prefs.frame_cache_bytes)`.
- "Add folder…" (File menu and an empty-library button): pick → `import::scan` on `Io` → `Msg::Scanned` → `import::run(catalog, folder_id, &candidates, 200, progress)` then `import::mark_missing(catalog, folder_id, &candidates)` on `Io` with its own `Catalog` (`mark_missing` runs only when the scan actually reached the folder: `scan` reports whether the root was readable, and an unreadable or empty-because-unplugged root marks nothing); progress in the status bar ("Importing 240 of 1,200"); on done, refresh `photos_in` and request thumbnails. "Rescan" on a folder runs the same pair; a missing photo shows a "Missing" badge and cannot be opened. A root on a network path (UNC, or a drive whose type is remote on Windows; a mount under `/mnt`, `/media`, `/net` or `/Volumes` whose filesystem is `cifs`/`nfs`/`smbfs` elsewhere) shows a warning before use: "Catalogues on network drives can be damaged. Choose a folder on this computer." with "Choose another" and "Use it anyway".
- `grid::Layout::compute(viewport_w, scroll_y, viewport_h, count, cell: 200.0, gap: 8.0) -> Visible { first, last, columns, row_height, total_height }` (pure, virtualised). Visible cells request `Priority::High`, the rest `Normal` once each (the pool serves fresh stored thumbnails without decoding); when the visible range jumps by more than two screens, `cancel_pending()` first, then re-request the visible cells; `take_ready` results become egui textures (`ctx.load_texture(name, ColorImage, LINEAR)`) kept in an LRU of 600 textures; `take_failed` writes the cell's error badge ("Can't open") with the message on hover (an entry with id 0 means a worker could not start: one error toast); `take_device_error` (the pool stops itself on a device error) shows the Review Focus 3 banner once, and the next library action (folder change, rescan, scroll) builds a fresh `Thumbnailer` when `gpu.is_lost()` is false.
- `Selection { anchor, current: Vec<i64> }` with `click(id)`, `shift_click(ids_in_order, id)`, `ctrl_click(id)`, `arrow(dir, columns, order)`; `select_all(order)`; the active photo is `current.last()`.
- Filmstrip (shared by Library and Develop): one horizontal row of 96 px thumbs for the current folder, active photo outlined in brand colour, `←`/`→` move the active photo, click selects; wheel scrolls.
- Left sidebar: folders (name, count), "Remove from catalogue" on right-click (asks first; never deletes files; copy says so). Status bar: "1,200 photos · 3 selected".
- Double-click a cell or press `D` → Develop (Task 4) with the active photo; `G` returns.

- [ ] **Step 1: Failing tests**: `photos_in_lists_in_capture_order_with_edited_flag` and `remove_folder_cascades` (lf-catalog); `layout_virtualises_rows` (10,000 cells, viewport 1000×800 → `last - first` ≤ columns × (rows visible + 1)); `shift_click_selects_range_both_directions`; `ctrl_click_toggles`; `arrow_moves_by_columns_and_clamps`.
- [ ] **Step 2: Implement; run with a folder of 50 mixed JPEG/RAW files from the corpus and paste the import summary line and thumbnail count. Land: "lf-app: catalogue root, import, library grid and filmstrip".**

---

### Task 4: Develop view: open, preview texture, zoom, before/after, histogram

**Files:**
- Create: `src/develop/{mod,view,preview,open,histogram,info}.rs`
- Modify: `crates/lf-pipeline/src/texture.rs` (make `Target::view` and `Target::texture` readable: `pub fn view(&self) -> &wgpu::TextureView`, if not already), `crates/lf-pipeline/src/renderer.rs` (`Renderer::from_frame(gpu, frame: &FloatFrame, full_w, full_h)` uploads inside its own error scope; `prepare` is wrapped in the same `scoped` helper), `crates/lf-raw/src/render.rs` (`renderer_for` uses `from_frame`, so a RAW upload's validation error is returned, not only logged), `src/app.rs`, `src/jobs.rs` (`Msg::Opened { photo_id, generation, result: Result<Box<Opened>, String> }`)
- Test: `src/develop/view.rs` unit tests, `src/develop/histogram.rs` throttle test, `crates/lf-pipeline/tests/target_view.rs` (if the accessor is new)

**Interfaces:**
- `open::open_cached(path, quality, settings: &DevelopSettings, max_edge, cache: &FrameCache) -> Result<Opened, RawError>`: for RAW, `CacheKey::for_file` → `cache.get` else `lf_raw::open` + `cache.put`; 8-bit files bypass the cache. `max_edge = gpu.max_edge().min(6144)` for the develop view (a 6k working copy keeps a 60 MP RAW under 300 MB of f16).
- Opening runs on the `Open` lane with a `generation: u64` bumped on each request; a stale `Msg::Opened` is dropped. Before the pixels are decoded, the lane reconciles the photo's sidecar with the catalogue (`sidecar::read(path_for(photo))` then `edits::reconcile`), so an edit made on another machine shows on reopen; a read error sets `sidecar_error` and skips the reconcile (never `reconcile(None)`), and the view shows the damaged-sidecar warning from Task 5. The next and previous photos in the filmstrip are pre-opened into the frame cache (RAW only) at the same quality after the current one shows.
- `preview::Preview` owns a `Target` canvas (`Rgba8Unorm`, viewport size in physical pixels, re-made on resize) registered with `egui_wgpu::Renderer::register_native_texture` (`frame.wgpu_render_state().renderer.write()`), updated in place via `update_egui_texture_from_wgpu_texture` when the size is unchanged, freed on drop; each frame with a dirty flag: `renderer.draw(&settings, &DrawOptions { before, crop, view }, &canvas)`; a `PipelineError` from the call (each renderer call reports its own errors) → error toast + log; `PipelineError::DeviceLost` → the device-lost path below.
- `view::Viewport { zoom: Zoom (Fit | Scale(f64)), centre: (f64, f64) }` with `fit(output_w, output_h, panel_w, panel_h) -> f64`, `view_rect(&self, output, panel) -> lf_pipeline::View` (fractions, clamped so the photo never leaves the panel), `zoom_at(&mut self, factor, cursor_frac)` (keeps the point under the cursor still), `pan(dx_px, dy_px)`, `toggle_fit_100()`; scroll = zoom, drag = pan, `Space` = toggle, double-click = toggle.
- Before/after: `\` held shows `before: true` (release restores); clicking the "Before" button toggles it; a label "Before" in the corner while shown.
- `histogram::Throttle::should_run(last_change: Instant, last_run: Option<Instant>, now) -> bool` (200 ms after the last change, never more than once per change); runs `renderer.histogram(&settings)` on the UI thread (small readback) and draws R, G, B filled curves plus luma in a 256×100 panel header.
- Info strip under the histogram: file name, dimensions (`full_width × full_height`), camera, lens, ISO, shutter (`1/250 s`), aperture (`f/2.8`), focal length (`85 mm`), capture date; kind badge (`RAW`, `JPEG`, …); "Reduced to 6144 px for editing" when `reduced`.
- Device lost: when `gpu.is_lost()` the view drops the renderer, shows the Review Focus 3 banner, and the next open rebuilds (the `Gpu` is kept; eframe recreates its device only on restart, so the banner also offers "Restart").
- Open errors (`RawError` → string): "This file can't be opened: <message>" toast, the cell badge updates, the view shows a neutral "Can't open" placeholder.

- [ ] **Step 1: Failing tests**: `fit_scales_to_panel_both_orientations`; `view_rect_clamps_at_edges`; `zoom_at_keeps_cursor_point` (zoom 2× at (0.25, 0.75): the same output fraction stays under the cursor); `toggle_between_fit_and_one` ; `throttle_runs_once_200ms_after_last_change`; `target_view_accessor_exists` (compiles; skip without GPU).
- [ ] **Step 2: Implement; open a corpus RAW and a JPEG, zoom to 100 %, pan, hold `\`; paste the log lines for open times (`open` logs `opened <kind> <w>×<h> in <ms>`). Land: "lf-app: develop view with shared-texture preview".**

---

### Task 5: Adjust panels, slider widget, commit model, undo/redo, develop settings

**Files:**
- Create: `src/develop/panels/{mod,slider,light,color,hsl,curve,grading,detail,effects,raw}.rs`, `src/develop/edit_state.rs`, `src/develop/debounce.rs`
- Modify: `src/develop/mod.rs`, `src/app.rs`, `src/jobs.rs` (`Msg::SidecarWritten(Result<(), String>)`)
- Test: unit tests in `slider.rs`, `curve.rs`, `edit_state.rs`, `debounce.rs`; kittest `tests/panels.rs`

**Interfaces:**
- `EditState { photo_id, settings: EditSettings, committed: EditSettings, dirty: bool }`: `set(f: impl FnOnce(&mut EditSettings))` marks dirty and redraws; `commit(catalog) -> Result<Sidecar>` calls `edits::set_settings` only when `settings != committed`, once per gesture (slider release, curve point release, toggle click), never per frame, so one drag is one history step; then hands the sidecar to the debouncer; `undo/redo(catalog)` call `edits::undo/redo` and load the returned sidecar's settings; `reset_groups(&[GroupKey])` = `apply_groups(settings, &default.to_value(), groups)` then commit.
- `Debouncer<T>`: `push(value, now)`, `due(now) -> Option<T>` (1 s after the last push), `flush() -> Option<T>`; `App::on_exit` flushes and writes synchronously. Sidecar writes run on the `Io` lane (`sidecar::write(path_for(photo), &sc)`). `sidecar::write` refuses to replace a damaged sidecar, and the app does not even try while `sidecar_error` is set for the photo (a sidecar whose read failed for a transient reason must not be overwritten before it has been reconciled): the edit is still in the catalogue, and the view shows a persistent warning "This photo's edit file is damaged, so edits are kept in the catalogue only. Fix or remove {name}.lensflow.json to save beside the photo again." (shown once per photo per session, with "Show file").
- `slider::edit(ui, label, value: &mut f64, Range { min, max, step, decimals, neutral }, track: Option<Track>) -> SliderResponse { changed, released }`: label left, value right (click to type), drag anywhere on the row, double-click resets to `neutral`, `Shift` drags 10× finer, arrow keys step; `changed` while dragging updates the preview, `released` commits. Ranges exactly as the web's `AdjustPanel.vue`: Exposure −5..5 step 0.05 (2 decimals), Contrast/Highlights/Shadows/Whites/Blacks −100..100, Temperature/Tint −100..100 (coloured tracks), Vibrance/Saturation −100..100, HSL bands −100..100 (hue/sat/lum tabs; in B&W only luminance), grading Hue 0..360 (rainbow track, `°`) and Amount 0..100 per wheel, Balance −100..100, Clarity/Texture/Dehaze −100..100, Sharpening 0..150 (radius 0.5..3 step 0.1 shown when > 0), Noise 0..100, Colour noise 0..100, Vignette −100..100 (midpoint 0..100 when ≠ 0, neutral 50), Grain 0..100 (size 0..100 when > 0, neutral 25); "Black & white" toggle.
- Sections in the web's order with the web's labels (`GROUPS` labels), each collapsible with a "Reset" link; the curve editor: master/R/G/B tabs, 256 px square, drag points, click adds, drag out or `Delete` removes (not the end points), x kept strictly increasing by `curve::clamp_point(points, i, p) -> CurvePoint`; the curve table preview from `curve_table`.
- `raw.rs` (shown for `Kind::Raw` only): Profile (Camera / Linear) and Highlights (Clip / Reconstruct) as segmented buttons; a change writes `DevelopSettings::write_into(&mut settings)`, commits, and re-opens the photo through `open_cached` (the key includes the settings).
- Undo/redo in the Edit menu with the shortcuts; disabled when `history_len` says nothing to do.

- [ ] **Step 1: Failing tests**: `slider_double_click_resets_to_neutral`, `slider_shift_drag_is_finer` (pure drag-math function), `curve_clamp_keeps_x_monotonic_and_endpoints_fixed`, `commit_only_when_changed` (a fake catalogue counting `set_settings` calls via a temp `Catalog`), `undo_then_redo_restores`, `debouncer_fires_once_after_quiet_second_and_flushes`, kittest `light_section_has_six_sliders_with_web_labels`.
- [ ] **Step 2: Implement; land: "lf-app: adjust panels, commit model, undo/redo, develop settings".**

---

### Task 6: Crop, straighten and spots tools

**Files:**
- Create: `src/develop/tools/{mod,crop,spots}.rs`, `src/develop/panels/crop.rs`
- Modify: `src/develop/{mod,preview}.rs` (overlays drawn with `egui::Painter` over the preview; the crop tool draws with `DrawOptions { crop: Some(whole) }`)
- Test: unit tests in `crop.rs` and `spots.rs`

**Interfaces:**
- Crop tool (`R`): the preview shows the whole turned frame dimmed outside the crop; eight handles and inside-drag move; `crop::drag(crop: Crop, handle: Handle, delta_frac, ratio: Option<f64>, tw, th, angle) -> Crop` keeps the aspect when locked and stays inside via `geometry::fit_crop`; aspect list as the web's `CropPanel.vue` ("Original", "Free", "1:1", "4:5", "5:7", "2:3", "3:4", "16:9" with "Switch portrait / landscape"); rotate left/right (`turn_settings`), flip H/V; "Straighten" −45..45 step 0.1 `°` with a thirds grid while dragging; "Reset crop & rotation"; `Enter` or leaving the tool commits, `Esc` restores the settings at tool entry.
- Spot tool (`Q`): click-drag on the photo places a spot of radius from the drag (fractions of the image, min 0.005), the source offset defaults to `(+2r, 0)` clamped inside; drag the source circle to move it; wheel over a spot resizes; `Delete` removes the selected one; mode Heal/Clone per spot in the panel; at `MAX_SPOTS` (48) a message "Up to 48 spots per photo."; each change commits (the web commits per spot).
- `spots::hit(spots, p_frac, aspect) -> Option<(usize, Part::{Target, Source})>` tested with circles in a non-square image (radii are fractions of the width, as the shader uses them: read `shaders.ts` SPOTS to confirm and state the rule in a doc comment).

- [ ] **Step 1: Failing tests**: `drag_corner_with_locked_ratio_keeps_ratio`, `drag_never_leaves_turned_frame`, `move_inside_clamps`, `spot_hit_prefers_source_handle_when_overlapping`, `max_spots_refused`.
- [ ] **Step 2: Implement; land: "lf-app: crop, straighten and spot tools".**

---

### Task 7: Looks, presets with sync, copy/paste, batch apply, versions

**Files:**
- Create: `src/develop/panels/{looks,presets,versions}.rs`, `src/dialogs/{group_picker,batch,save_preset}.rs`, `src/clipboard.rs`, `crates/lf-catalog/src/presets.rs` (`LocalPresetRow`, `list`, `upsert(name, settings, updated_at, remote_id, synced_at)`, `delete(id)`, `rename`), `src/presets_sync.rs`
- Modify: `crates/lf-catalog/src/lib.rs`, `src/jobs.rs` (`Msg::PresetsSynced(Result<SyncReport, String>)`, `Msg::BatchProgress { done, total, failed: Vec<(i64, String)> }`)
- Test: `crates/lf-catalog/tests/presets.rs`, `src/presets_sync.rs` (httpmock), `src/clipboard.rs`, `src/dialogs/batch.rs` unit tests

**Interfaces:**
- Looks: a 4-column grid of the 12 `lf_settings::looks::LOOKS` names; click = `apply_look(&settings, &look.settings)` then commit; the active look is not tracked (as the web).
- Presets panel ("Studio presets"): rows from `lf_catalog::presets::list`; click applies `apply_groups(settings, &preset.settings, &groups_in(&preset.settings))` and commits; "Save preset…" opens `save_preset` (Name ≤ 80 + `GroupPicker` with the 11 groups, `DEFAULT_COPY_GROUPS` ticked, All/None links) and writes `pick_groups`; delete asks first. Empty copy: "Save an edit you like as a preset, and use it on any photo with one tap. Presets are shared with your team."
- Deletes are tombstones: `lf_catalog::presets::delete` sets `deleted_at` (migration in this task) instead of removing the row; `list` hides tombstones. `lf_account::presets::Sync::merge` gains a `deleted: &[LocalPreset]` input and a `Plan.delete_remote: Vec<i64>` output (a tombstone with a `remote_id` still on the server → delete it there; a tombstone whose name is absent remotely → purge the row); a remote preset absent locally and not tombstoned → pull (as today); a local non-tombstone absent remotely but with a `remote_id` and `synced_at` → it was deleted on the server → purge locally. Tests in `lf-account` for each rule.
- `presets_sync::run(client, catalog) -> SyncReport { pushed, pulled, deleted, conflicts }`: `Sync::merge(local, deleted, remote)`; executes `push` (create or update), `pull` (upsert locally with `remote_id`, `synced_at = now`), `delete_remote`, purges, and for conflicts keeps both as the plan says (renamed local copy pushed, remote pulled); runs on the `account` lane at sign-in and after each local save when `decision.sync_allowed()`; `ReadOnly` → skipped with the banner, `Unauthorized` → `Session::on_unauthorized`. Backend note: `PhotoEditPresetResource` does not return `updated_at` today (ledger follow-up); until it does, remote `updated_at` is empty and `Sync::merge` treats it as older, so local wins ties: state this in a doc comment.
- Copy/paste: `Ctrl+Shift+C` opens the `GroupPicker` ("Include"), copies `pick_groups` as JSON to an in-app slot and to the system clipboard (`ctx.copy_text`); `Ctrl+Shift+V` applies the slot (or parses the system clipboard when the slot is empty and the text normalises) to every selected photo via `apply_groups` + `set_settings`, showing the batch dialog when more than one is selected.
- Batch dialog ("Apply to N photos": source = pasted settings / the current photo's settings / a preset; progress rows with Working/Done/Failed; "Keep this window open. Closing it stops after the current photo."; Stop/Done): work runs on the `Io` lane with its own `Catalog`, one `set_settings` + sidecar write per photo, a `stop: Arc<AtomicBool>` checked between photos.
- Versions panel: list (label, created date, 112 px thumb rendered via `renderer.pixels(&version.settings, 112)` on the UI thread when the panel opens, cached per version id), "Add version…" (label ≤ 80, default "Version N"), "Use" (asks "Replace the current edit with this version? Your current edit is kept in history."), delete (asks); the current settings compare equal to a version → "Current" tag.

- [ ] **Step 1: Failing tests**: `presets_crud_round_trip`; `sync_pushes_new_local_and_pulls_new_remote` and `sync_conflict_keeps_both` (httpmock on `galleries/edit-presets`); `copy_picks_only_chosen_groups`; `paste_applies_to_each_selected` (temp catalogue with 3 photos); `batch_stop_halts_after_current`.
- [ ] **Step 2: Implement; land: "lf-app: looks, presets with sync, copy/paste, batch apply, versions".**

---

### Task 8: Match a reference

**Files:**
- Create: `src/dialogs/match_reference/{mod,samples,steps,preview}.rs`
- Modify: `src/jobs.rs` (`Msg::Match(MatchStep)`), `src/app.rs`
- Test: `src/dialogs/match_reference/samples.rs` and `steps.rs` unit tests (synthetic images; AI via httpmock)

**Interfaces:**
- Entry: Photo menu "Match a reference…" with ≥ 1 selected photo. Steps as the web's `MatchReferencePanel.vue`: (1) Reference: a file picker ("Choose a reference picture") or a catalogued photo; (2) Cost line "Uses {n} of your {remaining} remaining AI requests (one more per step if the AI has to be asked again)." with `remaining` from the entitlement (`ai.remaining`, "unlimited" when `None`); toggle "Let AI check the result (1 more request)"; when `!decision.ai_allowed()` the AI steps are off and the copy says "AI is off for your studio: the grade is fitted from measurements only."; (3) Fit; (4) Preview with a per-tile before/after (press and hold), "{i} of {tiles}", Strength 0..100 % (default 100, `apply_strength`), "Match exposure per photo" toggle (`exposure_offset(reference_median, photo_median)` per photo, clamped by the library), "Save as preset (optional)" name; Apply.
- `samples::build(photos: &[PhotoRow], open: impl Fn(&Path) -> Result<RgbaImage, String>) -> Result<Sheet { ids, bitmaps, all_edited }, String>`: `pick_samples(cap 24, is_edited = row.edited)`, each opened at `Quality::Fast`/`AI_EDGE` through `open_cached` and rendered with *default* settings via a temporary `Renderer::pixels(default, TILE_EDGE)` (the web grades the unedited look), tiled with `sheet::tile(TILE_EDGE, SHEET_COLUMNS)`.
- `steps::run(ctx: MatchCtx) -> Result<Outcome, String>` on the `Open` lane (needs the GPU for the sheet renderer): measure the reference (`measure(scaled(ref, MEASURE_EDGE), 1)`), measure the sheet with defaults (anchors = percentile lumas), AI `extract` (reference JPEG and sheet JPEG via `sheet::jpeg(.., 0.85)`, examples = the 12 looks) when allowed else start from defaults, `calibrate` with `render = |s| measure(renderer_for_sheet.pixels(s, sheet_w))`, optional `review`, returning `Outcome { settings, residual, rounds, requests_used, scene_bands }`; progress messages "Reading the reference…", "Asking the AI…", "Fitting the grade (round r of 4)…", "Checking with the AI…".
- Apply: per selected photo `final = apply_strength(outcome.settings, strength)`, plus exposure offset when ticked, written with `set_settings` (replacing existing edits; the web's warning copy when `all_edited`); optional preset saved through Task 7's path. Each AI call is preceded by `refresh_now` (Global Constraints) and refused with the lock copy if the decision changed; a `PlanLimit` error shows "Your studio's AI requests are used up for this month."

- [ ] **Step 1: Failing tests**: `build_picks_at_most_24_and_prefers_unedited`; `calibrate_only_path_reduces_residual_on_synthetic_sheet` (a warm reference vs a neutral sheet: residual luma after < before); `ai_path_sends_two_jpegs_under_700kb_and_uses_proposal` (httpmock `desktop/grade` returning a fixed proposal; assert the multipart field names the server expects, read `GradeRequest.php`); `exposure_match_offsets_each_photo`; `plan_limit_shows_copy`.
- [ ] **Step 2: Implement; land: "lf-app: match a reference".**

---

### Task 9: Export to disk

**Files:**
- Create: `src/dialogs/export.rs`, `src/export_job.rs`
- Modify: `src/jobs.rs` (`Msg::ExportProgress { done, total }`, `Msg::ExportDone(Result<Vec<Exported>, String>)`), `src/prefs.rs` (`last_export`)
- Test: `src/dialogs/export.rs` unit tests (spec building), `src/export_job.rs` (temp catalogue, 2 PNGs, collision)

**Interfaces:**
- Dialog (`Ctrl+Shift+E`, "Export {n} photos"): Format (JPEG quality 1..100 default 93 / PNG / TIFF 16-bit), Size (Original / Long edge px / Short edge px / Megapixels / Percent), File name pattern (default `{name}`, tokens listed under the field: `{name} {seq} {date} {camera}`), Metadata (All / Copyright only / None), "Replace existing files" (off), Destination folder (picker; default the last one), Export. The form is remembered in `prefs.last_export` as the `ExportSpec` JSON.
- `export_job::run(gpu, catalogue_path, ids, spec, progress, stop)` on the `Io` lane with its own `Catalog`, calling `lf_catalog::export::run` whose progress callback returns `ControlFlow::Break` when `stop` is set or `gpu.is_lost()`; `lf_catalog::export::run` itself stops on a `DeviceLost` from any photo and reports it as the run's error (library change in this task, with a test using the device-loss hook if one exists, else the `is_lost` path); progress in the dialog ("12 of 40", a Stop button) and the status bar; done → "Exported 40 photos." with "Show folder" (opens the folder with the OS opener), or "Exported 38 of 40 photos." with the `ExportReport.failed` list (file name and the error sentence) when some failed, or "Stopped after 12 photos." on cancel; a `NameCollision` lists both names: "Two photos would get the same name: a.jpg (IMG_1.CR2 and IMG_1.JPG). Change the file name pattern."; `Exists` → "{file} already exists. Turn on 'Replace existing files' or choose another folder."; other errors verbatim.
- Validation before running: a folder chosen and writable (create a probe file), pattern non-empty, size > 0.

- [ ] **Step 1: Failing tests**: `spec_from_form_defaults`, `invalid_size_blocked`, `collision_message_names_both_files` (temp catalogue with `IMG_1.png` in two folders), `partial_failure_lists_failed_files` (one unreadable file among three), `stop_flag_breaks_after_current`. `export_job::run` takes no `Client` and no session (say so in its doc comment).
- [ ] **Step 2: Implement; export a JPEG and a TIFF from a corpus RAW and paste the file sizes. Land: "lf-app: export to disk".**

---

### Task 10: Menus, shortcuts, about, docs, CI builds

**Files:**
- Create: `src/menu.rs`, `src/shortcuts.rs`, `src/dialogs/about.rs`, `docs/app.md` (how the app is put together: lanes, session machine, preview texture, where files live, the keyboard map)
- Modify: `README.md` (Status; "Running the app"; the keyboard map), `.github/workflows/ci.yml` (job `app` on `windows-latest`, `macos-latest`, `ubuntu-24.04`: `cargo build -p lf-app --release`, `cargo test -p lf-app`, upload the binary as an artifact `lensflow-image-editor-<os>`; Linux installs `libgtk-3-dev` for `rfd`), `NOTICE`
- Test: `src/shortcuts.rs` unit test (every entry in the Global Constraints map is bound exactly once; no duplicates across platforms)

**Interfaces:**
- Menu bar: File (Add folder…, Export…, Quit), Edit (Undo, Redo, Copy settings…, Paste settings, Select all, Preferences…: catalogue root (read-only, with "Change…" that restarts the library), frame cache size, develop quality Best/Fast, log folder), View (Library, Develop, Before/After, Fit, 100 %), Photo (Match a reference…, Add version…, Reset all edits), Account (studio name, Switch studio, Check entitlement now, Sign out), Help (About, Show log folder, Keyboard shortcuts).
- `shortcuts::Map` as a table `[(Action, KeyboardShortcut)]` consumed by `update()` through `ctx.input_mut(|i| i.consume_shortcut(..))`; shown in Help → Keyboard shortcuts.
- About: logo, name, version, "Built with Rust, egui and wgpu.", the NOTICE text in a scroll area, a link to `https://lensflow.in`.
- Logging polish: an uncaptured GPU error is logged once (the library's handler already logs; the app's `take_error` tap only counts and shows a toast after three in a minute).

- [ ] **Step 1: Test; implement; CI green on all three OSes with artifacts; land: "lf-app: menus, shortcuts, about, docs, CI builds".**
