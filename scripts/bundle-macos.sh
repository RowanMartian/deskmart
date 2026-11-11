#!/bin/bash
# Builds dist/Deskmart.app (universal: Apple Silicon + Intel), ad-hoc signs it,
# and packages dist/Deskmart.dmg and dist/Deskmart-mac.zip. Run on macOS. No Xcode needed.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --locked --target aarch64-apple-darwin
cargo build --release --locked --target x86_64-apple-darwin

rm -rf dist
APP="dist/Deskmart.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/Deskmart" \
  target/aarch64-apple-darwin/release/deskmart \
  target/x86_64-apple-darwin/release/deskmart
sed "s/__VERSION__/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"
cp macos/Deskmart.icns "$APP/Contents/Resources/Deskmart.icns"
cp LICENSE.md NOTICE.md "$APP/Contents/Resources/"
plutil -lint "$APP/Contents/Info.plist"

# Ad-hoc signature (no Apple developer account needed). Required for Apple Silicon.
codesign --force --deep --sign - "$APP"
codesign --verify --verbose "$APP"

ditto -c -k --keepParent "$APP" dist/Deskmart-mac.zip

mkdir -p dist/dmg
cp -R "$APP" dist/dmg/
ln -s /Applications dist/dmg/Applications
hdiutil create -volname "Deskmart" -srcfolder dist/dmg -ov -format UDZO dist/Deskmart.dmg
rm -rf dist/dmg

lipo -info "$APP/Contents/MacOS/Deskmart"
ls -lh dist
