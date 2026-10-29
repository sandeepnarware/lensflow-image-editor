# How the desktop app is put together

`crates/lf-app` builds the `lensflow-image-editor` binary: one native window on
`eframe` (egui with the wgpu renderer). This page describes the threads it uses
(lanes), the session state machine, how the Develop preview reaches the screen,
where files live, and the keyboard map.

## Starting, one at a time, and quitting

`main.rs` reads the command line before anything else. `--version` (`-V`) prints
`lensflow-ImageEditor <version> (<target triple>)` and `--help` (`-h`) prints the
usage; both exit 0 before any window or graphics work. Any other flag prints the
usage to stderr and exits 2. A Windows release build has no console of its own, so
it attaches to the terminal it was started from to print these.

Only one copy runs at a time. The app holds an exclusive lock on
`<config dir>/instance.lock` (the operating system's file lock, so a crash lets
it go too). A second launch shows "lensflow-ImageEditor is already open." and
exits. A restarted process (after a device reset or an AppImage update) carries
`LENSFLOW_RESTARTED=1` and waits up to ten seconds for the old one's lock.

`on_exit` runs in this order: the open photo's gesture is committed; an export,
a batch, an import and a match are told to stop; the lanes finish their running
jobs (up to ten seconds); an export cut short has its temp files removed; the
waiting sidecars are written; the preferences are saved; the instance lock is let
go; and only then does a downloaded installer or a restart start.

### Updates

The update download runs on the `Long` lane into `<cache dir>/updates/<file>.part`.
A part left by an interrupted download is resumed with an HTTP `Range` request
when the server answers `206` (any other answer starts again). More bytes than
the release's size are refused and the part deleted. The SHA-256 is checked
before the file gets its own name, and older files in `updates/` are then
removed. A verified installer never starts while the app still holds its files:
the window closes (asking first if an export runs), and the end of `on_exit`
hands it over:

| System | Hand-off |
|--------|----------|
| Windows | `msiexec /i <msi> /passive` |
| macOS | `open <dmg>`; the disk image's window shows how to drag the app over |
| Linux, from an AppImage (`$APPIMAGE` set) | The new AppImage replaces it (a temp file in the same folder, mode 755, then a rename) and starts |
| Linux, from the .deb | `xdg-open <deb>`; the system installer asks for the password |

A restart from an AppImage starts `$APPIMAGE`, not `current_exe()`, which points
inside the AppImage's temporary mount.

### Going back to an older version

Opening a catalogue written by a newer version says so (`NewerSchema`) and
leaves it untouched. Preferences written by a newer version load: fields this
version does not know are ignored, and missing ones take their defaults.

Help → About shows the log folder ("Show log folder") and "Copy diagnostics",
which puts the version and target, the operating system, the graphics adapter
(name, backend, software or not, largest texture), the log folder, the catalogue
and the server on the clipboard for a support request. It holds nothing secret.

## The console's grade

`lensflow-image-editor grade` is "Match a reference…" without the window: it fits one
grade to a whole folder and writes it beside every photo, so a studio can grade a shoot
faster than by clicking through the dialog. It asks for the reference picture and the
folder, or takes them as `--reference`/`--folder`; `--out <dir>` writes graded JPEG
copies instead of edit files, `--strength` blends the grade, `--exposure` evens out
brightness photo by photo, `--recursive` walks subfolders, `--dry-run` only reports, and
`--yes` skips the confirmation. It needs no catalogue, no sign-in and no AI: up to 24
untouched photos are measured against the reference and the grade is calibrated from
those measurements (`ai: None`, `review: false`), exactly the dialog's own
`match_reference` code with its pixels opened straight from disk.

The photos are never touched. Without `--out`, each photo's `<photo>.lensflow.json` is
rewritten keeping its named versions, its unknown keys and its creation time, stamped
with the current time so it wins the newer-wins rule when the folder is added later; a
damaged sidecar is never overwritten, and that photo is skipped with the reason. A photo
keeps its own crop, spots and versions — the grade arrives as a look, as the dialog
applies it. Exit codes: 0 everything graded, 1 some photos skipped or the question
answered no, 2 nothing graded (bad paths, no photos, no graphics device).

The command runs before any window work and opens its own device (`gpu::headless`,
DirectX 12 and OpenGL on Windows, the same adapter choice as the window). A Windows
release build has no console: it joins the terminal it was started from if there is one
and otherwise makes a console window, which it keeps open after the report.

## The window and the one device

`main.rs` opens the window with `NativeOptions { renderer: Wgpu }` and asks
eframe for a device with the adapter's own limits (`gpu::request_full_limits`),
because egui's default 8192 px texture limit would refuse 45 to 100 MP photos.
The same call settles the backends: DirectX 12 and OpenGL on Windows, everything
eframe enables elsewhere (`WGPU_BACKEND` overrides). Windows leaves Vulkan out
because a Vulkan-enabled instance can report no OpenGL adapter at all on an old
NVIDIA driver, which leaves only the Basic Render Driver and draws every preview
on the CPU; the pair alone finds that machine's GPU. wgpu then picks the adapter
by `HighPerformance`, so a hardware GPU wins over a software rasteriser.
`App::new` wraps eframe's device once with `Gpu::from_device`, so the preview,
the thumbnails, the exports and "Match a reference" share that device. There is
never a second device.

Each `Renderer` call reports its own errors through per-call error scopes (in
`lf-pipeline`). An error raised outside any call is logged once by the library's
handler. The app's `gpu.take_error()` tap only counts these errors. Three in a
minute show one toast: "The graphics device reported several errors. If a photo
looks wrong, reopen it." A lost device (driver reset, sleep) shows the banner
"The graphics device was reset. Restart lensflow-ImageEditor to continue." and
offers a restart. The app never panics on it. A device error that stops only the
thumbnails says "Thumbnails stopped because of a graphics error. They will
restart when you change folders." An export cut short by a lost device logs how
far it got.

Every frame, `logic()` drains the jobs' messages, ticks the session, reads the
keyboard map, and taps the device. `ui()` then draws the ribbon, the banners,
the status bar, the screen and the dialogs.

## Lanes

Slow work never runs on the UI thread. `jobs::Jobs` keeps one serial worker
thread per lane. Each job returns a `Msg`, and `logic()` drains them every frame
(the worker wakes the UI with `request_repaint`). A job that panics becomes
`Msg::JobFailed`, and its lane keeps working.

