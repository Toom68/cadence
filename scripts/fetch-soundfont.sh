#!/usr/bin/env bash
# Download (once) and verify the General MIDI SoundFont Cadence ships with.
#
# Usage:  scripts/fetch-soundfont.sh [DEST_DIR]
#
# Reads the pinned URL and SHA-256 from packaging/soundfont/pin.txt, keeps a
# verified copy in the cache folder ($CADENCE_SOUNDFONT_CACHE, default
# .cache/soundfonts in the repo — gitignored) as "Cadence GM.sf2" next to
# its license text, and, given DEST_DIR, copies both there (that is how the
# packaging scripts stage the font into the app).
#
# Developer builds (`cargo run`) find the font in .cache/soundfonts
# automatically, so running this once with no arguments is all it takes to
# hear General MIDI tracks locally.
#
# Exit codes: 0 ok, 1 download failed (offline, URL gone), 2 bad arguments,
# 3 checksum mismatch (the bad file is deleted; packaging scripts treat this
# as fatal even for dev builds).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIN="$ROOT/packaging/soundfont/pin.txt"

[ "$#" -le 1 ] || { echo "usage: $0 [DEST_DIR]" >&2; exit 2; }
DEST="${1:-}"

pin() {
  local value
  value="$(sed -n "s/^$1=//p" "$PIN" | head -n 1 | tr -d '\r')"
  [ -n "$value" ] || { echo "error: $1 missing from $PIN" >&2; exit 1; }
  printf '%s\n' "$value"
}
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

if [ -n "$DEST" ]; then
  mkdir -p "$DEST"
  cp "$FONT" "$DEST/$FILE"
  cp "$ROOT/packaging/soundfont/$LICENSE" "$DEST/$LICENSE"
  echo "Staged $FILE into $DEST"
fi
