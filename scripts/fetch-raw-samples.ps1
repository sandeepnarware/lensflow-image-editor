# Download the CC0 RAW sample corpus listed in tests/raw-samples/manifest.json into
# tests/raw-samples/files/ and verify every SHA-256. Exits non-zero on a download
# failure, hash mismatch or malformed manifest.
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$raw = Get-Content -Raw (Join-Path $root 'tests/raw-samples/manifest.json')
$samples = @(($raw | ConvertFrom-Json).samples)
$expected = ([regex]::Matches($raw, '"sha256"')).Count
if ($samples.Count -ne $expected -or $samples.Count -eq 0) { Write-Host "parsed $($samples.Count) entries but the manifest has $expected"; exit 2 }
foreach ($s in $samples) {
  if ($s.name -notmatch '^[A-Za-z0-9._-]+$' -or $s.name.StartsWith('..')) { Write-Host "bad name in manifest: $($s.name)"; exit 2 }
  if (-not ([string]$s.url).StartsWith('https://')) { Write-Host "bad url for $($s.name): $($s.url)"; exit 2 }
  if ($s.sha256 -cnotmatch '^[0-9a-f]{64}$') { Write-Host "bad sha256 for $($s.name): $($s.sha256)"; exit 2 }
}
$dir = Join-Path $root 'tests/raw-samples/files'
New-Item -ItemType Directory -Force $dir | Out-Null
$fail = 0
foreach ($s in $samples) {
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