| Lane | Thread | What runs there |
|------|--------|-----------------|
| `Account` | `lf-account` | Sign-in, `auth/me`, the entitlement gate, preset sync, sign-out |
| `Open` | `lf-open` | Opening and decoding a photo for Develop (its `Renderer` is built there), pre-opening neighbours |
| `Io` | `lf-io` | Short disk work: opening the catalogue, sidecar writes, folder removal, marking photos missing, rebuilding the thumbnail pool |
| `Query` | `lf-query` | Quick catalogue reads (folders, a folder's photos), so they never wait behind an import |
| `Long` | `lf-long` | Work that can take minutes: scans and imports, exports, "Apply to N photos", "Match a reference" fits and applies, the installer download |

Long work has a lane of its own so that a sidecar write, a folder removal or the
opening of a photo never waits behind an export or an import. When the window
closes, the export, the batch, the import and a match are told to stop after
their current photo, every lane finishes its running job (up to ten seconds),
and only then are the waiting sidecars written (the whole order is under
"Starting, one at a time, and quitting").

An export's temp files are named `.lfexport-<name>.<pid>-<n>.tmp`, and its sweep
of unfinished files matches only those, so it never touches a sidecar being
written beside a catalogued photo in the same folder. A match fit or apply
queued behind an export or an import says "Waiting for the export to finish…" or
"Waiting for the import to finish…" until it starts. A folder being scanned or
imported cannot be removed from the catalogue, and a folder added with "Add
folder…" is shown when its import ends (a rescan leaves the view alone). An
edited photo keeps its old thumbnail on screen until the new one arrives.

`lf-catalog`'s thumbnail pool has its own threads. Folder pickers run on
short-lived threads of their own, so a picker that never answers holds up
nothing. Each job opens its own `Catalog` connection; the UI thread holds only
the open photo's edit session.

Messages that can arrive late carry what they belong to: a generation for photo
opens and account answers, a ticket for folder choices, and the catalogue root
for folder lists. A stale answer is dropped.

## The session machine

`session::Session` is a pure state machine fed by `lf_account::Decision`. The
screens follow from the state, and `session.rs` is unit-tested without a window.

```
SigningIn ──signed in──▶ ChoosingStudio ──chosen──▶ (entitlement gate)
                              ▲                          │
                              │ Switch studio            ├─ Allow / AllowStale ─▶ Active
                              │                          ├─ below minimum ─────▶ UpdateRequired
                              └──────────────────────────├─ Lock ──────────────▶ Locked
                                                         └─ no permission ─────▶ NoPermission
any state ──401 from any call──▶ SigningIn ("You were signed out. Sign in again to continue.")
```

- `SessionFlow` (in `session_flow.rs`) owns the account state, runs the calls on
  the `Account` lane and feeds their answers to the machine. Each sign-in, studio
  switch or server switch starts a new generation, and older answers are
  ignored.
- The gate is checked at launch, every 4 hours, before each AI request, and from
  Account → Check entitlement now. `AllowStale` shows "Last checked N hours
  ago" in the status bar.
- Read-only (a lapsed plan) shows a banner. Editing and export to disk keep
  working; preset sync and AI are off.
- A sign-out from a revoked token never stops a running export, because the
  export job holds no session. Edits are already in the catalogue.

## The preview texture

The open photo's `Renderer` lives on the UI thread. `develop::preview::Preview`
owns an `Rgba8Unorm` canvas the size of the photo's on-screen rectangle in
physical pixels. The canvas is registered with egui-wgpu as a native texture.
`Renderer::draw` renders straight into it at viewport size, and egui shows it
with no readback.

The preview is laid out inside a `PREVIEW_GUTTER` (12 points) margin, so the
photo never sits flush against the window edges or the adjust panel; the side
panel reserves the same 12 points each side around the 256-point histogram.

On a software adapter (no graphics driver: WARP, lavapipe, SwiftShader) the
develop shader runs on the CPU, so `Preview::canvas_size` caps the canvas at
`SOFTWARE_EDGE` (1024) long edge and egui stretches it over the same rectangle.
Framing and edits are unchanged; the preview is slightly softer, and a drag
costs a fraction of the CPU time. The banner says so and names the fix.

The canvas is drawn again only when its size, the view (zoom and pan), the
before/after state or the settings change. While a slider is dragged, each frame
draws once. The histogram is remade at most every 200 ms after the last change.
The gesture's end commits one history step, and the sidecar is written a quiet
second later on the `Io` lane.

## Where files live

The folders come from `directories::ProjectDirs::from("in", "lensflow",
"lensflow-ImageEditor")`:

| What | Where |
|------|-------|
| Preferences | `<config dir>/prefs.json` (File → Preferences) |
| Single-instance lock | `<config dir>/instance.lock` |
| Window size and position | `<config dir>/window.ron` |
| Cached entitlement | `<config dir>/entitlement.json` (the development server: `entitlement-dev.json`) |
| Sign-in token | The system keychain; without one, an encrypted file in the config dir |
| Log | `<data dir>/logs/app.log`, rotated at 5 MB, three old files kept (Help → Show log folder) |
| Decoded RAW frames | `<cache dir>/frames`, 4 GB by default (Preferences: frame cache size) |
| Downloaded updates | `<cache dir>/updates` (the latest installer only) |
| Catalogue | `<catalogue root>/.lensflow/catalog.sqlite`; the root is chosen on first run. Studio presets in it are kept per studio and per server (a development-server studio's key is its negated id) |
| Edits beside each photo | `<photo>.lensflow.json` sidecars |

On Windows the config and data dirs are
`%APPDATA%\lensflow\lensflow-ImageEditor\config` and `...\data`, and the cache
dir is `%LOCALAPPDATA%\lensflow\lensflow-ImageEditor\cache`. On macOS the config
and data dirs are both `~/Library/Application Support/in.lensflow.lensflow-ImageEditor`,
and the cache dir is `~/Library/Caches/in.lensflow.lensflow-ImageEditor`. On
Linux they follow XDG (`~/.config`, `~/.local/share` and `~/.cache`, each with a
`lensflow-imageeditor` folder).

The app never rewrites a source photo. Edits go to the catalogue and the
sidecars, and exports go to new files.

Preferences → Catalogue folder → Change… restarts the library on the first-run
screen. First the open photo's gesture is committed, a running import stops
after its batch, the waiting sidecars are written, and the old thumbnailer and
catalogue connections are dropped. The screen offers "Keep the current
catalogue" to go back. Change… is off while an export or "Apply to N photos" is
running, or while "Match a reference" is open.

## The ribbon and the keyboard map

The window has a ribbon in the manner of Microsoft Office (`menu.rs`) and one status bar.
The ribbon is a row of tabs; under the chosen tab are its groups of large buttons, each an
icon over its name, with the full name and key in its tooltip:

- **File**: Photos (Add folder, Export), App (Preferences, Quit).
- **Home**: History (Undo, Redo), Settings (Copy, Paste, Apply to selected), Selection
  (Select all).
- **Library**: Flag (Pick, Reject, Unflag), Rating (no stars to 5), Colour label (red to
  purple, none), Export.
- **Develop**: Tools (Crop, Spots, White balance), Auto (Auto tone, Auto WB), Photo (Match
  reference, Add version, Reset all), History.
- **View**: Mode (Go to Library, Go to Develop), Preview (Before/After, Fit, 100 %).
- **Account**: the studio's name, Switch studio, Check entitlement now, Sign out.
- **Help**: About, Show log folder, Keyboard shortcuts.

The ribbon follows the view: entering Develop brings the Develop tab forward and going back
brings the Library tab, once per change, so any tab can still be chosen. The arrow at its
right folds it down to the tabs. Buttons that cannot act now are faded and do nothing. The
icons are glyphs from egui's bundled fonts (Noto Emoji and emoji-icon-font, the
`monochrome_emoji_fonts` feature); a test checks every one is there, since a missing glyph
would draw as nothing.

Ribbon buttons and keys are the same `shortcuts::Action`s. The keyboard map,
`shortcuts::MAP`, is a table of `(Action, KeyboardShortcut)` read once per frame
in `logic()`; the ribbon's tooltips show its keys, and Help → Keyboard shortcuts lists it. A
press means the most specific matching entry, so `Ctrl+Shift+Z` is always redo,
never undo. A press whose action is not possible now is left for the widgets;
for example, `Delete` with no spot selected still removes a tone curve point.

Keys never fire while a widget has the keyboard (typing in a field, stepping a
slider with the arrows) or while a dialog is open. The `Esc` that ends typing in
a field does not also leave a tool. The tool keys act only in Develop. Holding
an arrow key or `Ctrl+Z` repeats; holding `Space` or a tool key does not. On macOS,
`Ctrl` reads as `Cmd`.

### Selecting photos in the grid

A click selects one photo. `Ctrl`+click adds or removes one without touching the
rest, and `Shift`+click selects the range from the last click. Dragging with the left
button draws a box (`grid::Marquee`) and selects every cell it covers; holding `Ctrl`
while dragging adds those to what was selected instead of replacing it. The box is
followed before the cells are painted, so the highlight tracks the pointer frame by
frame, and it needs 4 points in either direction to count, so a shaky click stays a
click. A drag that starts in the empty space below or beside the cells draws a box
too. Only the cells on screen are found by a box, and a box that stays in the gaps
between cells selects nothing until it reaches one.

### Colour spaces and soft-proofing

`lf_catalog::colour` wraps Little CMS (`lcms2`, built statically). `ExportSpec::space`
(`OutputSpace::{Srgb, DisplayP3, AdobeRgb, Profile(path)}`, default sRGB) converts the
finished 8- or 16-bit pixels from sRGB with the perceptual intent and embeds that profile
(Display P3 and Adobe RGB are built as matrix/TRC profiles); a CMYK profile is refused for
export. The dialog's Colour space list and Profile… picker (`Purpose::ExportProfile`) set it.

Soft proof (`Action::SoftProof`, `S`; `Action::ProofProfile` picks the file into
`prefs.proof_profile`): `Develop::refresh_proof` builds a 33³ LUT that applies the photo's LUT
look, then sends sRGB through the profile with `SOFT_PROOFING` (perceptual, proof relative
colorimetric) and back, and registers it as `lf_pipeline::lut::SOFT_PROOF`. `DrawOptions::proof`
makes the develop pass sample that LUT at full strength in place of the look; it is rebuilt
when the profile, the look or its amount changes. Renders and exports never use it.

### Export presets

The export form's Preset bar (`dialogs/export_presets.rs`) lists three built-in presets and the
studio's own (`prefs.export_presets`: a name and the `ExportSpec` JSON). Choosing one fills the
form (`export_presets::apply`); a preset whose folder is empty keeps the folder already chosen,
so "Client web 2048 px + watermark" can be saved before any folder is. "Save as preset…" names
the current form (a same-named preset is replaced, case ignored) and "Delete preset" removes the
chosen one of the studio's; both answer `ExportAction::SavePresets` and the library saves prefs.
The file-name pattern also takes `{title}` (the catalogue title, else the original stem).

### Export to a gallery

File → Export to gallery (or the Library tab's Export group) uploads the selected photos
to a LensFlow gallery. It is on only while the gate allows gallery export
(`Decision::gallery_export_allowed`: not read-only, not locked); the app hands the library
the studio's client each frame (`SessionFlow::gallery_link`), and none otherwise. The
dialog lists the studio's galleries that are not archived (most recently changed first,
searchable by title or customer) and the chosen one's sections, and asks for the size
(the original, or a long edge, 3000 px by default) and the JPEG quality (90).

The upload (`gallery_job::run`, on the `Long` lane) renders each photo to a JPEG through
the disk export (so it carries its EXIF and the sRGB profile) into
`<cache>/gallery-upload`, uploads it, and removes it, so one rendered photo at a time is
on disk. Uploads use the web uploader's resumable sessions
(`lf_account::galleries::upload`): `POST galleries/{g}/uploads` with the file's name, size,
type and SHA-1, then either the chunks through the API or, when the gallery's storage
gives a direct link, Azure blocks (ids as the web makes them) and a block list, then
`complete`. A photo the gallery already holds (409 with `duplicate_photo_id`) counts as
"already there", not a failure; a photo that fails is listed and the run goes on. Stop
ends it after the current photo; closing the window leaves it running, with "Uploading 12
of 40" in the status bar and a toast at the end. Quitting asks first.

As the design says, the rendered JPEG goes up as an ordinary new photo: the server's
`edit` record needs a photo already in the gallery, so the desktop settings are not
attached, and re-editing on the web starts from the JPEG.

### Lens corrections

The "Lens corrections" section (after the web's sections, for every photo) has Distortion
(above 0 straightens barrel, below 0 pincushion), Lens vignetting (lifts dark corners in
linear light), Red/cyan and Blue/yellow fringe (lateral chromatic aberration: the red or
blue channel sampled a little further in or out) and Vertical and Horizontal perspective
(keystone: each row or column scaled about the centre by how far down or across it is).
Its "Reset" puts every correction back to none. There is no lens profile database yet
(lensfun's is CC-BY-SA, which needs a licence review before it is bundled), and no
automatic upright.

They are desktop-only, in `desktop.lens` (`lf_settings::lens`), and happen first, before
the crop and turn. The shader maps each output pixel through the geometry to a corrected
position (`muv`, where masks are placed) and through `lens_to_source` to the photo as
shot (`uv`, where it samples, as do the spots); the texture derivatives follow the warp.
`Lens::to_source` is the same formula on the CPU, and `Lens::to_corrected` inverts it (a
few Newton steps), so the spot tool and the eyedropper map the pointer through it
(`SpotMap::through_lens`) and a spot lands where the shader samples. With no correction
`lens_on` is 0 and the pass is the web's, so the parity gate is untouched.

### Masks (local adjustments)

`M` (or the tool row's "Masks", or the ribbon's Develop → Masks) opens the masks tool. "Add
linear" puts a graduated filter on the top of the photo; "Add radial" an ellipse in the
middle; "Add brush" a mask to paint. Each mask has its own Exposure, Contrast, Highlights, Shadows, Whites, Blacks,
Temperature, Tint, Saturation and Clarity, added to the global sliders where it applies;
"Invert" turns it inside out, and a radial has Feather and Angle. Up to 8 masks.

On the photo, a linear gradient shows its start line (full effect behind it), its end line
(none past it) and a dashed middle line, each with a handle: the ends move one line, the
middle moves both. A radial shows its outline with a handle at its centre (moves it), on
its right and on its bottom (each sets that radius along the ellipse's own axes); a drag
inside moves it. A drag that catches no handle pans; a click picks the mask under it. The
mask being edited is drawn in white with brand-coloured handles. `Delete` removes it. Each
add, drag, slider gesture and removal is one history step.

While a brush mask is picked, a drag on the photo paints a stroke (a click, a dab) with the
panel's Size (percent of the photo's height), Feather and Flow; "Erase" (or Alt held)
rubs out instead. The brush shows as a ring under the pointer, and what the mask covers as a
red tint. Strokes are stored as points (photo fractions, four places, a new point once the
pointer has moved a fifth of the size; at most 400 strokes of 4,000 points) and each stroke
is one history step. The develop pass cannot draw strokes analytically: `lf_pipeline::brush`
draws them on the CPU (`lf_settings::masks::rasterize`) into an `R8` texture, one layer per
mask stacked top to bottom (long edge 512), redrawn only when the strokes change, and the
shader samples the mask's layer at the corrected position (bindings 13 and 14; a 1×1
stand-in when no mask is a brush).

Masks are desktop-only: they live in `desktop.masks` (`lf_settings::masks`), which the web
editor keeps in a sidecar it rewrites but does not draw. Shapes are in source-photo
fractions (as spots are), so they stay on the same part of the photo through crop, turn
and flip; a radial's radii are fractions of the photo's height, in a space whose x is
multiplied by the aspect, so a circle is round. The develop shader computes each mask's
weight analytically (`mask_weight`, the same formula as `lf_settings::masks::weight`) and
adds the masks' shares to the sliders it moves, clamped to each slider's range. With no
masks every value is the uniform itself, so the parity gate is untouched.

Masks work as layers: each has an `opacity` (0–100 %, default 100, left out of the JSON when
full) and `hidden` (left out when false); `Mask::strength` is the opacity, or 0 when hidden,
and goes to the shader as `kind.w`, which scales the mask's weight (sliders and white balance
alike). A hidden mask keeps its place, so brush layers keep their rows. The panel's list has a
tick per mask ("Show mask n"), the picked mask an Opacity slider and Duplicate (a copy added
last, within the eight).

A mask's `range` (`lf_settings::masks::Range`: `All`, `Luminance { low, high, smooth }`,
`Colour { hue, width, smooth }`) narrows it inside its shape. The shader's `range_weight`
multiplies the shape's weight by how well the photo's own colour at that pixel (before any
edit, so moving a slider never moves the mask) fits: luma between `low` and `high` with
smoothstep edges, or a hue (the angle in the opponent plane `r − (g+b)/2`, `√3/2 (g − b)`)
within `width` degrees, fading over `smooth`, greys (chroma < 0.02) never matching.
`Range::weight` is the CPU twin. `MaskU` gained a seventh row (`range`). The panel offers
Whole shape / Brightness range / Colour range with their sliders, and Alt-click on the photo
with a colour range picks the hue there (`Develop::pick_range_colour`, from the frame's
sample).

