# Competitor analysis: feature gaps

Last updated: 2026-10-28 (editor at 0.2.0, `main` at `d442a78`).

Compared against Lightroom Classic 15.6 (Sept 2026), Capture One 16.8 (May–Aug 2026),
DxO PhotoLab 9 (Sept 2025) and Luminar Neo (Fall 2025 upgrade). Update this file as gaps
close or competitors ship new features.

## Where we already match or beat them

- **Editing:** masks with brightness/colour ranges, AI subject and sky masks, healing
  brush, Upright (Vertical/Full), lensfun lens profiles, LUT looks.
- **Merges:** HDR with ghost removal, multi-row panorama, focus stacking (Lightroom has
  no focus stacking).
- **AI tools:** denoise, upscale, object removal, skin retouch, background removal.
- **Library and output:** albums and smart albums, map, soft proofing, Display P3 /
  Adobe RGB / ICC exports, export presets, print to PDF, import from card.
- **Ahead of some rivals:** Fujifilm X-Trans decoding (Capture One still lacks it),
  "Match a reference", export straight to a LensFlow gallery.

## Gaps

### Tier 1: worth doing next (wedding and portrait studios)

| Gap | Who has it | Why it matters |
|---|---|---|
| ~~AI-assisted culling: eyes-open and eye-focus scores, blur detection~~ (done: AI → Cull); best of a burst done through Auto-stack | Lightroom 15.2/15.4, Capture One 16.8 | Studios cull thousands of shots per event; the main reason people pay for culling tools. Our face detector (`lf_ai::faces`) is part of the groundwork. |
| ~~Duplicate and similar photos grouped into stacks~~ (done: Home → Stacks, Auto-stack; sharpest on top) | Lightroom 15.5 | We have no stacks at all, which also hurts burst shooting. |
| People masks: ~~face skin, body skin, hair, clothes, whole people~~ (done: Masks → People); ~~eyes, lips, teeth~~ (done: Masks → Face, with irises) | Lightroom, PhotoLab 9 | Portrait edits are aimed at these; we only have subject and sky. |
| ~~Object and background AI masks (click or box an object)~~ (done: Masks → Select object, click) | Lightroom | Masks any object, not just subject or sky. |
| ~~Faces view: find a person across the catalogue~~ (done: AI → Find people, People in the sidebar) | Lightroom | Pull every shot of the bride, for example. |
| ~~AI keywording and search ("beach", "cake")~~ (done: AI → Find keywords) | Lightroom, Luminar | Searchable catalogue without typing keywords. |

### Tier 2: professional-quality gaps

| Gap | Who has it |
|---|---|
| Denoise on raw sensor data, before demosaicing (ours runs after) | DxO DeepPRIME XD3, Lightroom Denoise, Capture One Enhanced Denoise |
| Camera-matching profiles (camera looks, ~~DCP profiles~~ (done: RAW → Camera profile, user-imported), ~~colour calibration~~ (done: Develop → Calibration)) | All |
| HDR editing and output for HDR screens, ~~gain-map JPEG~~ (done: Export → JPEG HDR, highlights only); HDR editing and gain-map AVIF still to do | Lightroom |
| Reflection, ~~wire~~ (done: Remove objects → Select wires) and ~~people removal~~ (done: Remove objects → Select people) (one-click distractions) | Lightroom 15.6 |
| ~~Lens blur (AI depth of field)~~ (done: AI → Enhance → Lens blur, as a new photo); ~~depth mask~~ (done: Masks → Foreground, Background) | Lightroom |
| ~~Layers (stacked local edits with opacity)~~ (done: masks have opacity, show/hide and Duplicate) | Capture One, Luminar |
| ~~Upright Guided (draw lines)~~ (done: Develop → Guided), ~~Level, Auto~~ (done: Lens corrections → Upright) | Lightroom |
| New-camera support: rawler can't open Nikon HE/HE* NEF, DNG 1.7 JPEG XL, some Sony ARW / Phase One IIQ | All, updated monthly |

### Tier 3: workflow conveniences

