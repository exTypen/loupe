#!/bin/sh
# Builds the memory-test copy of Loupe: scripts/memtest/mkcopy.sh <out-dir>  →  <out-dir>/LoupeMemTest.app
# Its own bundle id (dev.extypen.loupe.memtest, so its own UserDefaults domain) and process name (LoupeMemTest), no
# Sparkle feed, plain-HTTP loads allowed (the mock forge), and an ad-hoc signature without the hardened runtime so
# leaks/heap/vmmap can read it. Never installs anything: /Applications/Loupe.app is left alone.
set -e
OUT="${1:?usage: mkcopy.sh <out-dir>}"
cd "$(dirname "$0")/../.."
LOUPE_SIGN_ID=- scripts/bundle.sh   # ad-hoc: no Keychain access to a signing key, no prompt on the owner's screen
APP="$OUT/LoupeMemTest.app"
rm -rf "$APP" && mkdir -p "$OUT"
ditto build/Loupe.app "$APP"
mv "$APP/Contents/MacOS/Loupe" "$APP/Contents/MacOS/LoupeMemTest"
P="$APP/Contents/Info.plist"
pb() { /usr/libexec/PlistBuddy -c "$1" "$P"; }
pb "Set :CFBundleIdentifier dev.extypen.loupe.memtest"
pb "Set :CFBundleExecutable LoupeMemTest"
pb "Set :CFBundleName LoupeMemTest"
pb "Set :CFBundleDisplayName LoupeMemTest"
pb "Delete :SUFeedURL" 2>/dev/null || true
pb "Delete :SUPublicEDKey" 2>/dev/null || true
pb "Set :SUEnableAutomaticChecks false"
pb "Add :NSAppTransportSecurity dict"
pb "Add :NSAppTransportSecurity:NSAllowsArbitraryLoads bool true"
codesign --force --deep -s - "$APP"
codesign --verify --deep --strict "$APP"
echo "Built $APP"
