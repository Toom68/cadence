#!/usr/bin/env bash
# Build Cadence.app and distributable archives for macOS.
#
# Usage:  scripts/mac-package.sh            # native architecture
#         UNIVERSAL=1 scripts/mac-package.sh # arm64 + x86_64 universal binary
#         SKIP_DMG=1 scripts/mac-package.sh  # zip only
#         REQUIRE_SOUNDFONT=1 scripts/mac-package.sh # fail without the GM font (CI)
#
# Output: dist/Cadence.app, dist/Cadence-<version>-macos-<arch>.zip and,
# when hdiutil is available, dist/Cadence-<version>-macos-<arch>.dmg.
# The bundle is ad-hoc signed only (no Developer ID / notarization).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

DIST="$ROOT/dist"
APP="$DIST/Cadence.app"
BIN_NAME="cadence"
# Cargo builds into CARGO_TARGET_DIR when it is set (relative to the repo
# root, like cargo), else into ./target.
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
case "$TARGET_DIR" in
  /*) ;;
  *) TARGET_DIR="$ROOT/$TARGET_DIR" ;;
esac

if [ "${UNIVERSAL:-0}" = "1" ]; then
  ARCH="universal"
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --target "$target"
  done
  BIN_SRC="$TARGET_DIR/universal-$BIN_NAME"
  lipo -create \
    "$TARGET_DIR/aarch64-apple-darwin/release/$BIN_NAME" \
    "$TARGET_DIR/x86_64-apple-darwin/release/$BIN_NAME" \
    -output "$BIN_SRC"
else
  ARCH="$(uname -m)"
  [ "$ARCH" = "x86_64" ] || ARCH="arm64"
  cargo build --release --locked
  BIN_SRC="$TARGET_DIR/release/$BIN_NAME"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_SRC" "$APP/Contents/MacOS/$BIN_NAME"
chmod 755 "$APP/Contents/MacOS/$BIN_NAME"
cp assets/icon.icns "$APP/Contents/Resources/Cadence.icns"
# Font licenses (SIL OFL 1.1 for Inter, Bitstream Vera / DejaVu for DejaVu Sans);
# they must be in the bundle before it is signed.
mkdir -p "$APP/Contents/Resources/licenses"
cp assets/fonts/OFL.txt "$APP/Contents/Resources/licenses/Inter-OFL.txt"
cp assets/fonts/DejaVu-LICENSE.txt "$APP/Contents/Resources/licenses/DejaVu-LICENSE.txt"
# Buyer-facing files live inside the bundle's Resources (the zip contains
# only Cadence.app). EULA.txt ships when the owner provides one at repo root.
cp packaging/README.txt "$APP/Contents/Resources/README.txt"
# The built-in General MIDI SoundFont ("Cadence GM.sf2" = GeneralUser GS, see
# packaging/soundfont/README.md): downloaded once into .cache/soundfonts,
# SHA-256 verified, never committed. Without network a dev build warns and
# ships without it; REQUIRE_SOUNDFONT=1 (CI) makes that fatal, and a checksum
# mismatch is always fatal.
SOUNDFONT_DIR="$APP/Contents/Resources/soundfonts"
SOUNDFONT_STAGED=0
set +e
scripts/fetch-soundfont.sh "$SOUNDFONT_DIR"
fetch_status=$?
set -e
if [ "$fetch_status" -eq 0 ]; then
  SOUNDFONT_STAGED=1
  cp packaging/soundfont/GeneralUser-GS-LICENSE.txt "$APP/Contents/Resources/licenses/GeneralUser-GS-LICENSE.txt"
elif [ "$fetch_status" -eq 3 ] || [ "${REQUIRE_SOUNDFONT:-0}" = "1" ]; then
  echo "error: could not stage the General MIDI SoundFont (scripts/fetch-soundfont.sh exited $fetch_status)" >&2
  exit 1
else
  rm -rf "$SOUNDFONT_DIR"
  echo "warning: packaging WITHOUT the built-in General MIDI SoundFont (download failed); GM tracks will use fallback sounds" >&2
fi
cp THIRD-PARTY-NOTICES.txt "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt"
if [ -f EULA.txt ]; then
  cp EULA.txt "$APP/Contents/Resources/EULA.txt"
else
  echo "warning: no EULA.txt at the repo root — add your end-user license before selling" >&2
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Cadence</string>
  <key>CFBundleDisplayName</key><string>Cadence</string>
  <key>CFBundleIdentifier</key><string>studio.cadence.app</string>
  <key>CFBundleExecutable</key><string>$BIN_NAME</string>
  <key>CFBundleIconFile</key><string>Cadence</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSHumanReadableCopyright</key><string>Cadence contributors</string>
</dict>
</plist>
PLIST
printf 'APPL????' > "$APP/Contents/PkgInfo"

plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

# Smoke test: the bundled binary runs and reports its version.
"$APP/Contents/MacOS/$BIN_NAME" --version
# ...and finds its own copy of the GM SoundFont (an isolated data dir keeps the
# packager's own fonts and packs out of it; the bundle never falls back to the
# repo's dev cache once it has a soundfonts folder).
if [ "$SOUNDFONT_STAGED" = "1" ]; then
  SMOKE_DATA="$(mktemp -d)"
  LISTING="$(env -u CADENCE_SOUNDFONT_DIR -u CADENCE_FONTS_DIR -u CADENCE_PACKS_DIR \
    CADENCE_DATA_DIR="$SMOKE_DATA" "$APP/Contents/MacOS/$BIN_NAME" --list-instruments)"
  rm -rf "$SMOKE_DATA"
  case "$LISTING" in
    *"Cadence GM (built-in)"*) echo "Built-in SoundFont: OK" ;;
    *)
      echo "error: the packaged app does not list the built-in SoundFont" >&2
      exit 1
      ;;
  esac
fi

BASE="Cadence-$VERSION-macos-$ARCH"
rm -f "$DIST/$BASE.zip" "$DIST/$BASE.dmg"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$DIST/$BASE.zip"
echo "Created $DIST/$BASE.zip"

if [ "${SKIP_DMG:-0}" != "1" ] && command -v hdiutil >/dev/null 2>&1; then
  STAGE="$(mktemp -d)"
  trap 'rm -rf "$STAGE"' EXIT
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  if hdiutil create -volname "Cadence" -srcfolder "$STAGE" -ov -format UDZO "$DIST/$BASE.dmg" >/dev/null; then
    echo "Created $DIST/$BASE.dmg"
  else
    echo "warning: hdiutil could not create a dmg; the zip is still available" >&2
  fi
fi