- ~~Presets with an Amount slider~~ (done: looks and studio presets, 0–200 %); ~~adaptive presets that carry AI masks~~ (done: Save as preset → AI masks).
- ~~Auto-sync (edit one photo, the selection follows)~~ (done: Home → Settings → Auto-sync).
- ~~Batch rename of existing photos (PhotoLab 9)~~ (done: Home → Selection → Rename…).
- ~~Second-monitor view~~ (done: View → 2nd window), ~~reference view~~ (done: View → Reference view, `Shift+R`), ~~focus peaking~~ (done: View → Peaking, `Shift+J`), ~~100% loupe~~ (done: Library → Loupe, `E`).
- ~~Visualize spots~~ (done: View → Spots view).
- ~~Contact sheets~~ (done: Print's 9, 20 and 48 a page, with a sheet title, page numbers and Details captions) and ~~slideshows~~ (done: View → Slideshow).
- ~~Film negative conversion (Capture One 16.7, Negative Lab Pro)~~ (done: Develop → Film negative).
- Pixel-shift and ~~high-resolution merges~~ (done: File → Merge → High resolution, from a handheld burst; camera pixel-shift raw files are not read).
- ~~Panorama fill edges (Lightroom)~~ (done: Merge to panorama → Fill edges (AI)).
- ~~DNG export~~ (done: Export → DNG, linear) and ~~JPEG XL export~~ (done: Export → JPEG XL, lossless).

### More conveniences (added 2026-10-29)

- Info overlay over the photo (Lightroom's `I`): done, View → Info.
- Crop overlays (thirds, grid, golden ratio, diagonals, triangles, spiral) cycled with `O`: done.
- Before/after side by side (Lightroom's `Y`): done, View → Before/After side by side.
- Navigator panel (Lightroom's): done, Develop's side panel.
- Colour readout under the cursor (Lightroom's RGB under the histogram): done.
- Targeted adjustment for Colour mix (Lightroom's TAT): done.
- Defringe (purple and green amount and hue, Lightroom's): done, Develop → Defringe.
- Point Color (Lightroom 2023): done, Develop → Point colour (four colours).
- RAW+JPEG pairs shown as one (Lightroom's pairing): done, Home → Stacks → RAW+JPEG.
- Auto import from a watched folder (Lightroom's; a stand-in for tethering through the camera maker's own software): done, Home → Auto import.
- Export with previous (Lightroom's): done, `Ctrl+Alt+Shift+E`.
- Rotate in the Library grid (Lightroom's `Ctrl+[` / `Ctrl+]`): done.
- Thumbnail size slider (Lightroom's grid slider): done.
- Survey view (Lightroom's `N`): done, Library → Survey.
- History panel (Lightroom's): done, Develop → History.

### Tier 4: large or strategic

- Tethering: dropped 2026-10-28 (needs camera makers' SDKs). All three main rivals have
  it; Capture One competes on it.
- Cloud sync and a mobile app (Lightroom, Capture One).
- Video editing (Lightroom edits clips).
- Generative fill and expand (Photoshop; needs cloud models).
- ~~Sky replacement (Luminar)~~ (done: AI → Enhance → Replace sky).

## Recommended order

1. AI culling and stacks: biggest time saving for studios.
2. People masks and Faces: both build on the shipped face detector.
3. Raw-level denoise and camera profiles: the quality gaps reviewers notice first.
4. Presets with Amount, auto-sync, batch rename: small jobs each.

## Sources

- [Lightroom Classic 15.6 announcement](https://community.adobe.com/announcements-673/lightroom-classic-v15-6-is-here-cleaner-reflection-removal-smarter-masking-and-expanded-camera-lens-support-1644250)
- [What's new in Lightroom Classic](https://helpx.adobe.com/ca/lightroom-classic/desktop/introduction-to-lightroom-classic/whats-new.html)
- [Lightroom Classic release notes](https://helpx.adobe.com/ca/lightroom-classic/desktop/introduction-to-lightroom-classic/release-notes.html)
- [Capture One: what's new (August 2026)](https://www.captureone.com/explore-features/whats-new/16-8-5-upsell)
- [Capture One in 2026 (fotowissen)](https://www.fotowissen.eu/capture-one-im-jahr-2026-rauschentfernung-und-entwicklung/)
- [Capture One (Wikipedia)](https://en.wikipedia.org/wiki/Capture_One)
- [DxO PhotoLab 9 new features](https://support.dxo.com/hc/en-us/articles/7064070259997-What-are-the-new-features-of-DxO-PhotoLab-9)
- [PetaPixel on DxO PhotoLab 9](https://petapixel.com/2025/09/02/dxo-photolab-9-is-biggest-upgrade-to-the-raw-photo-editor-in-20-years)
- [DIY Photography on DxO PhotoLab 9](https://www.diyphotography.net/dxo-photolab-9-launches-ai-masks-local-noise-control-iphone-raw-support-and-more/)
- [Luminar Neo Fall Upgrade 2025](https://nikonrumors.com/2025/11/10/luminar-neo-fall-upgrade-2025-and-new-dxo-optics-modules-released.aspx)