AI masks: the panel's Select subject, Select sky and People buttons (People, Face skin, Body
skin, Hair, Clothes; `lf_ai::segment::Target`, enabled with `Decision::local_ai_allowed`)
set `MaskTool::ai_request`; the library opens the AI dialog with `AiTool::Mask` (started at once
when nothing needs downloading). `ai_job::mask` renders the photo with its edits but no crop,
turn or flip (lens corrections kept: masks live in that space) at 1024 px, runs IS-Net, SkySeg or
MediaPipe's selfie multiclass segmenter (256², NHWC RGB 0..1, six classes' logits softmaxed;
People is everything but background, Clothes takes accessories too), and returns `Shape::Image { what, width, height, data }` at most 512 px long:
8-bit coverage run-length coded and base64 (`masks::encode_coverage`), so the sidecar stays
small. `Develop::add_ai_mask` adds it as one history step. On the GPU it shares the brush
texture: `BrushTexture` resamples the picture into the mask's layer and the shader reads it as
kind 2; `masks::weight` samples it on the CPU. A picture that does not decode to its size is
dropped when read.

### Lens profiles

The Lens corrections section's "Apply lens profile" asks `Develop::take_lens_profile_request`
for the open photo's EXIF lens, maker, focal length and aperture (a toast when it has none);
`lens_profiles::run` on the `Io` lane fetches lensfun's `data/db` once (`assets::LENSFUN`, a
pinned 2.9 MB snapshot unpacked flat into `<cache>/ai/lensfun-db`), reads it once per run
(`roxmltree`, DTD allowed), matches the lens by its words (every number must agree; words
missing on either side count against it) and fits the sliders by least squares over the frame:
distortion (`ptlens`, `poly3`, `poly5`, blended between focal lengths) to the one-term radial
shape, colour fringes (`poly3`) to each channel's mean scale, and vignetting (`pa`, the nearest
aperture at the nearest focal length, corner at 1) to the linear gain, all clamped to the
sliders. Perspective is kept. The calibration's crop factor is not used: a profile made on a
different sensor size is applied as if on this one.

### Upright

The Lens corrections section's Upright: Vertical and Full. `develop::upright::segments` finds
straight edges in the photo's sample (Sobel, then a Hough transform voting only near each
pixel's gradient angle and only for lines within 30° of vertical or level; peaks strongest
first, each pixel claimed by one line, each line cut to its longest unbroken run).
`lf_settings::upright::fit` then searches the vertical (and in Full the horizontal) perspective
and the straightening angle, coarse to fine, minimising each line's leftover lean after
`Lens::to_corrected` and the turn (capped, so a slanted roof cannot pull it). The result is
written to `desktop.lens` and `angle` (the crop kept inside, as the Straighten slider does).
A quarter-turned or flipped photo is refused.

### Healing strokes

