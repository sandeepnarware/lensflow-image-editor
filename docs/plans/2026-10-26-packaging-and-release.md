# lensflow-ImageEditor, Part 2d: Packaging, Signing, Release and Updates

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One manual GitHub Actions run turns a version number into installers for Windows (MSI), macOS (universal DMG) and Linux (AppImage and .deb) that contain everything the app needs. When the owner's certificates are present they are signed and notarised; when publish is chosen they are uploaded to the private Azure container and registered with lensflow.in, so entitled studios see the Download page and the app's update prompt.

**Architecture:** A new `.github/workflows/build.yml` (manual) with a test job, three build jobs and a publish job. Packaging inputs live in `packaging/` (WiX source, macOS bundle files, Linux desktop entry and AppRun), produced from `assets/logo.png` by a checked-in icon script. Small shell/PowerShell scripts in `scripts/release/` do the version stamping, the SHA-256 manifest and the publish call, and each has a `--dry-run` that CI exercises on every push. The app gains the release plumbing the final review asked for: `--version`, a single-instance lock, a hand-off to the installer that waits for the app to exit, the AppImage restart path, a resumable size-checked update download on the long lane, and diagnostics.

**Tech Stack:** GitHub Actions (`windows-latest`, `macos-14`, `ubuntu-22.04` for the glibc baseline), WiX Toolset v4 (`wix` .NET tool, pinned), Apple `codesign`/`notarytool`/`hdiutil`/`lipo`/`iconutil`, `appimagetool` (pinned release with SHA-256), `cargo-deb` (pinned), Azure CLI with GitHub OIDC (`azure/login`), `curl` + `jq` for the release call.

