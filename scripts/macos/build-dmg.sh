#!/usr/bin/env bash
# Build "Matrix Finder.app" and a drag-to-Applications DMG (macOS only).
#
#   scripts/macos/build-dmg.sh               # app + DMG for this Mac's architecture
#   scripts/macos/build-dmg.sh --universal   # one binary for Apple silicon and Intel
#   scripts/macos/build-dmg.sh --app-only    # only target/macos/Matrix Finder.app
#   scripts/macos/build-dmg.sh --no-layout   # skip the Finder window styling (CI, SSH)
#   scripts/macos/build-dmg.sh --sign "Developer ID Application: Name (TEAMID)"
#
# Environment: BUNDLE_ID (default org.matrixfinder.MatrixFinder), FEATURES (cargo
# features, default "--features vendored-openssl"), BIN_PATH (use a prebuilt binary).
set -euo pipefail
cd "$(dirname "$0")/../.."

APP_NAME="Matrix Finder"
BIN=matrix_finder
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUNDLE_ID=${BUNDLE_ID:-org.matrixfinder.MatrixFinder}
FEATURES=${FEATURES:---features vendored-openssl}
OUT=target/macos

universal=0 layout=1 app_only=0 sign_id="-"
while [ $# -gt 0 ]; do
    case "$1" in
        --universal) universal=1 ;;
        --no-layout) layout=0 ;;
        --app-only) app_only=1 ;;
        --sign) sign_id="$2"; shift ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

rm -rf "$OUT"
mkdir -p "$OUT"

# ---- binary
if [ -n "${BIN_PATH:-}" ]; then
    cp "$BIN_PATH" "$OUT/$BIN"
    arch=$(uname -m)
elif [ "$universal" = 1 ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
        # shellcheck disable=SC2086
        MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target "$t" $FEATURES
    done
    lipo -create -output "$OUT/$BIN" \
        "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN"
    arch=universal
else
    # shellcheck disable=SC2086
    MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release $FEATURES
    cp "target/release/$BIN" "$OUT/$BIN"
    arch=$(uname -m)
fi

# ---- .app bundle
APP="$OUT/$APP_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
mv "$OUT/$BIN" "$APP/Contents/MacOS/$BIN"
cp assets/macos/MatrixFinder.icns "$APP/Contents/Resources/MatrixFinder.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key><string>$BIN</string>
    <key>CFBundleIconFile</key><string>MatrixFinder</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
PLIST
printf 'APPL????' > "$APP/Contents/PkgInfo"

if command -v codesign >/dev/null; then
    if [ "$sign_id" = "-" ]; then
        # Ad-hoc signature: required for Apple silicon; Gatekeeper still asks on first open.
        codesign --force --deep --sign - "$APP"
    else
        codesign --force --deep --options runtime --timestamp --sign "$sign_id" "$APP"
    fi
fi
echo "App: $APP"
[ "$app_only" = 1 ] && exit 0

# ---- DMG: the app, a link to /Applications and a background with an arrow
DMG="$OUT/MatrixFinder-$VERSION-macos-$arch.dmg"
STAGE="$OUT/dmg"
mkdir -p "$STAGE/.background"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
if command -v tiffutil >/dev/null; then
    BG=background.tiff
    tiffutil -cathidpicheck assets/macos/dmg-background.png assets/macos/dmg-background@2x.png -out "$STAGE/.background/$BG"
else
    BG=background.png
    cp assets/macos/dmg-background.png "$STAGE/.background/$BG"
fi
cp assets/macos/MatrixFinder.icns "$STAGE/.VolumeIcon.icns"

RW="$OUT/rw.dmg"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -fs HFS+ -format UDRW -ov "$RW" >/dev/null

if [ "$layout" = 1 ]; then
    attach=$(hdiutil attach -readwrite -noverify -noautoopen "$RW")
    dev=$(echo "$attach" | grep -E '^/dev/' | head -1 | awk '{print $1}')
    mnt=$(echo "$attach" | grep -o '/Volumes/.*' | tail -1)
    disk=$(basename "$mnt")
    command -v SetFile >/dev/null && SetFile -a C "$mnt" || true
    # Finder styling; give up after 60 s instead of hanging (e.g. no GUI session).
    osascript <<OSA &
tell application "Finder"
    tell disk "$disk"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {200, 120, 860, 560}
        set opts to the icon view options of container window
        set arrangement of opts to not arranged
        set icon size of opts to 112
        set text size of opts to 13
        set background picture of opts to file ".background:$BG"
        set position of item "$APP_NAME.app" of container window to {170, 200}
        set position of item "Applications" of container window to {490, 200}
        close
        open
        update without registering applications
        delay 1
        close
    end tell
end tell
OSA
    osa=$!
    ( sleep 60; kill "$osa" 2>/dev/null ) &
    watchdog=$!
    wait "$osa" || echo "warning: Finder layout skipped (the DMG still works)" >&2
    kill "$watchdog" 2>/dev/null || true
    sync
    hdiutil detach "$dev" >/dev/null || hdiutil detach -force "$dev" >/dev/null
fi

rm -f "$DMG"
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
rm -f "$RW"
rm -rf "$STAGE"
if [ "$sign_id" != "-" ] && command -v codesign >/dev/null; then
    codesign --force --sign "$sign_id" "$DMG"
fi
echo "DMG: $DMG"
