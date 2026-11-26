#!/usr/bin/env bash
# Download (once) and verify the SoundFonts Cadence ships with: the General
# MIDI font "Cadence GM.sf2" and the sampled "Cadence Harmonium.sf2".
#
# Usage:  scripts/fetch-soundfont.sh [DEST_DIR]
#
# Reads the pinned URL and SHA-256 from packaging/soundfont/pin.txt, keeps a
# verified copy in the cache folder ($CADENCE_SOUNDFONT_CACHE, default
# .cache/soundfonts in the repo — gitignored) as "Cadence GM.sf2" next to
# its license text. Then builds "Cadence Harmonium.sf2" there with
# scripts/build-harmonium-sf2.py (needs python3; it downloads and verifies
# the pinned recordings from packaging/soundfont/harmonium-pin.txt and checks
# the result against the pinned hash). Given DEST_DIR, copies both fonts and
# their license texts there (that is how the packaging scripts stage the
# fonts into the app).
#
# Developer builds (`cargo run`) find the fonts in .cache/soundfonts
# automatically, so running this once with no arguments is all it takes to
# hear General MIDI and harmonium tracks locally.
#
# Exit codes: 0 ok, 1 download/build failed (offline, URL gone, no python3),
# 2 bad arguments, 3 checksum mismatch (the bad file is deleted; packaging
# scripts treat this as fatal even for dev builds).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIN="$ROOT/packaging/soundfont/pin.txt"

[ "$#" -le 1 ] || { echo "usage: $0 [DEST_DIR]" >&2; exit 2; }
DEST="${1:-}"

pin_from() {
  local value
  value="$(sed -n "s/^$2=//p" "$1" | head -n 1 | tr -d '\r')"
  [ -n "$value" ] || { echo "error: $2 missing from $1" >&2; exit 1; }
  printf '%s\n' "$value"
}
pin() { pin_from "$PIN" "$1"; }
URL="$(pin SOUNDFONT_URL)"
SHA256="$(pin SOUNDFONT_SHA256)"
FILE="$(pin SOUNDFONT_FILE)"
LICENSE="$(pin SOUNDFONT_LICENSE)"
VERSION="$(pin SOUNDFONT_VERSION)"

CACHE="${CADENCE_SOUNDFONT_CACHE:-$ROOT/.cache/soundfonts}"
case "$CACHE" in
  /* | [A-Za-z]:*) ;;
  *) CACHE="$ROOT/$CACHE" ;;
esac
FONT="$CACHE/$FILE"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

mkdir -p "$CACHE"
if [ -f "$FONT" ] && [ "$(sha256_of "$FONT")" = "$SHA256" ]; then
  echo "SoundFont cached: $FONT"
else
  rm -f "$FONT"
  echo "Downloading $VERSION from $URL"
  TMP="$CACHE/.download-$$.tmp"
  trap 'rm -f "$TMP"' EXIT
  if ! curl -fL --retry 3 --retry-delay 2 --connect-timeout 20 -sS -o "$TMP" "$URL"; then
    echo "error: could not download the SoundFont from $URL" >&2
    exit 1
  fi
  GOT="$(sha256_of "$TMP")"
  if [ "$GOT" != "$SHA256" ]; then
    echo "error: SoundFont checksum mismatch for $URL" >&2
    echo "  expected $SHA256" >&2
    echo "  got      $GOT" >&2
    exit 3
  fi
  mv "$TMP" "$FONT"
  trap - EXIT
  echo "SoundFont verified: $FONT"
fi
cp "$ROOT/packaging/soundfont/$LICENSE" "$CACHE/$LICENSE"

# The harmonium is built, not downloaded as one file: the script fetches the
# pinned recordings (cached in .cache/soundfont-src/harmonium), verifies
# them, and only writes a font whose SHA-256 matches HARMONIUM_SHA256.
HPIN="$ROOT/packaging/soundfont/harmonium-pin.txt"
HFILE="$(pin_from "$HPIN" HARMONIUM_FILE)"
HLICENSE="$(pin_from "$HPIN" HARMONIUM_LICENSE)"
PYTHON="$(command -v python3 || command -v python || true)"
if [ -z "$PYTHON" ]; then
  echo "error: python3 is needed to build $HFILE (scripts/build-harmonium-sf2.py)" >&2
  exit 1
fi
set +e
"$PYTHON" "$ROOT/scripts/build-harmonium-sf2.py" --out "$CACHE/$HFILE"
build_status=$?
set -e
case "$build_status" in
  0) ;;
  3) exit 3 ;;
  *)
    echo "error: could not build $HFILE (scripts/build-harmonium-sf2.py exited $build_status)" >&2
    exit 1
    ;;
esac
cp "$ROOT/packaging/soundfont/$HLICENSE" "$CACHE/$HLICENSE"

if [ -n "$DEST" ]; then
  mkdir -p "$DEST"
  cp "$FONT" "$DEST/$FILE"
  cp "$ROOT/packaging/soundfont/$LICENSE" "$DEST/$LICENSE"
  cp "$CACHE/$HFILE" "$DEST/$HFILE"
  cp "$ROOT/packaging/soundfont/$HLICENSE" "$DEST/$HLICENSE"
  echo "Staged $FILE and $HFILE into $DEST"
fi
