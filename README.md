# LensFlow Image Editor

A native desktop RAW editor for LensFlow photo studios, written in Rust. It
brings the LensFlow web editor's develop pipeline to the desktop so that
studios can edit full-resolution RAW files locally and get the same look the
web editor produces.

## Status

Part 2a (core pipeline parity: the settings model and a headless GPU renderer
that reproduces the web editor's output), Part 2b (RAW decoding and
development, so `lf-render` opens camera files), Part 2c-i (the grade, account
and catalogue libraries) and Part 2c-ii (the desktop application, `lf-app`) are
built. Packaging (2d) follows.

The application signs in with LensFlow and enforces the studio's entitlement,
catalogues folders, and shows a library grid and filmstrip. Its Develop view
draws the preview straight into a texture egui shows, with every web control:
the adjust panels, the crop and straighten tool, and the heal and clone spot
tool, and Guided Upright (draw two edges that should be upright, or level, and the
perspective follows). It keeps one history step per gesture, with undo and redo, and writes
sidecars after a quiet second. It also has looks and studio presets synced with
LensFlow (the one applied last has an Amount slider, 0–200 %, until the photo is edited
another way); Auto-sync (with several photos selected, each change in Develop is made on
all of them, except their crop, spots and masks: Home → Settings → Auto-sync, or the box
in Develop); a Calibration section (the red, green and blue primaries' hue and saturation,
and a shadows tint); DNG camera profiles for RAW photos (`.dcp`, imported from the RAW
section: Adobe's DNG Converter installs Adobe Standard and Camera Matching profiles; none
ship with the app); copy and paste of edits, "Apply to N photos", "Rename…" for the
selected photos' files (a pattern with `{name}`, `{seq}`, `{date}`, `{camera}`; sidecars go
with them, and a name already taken refuses the whole batch), named versions,
"Match a reference…", and export to disk. The window has a ribbon, as in Microsoft Office
(File, Home, Library, Develop, View, Account and Help tabs), one status bar, Preferences, an About box
and a keyboard map. CI builds the app for Windows, macOS and Linux. How it is
put together is in [docs/app.md](docs/app.md).

## Running the app

```bash
cargo run --release -p lf-app
```

This opens the `lensflow-image-editor` window. Sign in with a LensFlow account
whose studio has lensflow-ImageEditor, then choose a folder for the catalogue.
Photos stay where they are; the catalogue only remembers them. Add a folder of
photos with File → Add folder…, then press `D` (or double-click a photo) to
develop it. Preferences, the log folder and the keyboard map are in the Edit and
Help menus.

Each CI run uploads a release build per platform as an artifact:
`lensflow-image-editor-windows`, `lensflow-image-editor-macos` and
`lensflow-image-editor-linux`. They are unsigned test builds; installers
come with packaging (2d). On Linux the folder picker uses the XDG desktop
portal (or zenity). Typing a folder path always works.

### Grading a whole folder from the console

```bash
lensflow-image-editor grade
```

It asks for a reference picture and a folder of photos, then fits one grade to
the folder exactly as "Match a reference…" does and writes it beside every
photo. There is no window, no catalogue and no sign-in, and it is quicker than
doing the same in the dialog. Options: `--reference <file>` and `--folder <dir>`
to skip the questions, `--out <dir>` to write graded JPEG copies instead of edit
files, `--strength <0-100>`, `--exposure` to even out brightness photo by photo,
`--recursive`, `--dry-run`, and `--yes` to skip the confirmation. `grade --help`
lists them.

The photos are never modified: the edit files are new `<photo>.lensflow.json`
sidecars (a damaged one is left alone and that photo skipped). `Ctrl`+click
adds or removes one photo and dragging a box over the grid selects everything
inside it.

Export to a gallery (File or Library tab): uploads the selected photos, rendered as JPEGs,
to a LensFlow gallery and section, while the plan allows it. Photos already there are
skipped as duplicates.