The spot tool's Brush paints healing (or cloning) strokes, desktop-only in `desktop.heal`
(`lf_settings::heal`): points, a radius (fraction of the width, as a spot's) and one source
offset, set beside the stroke's overall direction three radii away and moved by dragging the
dashed source line. The renderer heals a stroke as dabs every 0.4 radius
(`heal::dabs`), 48 to a spot pass, ping-ponged between the `spots` and `heal` targets so the
last pass lands in `spots`; the web's own spots come first. A dab carries the stroke's
direction in `uSpotB.zw`, and `spots.wgsl` then weights its tone-matching ring by how far
across the stroke each sample lies, so the blemish along the line never counts. The web never
sets that, so its spots render as before (the parity gate covers it).

### White balance and Auto tone

Presets (`shader_values::WB_PRESETS`, kelvin) become temperature and tint values, so they need
nothing new in the sidecar or the web editor: `kelvin_wb(shot, target)` takes both lights to
linear sRGB (daylight locus from 4000 K, Planckian below) and returns the sliders that make
the target light neutral in the photo as balanced (`neutral_wb`), within the sliders' range.
The as-shot temperature comes from the RAW's headers (`lf_raw::open::as_shot_kelvin`: one
over the white-balance multipliers through the D65 colour matrix, McCamy's CCT), read when the
photo opens (`OpenedPhoto::as_shot_kelvin`); other files are taken as 5500 K. The list names a
preset when the sliders hold its values.

Colour has a white-balance list (As shot, Auto) and a "Pick neutral" eyedropper (`W`, also
the tool row's "White balance"). A click on something that should be grey or white sets
temperature and tint so its colour (a 5×5 average) comes out neutral, as one history step,
and the tool closes, as Lightroom's does. Auto white balance (`Ctrl+Shift+U`) averages the
least saturated two fifths of the photo's midtones (dark and clipped pixels left out) and
makes that grey. Light has an "Auto tone" button (`Ctrl+U`): exposure brings the
log-average brightness to 18 % grey; whites and blacks move part way to the brightest and
darkest 0.5 %; highlights come down when the brightest twentieth sits above 0.8, shadows
come up when the darkest sits below 0.15; contrast is added to a flat photo.

All three only set sliders the web editor has, so a photo looks the same there. They read a
small copy of the photo (`develop::auto::Sample`, 256 pixels on the long side, made once
in the open job), so a click costs nothing. The solve inverts the shader's own gains
(`lf_settings::shader_values::wb_gains` and `neutral_wb`, kept side by side). The
temperature slider is a relative shift, not Kelvin: its whole range changes the red/blue
balance by about 1.9 times, so a very strong cast is only brought nearer neutral, and there
are no Kelvin presets (Daylight, Tungsten, ...) until RAW white balance can be redone in
the decoder.

### Culling: flags, stars and colour labels

`P`, `X` and `U` pick, reject and unflag; `0`–`5` set stars; `6`–`9` set a red,
yellow, green or blue label (purple and "no label" are on the Library tab).
A label already on every photo it is pressed on comes off instead. In the grid a mark
goes on every selected photo; in Develop, on the open one. The grid shows it at once
(`library::cull::apply`) and the catalogue is written on the `Io` lane
(`Catalog::mark`, one transaction); if that fails, a toast says so and the folder's
rows are read again, so the grid shows what was saved.

Marks are the `flag`, `rating` and `label` columns of `photos` (Lightroom's numbers
for flags: 1 picked, -1 rejected). They live only in the catalogue, not in the
sidecar: they are about choosing photos, not how one looks. Setting one never touches
`photos.updated_at`, which is part of a thumbnail's key. A label name this version
does not know reads as no label and is left alone by other marks.

A cell shows its stars under the thumbnail, a flag and a colour dot in its top-right
corner, and a rejected photo dimmed. The bar over the grid (`library::cull::Filter`)
shows only some flags ("Hide rejected", picked, ...), a minimum rating or one label,
and sorts by capture time, file name or rating (equal ratings stay in capture order).
The grid, the filmstrip and the arrow keys all follow its list. The filter is not
saved: one left on from last week would look like lost photos, so the status bar says
"12 of 340 photos" while it hides some. When the active photo is hidden (rejected with
"Hide rejected" on), the photo that took its place becomes active, so `X`, `X`, `X`
walks through a shoot. The photo open in Develop stays in the list while it is open,
even if the filter would hide it.

| Key | Action |
|-----|--------|
| `G` | Library |
| `D` | Develop (the active photo) |
| `Ctrl+Shift+E` | Export… |
| `Ctrl+E` | Edit in external editor… |
| `Ctrl+P` | Print… |
| `Ctrl+H` | Merge to HDR… |
| `Ctrl+M` | Merge to panorama… |
| `R` | Crop and straighten tool |
| `Q` | Heal and clone spots tool |
| `W` | White balance eyedropper (click something grey or white) |
| `M` | Masks: linear and radial gradients and a brush, with their own sliders |
| `Ctrl+U` | Auto tone |
| `Ctrl+Shift+U` | Auto white balance |
| `\` (held) | Before/After (View → Before/After toggles it) |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z`, `Ctrl+Y` | Redo |
| `Ctrl+Shift+C` | Copy settings… (opens the group picker) |
| `Ctrl+Shift+V` | Paste settings |
| `Ctrl+A` | Select all |
| `←` / `→` | Previous / next photo |
| `↑` / `↓` | Up / down a row in the grid |
| `Space` | Fit ⇄ 100 % |
| `Esc` | Leave the tool (the crop tool puts back what it started with) |
| `Enter` | Apply the crop |
| `Delete` | Remove the selected spot |
| `P` / `X` / `U` | Flag as pick / rejected / unflag |
| `0`–`5` | No stars to five stars |
| `6`–`9` | Red, yellow, green, blue label (again: off) |
| `F1` | About |

### Metadata, keywords and search

View → Metadata shows a panel on the Library's right (`library/meta_ui.rs`) for the selected
photos: Title, Caption, Creator and Copyright, and their keywords. A text field is saved when
it loses the keyboard (Enter or Tab), and only when it changed; with several photos selected,
a field they disagree on shows "Several values" and is left alone unless something is typed.
Keywords are typed separated by commas and added with Enter; each keyword on the selection is
a button that takes it off (`word (2/5)` when only some have it); "Keywords in this folder"
lists every keyword in the folder with its count, and a click puts it on the selection. Like
a mark, a change shows at once (`meta::Change::apply` on the rows in memory) and is saved on
the `Io` lane (`Catalog::change_meta`); a failed save reloads the rows.

The text fields are columns of `photos` and keywords a table of their own (`keywords`,
`COLLATE NOCASE`: a word is the same in any case, the first spelling kept), added in
schema v7 (`lf_catalog::meta`). Like marks they live only in the catalogue and never touch
`photos.updated_at`.

The search box in the bar over the grid (`Filter::text`) keeps the photos where every word
appears somewhere: the file name, camera, lens, title, caption, creator or a keyword (case
ignored). It is part of the filter, so the status bar's "12 of 340 photos" and "Clear
filter" cover it.

Exports carry the metadata: a JPEG gets an XMP APP1 (`exif_embed::xmp_segment`:
`dc:title`, `dc:description`, `dc:creator`, `dc:rights`, `dc:subject`) after its EXIF,
replacing any XMP the source had; "Copyright only" writes just the creator and copyright.
The catalogue's creator and copyright also replace the file's EXIF Artist and Copyright when
the EXIF is built (a JPEG's own EXIF is still copied byte for byte). PNG, WebP and TIFF carry
no XMP yet. Gallery uploads are JPEG exports, so they carry it too.

### Albums

Albums (`lf_catalog::albums`, schema v8: `albums` and `album_photos`) gather photos from any
folders. The sidebar lists them under the folders (`library/albums_ui.rs`); clicking one shows
it in the grid in place of a folder (`Library::current_album`; a folder and an album are never
shown together), and the filter bar, marks, Develop and export work on it as on a folder. A
plain album keeps its photos in the order they were added (`album_photos.position`); a smart
album keeps a `Rule` as JSON (minimum stars, flag, colour label, words as the search box
matches them, taken within N days) and holds no rows: `Catalog::album_photos` runs the rule
over every present photo when the album is shown, and again after each mark, so it keeps up.
Unknown rule fields are ignored; a rule that no longer reads holds nothing.

Home → Albums: New album (`Ctrl+N`, offering the selection), New smart album, Add to album
(`Ctrl+B`, a list of plain albums) and Remove from album (the album shown). An album's right-
click menu adds the selection, renames (or edits the rule) and deletes, after asking. Names
are unique without regard to case (`CatalogError::AlbumNameTaken`, shown in the dialog). Every
change runs on the `Io` lane (`Msg::AlbumChanged`), then the list is read again on `Query`
(`Msg::Albums`) and the album shown reloaded (`Msg::AlbumPhotos`). Removing a folder from the
catalogue takes its photos out of every album (`ON DELETE CASCADE`).

### Map

View → Map (`Action::Map`, `library/map.rs`) shows the folder's or album's photos that have a
place (`PhotoRow::gps`, schema v9: `lat`, `lon`, `gps_read`; import reads EXIF GPS through
`ExifSummary::gps`, JPEG/TIFF via kamadak and RAW via rawler) in place of the grid. Opening it
reads the photos not read yet, headers only, on the `Query` lane (`Catalog::fill_gps`,
`Msg::PlacesRead`) and reloads the rows. Web Mercator tiles from `tile.openstreetmap.org` with an
identifying User-Agent, at most six fetched at once on short threads (`Msg::Tile`), kept under
`<cache>/tiles/z/x/y.png`; the credit is drawn on the map. It fits every place on opening;
the wheel zooms about the pointer, a drag pans, pins within 22 points group with a count, and a
click selects a pin's photos. `Esc` or "Back to grid" closes it.

### Compare

`C` (`Action::Compare`, Library → View) with two to four photos selected shows them side by
side in place of the grid (`library/compare.rs`; a 2×2 grid for four). Each is rendered with
its saved edits on the `Open` lane (`compare::render`: through the frame cache with Develop's
working edge, so opening one afterwards is quick, then `Renderer::pixels` at up to 3000 px)
and comes back as `Msg::CompareRendered` for the comparison's generation; the pixels become
a texture on the next frame. Zoom and pan are shared (`compare::place` keeps each photo in
its cell): the wheel zooms, a drag pans, a double-click toggles fit and 1:1 (one rendered
pixel per screen pixel). The active photo is outlined; marks go to it alone, `←`/`→` move
it among the compared photos, and `Esc`, `C` or "Back to grid" closes. Showing another
folder or album closes it; Develop and back keeps it.

The loupe (`E`, `Action::Loupe`, Library → View) is the same view with the active photo alone
(`Compare::loupe`), rendered by `compare::render_at` at up to `LOUPE_EDGE` (8192) px so 1:1 shows
the photo's own pixels. A click zooms to 1:1 centred on the point clicked, and a second click
fits again; `←`/`→` render the grid's previous or next photo (selecting it) and keep the zoom
and place, so the same detail can be checked across a burst. `Esc`, `E` or `C` closes.

### Show clipping

`J` (`Action::Clipping`, View → Preview) toggles `Develop::clipping`, passed to the renderer as
`DrawOptions::clipping` (never in Before, never in `render`/`pixels`/exports). In
`develop.wgsl` the `clip_warn` uniform (the old padding word, so the layout is unchanged)
paints a pixel red when any channel reaches white (≥ 0.998 before the final clamp) and blue
when every channel is at black (≤ 0.002). It is part of `DrawKey`, so toggling redraws. The
ribbon button shows it switched on (`MenuState::clipping`).

### AI tools (on this computer)

`lf-ai` runs the models through ONNX Runtime, loaded at run time (`ort` with `load-dynamic`)
from Microsoft's official release. Nothing is bundled: `lf_ai::assets` pins each download
(URL, size, SHA-256) and `Engine::prepare` fetches what is missing into `<cache>/ai`
(`ai_job::ai_dir`, beside `frames/`) through a `.part` file, checks it, and takes the runtime
library out of its zip or tgz; a mismatch is deleted, never loaded. The environment is made
once per process. Models: SCUNet (denoise, tiles of 256 overlapping 32), Real-ESRGAN general
x4v3 (upscale, tiles of 192), IS-Net general use (subject, 1024²), SkySeg (sky, 320²), LaMa
(removal, 512²) and YuNet (faces, 640²). `lf_ai::tiles::run` blends overlapping tiles with
ramps, so no seams show and memory stays bounded.

The AI tab: AI Denoise, AI Upscale, Retouch portrait, Remove background
(`Action::{AiDenoise, AiUpscale, AiRetouch, AiCutout}`) open `dialogs/ai.rs` for the selection
(the open photo in Develop): the tool's setting, what the first use downloads, then
`ai_job::run` on the `Long` lane (`Msg::AiProgress` with the download's bytes, then photo
by photo; `Msg::AiDone` with a `Report`). Each photo is rendered at full size with its edits
(`ai_job::render`), worked on, and written beside the original through a temporary name:
`-Denoised.tif`, `-Upscaled-2x.tif`, `-Retouched.tif` (16-bit, sRGB profile) or `-Cutout.png`.
A photo that fails is listed and the rest go on. Upscaling renders smaller first when the
result would pass 120 MP. Retouching finds faces, builds a skin mask from each face's own
chroma inside an ellipse (eyes and mouth left out) and blends an edge-guarded blur with a
little of the fine texture back. The library rescans the catalogue folders written to. The
tools need `Decision::local_ai_allowed` (good standing); quitting or changing the catalogue
while one runs asks first (`quit::AI_RUNNING`).

AI → Remove objects (`Action::AiRemoveObjects`, one photo) opens `dialogs/remove.rs`: the photo
rendered with its edits on the `Open` lane (`compare::render`, `Msg::RemoveRendered`) with a
brush (Alt rubs out) painting a mask at most 1024 px long, shown as a red overlay. "Remove" hands
the mask to the AI dialog as `AiTool::Remove` (`AiDialog::mask`); the job grows it to the full
render and `lf_ai::inpaint::remove` fills each painted region: cut out with room around it,
shrunk to LaMa's 512², filled, grown back and blended in through a feathered mask, so only what
was painted changes. The result is `-Removed.tif`.

AI → Cull (`Action::AssistedCulling`, `AiTool::Cull`) scores the selection when several
photos are selected, else every photo the grid shows. `ai_job::cull` skips photos with fresh
scores (`Catalog::unscored`), renders the rest as shot at 2048 px with the preview demosaic
(`ai_job::render_at`) and scores them with `lf_ai::cull::Scorer` (YuNet and Intel's
open-closed-eye-0001, made once per run). Sharpness needs no model: the ratio of the finest
detail band to the next one, tile by tile, the 90th-percentile tile counting, so a sharp
subject on a soft background is sharp and contrast does not matter. Each face with eyes at
least 14 px apart has both eyes cut out (0.6 of the eye distance) and classified; the largest
face's eye region gives the eye sharpness. Scores go to the `scores` table (schema v10, keyed
to the file's size and mtime, so a changed file reads as unscored) and come back in
`PhotoRow::scores`; `lf_catalog::scores::Scores` holds the verdicts (blurry below 0.35, or
eyes below 0.3; any face with both eyes closed). The dialog says what it found and offers
"Show them" (`ReviewFilter::Issues`) and "Reject them" (`Library::set_mark` with
`Flag::Rejected`). The grid paints `cull_ui::paint_issue`; `Sort::Sharpness` puts the
sharpest first.

Stacks (`lf_catalog::stacks`, schema v11: `stacks`, `photos.stack_id` with `ON DELETE SET
NULL`, `fingerprints`) come back in `PhotoRow::stack` (id, top, size, collapsed).
`Filter::arrange` shows a collapsed stack's top alone (its first shown photo when the filter
hides the top). `library/stacks_ui.rs` saves changes on the `Io` lane (`Msg::Stacked`, then
the rows are read again); Expand / collapse changes the rows on screen first. Auto-stack…
(`dialogs/auto_stack.rs`, `stack_job::run` on `Long`, `Msg::StackProgress`) renders photos
without a fresh fingerprint at 256 px as shot and stores a 64-bit difference hash, then
`stacks::group` joins neighbours in capture order within the burst gap and 16 bits, and (if
asked) any two photos within 4 bits, through union-find; the top is the sharpest scored
photo, else the first taken. Photos already in a stack are left alone; a stack is never
left with fewer than two photos.

People (AI → Find people, `AiTool::FindPeople`, `ai_job::find_people` on `Long`) renders
photos without a fresh scan as shot at 2048 px and asks `lf_ai::identity::Identifier`: YuNet's
faces with eyes at least 18 px apart are aligned to SFace's 112 × 112 template (a least-squares
similarity fit of the five landmarks) and described as 128 unit-length numbers; a 96 px face
picture is kept as a JPEG. `lf_catalog::people` (schema v12: `people`, `faces`, `face_scans`
keyed to size and mtime) stores them, then `regroup_faces` runs: each face not named by hand
goes to the named person whose faces' mean it is most like (cosine at least 0.42; never to a
person it was taken from, `not_person_id`), and the rest are gathered greedily into suggested
groups (same threshold, one reassignment pass). A group is offered only when it spans two
photos, and is keyed by its first face's id so naming one group leaves the others' keys as
the list read them. Naming confirms every face of the person; naming after an existing name
merges. `library/people_ui.rs` lists people (`Msg::People`, pictures decoded on `Query`),
shows a person's photos in place of a folder (`Msg::PersonPhotos`) and changes them on `Io`
(`Msg::PeopleChanged`). `PhotoRow::people` carries the names, so the search box and smart
albums' words find them.

