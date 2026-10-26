#!/usr/bin/env bash
# Regenerates packaging/icons/ from assets/logo.png (the image-crate helper; no system tools).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo run -q -p lf-app --example make_icons -- assets/logo.png packaging/icons
