#!/usr/bin/env bash
# Checks the packaging inputs in packaging/ (and lf-app's [package.metadata.deb]):
# every file exists, every file they reference exists, the XML and plists are well formed
# (xmllint), the desktop entry is valid (desktop-file-validate), and the fixed values the
# installers depend on are present. Runs on Linux CI, which installs libxml2-utils and
# desktop-file-utils. Without those tools (a developer machine) the tool checks are skipped
# with a warning, except under CI, where a missing tool fails.
#
# Usage: bash scripts/release/check-packaging.sh
set -uo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root" || exit 1

failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }
ok() { echo "ok:   $*"; }

need_file() {
  if [ -f "$1" ]; then ok "$1 exists"; else fail "$1 is missing"; fi
}

# A line containing a fixed string.
need_text() { # file text description
  if [ -f "$1" ] && grep -qF -- "$2" "$1"; then ok "$3"; else fail "$3 ($1 lacks: $2)"; fi
}

have_tool() { # tool apt-package
  if command -v "$1" >/dev/null 2>&1; then return 0; fi
  if [ -n "${CI:-}" ]; then
    fail "$1 is not installed (apt-get install $2)"
  else
    echo "warn: $1 is not installed (apt-get install $2); its checks are skipped"
  fi
  return 1
}

# 1. The files themselves.
icon_sizes="16 24 32 48 64 128 256 512"
files=(
  assets/logo.png
  NOTICE
  LICENSE
  scripts/release/make-icons.sh
  scripts/release/make-icons.ps1
  crates/lf-app/examples/make_icons.rs
  crates/lf-app/build.rs
  packaging/icons/app.ico
  packaging/icons/app.icns
  packaging/windows/main.wxs
  packaging/windows/License.rtf
  packaging/macos/Info.plist
  packaging/macos/entitlements.plist
  packaging/macos/dmg-readme.txt
  packaging/linux/lensflow-image-editor.desktop
  packaging/linux/AppRun
)
for s in $icon_sizes; do files+=("packaging/icons/$s.png"); done
for f in "${files[@]}"; do need_file "$f"; done

