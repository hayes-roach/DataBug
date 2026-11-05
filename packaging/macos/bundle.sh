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

# ad-hoc signature so Apple silicon will run it
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
rm -f "dist/DataBug-macos-$ARCH.zip"
(cd dist && ditto -c -k --keepParent DataBug.app "DataBug-macos-$ARCH.zip")
# installer disk image: the app next to a link to /Applications
STAGE=$(mktemp -d)/DataBug
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/Install DataBug.txt" <<TXT
To install DataBug, drag DataBug.app onto the Applications folder.

First launch: if macOS says the app is from an unidentified developer,
right-click DataBug in Applications and choose Open, then Open again.
TXT
DMG="dist/DataBug-macos-$ARCH.dmg"
rm -f "$DMG"
hdiutil create -volname "DataBug $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
echo "Built $ROOT/$APP, $ROOT/dist/DataBug-macos-$ARCH.zip and $ROOT/$DMG"
