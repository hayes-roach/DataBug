#!/bin/sh
# Build DataBug.app for this Mac without cargo-bundle.
#   packaging/macos/bundle.sh            # native architecture
#   packaging/macos/bundle.sh universal  # Apple silicon + Intel (needs both rustup targets)
# Output: dist/DataBug.app, dist/DataBug-macos-<arch>.zip and the installer
#         dist/DataBug-macos-<arch>.dmg (drag DataBug to Applications)
set -eu
cd "$(dirname "$0")/../.."
ROOT=$(pwd)
MODE=${1:-native}
# extra cargo flags, e.g. DATABUG_FEATURES="--features kerberos"
FEATURES=${DATABUG_FEATURES:-}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

if [ "$MODE" = universal ]; then
  cargo build --release --locked --target aarch64-apple-darwin $FEATURES
  cargo build --release --locked --target x86_64-apple-darwin $FEATURES
  mkdir -p target/universal
  lipo -create -output target/universal/databug target/aarch64-apple-darwin/release/databug target/x86_64-apple-darwin/release/databug
  BIN=target/universal/databug
  ARCH=universal
else
  cargo build --release --locked $FEATURES
  BIN=target/release/databug
  ARCH=$(uname -m)
fi

APP=dist/DataBug.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/logos"
cp "$BIN" "$APP/Contents/MacOS/databug"
cp crates/databug-app/assets/logos/* "$APP/Contents/Resources/logos/"

# icon: icon-1024.png -> AppIcon.icns
ICONSET=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET"
SRC=crates/databug-app/assets/icon/icon-1024.png
for s in 16 32 128 256 512; do
  sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DataBug</string>
  <key>CFBundleDisplayName</key><string>DataBug</string>
  <key>CFBundleIdentifier</key><string>dev.databug.DataBug</string>
  <key>CFBundleExecutable</key><string>databug</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Signing and notarization, when the release provides them:
#   MACOS_SIGN_IDENTITY (and MACOS_SIGN_KEYCHAIN): DataBug's Developer ID
#     certificate. The app gets the hardened runtime and Apple's timestamp,
#     and keeps the same code identity across versions, so keychain access
#     granted with "Always Allow" carries over to updates.
#   APPLE_API_KEY_PATH, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID: an App Store
#     Connect API key. The app and the disk image are notarized by Apple and
#     the tickets stapled, so Gatekeeper opens them without a warning.
# Without them: an ad-hoc signature (needed to run on Apple silicon).
sign() {
  if [ -n "${MACOS_SIGN_KEYCHAIN:-}" ]; then
    codesign --force --timestamp --keychain "$MACOS_SIGN_KEYCHAIN" --sign "$MACOS_SIGN_IDENTITY" "$@"
  else
    codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$@"
  fi
}
notarize() {
  out=$(xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" --wait --timeout 30m --output-format json) || true
  echo "$out"
  if ! printf '%s' "$out" | grep -q '"status":"Accepted"'; then
    id=$(printf '%s' "$out" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
    [ -n "$id" ] && xcrun notarytool log "$id" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" || true
    echo "Apple didn't notarize $1" >&2
    exit 1
  fi
}
NOTARIZE=0
if [ -n "${MACOS_SIGN_IDENTITY:-}" ] && [ -n "${APPLE_API_KEY_PATH:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ] && [ -n "${APPLE_API_ISSUER_ID:-}" ]; then
  NOTARIZE=1
fi

if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  sign --deep --options runtime --entitlements packaging/macos/entitlements.plist "$APP"
  codesign --verify --deep --strict "$APP"
  codesign -d -r- "$APP" 2>&1 | grep designated
else
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
fi
ZIP="dist/DataBug-macos-$ARCH.zip"
rm -f "$ZIP"
(cd dist && ditto -c -k --keepParent DataBug.app "DataBug-macos-$ARCH.zip")
if [ "$NOTARIZE" = 1 ]; then
  notarize "$ZIP"
  xcrun stapler staple "$APP"
  # zip again, now with the ticket stapled to the app
  rm -f "$ZIP"
  (cd dist && ditto -c -k --keepParent DataBug.app "DataBug-macos-$ARCH.zip")
  spctl --assess --type execute --verbose=2 "$APP"
fi
# installer disk image: the app next to a link to /Applications
STAGE=$(mktemp -d)/DataBug
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/Install DataBug.txt" <<TXT
To install DataBug, drag DataBug.app onto the Applications folder.

First launch: if macOS says the app is from an unidentified developer
(builds you make yourself aren't notarized), right-click DataBug in
Applications and choose Open, then Open again.

DataBug is free and open source under the Apache License 2.0. Use it at
your own risk: it is provided "as is", without warranty of any kind, and
to the fullest extent permitted by law the authors are not responsible for
anything that happens as a result of using it. Review the SQL you run and
keep backups. DataBug is not affiliated with JetBrains or any database
vendor; product names and logos belong to their owners.
TXT
cp LICENSE "$STAGE/LICENSE.txt"
cp NOTICE "$STAGE/NOTICE.txt"
DMG="dist/DataBug-macos-$ARCH.dmg"
rm -f "$DMG"
# hdiutil sometimes fails with "Resource busy" on build machines (a disk
# image still attached, Spotlight indexing): wait and try again
for try in 1 2 3 4 5; do
  if hdiutil create -volname "DataBug $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null; then
    break
  fi
  [ "$try" = 5 ] && { echo "hdiutil couldn't create $DMG" >&2; exit 1; }
  echo "hdiutil failed (attempt $try), retrying" >&2
  sleep $((try * 5))
done
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  sign "$DMG"
fi
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi
echo "Built $ROOT/$APP, $ROOT/dist/DataBug-macos-$ARCH.zip and $ROOT/$DMG"
