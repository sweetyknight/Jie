#!/bin/sh
# Native AppKit regression checks in an isolated test app; nothing here ships in Jie.app.
set -eu
cd "$(dirname "$0")/../.."
cargo build --example mac_smoke
JIE_SMOKE_APP="$PWD/target/test-tmp/Jie Smoke.app"
rm -rf "$JIE_SMOKE_APP"
mkdir -p "$JIE_SMOKE_APP/Contents/MacOS"
cp target/debug/examples/mac_smoke "$JIE_SMOKE_APP/Contents/MacOS/mac_smoke"
cp assets/Info.plist "$JIE_SMOKE_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleExecutable mac_smoke' "$JIE_SMOKE_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier com.jie.smoke' "$JIE_SMOKE_APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleName Jie Smoke' "$JIE_SMOKE_APP/Contents/Info.plist"
/usr/bin/codesign --force --sign - "$JIE_SMOKE_APP"
"$JIE_SMOKE_APP/Contents/MacOS/mac_smoke" "$@"
