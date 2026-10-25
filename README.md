# LensFlow Image Editor

A native desktop RAW editor for LensFlow photo studios, written in Rust. It
brings the LensFlow web editor's develop pipeline to the desktop so that
studios can edit full-resolution RAW files locally and get the same look the
web editor produces.

## Status

Part 2a (core pipeline parity: the settings model and a headless GPU renderer
that reproduces the web editor's output) and Part 2b (RAW decoding and
development, so `lf-render` opens camera files) are built. The app UI comes in
a later part.

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
  or AHD; X-Trans is simple), white-balances, converts to linear sRGB, applies
  the develop profile and highlight handling, and returns an `Opened` picture.
  `render::renderer_for` turns it into a `Renderer`. It also holds the
  `lf-render` example (see below).
- `lf-fixtures`: loads the golden fixtures in `tests/fixtures` and compares
  renders against them (mean, 99.9th percentile and maximum error).

## Supported files

- RAW (checked against the sample corpus): Canon CR2 and CR3, Nikon NEF,
  Sony ARW, Fujifilm RAF (X-Trans), Olympus ORF, Panasonic RW2, Pentax PEF
  and DNG.
- JPEG, PNG, TIFF and WebP.
- HEIC and AVIF are not supported yet: opening one gives a plain error.
  (Their embedded previews are HEVC too, so they cannot be shown without an
  HEVC decoder.)

## Headroom rule

A RAW file records light above what an 8-bit picture can hold. Development
keeps those values as unclamped half floats (above 1.0) all the way to the
develop pass, so pulling highlights down later brings detail back instead of
flat white. By default the profile is `camera` (a gentle toe and shoulder
curve) and highlights are `reconstruct` (over-range colour is pulled toward
neutral, keeping its brightness); `desktop.develop` in the settings can pick
`linear` and `clip` (clamp each channel at 1.0). An 8-bit file has no
headroom, so it takes the web-identical 8-bit path and its output is the one
the parity gate covers.

## 8-bit and float paths

`render::renderer_for` picks the path from the picture: `Picture::EightBit`
goes through `Renderer::new` (RGBA8 chain, as the web editor); `Picture::Float`
(RAW) is uploaded as `Rgba16Float` with mips and goes through
`Renderer::from_texture`, whose whole chain stays half float. Float
intermediates can differ from the 8-bit chain by under 1/255 on blur-dependent
settings; the parity gate, not bit-identity, is the contract.

## RAW sample corpus

`tests/raw-samples/` pins nine small CC0 files from raw.pixls.us (one per
format, about 80 MB). Fetch them with `scripts/fetch-raw-samples.sh` (or
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
  [--develop camera|linear] [--highlights clip|reconstruct] [--quality fast|best]
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
