#!/bin/sh
# Wraps target/IDMX.app in a signed-ad-hoc disk image for downloading: target/IDMX-<version>-macos-<arch>.dmg
# Run ./scripts/mac-app.sh --release first. (Not notarized: macOS asks you to right-click → Open the first time.)
set -e
cd "$(dirname "$0")/.."
# TARGET=x86_64-apple-darwin packages the Intel build made by mac-app.sh
case "$TARGET" in x86_64*) ARCH=x64; APP=target/IDMX-x64.app;; *) APP=target/IDMX.app;; esac
VERSION=$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed 's/[^0-9.]//g')
[ -z "$ARCH" ] && case "$(uname -m)" in arm64) ARCH=arm64;; *) ARCH=x64;; esac
OUT="target/IDMX-$VERSION-macos-$ARCH.dmg"
codesign --force --deep --sign - "$APP"
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT"
hdiutil create -volname "IDMX" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null
rm -rf "$STAGE"
echo "Built $OUT ($(du -h "$OUT" | cut -f1))"
