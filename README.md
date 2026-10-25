# LensFlow Image Editor

A native desktop RAW editor for LensFlow photo studios, written in Rust. It
brings the LensFlow web editor's develop pipeline to the desktop so that
studios can edit full-resolution RAW files locally and get the same look the
web editor produces.

## Build and test

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Rust stable is pinned by `rust-toolchain.toml`. GPU tests can run on
software Vulkan: set `WGPU_BACKEND=vulkan` and `LF_ALLOW_FALLBACK_ADAPTER=1`
(on Linux, install `mesa-vulkan-drivers libvulkan1`).

## Parity rule

`tests/fixtures/` is a verbatim copy of the web editor's golden renders:
six source images (`sources/`), 38 cases of normalised settings (`cases/`),
the web renderer's output for every source and case (`renders/`), and
`manifest.json` with a SHA-256 for each file.

For each `renders/<source>__<case>.png`, rendering `sources/<source>.png` with
`cases/<case>.json` must match per channel (R, G, B on a 0..1 scale):

- mean absolute error under 1/255, and
- maximum absolute error under 4/255.

Output sizes must match exactly. To update the fixtures, replace the whole
folder with the web repo's `frontend/parity-fixtures/` (do not merge) and
check the manifest hashes.

## Licence

Apache-2.0, see `LICENSE` and `NOTICE`.
