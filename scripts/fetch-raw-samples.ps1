# Download the CC0 RAW sample corpus listed in tests/raw-samples/manifest.json into
# tests/raw-samples/files/ and verify every SHA-256. Exits non-zero on failure.
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content -Raw (Join-Path $root 'tests/raw-samples/manifest.json') | ConvertFrom-Json
$dir = Join-Path $root 'tests/raw-samples/files'
New-Item -ItemType Directory -Force $dir | Out-Null
$fail = 0
foreach ($s in $manifest.samples) {
  $f = Join-Path $dir $s.name
  if ((Test-Path $f) -and ((Get-FileHash $f -Algorithm SHA256).Hash.ToLower() -eq $s.sha256)) { Write-Host "ok      $($s.name)"; continue }
  Remove-Item -Force $f -ErrorAction SilentlyContinue
  $got = $false
  for ($try = 0; $try -lt 3 -and -not $got; $try++) {
    try { Invoke-WebRequest -Uri $s.url -OutFile $f -UseBasicParsing; $got = $true } catch { Start-Sleep -Seconds 2 }
  }
  if (-not $got) { Write-Host "FAILED  $($s.name): download error"; $fail = 1; continue }
  if ((Get-FileHash $f -Algorithm SHA256).Hash.ToLower() -ne $s.sha256) {
    Write-Host "FAILED  $($s.name): sha256 mismatch"; Remove-Item -Force $f; $fail = 1; continue
  }
  Write-Host "fetched $($s.name)"
}
exit $fail