# Icon file formats (magic bytes) and PNG sizes (IHDR width/height).
magic() { head -c "$2" "$1" 2>/dev/null | od -An -tx1 | tr -d ' \n'; }
[ -f packaging/icons/app.ico ] && { [ "$(magic packaging/icons/app.ico 4)" = "00000100" ] && ok "app.ico is an ICO" || fail "app.ico is not an ICO"; }
[ -f packaging/icons/app.icns ] && { [ "$(magic packaging/icons/app.icns 4)" = "69636e73" ] && ok "app.icns is an ICNS" || fail "app.icns is not an ICNS"; }
for s in $icon_sizes; do
  f="packaging/icons/$s.png"
  [ -f "$f" ] || continue
  hex=$(head -c 24 "$f" | od -An -tx1 | tr -d ' \n')
  w=$((16#${hex:32:8})); h=$((16#${hex:40:8}))
  if [ "${hex:0:16}" = "89504e470d0a1a0a" ] && [ "$w" -eq "$s" ] && [ "$h" -eq "$s" ]; then
    ok "$f is a ${s}x${s} PNG"
  else
    fail "$f is not a ${s}x${s} PNG (${w}x${h})"
  fi
done

# Executable bits (git keeps them; a checkout on Windows does not show them on disk).
for f in packaging/linux/AppRun scripts/release/make-icons.sh scripts/release/check-packaging.sh; do
  mode=$(git ls-files -s -- "$f" | cut -d' ' -f1)
  if [ "$mode" = "100755" ]; then ok "$f is executable in git"; else fail "$f is not executable in git (mode ${mode:-untracked})"; fi
done

# 2. Every file the packaging sources reference.
wxs=packaging/windows/main.wxs
if [ -f "$wxs" ]; then
  # $(sys.SOURCEFILEDIR) is the .wxs file's folder; the binary comes from -d BinDir=.
  refs=$(grep -oE '\$\(sys\.SOURCEFILEDIR\)[^"]+' "$wxs" | sed 's/^\$(sys\.SOURCEFILEDIR)//' | tr '\\' '/')
  [ -n "$refs" ] || fail "$wxs references no files"
  for r in $refs; do need_file "$(realpath -m --relative-to="$root" "packaging/windows/$r")"; done
fi
deb_refs=$(sed -n '/^\[package\.metadata\.deb\]/,/^\[/p' crates/lf-app/Cargo.toml | grep -oE '"\.\./\.\./[^"]+"' | tr -d '"')
[ -n "$deb_refs" ] || fail "crates/lf-app/Cargo.toml has no [package.metadata.deb] assets"
for r in $deb_refs; do need_file "$(realpath -m --relative-to="$root" "crates/lf-app/$r")"; done
icon_ref=$(grep -oE 'const ICON: &str = "[^"]+"' crates/lf-app/build.rs 2>/dev/null | sed 's/.*= "//; s/"$//')
if [ -n "$icon_ref" ]; then
  need_file "$(realpath -m --relative-to="$root" "crates/lf-app/$icon_ref")"
else
  fail "crates/lf-app/build.rs names no icon"
fi

# 3. Fixed values.
need_text "$wxs" 'UpgradeCode="315A3A2A-2148-4FEF-9433-0E79AF16981C"' "WiX UpgradeCode is the committed one"
need_text "$wxs" '<MajorUpgrade DowngradeErrorMessage="A newer version of lensflow-ImageEditor is already installed." />' "WiX MajorUpgrade"
need_text "$wxs" 'Version="$(var.Version)"' "WiX product version from -d Version="
need_text "$wxs" 'Scope="perMachine"' "WiX per-machine install"
need_text "$wxs" 'StandardDirectory Id="ProgramFiles64Folder"' "WiX installs under Program Files"
need_text "$wxs" 'Name="lensflow-ImageEditor"' "WiX install folder name"
if [ -f "$wxs" ] && grep -qE '<CustomAction|AppDataFolder|LocalAppDataFolder|PersonalFolder|HKCU|HKEY_CURRENT_USER' "$wxs"; then
  fail "$wxs has a custom action or writes under the user's profile"
else
  ok "WiX: no custom actions, nothing under the user's profile"
fi
plist=packaging/macos/Info.plist
need_text "$plist" '<string>in.lensflow.imageeditor</string>' "Info.plist bundle identifier"
need_text "$plist" '<key>LSMinimumSystemVersion</key>' "Info.plist minimum macOS"
need_text "$plist" '<string>11.0</string>' "Info.plist minimum macOS 11.0"
need_text "$plist" '<key>NSHighResolutionCapable</key>' "Info.plist high resolution"
need_text "$plist" '<key>CFBundleIconFile</key>' "Info.plist icon"
need_text "$plist" '<string>@VERSION@</string>' "Info.plist version placeholder"
need_text "$plist" '<string>lensflow-image-editor</string>' "Info.plist executable"
need_text packaging/macos/dmg-readme.txt 'Drag lensflow-ImageEditor to Applications.' "DMG read-me"
desktop=packaging/linux/lensflow-image-editor.desktop
need_text "$desktop" 'Name=lensflow-ImageEditor' "desktop entry name"
need_text "$desktop" 'Exec=lensflow-image-editor' "desktop entry exec"
need_text "$desktop" 'Icon=lensflow-image-editor' "desktop entry icon"
need_text "$desktop" 'Categories=Graphics;Photography;' "desktop entry categories"
need_text packaging/linux/AppRun 'LD_LIBRARY_PATH' "AppRun sets LD_LIBRARY_PATH"
need_text crates/lf-app/Cargo.toml 'depends = "$auto, libvulkan1"' ".deb depends"
need_text crates/lf-app/Cargo.toml '"usr/share/doc/lensflow-image-editor/NOTICE"' ".deb installs NOTICE"

# 4. Tools.
if have_tool xmllint libxml2-utils; then
  for f in "$wxs" "$plist" packaging/macos/entitlements.plist; do
    [ -f "$f" ] || continue
    if xmllint --noout "$f"; then ok "$f is well-formed XML"; else fail "$f is not well-formed XML"; fi
  done
fi
if have_tool desktop-file-validate desktop-file-utils; then
  if [ -f "$desktop" ]; then
    out=$(desktop-file-validate "$desktop" 2>&1)
    if [ -z "$out" ]; then ok "$desktop validates"; else echo "$out"; fail "$desktop does not validate cleanly"; fi
  fi
fi

if [ "$failures" -gt 0 ]; then
  echo "check-packaging: $failures failure(s)"
  exit 1
fi
echo "check-packaging: all checks passed"
