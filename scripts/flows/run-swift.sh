#!/bin/sh
# Builds the Swift Loupe at ~/loupe's HEAD as LoupeFlows.app and runs it on the fixture:
#   scripts/flows/run-swift.sh <out> [--rebuild]
# Built the way scripts/memtest/mkcopy.sh does (scripts/bundle.sh in the Swift repo, ad-hoc signed, so it writes that
# repo's build/ and .build/), then copied to <out>/LoupeFlows.app with its own bundle id (dev.extypen.loupe.flows: its
# own defaults, which StressDriver wipes), no Sparkle feed and plain-HTTP loads allowed. Rebuilt when HEAD moved.
# LOUPE_STRESS=<out>/scenario.json makes StressDriver open the fixture's repos and hold the mock account and token in
# memory (no Keychain). The scenario has no steps: the driver selects the main repo once and then leaves the app alone.
set -e
OUT="${1:?usage: run-swift.sh <out> [--rebuild]}"; OUT=$(cd "$OUT" && pwd -P)
SRC=${LOUPE_SWIFT_REPO:-/Users/extypen/loupe}
G=/opt/homebrew/bin/git
[ -f "$OUT/scenario.json" ] || { echo "no fixture in $OUT: run up.sh first" >&2; exit 1; }
REV=$($G -C "$SRC" rev-parse HEAD)
[ -z "$($G -C "$SRC" status --porcelain --untracked-files=no)" ] || echo "warning: $SRC has uncommitted changes; the build includes them" >&2
APP="$OUT/LoupeFlows.app"
if [ "$2" = --rebuild ] || [ "$(cat "$APP/Contents/Resources/flows-rev" 2>/dev/null)" != "$REV" ]; then
  echo "building $SRC at $(echo "$REV" | cut -c1-7)"
  (cd "$SRC" && LOUPE_SIGN_ID=- scripts/bundle.sh)
  rm -rf "$APP"
  ditto "$SRC/build/Loupe.app" "$APP"
  mv "$APP/Contents/MacOS/Loupe" "$APP/Contents/MacOS/LoupeFlows"
  P="$APP/Contents/Info.plist"
  pb() { /usr/libexec/PlistBuddy -c "$1" "$P"; }
  pb "Set :CFBundleIdentifier dev.extypen.loupe.flows"
  pb "Set :CFBundleExecutable LoupeFlows"
  pb "Set :CFBundleName LoupeFlows"
  pb "Set :CFBundleDisplayName LoupeFlows"
  pb "Delete :SUFeedURL" 2>/dev/null || true
  pb "Delete :SUPublicEDKey" 2>/dev/null || true
  pb "Set :SUEnableAutomaticChecks false"
  pb "Add :NSAppTransportSecurity dict"
  pb "Add :NSAppTransportSecurity:NSAllowsArbitraryLoads bool true"
  echo "$REV" >"$APP/Contents/Resources/flows-rev"
  codesign --force --deep -s - "$APP"
  codesign --verify --deep --strict "$APP"
fi
URL=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["forge"])' "$OUT/scenario.json")
curl -sf -m 2 "$URL/_mock" >/dev/null || echo "warning: the mock forge isn't answering on $URL (run up.sh)" >&2
[ -f "$OUT/swift.pid" ] && kill "$(cat "$OUT/swift.pid")" 2>/dev/null && sleep 1
HOME="$OUT/home" GIT_CONFIG_NOSYSTEM=1 LOUPE_STRESS="$OUT/scenario.json" "$APP/Contents/MacOS/LoupeFlows" >"$OUT/swift.log" 2>&1 &
echo $! >"$OUT/swift.pid"
echo "LoupeFlows running (pid $(cat "$OUT/swift.pid"), log $OUT/swift.log)"
