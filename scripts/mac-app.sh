#!/bin/sh
# Builds IDMX and wraps it in a real macOS app bundle (target/IDMX.app), so the Dock, menu bar and
# Cmd-Tab show "IDMX" and its icon rather than the raw program name. Usage: ./scripts/mac-app.sh [--release] [--open]
set -e
cd "$(dirname "$0")/.."
PROFILE=debug; FLAG=""
case "$*" in *--release*) PROFILE=release; FLAG="--release";; esac
npm --prefix ui run build
# TARGET=x86_64-apple-darwin builds the Intel version (works from an Apple-silicon Mac).
if [ -n "$TARGET" ]; then
  cargo build $FLAG -p idmx-app --target "$TARGET"
  BIN="target/$TARGET/$PROFILE/idmx-app"
  case "$TARGET" in x86_64*) SUFFIX=-x64;; *) SUFFIX=-arm64;; esac
else
  cargo build $FLAG -p idmx-app
  BIN="target/$PROFILE/idmx-app"
  SUFFIX=""
fi
APP=target/IDMX$SUFFIX.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/IDMX"
cp src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"
VERSION=$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed 's/[^0-9.]//g')
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>IDMX</string>
  <key>CFBundleDisplayName</key><string>IDMX</string>
  <key>CFBundleExecutable</key><string>IDMX</string>
  <key>CFBundleIdentifier</key><string>app.idmx.desktop</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key><array><dict>
    <key>CFBundleURLName</key><string>Magnet link</string>
    <key>CFBundleURLSchemes</key><array><string>magnet</string></array>
  </dict></array>
  <key>CFBundleDocumentTypes</key><array><dict>
    <key>CFBundleTypeName</key><string>BitTorrent file</string>
    <key>CFBundleTypeExtensions</key><array><string>torrent</string></array>
    <key>CFBundleTypeRole</key><string>Viewer</string>
  </dict></array>
</dict></plist>
PLIST
echo "Built $APP"
case "$*" in *--open*) open "$APP";; esac
