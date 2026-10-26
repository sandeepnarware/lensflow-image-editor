# Regenerates packaging/icons/ from assets/logo.png (the image-crate helper; no system tools).
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..\..')
cargo run -q -p lf-app --example make_icons -- assets/logo.png packaging/icons
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
