#!/bin/sh
set -eu
JIE_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$JIE_ROOT"
JIE_IDENTITY=${JIE_SIGN_IDENTITY:-Jie Local Development}
if [ "$JIE_IDENTITY" != '-' ] && ! security find-certificate -c "$JIE_IDENTITY" >/dev/null 2>&1; then
    printf '%s\n' 'Run sh scripts/macos/setup-signing.sh once, then package again.' >&2
    exit 1
fi
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release --target aarch64-apple-darwin
JIE_VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
mkdir -p dist
JIE_TMP=$(mktemp -d "$JIE_ROOT/dist/.mac-XXXXXX")
trap 'rm -rf "$JIE_TMP"' EXIT HUP INT TERM
JIE_APP="$JIE_TMP/Jie.app"
mkdir -p "$JIE_APP/Contents/MacOS" "$JIE_APP/Contents/Resources"
cp target/aarch64-apple-darwin/release/jie "$JIE_APP/Contents/MacOS/jie"
cp assets/Info.plist "$JIE_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $JIE_VERSION" "$JIE_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $JIE_VERSION" "$JIE_APP/Contents/Info.plist"
/usr/bin/swift scripts/macos/icon.swift "$JIE_TMP/Jie.iconset"
/usr/bin/iconutil -c icns "$JIE_TMP/Jie.iconset" -o "$JIE_APP/Contents/Resources/Jie.icns"
/usr/bin/codesign --force --sign "$JIE_IDENTITY" --identifier com.jie.app "$JIE_APP"
/usr/bin/codesign --verify --strict "$JIE_APP"
/usr/bin/plutil -lint "$JIE_APP/Contents/Info.plist"
rm -rf dist/Jie.app
mv "$JIE_APP" dist/Jie.app
/usr/bin/ditto -c -k --sequesterRsrc --keepParent dist/Jie.app "dist/Jie-$JIE_VERSION-mac-arm64.zip"
printf '%s\n' "$JIE_ROOT/dist/Jie.app"