AI keywords (AI → Find keywords, `AiTool::FindKeywords`, `ai_job::find_keywords` on `Long`)
render photos without a fresh look as shot at 512 px. `lf_ai::keywords::Tagger` runs CLIP
ViT-B/32's image model (int8, 224 × 224 centre square, CLIP's mean and std) and compares the
unit embedding with each word's text embedding, worked out ahead of time over six prompt
templates by `scripts/keywords/embed.py` (needs CLIP's text model and tokenizer; the app
never does) and shipped as `data/keywords.f16`. A word is suggested at cosine 0.27 or more
with at least 8 % of the softmax (scale 100) over the list, five at most. `lf_catalog::tags`
(schema v13: `ai_keywords`, `tag_scans` keyed to size and mtime) keeps them apart from
`keywords`, so they never reach sidecars or XMP; `PhotoRow::ai_keywords` carries them to
search, and the Metadata panel's "Suggested by AI" turns one into a keyword
(`Change::AddKeywords`).

Select object (the Masks tool) arms `MaskTool::picking_object`; the next click on the photo
(mapped into mask space by `SpotMap::uv`, so it lands right under a crop, turn or lens
correction) becomes `AiTool::ObjectMask { u, v }`, run like the other AI masks
(`ai_job::mask_source`: edited, before crop and turn). `lf_ai::objects` scales the photo to
1024 on its longest side for MobileSAM's encoder (which normalises and pads in its graph),
asks Segment Anything's multi-mask decoder with the point and SAM's padding point, keeps the
mask it rates best and softens its logits with a sigmoid.

Replace sky (`AiTool::ReplaceSky`, a new `<name>-NewSky.tif`): the dialog asks for a photo of a
sky (`AiDialog::sky`, `Purpose::SkyFile`; Start waits for it). `lf_ai::sky::replace` finds the
sky with SkySeg at about 1024 pixels (refusing when there is none), softens the mask's edge,
scales the new sky to cover the width and the sky's depth (top kept, centred), blends it in,
and moves the land's colour `HARMONY` (0.2) of the way towards the ratio of the new sky's
mean colour to the old one's.

Lens blur (`AiTool::LensBlur`, a new 16-bit TIFF like AI Denoise) renders the photo with its
edits, asks `lf_ai::depth` (Depth Anything V2 Small: shorter side about 518, sides multiples of
14, ImageNet mean and std; its relative inverse depth spread to 0 far .. 1 near and resized to
the photo) and applies `lens_blur::lens_blur`. The focus is the 90th percentile of depth in
the middle half of the frame; each pixel's circle of confusion is 0 within half the focus range
of it and full a third of the depth range beyond. The blur works in linear light (so highlights
bloom) as seven layers of growing disc radius (largest 3 % of the long side at full amount),
each pixel mixing the two either side of its circle; samples are weighted by their own circle,
so the sharp subject does not halo into the background. Discs are read through row prefix
sums; a radius over 12 px is blurred on a picture made smaller by as much. 24 MP takes about
12 s in release on this machine.

Adaptive presets (`lf_settings::adaptive`): "Save as preset" offers "AI masks (found again on
each photo)" when the photo has AI masks whose `what` is an `lf_ai::segment::Target` (a
click-picked object cannot be found again; hidden masks are left out). They go into the preset's
JSON as `desktop.adaptiveMasks` (what to find, invert, opacity, local sliders, range; no
coverage), which the web's `applyLook` never reads and the server stores as it is. Applying the
preset queues them (`Develop::queue_adaptive`; skipped with a note when the AI tools are not
allowed); `take_ai_mask_request(busy)` hands out one `AiTool::Mask` at a time while no other AI
job runs, and `add_ai_mask` gives the mask that comes back the preset's settings when its
`what` matches the one awaited. A mask not found (the job ends without it, `ai_job_ended`) is
let go and the next one is asked for; opening another photo drops the queue.

Reference view (`Shift+R`, View → Reference view, `library/reference.rs`) splits Develop's
preview area in two (`reference::split`): on the left a reference photo, fitted, with its saved
edits (rendered once by `compare::render` on `Open`, `Msg::ReferenceRendered`); on the right
the photo being edited, live. The reference is the photo chosen with the filmstrip's
right-click "Use as reference" (`Library::reference_pick`), else the one open in Develop
before this one (`previous_developed`); with neither, a toast says how to pick one. It stays
while other photos are opened, so a set can be matched to it one by one.