**Spec:** `D:\pet-projects\photo-studio-management\docs\superpowers\specs\2026-10-25-lensflow-image-editor-design.md` (Goals 6 and 8; Constraints → Hosting, Code signing; Architecture → Build and release pipeline, Account → Updates, Security; Approaches → Installer hosting and gating; Owner's answers). Server contracts on the monorepo's `origin/main`: `backend/routes/api/platform/admin.php` (`POST platform/releases` behind `EnsurePlatformAdmin:desktop.release`), `Platform/Admin/DesktopReleaseController::store` (validation below), `Models/DesktopRelease::PLATFORMS`, `Desktop/DownloadController`, `Services/Desktop/InstallerStore`, `config/filesystems.php` `installers` (container `installers` + instance suffix, links valid 900 s), `Console/Commands/DesktopReleaseToken` (`php artisan desktop:release-token {email}` → a 365-day token with ability `desktop.release`).

Depends on Part 2c-ii being on `main`, including its final fix wave (`Lane::Long`, visible start failure).

## Global Constraints

- Repository `D:\pet-projects\lensflow-image-editor`, `main`; each task in a sibling worktree on `wip/release-task-N`, landed with `gh pr merge --squash --delete-branch` after CI is green; no force-pushes. `export PATH="$HOME/.cargo/bin:$PATH"`. Commit trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; PR bodies end with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Self-contained installers (spec Goal 8): the only machine dependency is the OS's GPU driver. Windows: static CRT (`-C target-feature=+crt-static`), so no Visual C++ redistributable; DX12 and WARP are part of Windows 10+. macOS: Metal only, minimum macOS 11. Linux: the AppImage bundles every non-glibc library the binary loads at runtime except the GPU driver stack (`libvulkan.so.1` is bundled; Mesa/vendor ICDs are the driver); the .deb declares its few runtime libraries in `Depends:` so the package manager installs them. A job step prints the runtime library list (`dumpbin /dependents`, `otool -L`, `ldd`) and fails on anything outside an allow-list.
- Platform names, exactly the server's: `windows`, `macos`, `linux-appimage`, `linux-deb`.
- Release registration body (server validation, verbatim): `version` `^\d+\.\d+\.\d+$` and unique; `channel` `stable` | `beta`; `notes` ≤ 4000 characters; `min_version` `^\d+\.\d+\.\d+$` or absent; `files` 1–4 entries, each `{platform (distinct), blob (≤255, path inside the container), sha256 (64 lowercase hex), size (bytes, ≥1)}`. Endpoint `POST {API_BASE}/platform/releases`, `Authorization: Bearer <desktop.release token>`, `Accept: application/json`.
- Blob layout: `<version>/<file name>` inside the installers container; file names `lensflow-ImageEditor-<version>-windows-x64.msi`, `lensflow-ImageEditor-<version>-macos-universal.dmg`, `lensflow-ImageEditor-<version>-linux-x86_64.AppImage`, `lensflow-ImageEditor_<version>_amd64.deb`.
- Version consistency: the workflow input version is written into the workspace `Cargo.toml` (`[workspace.package] version`, and `lf-app` inherits it) before building, so `lensflow-image-editor --version`, the installer's product version and the registered release all match; the build fails if they do not.
- Signing is optional per run: a signing step runs only when its secrets exist; otherwise the artifact name gets `-unsigned` and the run summary says "Unsigned build: Windows and macOS will warn on first launch." `publish=true` with `channel=stable` refuses unsigned Windows or macOS artifacts; `channel=beta` allows them.
- Secrets (names exactly): `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD`; `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD`; `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`; `LENSFLOW_RELEASE_TOKEN`, `LENSFLOW_RELEASE_TOKEN_DEV`. Variables: `INSTALLERS_STORAGE_ACCOUNT`. Never echo a secret; mask derived values (`::add-mask::`); signing material is written to the runner's temp dir and deleted in an `always()` step.
- Instances: input `instance` = `prod` (API `https://lensflow.in/api`, container `installers`, token `LENSFLOW_RELEASE_TOKEN`) or `dev` (`https://dev.lensflow.in/api`, `installers-dev`, `LENSFLOW_RELEASE_TOKEN_DEV`).
- Third-party tools are pinned by version and, when downloaded as files, by SHA-256 checked before use; downloads go to an empty temp directory and are never executed from the repository tree.
- Upgrades never touch user data: config, data, cache directories and catalogues are outside the install location; uninstall leaves them in place.

## Review Focus

1. A photographer installs on a clean machine (no developer tools, no runtimes) and the app starts. Pinned in Task 3 by the runtime-library allow-list step on each OS and a `--version` smoke run of the packaged binary (from the extracted MSI, the mounted DMG, the AppImage with `--appimage-extract-and-run`, and the unpacked .deb).
2. Updating over an older version keeps the photographer's catalogue, prefs, token and frame cache, and the update never runs while the old app still holds files. Pinned in Task 1 (installer hand-off waits for exit; single-instance lock) and Task 2 (WiX MajorUpgrade with the same UpgradeCode; data paths outside the install dir).
3. A half-finished run never publishes: a failed signing, notarisation, smoke test or upload stops before the release is registered, and a release is registered only after every file is uploaded and its SHA-256 recomputed from the uploaded bytes. Pinned in Task 4 (job ordering and a dry-run test).
4. The SHA-256 the app verifies is the SHA-256 of the exact bytes a studio downloads. Pinned in Task 4 (hash computed from the downloaded blob, not the local file) and Task 1 (size cap against `link.size`).
5. A beta release reaches only the platform admin's own studios. Pinned in Task 4 (`channel` passed through; the server's `latest()` already filters) and documented in Task 5.

---

### Task 1: App release plumbing

**Files:**
- Create: `crates/lf-app/src/cli.rs`, `crates/lf-app/src/single_instance.rs`, `crates/lf-app/src/diagnostics.rs`
- Modify: `crates/lf-app/src/{main.rs,app.rs,update.rs,device_reset.rs,session_flow.rs,dialogs/about.rs}`, `Cargo.toml` (workspace `version` field present and inherited by `lf-app`), `.cargo/config.toml` (Windows static CRT for the MSVC target)
- Test: unit tests in each new module; `crates/lf-app/tests/cli.rs` (runs the built binary with `--version` and `--help`)