Lens corrections (a section after Effects): distortion, lens vignetting, red/cyan and
blue/yellow fringes, and vertical and horizontal perspective, kept for the desktop only.
"Apply lens profile" sets distortion, lens vignetting and fringes for the photo's lens at its
focal length and aperture, from the lensfun database (downloaded once, a few MB).
Upright (Vertical or Full) finds the straight lines in the photo and sets the perspective
and straightening that make verticals upright and the horizon level.

Masks (`M`): linear and radial gradients and a brush, each with its own exposure, tone,
white balance, saturation and clarity. Gradients are moved by their handles on the photo; a
brush mask is painted (size, feather, flow; Alt or "Erase" rubs out). A Range narrows any
mask to a brightness band (only the highlights of the sky) or a colour (only the blues;
Alt-click the photo to pick it). Select subject and Select sky (AI, on this computer)
add a mask of the main subject or the sky, found in the photo; Select object then a click
on the photo masks whatever was clicked (MobileSAM); Foreground and Background mask what is
near or far (from the photo's depth); the Face buttons mask every face's
eyes, irises, lips or teeth (MediaPipe's face landmarks); the People buttons add a
mask of everyone in it, or of their face skin, body skin, hair or clothes (best on people
who fill a good part of the frame). They are kept for
the desktop only (the web editor keeps them but does not draw them).

White balance: the Colour panel's list (As shot, Auto, and presets: Daylight, Cloudy, Shade,
Tungsten, Fluorescent, Flash, worked out from the RAW's as-shot colour temperature, shown
under the list) and its eyedropper (`W`: click
something grey or white). Auto tone is a button in Light (`Ctrl+U`); Auto white balance is
`Ctrl+Shift+U`.

Culling follows Lightroom: `P`, `X` and `U` pick, reject and unflag; `0`–`5`
set stars; `6`–`9` set a red, yellow, green or blue label (pressed again, it
comes off). In the grid they mark every selected photo, in Develop the open one.
The bar over the grid shows only some flags, ratings or labels and sorts by
capture time, file name or rating. Marks live in the catalogue, not in the edit
files. See [docs/app.md](docs/app.md).

Edit in external editor (`Ctrl+E`): a 16-bit TIFF copy with the edits, beside the photo,
opened in the application you choose once (Photoshop, Affinity, …) and catalogued next to
the original.

Merge to HDR (`Ctrl+H`, File → Merge): two to seven bracketed photos lined up and
blended into one natural-looking 16-bit TIFF beside the first, catalogued and ready to edit.
"Remove ghosts" (on by default) shows something that moved between the shots once, as in
the middle photo, instead of as a faint double.
Merge to panorama (`Ctrl+M`): up to 24 overlapping photos stitched into one wide photo,
in one row or up to four (choose Rows, and select them row by row, top row first).
Focus stack (File → Merge): up to thirty photos of one subject focused at different depths,
lined up and blended so each part comes from the photo where it is sharp.

Print (`Ctrl+P`): the selection laid out on A4, Letter, A3 or photo papers, one a page or a
contact sheet, with captions; opened as a PDF to print, or saved for a lab.

LUT looks (Develop → LUT look): `.cube` 3D LUTs imported into the catalogue, applied to
the finished colour with an amount; desktop only (`desktop.lut`).

Import from card (File → From card): copies the photos on a memory card (or any
folder) into a folder you name, optionally renamed (`{date}_{seq}`, ...) and copied to a
backup folder too, skipping photos the catalogue already has, then catalogues the
copies. The card is only read.

Metadata (View → Metadata): a title, caption, creator, copyright and keywords for
the selected photos, with the folder's keywords a click away. The search box over
the grid finds photos by those, the file name, camera or lens. Exported JPEGs
carry them as XMP (and the creator and copyright in EXIF).

Albums (Home → Albums, or the sidebar under the folders): gather photos from any
folders into an album (`Ctrl+N` makes one with the selection, `Ctrl+B` adds to one),
in the order you add them. A smart album keeps a rule instead (stars, flag, colour
label, words, taken within the last week or month) and always holds what matches.
Albums live in the catalogue; deleting one never touches its photos.

Map (View → Map): where the photos shown were taken, from their EXIF GPS, as pins on
OpenStreetMap; a click on a pin selects its photos. Tiles download when shown (only the
tile's address is sent) and are kept for next time. Photos added before this version are read
for their place the first time the map shows them.

Compare (`C`, Library → Compare): two to four selected photos side by side, large
and with their edits, zoomed and panned together to pick the sharpest of a burst.
Click one to make it active; flags, stars and labels go to it alone; `←`/`→` move
between them; `Esc` goes back to the grid.

Show clipping (`J`, View → Clipping): in Develop, highlights clipped to white show
red and shadows crushed to black show blue, while you edit. The preview only:
exports never carry it.

Soft proof (`S`, View → Soft proof): the preview shows the photo as a printer and paper ICC
profile (a lab's, RGB or CMYK) would print it. View → Proof profile… picks the profile;
it is remembered.

AI tools (the AI tab), run on this computer, nothing uploaded: AI Denoise, AI Upscale
(2× or 4×), Remove objects (paint over what should go), Retouch portrait (smooths the skin of the faces found) and Remove background
(a PNG with transparency). Each photo is rendered with its edits and the result saved
beside it as a new file, catalogued next to the original, which is never changed. The
first use of a tool downloads the AI engine (ONNX Runtime) and that tool's model once
(the dialog says how much); each file is checked against a pinned SHA-256. They need the
studio in good standing, and use none of the plan's server AI allowance.

Lens blur (AI → Enhance → Lens blur): finds how far each part of the photo is (Depth
Anything V2 Small, on this computer) and blurs what lies in front of and behind the subject
as a wide aperture would, bright points blooming into discs; the subject (the nearest part
of the middle of the frame) stays sharp within the focus range. Saved beside the photo as
`<name>-LensBlur.tif`.

Assisted culling (AI → Cull): scores the selected photos (or every photo shown) for blur,
missed focus on the eyes and closed eyes, then offers to show or reject the ones that look
wrong. Nothing is flagged until you say so. The grid marks a likely reject with an amber
triangle (hover says why); the filter bar's "Possible rejects", "Blurry", "Eyes closed" and
"Sharp, eyes open" choices and the Sharpness sort use the scores. A photo is scored once,
until its file changes.

Stacks (Home → Stacks): Group into stack (`Ctrl+G`) gathers the selected photos under the
active one; Unstack (`Ctrl+Shift+G`) takes them apart; Expand (`Shift+S`) opens or closes a
stack. A collapsed stack shows its top photo with how many it holds. Auto-stack… finds
bursts (frames taken within a set gap that look alike) and, if asked, duplicates anywhere in
the folder, and stacks them with the sharpest photo on top when Assisted culling has scored
them.

People (AI → Find people): finds the faces in the selected photos (or every photo shown)
and groups the ones that look like the same person under People in the sidebar. Name a
group to see every photo of that person; later scans match new photos of them by
themselves. Typing a name that is already taken merges the two; "Not <name>" on selected
photos takes them out (they are never matched to that person again); Forget drops a name
and leaves the faces to be named again. Names are found by the search box. A photo is
scanned once, until its file changes; nothing leaves the computer.

Keywords (AI → Find keywords): suggests up to five words per photo from a list of about
two hundred (beach, cake, bride, sunset, dog...), so the search box finds photos nobody has
keyworded. Suggestions are kept apart from your keywords and never written to sidecars or
exports; the Metadata panel shows them under "Suggested by AI", and a click keeps one as an
ordinary keyword. The word list is `crates/lf-ai/data/keywords.txt`.

The window needs a graphics adapter. With no driver it runs on a software
adapter (WARP on Windows, lavapipe on Linux) and says "No graphics driver found:
previews will be slow."

### Keyboard map

The keys follow Lightroom's defaults where Lightroom has one. On macOS, `Ctrl`
reads as `Cmd`. Keys do nothing while you type in a field or a dialog is open,
and the tool keys work only in Develop.

| Key | Action |
|-----|--------|
| `G` / `D` | Library / Develop |
| `Ctrl+Shift+E` | Export… |
| `Ctrl+E` | Edit in external editor… |
| `Ctrl+P` | Print… |
| `Ctrl+H` | Merge to HDR… |
| `Ctrl+M` | Merge to panorama… |
| `R` | Crop and straighten |
| `Q` | Heal and clone spots; Brush paints a healing stroke along a hair or a wire |
| `W` | White balance eyedropper (click something grey or white) |
| `M` | Masks: linear and radial gradients and a brush, with their own sliders |
| `Ctrl+U` | Auto tone |
| `Ctrl+Shift+U` | Auto white balance |
| `\` (held) | Before/After |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z`, `Ctrl+Y` | Redo |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy settings… / paste settings |
| `Ctrl+A` | Select all |
| `←` / `→` | Previous / next photo (`↑` / `↓` move by a row in the grid) |
| `Space` | Fit ⇄ 100 % |
| `Esc` | Leave the tool |
| `Enter` | Apply the crop |
| `Delete` | Remove the selected spot |
| `P` / `X` / `U` | Flag as pick / rejected / unflag |
| `0`–`5` | No stars to five stars |
| `6`–`9` | Red, yellow, green, blue label (again: off) |
| `F1` | About |

## Crates

- `lf-settings`: the develop settings model and `normalise`, plus curves,
  shader values, geometry, looks and settings groups, ported from the web
  editor.
- `lf-pipeline`: the headless wgpu renderer (`Gpu`, textures with mip chains,
  and the copy, gaussian, spots and develop passes) behind `Renderer`, which
  renders, draws a preview, and returns pixels and a histogram. It also holds
  `Renderer::new` (8-bit source), `Renderer::from_texture` (half-float
  source with headroom above 1.0) and `Renderer::from_frame` (uploads a half-float
  frame under the call's own error scopes). Each renderer call runs under its own
  (per-thread) wgpu error scopes, so on a device shared with the UI or other
  threads it reports only the errors it caused (`PipelineError::Gpu`,
  `OutOfMemory`) and then drops its caches; errors raised outside any call are
  logged and kept for `Gpu::take_error`.
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
`https://dev.lensflow.in/api` (`Instance::Prod` / `Dev`; `Instance::custom(url)`
for anything else, which refuses a URL that is not `https`, except `http` to
`localhost` or `127.0.0.1` for local development and tests). Every request carries `Accept: application/json`,
`X-LensFlow-Client: desktop` and `User-Agent: lensflow-ImageEditor/<version>
(<os>)`; once signed in, `Authorization: Bearer <token>`; once a studio is
chosen, `X-Studio: <slug>`. Timeouts: 20 s to connect, 30 s to read (120 s for
grading).

Endpoints used: `POST auth/login` (email, password, optional
`two_factor_code`; a reply with `two_factor_required` means ask for the code
and call again), `GET auth/me`, `POST auth/logout`, `GET desktop/entitlement`,
`GET desktop/download/{platform}`, `POST desktop/grade` (multipart: mode
`extract` or `review`, a reference JPEG and a contact-sheet JPEG each under
700 KB, up to 16 example looks, and for a review the current settings; the
examples JSON must stay within 16,384 characters and the settings JSON within
8,192, checked before sending), and
`galleries/edit-presets` (list, create, PATCH, delete). Preset sync names a
local preset `<name> (desktop)` (or `(desktop 2)`, ...) when the name is taken.
Presets deleted on the desktop are tombstones until the next sync deletes them on
the server; a synced preset deleted on the web is removed locally.

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
| 200 the client cannot read | Transient (`BadReply`, logged): a payload change never locks the app |
| any other 4xx | Refused: locked, with the server's message |

Entitlement rules. The reply carries `enabled`, `standing`
(`active`, `read_only`, `paused`, `suspended`; a standing this version does not
know is read as `Unknown` and treated as read-only), AI availability and
remaining count (optional), the studio, `min_version` and the latest release.
A studio from sign-in can use the desktop editor when its member has
`galleries.manage` (`StudioSummary::can_use_desktop`).

- Active and enabled: everything works. AI grading only while the AI
  allowance is available.
- Read-only: edit locally, but no export to the gallery, no AI and no preset
  sync (`gallery_export_allowed`, `ai_allowed` and `sync_allowed` are false).
  Export to disk is always allowed while the app is unlocked.
- Paused, suspended, or not enabled for the studio: locked.
- Offline: after a network failure, 429 or 5xx, a cached entitlement is trusted
  for 7 days from its `checked_at` (`AllowStale`), unless it was itself a lock.
  Older than 7 days, dated more than 5 minutes in the future (a clock set
  back), or missing: locked with "Connect to the internet to keep using
  lensflow-ImageEditor." A definite answer (401, a lock, studio not found)
  clears the cache, so an older active entitlement is never replayed.
  `Gate::evaluate`/`refresh` take the studio id asked about; a cached
  entitlement for another studio counts as missing.

Where things live. The sign-in token goes in the OS keychain (service
`lensflow-ImageEditor`, account = the API base URL). With no usable keychain
(mostly Linux without a Secret Service) it falls back to a file `token-<hash>`
(0600 on Unix) in the `directories` project config dir (`ProjectDirs::from("in",
"lensflow", "lensflow-ImageEditor")`), with a logged warning; on a system with
no config dir, `<temp dir>/lensflow-ImageEditor` (never the current directory).
The file is encrypted (ChaCha20-Poly1305) with a key bound to this machine and
user account: HKDF-SHA256 over the machine id and the user id (plus the user
name on Windows), with a random salt kept next to it in `token-salt` (0600) and
the base URL mixed in. That protects the token file copied on its own, or the
config folder copied without the machine's identity, and casual reading. A full
copy of the disk can be read, as can software running as the same user; the OS
keychain cannot fully prevent the latter either. A file that cannot be read on this
machine is deleted and you sign in again. A plaintext token file from an older
build is read once and re-saved encrypted. The entitlement cache is
`entitlement.json`, written atomically in a folder the app chooses
(`entitlement::Cache::new(dir)`); the crate does not pick that folder.

## Catalogue (lf-catalog)

`Catalog::open(root)` creates `<catalogue root>/.lensflow/catalog.sqlite` in
WAL mode (bundled SQLite) and migrates it under a write lock; the schema
version is 6 (six migrations). A catalogue with a higher version was made by a
newer app and is refused (`CatalogError::NewerSchema`, "This catalogue was made
by a newer lensflow-ImageEditor."), never downgraded. Tables: folders, photos
(with `error`, `sidecar_error` and `missing`), edits, history and history_cursor, versions,
thumbnails, presets (per studio, with `deleted_at` tombstones), export_jobs and
sidecar_meta. Each thumbnail worker opens
its own connection.

Sidecars. Edits are also written beside each photo as
`<file>.lensflow.json`: `v` (format version, currently 2), `settings` (the
edit settings), `versions[]` (`id`, `label`, `settings`, `created_at`) and
`updated_at`. Keys this version does not know are kept and written back
unchanged. A version without an `id` or `created_at` is kept (a new id, the
epoch date). Writes are atomic (temp file in the same folder, then rename), and
an existing file that is not a valid sidecar is never replaced (`Corrupt`).
Conflict rule on reopen and on import (`edits::reconcile`): the newer
`updated_at` wins. A sidecar must be strictly newer to replace the catalogue;
adopting it takes its settings and versions and restarts the undo history from
that state, and a stamp in the future is adopted as now (the sidecar is then
rewritten with it); a stamp finer than milliseconds is adopted truncated to the
stored milliseconds and the sidecar rewritten, so it cannot win every reopen.
Otherwise the catalogue wins, and the sidecar is rewritten
only when the catalogue is strictly newer or the content (`v`, settings,
versions) differs: an equal sidecar is left alone, so reopening does not touch
every file. A sidecar read successfully (or absent) clears `photos.sidecar_error`.
`edits::reconcile_on_open(catalog, photo, path)` (used when Develop opens a photo)
reads the sidecar from disk and reconciles it; one that cannot be read is recorded in
`photos.sidecar_error` and skips the reconcile, so it is never treated as absent.
`edits::history_steps(catalog, photo)` says how many steps undo and redo can take.

Import. `import::scan` lists supported files under a folder (RAW and
JPEG/PNG/TIFF/WebP by extension; hidden entries, `.lensflow` and sidecars are
skipped), and `import::run` records them in batches, one transaction each. It
reads headers and EXIF only. A file already catalogued with the same size and
mtime is skipped, so a rerun resumes. A file that cannot be read is still
added, with kind `unreadable` and the reason in `photos.error`; a decoder panic
is caught and recorded as "the decoder crashed on this file". For every new or
changed file the sidecar beside it is read and reconciled in the batch's
transaction, so edits, versions and unknown keys made elsewhere are adopted
before anything can overwrite them; a sidecar that cannot be read is reported
in `photos.sidecar_error` (apart from the file's own `photos.error`) and left
untouched. `import::scan` also reports whether the folder itself could be read
(`Scan::root_readable`). `import::mark_missing(catalog, folder,
&scan)` flags rows whose file is gone (`photos.missing`); `run` clears the
flag when the file is seen again; a scan that could not read the folder, or
found no files at all (an unplugged drive), marks nothing; `import::mark_missing_confirmed`
flags them all once the user confirms the folder was emptied. The progress callback can stop the import
after the batch it reports.

Thumbnails. `Thumbnailer` runs a small worker pool (default `min(2, cores)`)
over a priority queue (`High` for what is on screen, `Normal` otherwise),
renders 256 px thumbnails through the shared GPU, and stores JPEGs in the
catalogue. A stored thumbnail is served without decoding while it is fresh
(`Catalog::fresh_thumbnail`): its key is the edit stamp (`edits.updated_at`, or
none), the photo row's `updated_at`, size, mtime and the demosaic quality, so
an edit, a changed file or a thumbnailer at another quality makes it stale.
`take_ready` returns finished ones; `take_failed` returns
`(photo id, reason)` for photos that could not be made (the reason is also
stored in `photos.error` and logged; id 0 means a worker could not open the
catalogue). A graphics device error is not the photo's fault: nothing is
stored, `take_device_error` reports it once and the workers stop. A successful
thumbnail clears `photos.error` (never `sidecar_error`).
`cancel_pending` forgets queued requests; dropping the `Thumbnailer` cancels
work in progress between stages. The demosaic `Quality` is a constructor
parameter. Memory: each in-flight open of a 60 MP RAW can use about 1 GB at
`Quality::Fast` (a quarter of that at `Preview`), which is why the pool is
small.

Developed-frame cache. `lf_raw::cache::FrameCache` is a folder the app picks:
one `<sha256>.lff` per file, quality, max edge, develop-settings and `DEVELOP_ALGO_VERSION` key. Format: `LFF1`
magic, a version (currently 2), sizes, flags, an LZ4 block of half-float RGBA
and a CRC32 covering the header and the block. A corrupt or old-version entry is a miss and is deleted. The cache
is trimmed to a byte cap (given to `FrameCache::open`), least recently used
first.

Export. `export::run` writes photos to a folder, one at a time, at full
resolution through the GPU pipeline.

- Formats: JPEG (quality 1..100), PNG (8-bit), lossless WebP (8-bit; the encoder has no
  lossy mode) and 16-bit TIFF (from the float render, clamped to 0..1 only at the encode).
- Every file carries an sRGB ICC v2 profile (`finish::srgb_icc`, built in code), so
  colour-managed viewers show it as the editor does.
- EXIF goes into JPEG, PNG (`eXIf`) and WebP; TIFF carries none (the encoder cannot add
  it). Metadata `All` carries the source JPEG's EXIF byte for byte (or a minimal EXIF
  built from the file's facts for other sources), `CopyrightOnly` writes Artist and
  Copyright, `None` writes nothing.
- After resizing (`finish`): output sharpening for the screen (None, Low, Standard, High:
  an unsharp mask of radius 0.5, 0.8 or 1.2 px that leaves flat areas alone), then the
  watermark: text (set in Ubuntu-Light, white with a soft dark edge) or a logo picture,
  as wide as asked (a fraction of the photo's width), as opaque as asked, in a corner or
  the centre, 3 % of the short side in. A watermark logo that cannot be read, or empty
  text, fails the run before anything is written.
- Formats: JPEG, PNG, WebP (lossless, or lossy at a quality), AVIF (quality; rav1e at speed
  7) and 16-bit TIFF. PNG carries XMP in `iTXt`; WebP is written as an extended file with
  the profile, EXIF and XMP chunks (`lf_catalog::containers`); TIFF carries the profile,
  XMP and the artist, copyright, description, make and model tags. AVIF carries no EXIF or
  XMP yet (its encoder has no way in).
- Colour space: sRGB, Display P3, Adobe RGB (1998) or a lab's RGB ICC profile (Little
  CMS, perceptual), with that profile embedded. The render is sRGB, so a wider space holds
  the same colours. AVIF exports are sRGB only.
- Presets: the export dialog's Preset bar applies a built-in preset (Web 2048 px
  JPEG, Full size JPEG, Print 16-bit TIFF) or one of the studio's own, saved with
  "Save as preset…" (`prefs.export_presets`); a preset saved without a folder
  keeps the folder already chosen.
- Size: `Original`, `LongEdge`, `ShortEdge`, `Megapixels`, `Percent`. Only
  `Percent` can enlarge; the others never upscale.
- Size: a `Percent` that would exceed the device's largest texture edge is
  refused for that photo (`TooLarge`).
- Rename pattern tokens: `{name}` (original stem), `{seq}` (1-based, padded to
  the count's width), `{date}` (`YYYY-MM-DD`, capture date else file date),
  `{camera}` (make and model with `-`, or `unknown`), `{title}` (the catalogue
  title, else the original stem). Path-illegal characters
  become `-`, and Windows reserved names get a `_` (checked on the whole file
  name, so a pattern of just `CON` is safe too).
- Collisions are refused before anything is rendered: two photos on one path
  (compared case-insensitively) is `NameCollision` (add `{seq}`). With
  `overwrite: false` an existing file is `Exists` and nothing is written; with
  `overwrite: true` it is replaced through a temp file and a rename, so a
  failure never leaves a truncated file.
- One bad photo does not stop the run: `run` returns an `ExportReport`
  (`exported`, `failed` with each photo's error, `cancelled`). The progress
  callback returns `ControlFlow`; `Break` cancels after the current photo. The
  job is recorded in `export_jobs` as `done`, `failed` (with how many failed)
  or `cancelled`. A decoder panic fails only that photo.
- A lost graphics device stops the run where it is: the report comes back with
  `device_lost` set and the photos already written (the job is `failed` with
  `DeviceLost`'s message).
- An export never writes over a source photo: an output path equal to any
  catalogued photo (case-insensitive, after resolving the path) or a sidecar
  name is `WouldOverwriteOriginal`, even with `overwrite: true`, before
  anything is written.

## Supported files

- RAW (checked against the sample corpus): Canon CR2 (full resolution and
  sRAW) and CR3, Nikon NEF, Sony ARW, Fujifilm RAF (X-Trans), Olympus ORF,
  Panasonic RW2, Pentax PEF and DNG.
- JPEG, PNG, TIFF and WebP. TIFFs exported by Lightroom, Photoshop or DPP open
  as pictures even when they carry the camera's `Make` (Canon, Hasselblad...):
  a TIFF is treated as RAW only on RAW evidence (a `DNGVersion` tag, the CR2
  signature, or CFA / LinearRaw sensor data in an IFD or SubIFD).
- HEIC (iPhone photos) opens through the computer's own decoder: on Windows, Microsoft's
  "HEIF Image Extensions" and "HEVC Video Extensions" (Microsoft Store); on macOS, built in
  (`sips`); on Linux, libheif's tools (`heif-dec`). Without one, the error says what to
  install. AVIF input is not supported yet.
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
format, plus a full-resolution CR2 next to the Canon sRAW one; about 88 MB). Fetch them with `scripts/fetch-raw-samples.sh` (or
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