Second window (View → 2nd window, `library/second_window.rs`) is an egui immediate viewport
(its own OS window, for another monitor; embedded where the backend has one window only)
showing the photo worked on, fitted on black with its name and marks: Develop's open photo,
else the Library's active one. `Library::follow_second_window` runs each frame: a different
photo, or a `take_edited` that names the one shown (Develop's quiet time, as for thumbnails),
renders it again with `compare::render` on `Open` (`Msg::SecondWindowRendered`, by
generation). Closing the window turns it off.

Slideshow (View → Slideshow, `library/slideshow.rs`) covers the window on black with the
selection (or the grid's photos), one at a time, each rendered with its edits on the `Open`
lane as Compare renders (`Msg::SlideRendered`, by generation); the next one renders while
this one shows, and slides behind are let go. It moves on every 4 s once a photo is up;
Space pauses, the arrows step (wrapping round), Esc or a click ends it.

The info overlay (`I`, `Action::InfoOverlay`, `Develop::info_overlay`) paints
`info::overlay_lines` (name; size and kind; the info strip's camera lines) bottom left over the
preview on a translucent panel (`Develop::paint_info`), also when there is no preview canvas,
and names it as one label for screen readers.

The crop tool's composition overlay (`develop/tools/overlay.rs`, `Tools::crop_overlay`,
`Action::CropOverlay` on `O` while the crop tool is open) is a list of polylines in the crop
frame's fractions: thirds, an eighths grid, golden ratio lines at 0.382/0.618, 45° diagonals
from each corner (square in the photo, so the frame's aspect is taken into account), triangles
(the diagonal and perpendiculars to it from the other corners), and a golden spiral of ten
quarter arcs through squares cut in turn. `paint_crop` draws it at alpha 70, 130 while
dragging.

Side by side (`Y`, `Action::SideBySide`, `Develop::side_by_side`) splits the preview area
(`reference::split`): the right half is the ordinary preview (the rect every tool, zoom and pan
use); the left half is a second canvas (`preview_before`, its own `Preview`) drawn with
`DrawOptions::before` (no geometry, no development), fitted, and drawn again only when its size
or view changes (`last_before`, reset with `last_drawn`). Each half carries a Before or After
label. Holding `\` (before) shows the whole area as before, as without it.

The Navigator (`develop/navigator.rs`) is a third canvas of its own (`Develop::navigator`)
drawn with the current settings at the whole view (and the crop tool's whole frame while
cropping), sized to the side panel's width and at most 180 points tall, and drawn again only
when its `NavKey` (canvas size, settings, crop drawn) changes or another photo opens. The
preview stores the part it showed (`last_view`); the navigator frames it (`frame_of`) when it
is less than the whole photo, and a click or drag sets `viewport.centre` to the point under the
pointer (`point_at`), which the viewport then clamps.

The colour readout (`develop/readout.rs`) shows the edited photo's RGB (8-bit display values
as percentages) at the point under the pointer, under the histogram. The preview stores the
photo point (`readout_at`, through `Viewport::output_at`; none over the photo as it was or the
crop tool's whole frame); while there is one, `update_histogram` keeps a picture of the edit at
800 px (`Renderer::pixels`), made once when the pointer arrives and again after each histogram
run (each edit, at most every 200 ms). Nothing is read back while the pointer is elsewhere.

The targeted Colour mix adjustment (`ToolKind::Targeted`, `develop/tools/targeted.rs`) is a tool
opened by the Colour mix section's "🎯 Drag in photo" (`PanelOutput::target_hsl`). At a drag's
start it takes the edited colour under the pointer (the colour readout's picture), its hue
(`masks::hue_of`; a grey spot moves nothing and says so) and each band's share of it with the
shader's own weighting (smoothstep of the hue distance over the band's width, scaled so the
strongest is 1). Moving up raises, down lowers, half a slider unit per point, each band by its
share, for the property the section shows (`PanelState::hsl_mode`, followed while the tool is
open); release is one history step.

Defringe (`lf_settings::defringe`, `desktop.defringe`; `panels/defringe.rs`) runs in the
develop shader's input stage, after lateral CA and before a film negative (`defringe`): a
pixel is near an edge by the largest brightness step to its neighbours 1, 2 and 4 source pixels
away (smoothstep 0.05..0.2), and is greyed towards its own brightness by its amount times that,
times how close its hue is to the purple or green hue chosen (full within 17.5°, none past
35°), times its saturation (smoothstep 0.05..0.2). `fringe_on.x` is 0 when both amounts are 0,
so the pass is the web's.

Point colour (`lf_settings::point_colour`, `desktop.pointColour`; `panels/point_colour.rs`)
holds up to four points: the picked colour (hue, saturation, brightness, from the colour
readout's picture under the pointer when `ToolKind::PointColour` is clicked; the picker then
closes and the new point is chosen) and its range and shifts. The develop shader applies it on
the edited colour after `develop()` and before the LUT (`point_colour`): each point weighs a
colour by smoothsteps of its hue distance (8..22° times a reach of 0.4..2 from the range),
saturation (0.15..0.4 times it) and brightness (0.2..0.5 times it), and not on near-greys; the
weighted hue shift (±30°), saturation scale and luminance (towards white or black) are summed.
Points with no shift are not sent (`Points::active`), so with none the pass is the web's.

Stack RAW+JPEG pairs (`Action::StackPairs`, `Library::stack_pairs`) pairs the folder's photos
with `lf_catalog::stacks::raw_jpeg_pairs`: a raw file (one of `RAW_EXTENSIONS`) and a .jpg or
.jpeg with the same stem, case aside, in the same folder; pairs already stacked together are
skipped. Each pair is stacked on `Io` with the raw file on top (`Catalog::stack`, which takes
them out of any other stack), and the toast says how many.

Auto import (`Action::AutoImport`, `dialogs/auto_import.rs`, `library/auto_import_ui.rs`,
`lf_catalog::auto_import`; `prefs.auto_import` while on) looks at the watched folder on the
`Io` lane every 3 s (`follow_auto_import`, one look at a time). A photo (the import's own
extensions) is ready when its size and modification time match the previous look and it is
not empty (`ready`); it is moved into the destination under a free name (`-2`, `-3`…;
across drives: copy, check the size, then remove), and the destination is rescanned
(`Msg::AutoImported`; a toast says how many). The watched folder may not be, or be inside, a
catalogued folder. A failure to read it is said once until it changes.

Export with previous (`Action::ExportPrevious`, `Library::export_previous`) reads
`prefs.last_export` into a form; when it makes a spec (a folder is set) the export starts at once
with the dialog hidden (its end is a toast, as for a dialog closed while running), else the dialog
opens to be filled in. `start_export` is the dialog's Start, shared.

Rotate left and right (`Action::{RotateLeft, RotateRight}`, `Library::turn_photos`): in Develop,
`Develop::turn` (one history step); in the grid, every selected photo through `batch::run` on
`Long` with `Recipe::Turn` (`crop::turn`: the crop and a locked shape turn with it), answered by
`Msg::Turned`, which reloads their rows and thumbnails.

The grid's cell size is `Library::cell` (`prefs.grid_cell`, 110..420 points, 200 by default),
set by the slider under the filter bar and saved when a drag ends; `grid::show`, `cell_box` and
`cells_in` (the marquee) all take it.

Survey (`Action::Survey`, `Compare::survey`) is Compare with up to `MAX_SURVEY` (12) photos laid
out by `compare::grid_of` (side by side up to three, then the squarest grid a little wider than
tall); each cell has × (`leave_survey`: out of the survey and the selection; one left closes
it). Compare's hint lost its "⇄", which egui's fonts lack (Compare panicked on opening).

The History section (`develop/history_panel.rs`, at the bottom of Develop's side panel) lists `lf_catalog::edits::history` (the photo's
steps, oldest first, and the cursor), newest first, each named by `describe` (the first setting
that differs from the step before, desktop keys by their own name, "+n" for more; the first is
"Opened"). It is read again only when the photo, its committed settings or its undo position
change. A click is `EditOp::Jump` → `EditState::jump` → `edits::jump` (the cursor moved to
that step, its settings put back, a sidecar written), as several undos would.

The filter bar's camera and lens lists (`Filter::{camera, lens}`, `cull_ui::pick_one`) offer the
folder's distinct EXIF names (`cull_ui::distinct`), shown when there are two or more (or one is
chosen); a photo is kept when its `PhotoRow::camera` / `lens` is the one chosen.

Quick develop (`library/quick_develop.rs`, `Action::QuickDevelop`) is a window over the grid
with one row per setting (`ROWS`: key, name, small and large step, range). A click runs
`batch::run` on `Long` over the selection with `Recipe::Nudge(key, delta)` (`quick_develop::nudge`:
the photo's own value plus the step, kept in range, rounded to thousandths), one history step a
photo; "Reset all" is a paste of the defaults without geometry. Answers come back as
`Msg::Turned` (rows and thumbnails read again; failures in one toast).

Auto straighten (`panels::crop::auto_straighten`) is Upright's Level on its own: the photo's
line segments (`develop::upright::segments`) fitted with `upright::fit(.., Mode::Level)`, and only
its angle set (`crop::straighten`, so the crop stays inside the turned photo); one history step.
A turned or flipped photo, or one with too few lines, is left alone with a note.

Geotagging (`lf_catalog::gpx`, `dialogs/geotag.rs`, `library/geotag_ui.rs`,
`Action::GeotagGpx`): the track's points with times (`trkpt`, `rtept`, `wpt`; a small scanner,
not an XML parser), the photo's EXIF time read as local and moved to UTC by the zone chosen
(default this computer's), then `locate`: interpolated between points up to 10 minutes apart,
else the nearest point within 5 minutes, else no place. Works on the selection (several) or the
folder, on `Io`, through `Catalog::set_gps`; `Msg::Geotagged` toasts how many were placed.

Hover preview: `looks::show` and `presets::show` report the row under the pointer; `extras_ui`
(reset each frame) makes `Develop::hover_settings` the photo's settings with it applied (as a
click would, `apply_look` / `Recipe::Look`), and the preview draws those instead of the edit's,
redrawing when they change (`hover_drawn`). Nothing is committed; the histogram, tools and
export keep reading the edit.

Metadata presets (`prefs.meta_presets`, `MetaPreset`): the Metadata panel's "Apply a preset…"
list and "Save as preset" (the first selected photo's creator, copyright and keywords, under a
name; the same name replaces). Applying is `meta_ui::preset_changes` (creator and copyright set
when the preset has them, keywords added) through `change_meta`, as typing them would.

The Keywords list (`library/keywords_ui.rs`) builds a tree from the folder's keywords
(`tree`: each keyword and every parent of its "a > b > c" path, counting photos under it); a
click sets `Filter::keyword` (toggled), which keeps photos with that keyword or one below it,
case ignored (`keywords_ui::under`). Keywords are stored as typed, so a hierarchy is a naming
habit, not a separate table.

Web gallery (`Action::WebGallery`, `library/web_gallery_ui.rs`, `lf_catalog::web_gallery`) runs on
`Long`: the selection exported through `export_job::run` (JPEG 85, 2048 px long edge, the photos'
edits) into `<folder>/images`, then `web_gallery::write`: 480 px thumbnails in `thumbs/` and an
`index.html` with inline CSS and a few lines of script (no outside files), captions the
catalogue titles or file names, every text HTML-escaped and file names URL-encoded. The page is
opened when it is done (`platform::open_file`).

Focus peaking (View → Peaking, `Shift+J`) and Visualize spots (View → Spots view) are preview
overlays, never in renders or exports: `DrawOptions::{peaking, spots_view}` become bits 2 and 4
of the develop pass's `clip_warn` (bit 1 is clipping). Both take a 5-tap Laplacian of the
photo's brightness after spot removal at one output pixel's spacing (`grad_x`, `grad_y`), so
they follow the zoom; peaking tints edges above 0.12 green, Spots view shows the edges white on
black (×10), where dust stands out as rings.

Upright's Level fits the straightening angle alone (`upright::Mode::Level`, from any two
lines); Auto fits Full (or Vertical when there are no level lines) and takes the perspective
`AUTO_SHARE` (0.6) of the way, with the angle fitted again for it: a balanced correction,
as Lightroom's Auto is.

Guided Upright (Develop's Guided tool) takes drags on the photo as guides, stored in the photo
as shot (`SpotMap::uv` through the lens corrections), so they stay on the edges they were
drawn on as the correction changes; up to four, a fifth replacing the oldest. After each guide
`upright::guided` fits the perspective and straightening: every guide counts in full (no cap,
unlike the automatic fit, which distrusts steep lines), and what is fitted follows what was
drawn (two upright guides: vertical perspective; two level ones: horizontal; both: both). The
correction goes into `desktop.lens` and the angle, with the crop kept inside, as one history
step. Photos turned or flipped are refused. Guides last while the tool is open.

Rename… (Home → Selection, `Action::RenamePhotos`, Library mode) previews the first new names
from the rows in memory (`rename::new_name`). On Rename, Develop closes (so it holds no old
path) and an `Io` job flushes the sidecar saver first (so no queued write lands under an old
name), then `rename::plan` works out every name and refuses the batch when one is taken by a
file (or a sidecar) outside it, or given twice in a folder; `rename::run` moves each photo and
its sidecar to a temporary name, then to its new one (so photos of the batch can swap names),
undoing every move on a failure, and updates `photos.path` in one transaction through
placeholder paths (the column is unique). Contents never change; thumbnails, edits and the
frame cache are keyed by id or content, so they follow.

Auto-sync (`prefs.auto_sync`; Home → Settings → Auto-sync, or Develop's checkbox when other
photos are selected): `Develop::run_edit` keeps the committed settings before a commit or a
reset and, when the commit lands, `sync::changes` gives the keys that changed (inside
`desktop` too), never the photo's own crop, turn, flips, aspect, spots, healing or masks.
Patches not yet sent merge (`sync::merge`). Each frame `Library::auto_sync` tells Develop how
many other photos are selected and sends a waiting patch to them on the `Long` lane
(`batch::run` with `Recipe::Sync`, sidecars through the saver); `Msg::Synced` refreshes their
thumbnails and reports failures. Undo and redo stay on the open photo.

Film negative (Develop → Film negative, `desktop.negative`): "Convert negative" measures the
photo's sample (`Negative::measure`: in linear light, each channel's 99.5th percentile is the film
base, its 0.5th the densest point) and the develop shader converts first of all (before
calibration and every slider): the light passed as a share of the base's, to density
(`−log₁₀`, capped at 4), scaled per channel by the photo's range, so the orange mask cancels and
a grey scene comes out grey. White balance and the tone sliders then work on the positive.

Calibration (Develop → Calibration, `desktop.calibration`) moves the red, green and blue
primaries: `Calibration::matrix` turns each about the grey axis (±30° at ±100) and scales its
distance from grey, then weights the columns so white maps to white, so greys never move. The
develop shader applies it first, in linear light (`calibrate`, behind `calib_tint.y`, so
neutral settings are the web's pass exactly), then the shadows tint (green–magenta, weighted
by `(1 − smoothstep(0, 0.5, luma))²`).

Camera profiles (Develop → RAW → Camera profile) are DNG `.dcp` files the user imports
(`profiles::import` copies them into `<root>/.lensflow/profiles/`; `profiles::load_all` reads
them into `lf_raw::dcp`'s registry when the catalogue opens, before thumbnails start). A photo
stores the name (`desktop.develop.dcp`); `DevelopSettings::dcp` is its `lf_settings::dcp_id`,
so the settings stay `Copy` and the RAW reopens when it changes. `develop` fixes the profile
for the shot (`Dcp::prepare`: forward matrices and HueSatMaps blended by the as-shot
temperature in inverse kelvin) and replaces the colour matrix with it: forward matrix → linear
ProPhoto → HueSatMap → LookTable (trilinear, hue wrapping, value axis clamped so headroom
survives) → tone curve (Adobe's hue-keeping way; a straight curve counts as none and the
base curve stays) → linear sRGB. A profile with no forward matrix uses the file's own
matrix. The frame cache key adds the profile's SHA-256 (or "missing"), so a replaced file
develops again; a profile that is not loaded is left out and the RAW panel says so.

Remove objects' "Select people" sends the dialog's picture (kept at the mask's size) to the
`Long` lane, which runs the people model (`Target::People`, downloading it once if needed) and
answers `Msg::PeopleSelected`; everyone found (coverage ≥ 0.5) is widened by 0.8 % of the long
edge (`remove::widen`, rows then columns) and added to what is painted. The user rubs out
whoever should stay; Remove then fills the rest with LaMa as for any painted mask.

"Select wires" works the same way (`Msg::WiresSelected`, widened by 0.3 %) with
`lf_ai::wires::find`: no model draws wires, so they are found by shape. Within the sky (SkySeg,
widened by three closing radii so a wire's ends at a pole or roof come too), a black-hat (the
brightness closed by a square of radius long edge / 250, at least 2 px, minus the brightness;
running min/max through monotonic queues, so linear time) marks thin lines at least 0.04 darker
than the sky about them. Those are grouped into 8-connected pieces and kept when long (box
diagonal at least 8 % of the long edge) and thin (pixels per unit length at most two radii):
birds and clouds' edges go, wires stay. A branch against the sky can be kept too; it is rubbed
out before Remove.

Depth masks (Masks → Foreground, Background) are `Target`s sent to `lf_ai::depth::mask`: the
depth map at the mask's size, split where Otsu's threshold best separates near from far (the
middle of the best-scoring run, so an empty gap splits in its centre), with a soft edge 0.08
wide.

Face-part masks (Masks → Face: Eyes, Irises, Lips, Teeth) are `Target`s whose
`face_part()` sends `segment::mask` to `lf_ai::features`: YuNet's faces are cut out as
MediaPipe does (a square 1.5 × the box, turned so the eyes are level, 256 px), the face
landmarker answers 478 points (skipped when its presence logit is negative), and MediaPipe's
outlines are filled at the mask's size with 4 × 4 samples per pixel. Irises are circles
through each iris ring, kept inside the eye openings; lips are the outer outline less the
mouth's opening; teeth are the opening weighted by how light and colourless each pixel is.

`crates/lf-ai/tests/models.rs` runs the real models when `LF_AI_DIR` names a folder holding
them (`LF_AI_DOWNLOAD=1` fetches them there); otherwise those tests return at once. Run them
with `--test-threads=1` (parallel tests would download into one folder at once). The face-part
test also needs `LF_AI_FACE_PHOTO`: a PNG of a real face.

### Import from a card

File → From card (`Action::ImportFromCard`) opens "Import from a card"
(`dialogs/ingest.rs`, `library/ingest_ui.rs`). The card (or any folder) is chosen with a
picker or a typed path, then scanned and planned on the `Long` lane
(`lf_catalog::ingest::plan`): each file's capture time and camera are read from its headers,
and a file is a suspected duplicate when a catalogued photo has the same size and capture
time (so one imported before under another name is still found) or, with no capture time,
the same name and size. The form then asks where the copies go (a folder, default the last
one or the catalogue root, and a folder name inside it, default the shoot's first date), an
optional rename (the export's tokens `{name}`, `{seq}`, `{date}`, `{camera}`, with an
example), an optional backup folder (the same folder name inside it) and whether to skip
suspected duplicates (on by default). It is remembered in `prefs.last_ingest`.

The copy runs on the `Long` lane (`ingest::run`), with progress and Stop; closing the window
leaves it running, and quitting asks first. Each file is copied under a temporary name
(`.lfingest-<name>.<pid>.tmp`), its size checked, then renamed, so a pulled card or a full
disk never leaves a half-copied photo under a real name. A name already taken in the
destination gets `-2`, `-3`, …; a file there with the same name and size is the same photo
and is skipped. A backup copy that fails is counted, not fatal. The card is only read.

When the copy ends, the destination is catalogued: rescanned with the catalogue folder it
lies in, or added as a new folder (as "Add folder…" would). If another import holds the
catalogue then, a toast says where the copies are, to add by hand.

### LUT looks

Develop's "LUT look" section (`develop/panels/lut.rs`) picks a 3D LUT and its Amount;
"Import .cube…" copies a `.cube` file into the catalogue's `.lensflow/luts/` (`luts.rs`,
replacing one of the same name) and puts it on the open photo. A photo stores only the
LUT's name and amount (`desktop.lut`, `lf_settings::lut::LutChoice`), so the web editor
ignores it and a sidecar stays small. The `.cube` reader (`Lut::parse_cube`) takes 3D tables
of size 2 to 129 with the 0..1 domain; 1D and log/HDR (other domain) LUTs are refused with a
reason.

The tables live in a process-wide registry (`lf_pipeline::lut`), filled from the LUT folder
when the catalogue opens (in `catalog_ext::open`, before the thumbnail pool starts) and when
one is imported; the preview, thumbnails and exports all find them by name. A renderer
uploads the chosen table into a 3D `Rgba16Float` texture (binding 15, its sampler 16) when
the choice changes; the develop shader looks the finished colour up (after the develop step,
before vignette and grain, each grid point at a texel centre) and mixes it in by
`lut_amount` (the uniform that was the lens block's padding). A LUT that is not loaded (a
sidecar from another machine) renders as none, and the panel says so. With no LUT,
`lut_amount` is 0 and the pass is the web's: the parity gate is untouched.

### Edit in external editor

`Ctrl+E` (Develop → Edit in…; `Action::EditIn`) renders the photo open in Develop (or the
active one) with its edits to a 16-bit TIFF beside it (`edit_in::render_copy`, through the
disk export: `<name>-Edit.tif`, `-Edit-2`, … when taken), opens it in the external editor,
and rescans the catalogue folder it lies in so the copy shows next to the original. The
editor is chosen once with an application picker (an `.exe` on Windows, an `.app` on macOS)
and kept in `prefs.external_editor`; Preferences shows it with "Forget". When the editor
cannot be started the copy is still made and catalogued, and a toast says why. The original
is never touched; the editor saves into the copy, and a rescan picks its changes up.

### Print

`Ctrl+P` (File → Print; `Action::Print`) opens the print dialog (`dialogs/print.rs`) for the
selection or the photo open in Develop: paper (A4, Letter, A3, 4×6, 5×7, 8×10, 12×18 in),
photos per page (1, 2, 4, or 9 and 20 for a contact sheet), orientation (automatic follows
each page's first photo), margins and a caption (file name, or the catalogue's title).
"Print…" writes a PDF into the app's cache (`print/`) and opens it in the system's viewer
to print; "Save PDF…" asks where (for a lab). The run (`print_job::run`, `Long` lane, with
Stop) renders each photo through the disk export as a JPEG sized for 300 dpi in its cell
(`PrintSpec::render_edge`, at most 6000 px, standard output sharpening), and
`lf_catalog::print::pdf` writes the pages: a small hand-written PDF 1.4 with the JPEGs placed
as they are (`DCTDecode`, nothing compressed twice) and captions in the built-in Helvetica
(characters outside WinAnsi print as `?`).

### Merge to HDR

`Ctrl+H` (File → Merge; `Action::MergeHdr`) merges two to seven selected photos, an
exposure bracket, into one (`dialogs/merge.rs`, `library/merge_ui.rs`). The run
(`merge_job::run`, `Long` lane, with Stop) develops each frame with the default settings (the
photo as shot, through the develop pass, so RAW frames keep their highlights until then) at
most 5000 px on the long side, keeps it as 16-bit, lines each frame up with the middle one
(`lf_merge::align`: Ward's median threshold bitmaps over a pyramid, so brightness does not
matter, up to ±64 px), then fuses them (`lf_merge::fuse`: exposure fusion after Mertens,
Kautz and Van Reeth, as Enfuse does: each pixel weighted by contrast, saturation and how
close it is to mid grey, blended through Laplacian pyramids so no seam shows). The result is
a finished, natural-looking photo (no tone mapping step), written as a 16-bit TIFF with the
sRGB profile beside the first frame (`<name>-HDR.tif`, `-HDR-2`, …); the catalogue folder is
rescanned so it shows beside the bracket. Frames of different sizes are refused. Moving
subjects are not de-ghosted.

`lf-merge` is CPU-only (rayon), with no GPU and no catalogue.

Ghosts: with "Remove ghosts" on (`MergeDialog::deghost`, the default) the bracket is fused by
`lf_merge::fuse_deghosted` with the middle frame as the reference. Each other frame is brought
to its exposure (the median brightness ratio where both are well exposed); where it still
differs by more than about 40 % (|ln| > 0.35, clipped pixels not judged) its weight is taken
away through a grown and softened ghost map (`ghost_map`), so a moving subject comes from the
reference alone. The reference keeps a small weight everywhere, so no pixel is left with none.

### Panorama rows

The panorama dialog's Rows (1 to 4, `MergeOptions::rows`) takes the selection row by row, top row
first, as many in each row (the last may have fewer). `lf_merge::panorama::stitch_rows`
stitches each row as before, cuts the strips to their common size about their middles, turns
them a quarter clockwise so they lie side by side, stitches those, and turns the result back.
A panorama takes up to 24 photos; past 12 each is worked at 2000 px rather than 3000, since
every frame is held at once.

### Fill edges

The panorama dialog's "Fill edges (AI)" (`MergeOptions::fill_edges`, the AI folder; enabled
when the AI tools are allowed) keeps the whole outline instead of cropping:
`stitch_rows_uncropped` crops the last stitch to every pixel any frame covers and says which are
covered (with several rows the rows are still cut to a common size; only the last step keeps its
edges). `lf_ai::inpaint::fill_edges` then fills the empty ones with LaMa in squares along them
(`edge_tiles`: a side four times the holes' depth, 512 to 1536 px, three quarters of a side
apart), each square from the picture and the squares filled before it; covered pixels are never
changed. The model is downloaded first if need be.

### Focus stack

File → Merge → Focus stack (`Action::MergeFocus`, `MergeKind::Focus`: 2 to 30 frames, worked at
4000 px, `-Focus.tif`) lines every frame up with the middle one (`lf_merge::align`) and blends
them with `lf_merge::stack_focus`: the same Laplacian-pyramid blend as exposure fusion, with
each frame's weight its local sharpness (|Laplacian| of the grey image, box-averaged over about
1/150 of the frame, to the fourth power, so the sharpest frame clearly wins). Lining up is
translation only: a lens whose framing breathes with focus may leave soft edges.

### HDR JPEG export

Export → JPEG HDR (`Format::JpegHdr { quality }`, `lf_catalog::ultrahdr`) writes an Ultra HDR
file: the ordinary JPEG, then a grey gain map JPEG. The develop pass ends in 0..1, so the
brighter rendition is a second `render_f16` of the edit 2 EV darker (`HEADROOM_STOPS`), back up
in linear light: what clipped in the ordinary render still has detail there when the source has
headroom (a raw file). `log_gains` takes the luminance ratio (offsets 1/64, clamped to 1..4)
and applies its log2 only where the ordinary render's largest channel passes 0.75, fully from
0.97 (a smoothstep), so midtones match on every screen and only highlights glow. Gains are
stored as 8-bit recovery over 0..the largest log2 present (gamma 1), at half the photo's size.
`assemble` puts the gain map's own XMP (`hdrgm:GainMapMin/Max`, `Gamma`, `OffsetSDR/HDR`,
`HDRCapacityMin/Max`, `BaseRenditionIsHDR="False"`) into it, gives the primary an XMP packet with
`hdrgm:Version="1.0"` and a GContainer directory (Primary, GainMap with its length) beside the
photo's own title and keywords (a second `rdf:Description` in the same packet), and inserts an
MPF APP2 index (CIPA DC-007: both images' sizes, the gain map's offset from the MPF TIFF
header) after the primary's APPn segments. sRGB only. An 8-bit source has no headroom, so its
gain map is all zero and the file shows the same everywhere.

### High resolution

File → Merge → High resolution (`Action::MergeHighRes`, `MergeKind::HighRes`: 2 to 16 frames,
worked at 3000 px, `-HiRes.tif` at twice the size each way) merges a handheld burst with
`lf_merge::superres::merge`. Each frame is lined up with the middle one: `lf_merge::align` for
whole pixels, then a parabola through the mean squared difference at the best shift and one
pixel either way (on grey pictures softened by two 3 × 3 boxes, so aliasing does not pull it),
in x and in y. Every output pixel then gathers the 2 × 2 nearest pixels of every frame,
weighted by a Gaussian (sigma 0.6 output pixels) of where each lands and by how well it agrees
with the middle frame enlarged there (a Gaussian of 0.08 on the largest channel difference, so
what moved shows as in the middle frame); the middle frame enlarged carries a weight of 0.05,
filling in where no sample lands. A light unsharp mask (3 × 3, amount 0.5) undoes the
gathering's softness. On the test scene the result is about 28 % closer to the true detail
than one frame enlarged.

### Merge to panorama

`Ctrl+M` (File → Merge; `Action::MergePanorama`) stitches two to twelve selected photos,
one row taken turning the camera, in the grid's order (left to right or right to left),
into one wide photo (`merge_job` with `MergeKind::Panorama`). Each frame is developed as
shot at most 3000 px on the long side (a panorama holds every frame at once), then
`lf_merge::panorama::stitch`:

- projects the frames onto a cylinder, so turning the camera becomes sliding the picture;
  the cylinder's radius (the lens's focal length in pixels) is not in the files, so each of
  a dozen candidates (and "no cylinder", for a flat scene or a long lens, preferred on a
  near tie) is tried and the best-matching one kept;
- finds each neighbour's offset by normalised cross-correlation (so brightness differences
  do not matter) on small copies (96 px tall, every overlap from 5 % to 95 % either way and
  up to a tenth up or down), refined at 192 and 384 px; a neighbour that matches worse
  than 0.5 (one minus the correlation) is not a match, and the merge says the photos could
  not be lined up;
- evens the frames' brightness from their overlaps, lays them on one canvas (at most
  60 MP) with feathered edges, and crops to the rectangle every column covers.

The result is a 16-bit TIFF with the sRGB profile beside the first photo (`<name>-Pano.tif`),
catalogued by a rescan. Several rows, or a panorama shot turning up and down, are not
stitched; a subject moving in an overlap may show twice, faintly.
