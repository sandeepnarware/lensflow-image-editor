# How the desktop app is put together

`crates/lf-app` builds the `lensflow-image-editor` binary: one native window on
`eframe` (egui with the wgpu renderer). This page describes the threads it uses
(lanes), the session state machine, how the Develop preview reaches the screen,
where files live, and the keyboard map.

## The window and the one device

`main.rs` opens the window with `NativeOptions { renderer: Wgpu }` and asks
eframe for a device with the adapter's own limits (`gpu::request_full_limits`),
because egui's default 8192 px texture limit would refuse 45 to 100 MP photos.
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
keyboard map, and taps the device. `ui()` then draws the menu bar, the banners,
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
and only then are the waiting sidecars written.

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
| Preferences | `<config dir>/prefs.json` (Edit → Preferences…) |
| Window size and position | `<config dir>/window.ron` |
| Cached entitlement | `<config dir>/entitlement.json` (the development server: `entitlement-dev.json`) |
| Sign-in token | The system keychain; without one, an encrypted file in the config dir |
| Log | `<data dir>/logs/app.log`, rotated at 5 MB, three old files kept (Help → Show log folder) |
| Decoded RAW frames | `<cache dir>/frames`, 4 GB by default (Preferences: frame cache size) |
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

## Menus and the keyboard map

The window has one menu bar and one status bar:

- **File**: Add folder…, Export…, Quit.
- **Edit**: Undo, Redo, Copy settings…, Paste settings, Apply to selected…,
  Select all, Preferences….
- **View**: Library, Develop, Before/After, Fit, 100 %.
- **Photo**: Match a reference…, Add version…, Reset all edits.
- **Account**: the studio's name, Switch studio, Check entitlement now, Sign out.
- **Help**: About, Show log folder, Keyboard shortcuts.

Menu items and keys are the same `shortcuts::Action`s. The keyboard map,
`shortcuts::MAP`, is a table of `(Action, KeyboardShortcut)` read once per frame
in `logic()`; the menus show its keys, and Help → Keyboard shortcuts lists it. A
press means the most specific matching entry, so `Ctrl+Shift+Z` is always redo,
never undo. A press whose action is not possible now is left for the widgets;
for example, `Delete` with no spot selected still removes a tone curve point.

Keys never fire while a widget has the keyboard (typing in a field, stepping a
slider with the arrows) or while a dialog is open. The `Esc` that ends typing in
a field does not also leave a tool. The tool keys act only in Develop. Holding
an arrow key or `Ctrl+Z` repeats; holding `Space` or a tool key does not. On macOS,
`Ctrl` reads as `Cmd`.

| Key | Action |
|-----|--------|
| `G` | Library |
| `D` | Develop (the active photo) |
| `Ctrl+Shift+E` | Export… |
| `R` | Crop and straighten tool |
| `Q` | Heal and clone spots tool |
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
| `F1` | About |
