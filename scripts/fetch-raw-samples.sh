#!/usr/bin/env bash
# Download the CC0 RAW sample corpus listed in tests/raw-samples/manifest.json into
# tests/raw-samples/files/ and verify every file's SHA-256. Exits non-zero on a
# download failure, hash mismatch or malformed manifest. Needs jq. Files are only ever
# fed to the decoder.
set -u
root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/tests/raw-samples/manifest.json"
dir="$root/tests/raw-samples/files"
if ! command -v jq >/dev/null; then
  echo "jq is required (https://jqlang.github.io/jq/) but was not found on PATH" >&2
  exit 2
fi
mkdir -p "$dir"
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

expected=$(jq -er '.samples | length' "$manifest") || { echo "cannot read manifest" >&2; exit 2; }
rows=$(jq -r '.samples[] | [.name, .url, .sha256] | @tsv' "$manifest") || { echo "cannot parse manifest" >&2; exit 2; }
count=0
while IFS=$'\t' read -r n u h; do
  [ -n "$n" ] || continue
  count=$((count + 1))
  case "$n" in */*|*\*|..*|"") echo "bad name in manifest: $n" >&2; exit 2;; esac
  case "$u" in https://*) ;; *) echo "bad url for $n: $u" >&2; exit 2;; esac
  if ! printf '%s' "$h" | grep -Eq '^[0-9a-f]{64}$'; then echo "bad sha256 for $n: $h" >&2; exit 2; fi
done <<< "$rows"
if [ "$count" != "$expected" ]; then
  echo "parsed $count entries but the manifest has $expected" >&2; exit 2
fi

fail=0
while IFS=$'\t' read -r n u h; do
  f="$dir/$n"
  if [ -f "$f" ] && [ "$(sha "$f")" = "$h" ]; then echo "ok      $n"; continue; fi
  rm -f "$f"
  if ! curl -L --fail -sS --retry 3 --retry-delay 2 -o "$f" "$u"; then
    echo "FAILED  $n: download error" >&2; rm -f "$f"; fail=1; continue
  fi
  if [ "$(sha "$f")" != "$h" ]; then
    echo "FAILED  $n: sha256 mismatch" >&2; rm -f "$f"; fail=1; continue
  fi
  echo "fetched $n"
done <<< "$rows"
exit $fail
