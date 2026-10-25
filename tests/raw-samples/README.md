# RAW sample corpus

Real-camera RAW files used by `crates/lf-raw/tests/samples.rs` as decoder smoke tests: one small
file per format (CR2, CR3, NEF, ARW, RAF, ORF, RW2, PEF, DNG), about 80 MB in total.

- Source: <https://raw.pixls.us>, which hosts RAW samples released under CC0 1.0 (public domain).
- `manifest.json` pins each file's URL, SHA-256, size, camera and licence.
- Fetch: `scripts/fetch-raw-samples.sh` (bash; needs `jq`, preinstalled on GitHub runners) or
  `scripts/fetch-raw-samples.ps1` (PowerShell). Both validate the manifest, skip files already present with the right hash, and exit non-zero on a download failure or
  hash mismatch.
- The files land in `tests/raw-samples/files/`, which is git-ignored. **Never commit them.**
- Without the files the tests print `skipped: no sample for <name>` and pass.
  With `LF_REQUIRE_SAMPLES=1` they fail instead if any file is missing (CI sets it when all are present).
- Treat downloads as untrusted data: only the decoder opens them.
