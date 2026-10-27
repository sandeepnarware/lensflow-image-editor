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
middle. Each mask has its own Exposure, Contrast, Highlights, Shadows, Whites, Blacks,
Temperature, Tint, Saturation and Clarity, added to the global sliders where it applies;
"Invert" turns it inside out, and a radial has Feather and Angle. Up to 8 masks.

On the photo, a linear gradient shows its start line (full effect behind it), its end line
(none past it) and a dashed middle line, each with a handle: the ends move one line, the
middle moves both. A radial shows its outline with a handle at its centre (moves it), on
its right and on its bottom (each sets that radius along the ellipse's own axes); a drag
inside moves it. A drag that catches no handle pans; a click picks the mask under it. The
mask being edited is drawn in white with brand-coloured handles. `Delete` removes it. Each
add, drag, slider gesture and removal is one history step.

Masks are desktop-only: they live in `desktop.masks` (`lf_settings::masks`), which the web
editor keeps in a sidecar it rewrites but does not draw. Shapes are in source-photo
fractions (as spots are), so they stay on the same part of the photo through crop, turn
and flip; a radial's radii are fractions of the photo's height, in a space whose x is
multiplied by the aspect, so a circle is round. The develop shader computes each mask's
weight analytically (`mask_weight`, the same formula as `lf_settings::masks::weight`) and
adds the masks' shares to the sliders it moves, clamped to each slider's range. With no
masks every value is the uniform itself, so the parity gate is untouched.

### White balance and Auto tone

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
| `R` | Crop and straighten tool |
| `Q` | Heal and clone spots tool |
| `W` | White balance eyedropper (click something grey or white) |
| `M` | Masks: linear and radial gradients with their own sliders |
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