**Interfaces:**
- `cli::parse(args) -> Cli { Version, Help, Run }`: `--version` prints `lensflow-ImageEditor <version> (<target triple>)` and exits 0 before any window or GPU work; `--help` prints usage (the two flags) and exits 0; unknown flags print usage to stderr and exit 2. On Windows release builds (`windows_subsystem = "windows"`) attach to the parent console (`AttachConsole(ATTACH_PARENT_PROCESS)`) so the text is visible from a terminal.
- `single_instance::acquire(config_dir) -> Result<Guard, AlreadyRunning>`: an exclusive OS file lock on `<config dir>/instance.lock` (`fs4`-style `try_lock_exclusive`; MIT/Apache crate allowed) held for the process lifetime; a second launch shows a native message "lensflow-ImageEditor is already open." and exits 0. `Guard::release()` is called at the end of `on_exit` before any restart/installer spawn; the restarted process retries acquiring for up to 10 s.
- Restart and update hand-off spawn at the very end of `on_exit` (after the lock is released). AppImage: when `$APPIMAGE` is set, restart and update use that path (not `current_exe()`, which points inside the temporary mount).
- `update::run` moves to `Lane::Long`; streams to `<cache>/updates/<file>.part` with HTTP `Range` resume when a `.part` exists and the server answered `206`; refuses and deletes when the byte count would exceed `link.size`; verifies SHA-256 before rename; deletes other files in `updates/` older than the current download on success.
- Installer launch per OS after exit: Windows `msiexec /i <path> /passive`; macOS `open <dmg>` (the user drags the app; the DMG window says how); Linux AppImage: replace `$APPIMAGE` atomically with the new file (same directory, temp + rename, `chmod 755`) then relaunch it; .deb: `xdg-open <deb>` (the system installer asks for the password). Which Linux path is used is decided by whether `$APPIMAGE` is set.
- `diagnostics::text(gpu, dirs) -> String` (version, OS, adapter name/backend/software flag, max texture size, log folder, catalogue root, instance); About shows the log folder with "Show log folder" and a "Copy diagnostics" button (`ctx.copy_text`).
- Release profile keeps `strip = true` and gains `split-debuginfo = "packed"` where supported; the build workflow (Task 3) keeps the debug files as a separate artifact, never inside the installer.
- Downgrade: opening a catalogue from a newer app version already shows `NewerSchema`; prefs written by a newer version load with defaults for unknown fields (serde default) — state both in `docs/app.md`.

- [ ] **Step 1: Failing tests**: `version_flag_prints_and_exits_before_any_window` (integration test runs the binary; asserts stdout `lensflow-ImageEditor 0.` prefix and exit 0); `unknown_flag_exits_2`; `second_acquire_fails_while_first_held` and `released_lock_can_be_taken_again`; `appimage_restart_uses_the_appimage_env`; `download_resumes_from_a_part_file` and `download_larger_than_link_size_is_refused` (httpmock with Range); `old_update_files_are_cleaned`; `diagnostics_names_the_log_folder_and_adapter`.
- [ ] **Step 2: Implement; check on Windows that `target\release\lensflow-image-editor.exe --version` prints in a terminal; land: "lf-app: version flag, single instance, update hand-off, diagnostics".**

---

### Task 2: Packaging inputs (icons, WiX, macOS bundle, Linux entries)

**Files:**
- Create: `scripts/release/make-icons.{sh,ps1}` (from `assets/logo.png` → `packaging/icons/{app.ico (16–256), app.icns, 16..512 png}`; uses the `image`-based helper below, no system tools), `crates/lf-app/examples/make_icons.rs` (the helper), `packaging/icons/*` (generated, committed), `packaging/windows/main.wxs`, `packaging/windows/License.rtf` (Apache-2.0 + pointer to NOTICE), `packaging/macos/Info.plist`, `packaging/macos/entitlements.plist`, `packaging/macos/dmg-readme.txt`, `packaging/linux/lensflow-image-editor.desktop`, `packaging/linux/AppRun`, `crates/lf-app/Cargo.toml` `[package.metadata.deb]`
- Modify: `crates/lf-app/build.rs` (Windows: embed `app.ico` and version info via `winresource`, MIT)
- Test: `scripts/release/check-packaging.sh` run in CI (validates the XML/plist with `xmllint`, the desktop entry with `desktop-file-validate`, and that every referenced file exists)

