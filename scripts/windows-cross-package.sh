#!/usr/bin/env bash
# Cross-build the Windows x64 portable archive on macOS or Linux.
#
# Produces:
#   dist/Cadence-<version>-windows-x64/Cadence.exe (+ soundfonts/Cadence GM.sf2
#   and soundfonts/Cadence Harmonium.sf2)
#   dist/Cadence-<version>-windows-x64.zip
#
# Requirements (this script installs nothing):
#   rustup target add x86_64-pc-windows-gnu
#   mingw-w64 on PATH:
#     macOS:           brew install mingw-w64   (/opt/homebrew/bin or /usr/local/bin)
#     Debian/Ubuntu:   sudo apt install gcc-mingw-w64-x86-64
#
# Compatible with macOS /bin/bash 3.2 (no mapfile, no empty-array expansion).
#
# x86_64-pc-windows-msvc remains the primary native Windows build (CI). This
# GNU cross-build is an additional portable artifact. The exe is unsigned and
# has NOT been executed under Windows; verify on a real Windows machine.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# rustup/cargo may not be on PATH (Rust installed with --no-modify-path).
export PATH="$HOME/.cargo/bin:$PATH"
# Homebrew prefixes: prepended only when present (no-op on most Linux systems).
for prefix in /opt/homebrew/bin /usr/local/bin; do
  [ -d "$prefix" ] && PATH="$prefix:$PATH"
done
export PATH

TARGET="x86_64-pc-windows-gnu"
TRIPLE="x86_64-w64-mingw32"

command -v cargo >/dev/null 2>&1 || { echo "cargo not found; install Rust first" >&2; exit 1; }
command -v "${TRIPLE}-gcc" >/dev/null 2>&1 || {
  echo "missing ${TRIPLE}-gcc; install mingw-w64 (brew install mingw-w64)" >&2; exit 1; }
command -v "${TRIPLE}-objdump" >/dev/null 2>&1 || {
  echo "missing ${TRIPLE}-objdump; install mingw-w64" >&2; exit 1; }
command -v "${TRIPLE}-windres" >/dev/null 2>&1 || {
  echo "missing ${TRIPLE}-windres; install mingw-w64 (needed to embed the icon)" >&2; exit 1; }

if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "Rust target $TARGET is not installed; run: rustup target add $TARGET" >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

echo "==> cargo build --release --locked --target $TARGET"
cargo build --release --locked --target "$TARGET"

