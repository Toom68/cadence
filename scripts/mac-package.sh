#!/usr/bin/env bash
# Build Cadence.app and distributable archives for macOS.
#
# Usage:  scripts/mac-package.sh            # native architecture
#         UNIVERSAL=1 scripts/mac-package.sh # arm64 + x86_64 universal binary
#         SKIP_DMG=1 scripts/mac-package.sh  # zip only
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

if [ "${UNIVERSAL:-0}" = "1" ]; then
  ARCH="universal"
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --target "$target"
  done
  BIN_SRC="$ROOT/target/universal-$BIN_NAME"
  lipo -create \
    "target/aarch64-apple-darwin/release/$BIN_NAME" \
    "target/x86_64-apple-darwin/release/$BIN_NAME" \
    -output "$BIN_SRC"
else
  ARCH="$(uname -m)"
  [ "$ARCH" = "x86_64" ] || ARCH="arm64"
  cargo build --release --locked
  BIN_SRC="$ROOT/target/release/$BIN_NAME"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_SRC" "$APP/Contents/MacOS/$BIN_NAME"
chmod 755 "$APP/Contents/MacOS/$BIN_NAME"
cp assets/icon.icns "$APP/Contents/Resources/Cadence.icns"

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
