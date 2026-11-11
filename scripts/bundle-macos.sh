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
COMMIT="${DESKMART_COMMIT:-local}"
sed -e "s/__VERSION__/$VERSION/g" -e "s/__COMMIT__/$COMMIT/g" macos/Info.plist > "$APP/Contents/Info.plist"
cp macos/Deskmart.icns "$APP/Contents/Resources/Deskmart.icns"
cp LICENSE.md NOTICE.md "$APP/Contents/Resources/"
plutil -lint "$APP/Contents/Info.plist"

# Signing. With the DESKMART_SIGNING_P12 secret (see scripts/make-signing-cert.sh), every build is signed by
# the same self-made certificate, so macOS keeps Full Disk Access and other permissions across updates.
# Without it, an ad-hoc signature (no Apple developer account needed; required for Apple Silicon).
SIGN_ID="-"
if [ -n "${DESKMART_SIGNING_P12:-}" ]; then
  KC="${RUNNER_TEMP:-/tmp}/deskmart-signing.keychain-db"
  P12="${RUNNER_TEMP:-/tmp}/deskmart-signing.p12"
  printf '%s' "$DESKMART_SIGNING_P12" | base64 --decode > "$P12"
  security delete-keychain "$KC" 2>/dev/null || true
  security create-keychain -p deskmart "$KC"
  security set-keychain-settings -lut 3600 "$KC"
  security unlock-keychain -p deskmart "$KC"
  if security import "$P12" -k "$KC" -P "${DESKMART_SIGNING_PASSWORD:-}" -T /usr/bin/codesign \
     && security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k deskmart "$KC" >/dev/null; then
    security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
    SIGN_ID="$(security find-identity -p codesigning "$KC" | awk '/"/{print $2; exit}')"
    [ -n "$SIGN_ID" ] || SIGN_ID="-"
  fi
  rm -f "$P12"
fi
if ! codesign --force --deep --sign "$SIGN_ID" "$APP"; then
  echo "Signing with the certificate failed; falling back to an ad-hoc signature." >&2
  SIGN_ID="-"
  codesign --force --deep --sign - "$APP"
fi
[ "$SIGN_ID" = "-" ] && echo "Signed ad hoc." || echo "Signed with certificate $SIGN_ID."
codesign --verify --verbose "$APP"
codesign -d -r- "$APP" 2>&1 | grep designated || true

ditto -c -k --keepParent "$APP" dist/Deskmart-mac.zip

mkdir -p dist/dmg
cp -R "$APP" dist/dmg/
ln -s /Applications dist/dmg/Applications
hdiutil create -volname "Deskmart" -srcfolder dist/dmg -ov -format UDZO dist/Deskmart.dmg
rm -rf dist/dmg

lipo -info "$APP/Contents/MacOS/Deskmart"
ls -lh dist