**Interfaces:**
- WiX v4 (`main.wxs`): per-machine install to `ProgramFiles64Folder\lensflow-ImageEditor`; fixed `UpgradeCode` GUID (generate once, commit, never change) with `<MajorUpgrade DowngradeErrorMessage="A newer version of lensflow-ImageEditor is already installed." />`; Start-menu shortcut with the icon; `ProductVersion` from a `-d Version=` define; the NOTICE file installed beside the exe; no custom actions; nothing written under the user's profile.
- macOS: `Info.plist` with `CFBundleIdentifier` `in.lensflow.imageeditor`, `CFBundleShortVersionString`/`CFBundleVersion` substituted from the version, `LSMinimumSystemVersion` `11.0`, `NSHighResolutionCapable` true, `CFBundleIconFile` `app`; `entitlements.plist` for the hardened runtime (no extra entitlements unless a test shows one is needed; document any added); the DMG contains the `.app`, an `Applications` symlink and `dmg-readme.txt` ("Drag lensflow-ImageEditor to Applications.").
- Linux: desktop entry (`Name=lensflow-ImageEditor`, `Exec=lensflow-image-editor`, `Icon=lensflow-image-editor`, `Categories=Graphics;Photography;`); `AppRun` sets `LD_LIBRARY_PATH` to the bundled `usr/lib` and execs the binary; `.deb` metadata: maintainer, `Depends:` computed for the binary (`$auto` via `cargo-deb`) plus `libvulkan1`, icon and desktop entry installed to the standard paths, NOTICE to `/usr/share/doc/lensflow-image-editor/`.

- [ ] **Step 1: Write `check-packaging.sh` first (fails: files missing); generate icons; write the packaging files; the check passes. Add the check as a step in `ci.yml` on Linux.**
- [ ] **Step 2: Locally build the MSI once on Windows (`dotnet tool install --global wix --version <pinned>`; `wix build packaging/windows/main.wxs -d Version=0.1.0 ...`), install it, run the app, uninstall; paste the outcome. Land: "Packaging: icons, WiX, macOS bundle, Linux entries".**

---

### Task 3: Build workflow (unsigned and signed)

**Files:**
- Create: `.github/workflows/build.yml`, `scripts/release/set-version.sh` (writes the workspace version; fails on a malformed version), `scripts/release/check-runtime-libs.{sh,ps1}` (allow-lists per OS), `scripts/release/smoke.{sh,ps1}` (runs the packaged binary with `--version` and compares to the input)
- Modify: `.github/workflows/ci.yml` (a job that runs `set-version.sh --dry-run`, `check-runtime-libs` on the CI debug binary for the current OS, and `check-packaging.sh`)

**Interfaces:**
- `build.yml` `workflow_dispatch` inputs: `version` (string), `channel` (`stable`|`beta`, default `beta`), `instance` (`prod`|`dev`, default `dev`), `platforms` (booleans `windows`, `macos`, `linux`, all default true), `publish` (bool, default false), `notes` (string, optional), `min_version` (string, optional).
- Jobs: `test` (Linux: fmt, clippy, `cargo test --workspace` with the corpus and lavapipe, same as ci.yml) → `build-windows` (`windows-latest`: set version, `cargo build -p lf-app --release`, `wix build`, sign the exe before packaging and the MSI after with `signtool` when the Windows secrets exist (timestamp `http://timestamp.digicert.com`), runtime-lib check, smoke from an administrative extract `msiexec /a`), `build-macos` (`macos-14`: build `aarch64-apple-darwin` and `x86_64-apple-darwin`, `lipo -create`, assemble the `.app`, `codesign --options runtime --timestamp --entitlements` when the Apple secrets exist (temporary keychain created and deleted in `always()`), `hdiutil create` the DMG, sign the DMG, `xcrun notarytool submit --wait` then `xcrun stapler staple`, `spctl --assess` when signed, runtime-lib check, smoke from the mounted DMG), `build-linux` (`ubuntu-22.04`: build, AppImage via pinned `appimagetool` (SHA-256 checked) with `usr/lib/libvulkan.so.1` bundled, `.deb` via pinned `cargo-deb`, runtime-lib check, smoke from `--appimage-extract-and-run` and from `dpkg-deb -x`). Each build job uploads its installer, a `<file>.sha256`, and (separately) its debug symbols as artifacts kept 90 days.
- Every build job writes a run-summary section: file, size, SHA-256, signed or `-unsigned`.

- [ ] **Step 1: `ci.yml` dry-run job first (fails until the scripts exist); write scripts; green.**
- [ ] **Step 2: Write `build.yml`; run it on the branch with `publish=false` (`gh workflow run build.yml --ref wip/release-task-3 -f version=0.1.0 ...`); all three build jobs green, unsigned; paste the summary. Land: "Release: build workflow".**

---

### Task 4: Publish job

