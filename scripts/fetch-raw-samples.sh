#!/usr/bin/env bash
# Download the CC0 RAW sample corpus listed in tests/raw-samples/manifest.json into
# tests/raw-samples/files/ and verify every file's SHA-256. Exits non-zero on a
# download failure or hash mismatch. Files are only ever fed to the decoder.
set -u
root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/tests/raw-samples/manifest.json"
dir="$root/tests/raw-samples/files"
mkdir -p "$dir"
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
field() { grep "\"$1\"" | sed 's/.*: *"\{0,1\}\([^",]*\)"\{0,1\},\{0,1\}$/\1/'; }
names=(); urls=(); hashes=()
while IFS= read -r l; do names+=("$l"); done < <(field name < "$manifest")
while IFS= read -r l; do urls+=("$l"); done < <(field url < "$manifest")
while IFS= read -r l; do hashes+=("$l"); done < <(field sha256 < "$manifest")
fail=0
for i in "${!names[@]}"; do
  n=${names[$i]}; f="$dir/$n"
  if [ -f "$f" ] && [ "$(sha "$f")" = "${hashes[$i]}" ]; then echo "ok      $n"; continue; fi
  rm -f "$f"
  if ! curl -L --fail -sS --retry 3 --retry-delay 2 -o "$f" "${urls[$i]}"; then
    echo "FAILED  $n: download error" >&2; rm -f "$f"; fail=1; continue
  fi
  if [ "$(sha "$f")" != "${hashes[$i]}" ]; then
    echo "FAILED  $n: sha256 mismatch" >&2; rm -f "$f"; fail=1; continue
  fi
  echo "fetched $n"
done
exit $fail