# Cargo builds into CARGO_TARGET_DIR when it is set (relative to the repo
# root, like cargo), else into ./target.
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
case "$TARGET_DIR" in
  /* | [A-Za-z]:*) ;;
  *) TARGET_DIR="$ROOT/$TARGET_DIR" ;;
esac
SRC="$TARGET_DIR/$TARGET/release/cadence.exe"
[ -f "$SRC" ] || { echo "expected binary not found: $SRC" >&2; exit 1; }

file "$SRC" | grep -qi 'pe32' || { echo "$SRC is not a PE binary" >&2; exit 1; }

DIST="$ROOT/dist"
BASE="Cadence-$VERSION-windows-x64"
STAGE="$DIST/$BASE"
rm -rf "$STAGE" "$DIST/$BASE.zip"
mkdir -p "$STAGE"
cp "$SRC" "$STAGE/Cadence.exe"

# Font licenses (SIL OFL 1.1 for Inter, Bitstream Vera / DejaVu for DejaVu Sans).
mkdir -p "$STAGE/licenses"
cp assets/fonts/OFL.txt "$STAGE/licenses/Inter-OFL.txt"
cp assets/fonts/DejaVu-LICENSE.txt "$STAGE/licenses/DejaVu-LICENSE.txt"

# Buyer-facing files: README, third-party notices, and the EULA when the
# owner has provided one at the repo root.
cp packaging/README.txt "$STAGE/README.txt"
# The built-in SoundFonts (GM + harmonium) in soundfonts/ next to Cadence.exe (see
# scripts/mac-package.sh): a checksum mismatch is fatal, a failed download only
# with REQUIRE_SOUNDFONT=1.
set +e
scripts/fetch-soundfont.sh "$STAGE/soundfonts"
fetch_status=$?
set -e
if [ "$fetch_status" -eq 0 ]; then
  cp packaging/soundfont/GeneralUser-GS-LICENSE.txt "$STAGE/licenses/GeneralUser-GS-LICENSE.txt"
  cp packaging/soundfont/Cadence-Harmonium-LICENSE.txt "$STAGE/licenses/Cadence-Harmonium-LICENSE.txt"
elif [ "$fetch_status" -eq 3 ] || [ "${REQUIRE_SOUNDFONT:-0}" = "1" ]; then
  echo "error: could not stage the built-in SoundFonts (scripts/fetch-soundfont.sh exited $fetch_status)" >&2
  exit 1
else
  rm -rf "$STAGE/soundfonts"
  echo "warning: packaging WITHOUT the built-in SoundFonts (download/build failed)" >&2
fi
cp THIRD-PARTY-NOTICES.txt "$STAGE/THIRD-PARTY-NOTICES.txt"
if [ -f EULA.txt ]; then
  cp EULA.txt "$STAGE/EULA.txt"
else
  echo "warning: no EULA.txt at the repo root — add your end-user license before selling" >&2
fi

# Locate a mingw runtime DLL by name. Tries, in order:
#   1. <triple>-gcc -print-file-name=<dll>   (gcc's own search logic)
#   2. the directory containing <triple>-gcc
#   3. <triple>-gcc -print-sysroot subdirectories
# Prints the resolved path on stdout; returns 1 when not found.
find_runtime_dll() {
  local dll="$1"
  local candidate sysroot d
  candidate="$("${TRIPLE}-gcc" -print-file-name="$dll" 2>/dev/null || true)"
  if [ -n "$candidate" ] && [ "$candidate" != "$dll" ] && [ -f "$candidate" ]; then
    printf '%s\n' "$candidate"
    return 0
  fi
  candidate="$(dirname "$(command -v "${TRIPLE}-gcc")")/$dll"
  if [ -f "$candidate" ]; then
    printf '%s\n' "$candidate"
    return 0
  fi
  sysroot="$("${TRIPLE}-gcc" -print-sysroot 2>/dev/null || true)"
  if [ -n "$sysroot" ]; then
    for d in "$sysroot/mingw/bin" "$sysroot/${TARGET}/bin" "$sysroot/mingw/lib" "$sysroot/lib"; do
      if [ -f "$d/$dll" ]; then
        printf '%s\n' "$d/$dll"
        return 0
      fi
    done
  fi
  return 1
}

# Inspect the PE import table. Imports matching mingw runtime DLL names
# (libgcc*, libstdc++*, libwinpthread*, libgomp*, libssp*, libatomic*,
# libquadmath*, libclang_rt*) are REQUIRED: with +crt-static none should be
# imported, and any that are must be bundled or the build fails honestly.
# Every other import is assumed to be a Windows system DLL and is never copied
# — the full list is printed for manual review.
echo "==> PE imports:"
DLL_LIST="$("${TRIPLE}-objdump" -p "$STAGE/Cadence.exe" \
  | sed -n 's/^[[:space:]]*DLL Name: //p' | sort -u)"
printf '%s\n' "$DLL_LIST" | sed 's/^/  /'

shopt -s nocasematch
missing=""
bundled=""
count=0
while IFS= read -r dll; do
  [ -n "$dll" ] || continue
  count=$((count + 1))
  case "$dll" in
    libgcc*|libstdc++*|libwinpthread*|libgomp*|libssp*|libatomic*|libquadmath*|libclang_rt*)
      if found="$(find_runtime_dll "$dll")"; then
        cp "$found" "$STAGE/"
        bundled="$bundled $(basename "$found")"
        echo "  bundled required runtime DLL: $dll <- $found"
      else
        missing="$missing $dll"
        echo "  MISSING required runtime DLL: $dll" >&2
      fi
      ;;
    *)
      : # assumed Windows system DLL; never copied
      ;;
  esac
done <<EOF_DLLS
$DLL_LIST
EOF_DLLS
shopt -u nocasematch

if [ "$count" -eq 0 ]; then
  echo "error: no DLL imports found; objdump output unexpected" >&2
  exit 1
fi
if [ -n "$missing" ]; then
  echo "error: required mingw runtime DLLs not found:$missing" >&2
  echo "The exe would not run on a clean Windows install. Fix static linkage" >&2
  echo "or install the missing DLLs next to the toolchain, then re-run." >&2
  exit 1
fi
if [ -n "$bundled" ]; then
  echo "==> Bundled mingw runtime DLLs:$bundled"
else
  echo "==> No mingw runtime DLLs imported; static linkage confirmed by import table."
fi
echo "==> Remaining imports are treated as Windows system DLLs; review the list above."

if command -v zip >/dev/null 2>&1; then
  (cd "$DIST" && zip -q -r "$BASE.zip" "$BASE")
else
  python3 - "$DIST" "$BASE" <<'PY'
import os
import sys
import zipfile

dist, base = sys.argv[1], sys.argv[2]
out = os.path.join(dist, base + ".zip")
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zf:
    for root, _dirs, files in os.walk(os.path.join(dist, base)):
        for name in files:
            path = os.path.join(root, name)
            zf.write(path, os.path.relpath(path, dist))
print("Created", out)
PY
fi

echo "Created $STAGE/Cadence.exe"
echo "Created $DIST/$BASE.zip"
echo "NOTE: unsigned binary, not run under Windows. Verify on a real Windows machine."