**Files:**
- Create: `scripts/release/publish.sh` (`--dry-run` prints the JSON body and the blob names without network calls), `scripts/release/test-publish.sh` (runs `publish.sh --dry-run` against fixture artifacts and checks the JSON with `jq` against the server rules)
- Modify: `.github/workflows/build.yml` (job `publish`, `needs:` every selected build job, `if: inputs.publish`, `environment: release` so it can be gated by a required reviewer), `.github/workflows/ci.yml` (runs `test-publish.sh`)

**Interfaces:**
- `publish` job: refuses (before any upload) when `channel=stable` and a Windows/macOS artifact is `-unsigned`; `azure/login` with OIDC (`permissions: id-token: write`); for each artifact: `az storage blob upload --auth-mode login --account-name $INSTALLERS_STORAGE_ACCOUNT --container-name <installers|installers-dev> --name <version>/<file> --overwrite false`; then downloads each blob back and recomputes SHA-256 and size; refuses if any differs from the build's `.sha256`; only then `curl --fail-with-body -X POST <API_BASE>/platform/releases` with the body built by `jq` (`version`, `channel`, `notes` truncated to 4000, `min_version` when given, `files[]` with `platform`, `blob`, `sha256`, `size`); prints the server's reply `data.id`/`version`; a `409`/`422` (duplicate version) fails the job with the server message. The token comes from the instance's secret; masked.
- Idempotence: blobs are uploaded with `--overwrite false`; a re-run for the same version fails at upload with a plain message ("Version X is already uploaded. Bump the version.").

- [ ] **Step 1: `test-publish.sh` first (fixture artifacts in `scripts/release/fixtures/`, tiny fake files); write `publish.sh`; green in CI.**
- [ ] **Step 2: Wire the job; one real run against `instance=dev`, `channel=beta`, `publish=true` is the owner's (it needs the Azure identity and the dev token — Task 5's runbook); the implementer verifies the workflow with `publish=false` and with the dry-run. Land: "Release: publish job".**

---

### Task 5: Release runbook and owner setup

**Files:**
- Create: `docs/release.md`
- Modify: `README.md` (Status; a "Releasing" pointer), `docs/app.md` (update hand-off, single instance, downgrade notes)

**Content (`docs/release.md`):** a checklist the owner can follow without the code:
1. One-time Azure setup (the owner runs these; the assistant's environment cannot create identities or role assignments): create containers `installers` and `installers-dev` in the storage account; create a user-assigned managed identity (or reuse `id-studioflow-github` in resource group `personal`, subscription "Visual Studio Enterprise") with **Storage Blob Data Contributor** on both containers; add a federated credential for the public repo with the repository's **immutable** OIDC subject for environment `release` (`repo:sandeepnarware@<owner id>/lensflow-image-editor@<repo id>:environment:release` — show how to read the ids with `gh api repos/sandeepnarware/lensflow-image-editor --jq '.owner.id, .id'`); set `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID` secrets and the `INSTALLERS_STORAGE_ACCOUNT` variable; set the Laravel `.env` keys the server's `installers` disk reads (`AZURE_STORAGE_ACCOUNT`, `AZURE_STORAGE_KEY`, `INSTALLERS_DRIVER=azure`, `APP_INSTANCE`).
2. Release tokens: `php artisan desktop:release-token <platform admin email>` on prod and on dev (via SSH on Hostinger), stored as `LENSFLOW_RELEASE_TOKEN` and `LENSFLOW_RELEASE_TOKEN_DEV`; they expire after 365 days — note the date.
3. Signing: Windows code-signing certificate (OV or EV; or Azure Trusted Signing if eligible — note the alternative and that the workflow would need its own step) exported as PFX → base64 → secrets; Apple Developer Program membership, a "Developer ID Application" certificate exported as .p12 → base64 → secrets, the team id, an app-specific password for the Apple ID.
4. The `release` environment with the owner as required reviewer.
5. Cutting a release: run `build.yml` (inputs explained), check artifacts, re-run with `publish=true`; how to withdraw (`POST platform/releases/{id}/withdraw` from the admin UI); how `min_version` forces updates; beta → stable.
6. What a photographer sees for an unsigned build on each OS, and how to open it anyway (for beta testers).

- [ ] **Step 1: Write the runbook against the real workflow and scripts (every secret, input and command name must match the files); land: "Docs: release runbook".**
