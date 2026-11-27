#!/bin/sh
# The only way this agent launches the Swift copy: run-swift.sh <scenario.json under /private/tmp/flows-z-perf> [trace-dir]
# The prebuilt LoupeFlows.app (never rebuilt), re-signed under its own bundle id dev.extypen.loupe.flows.flows-z-perf
# (run-swift.sh in the GPUI repo does the same); LOUPE_STRESS and HOME are set and checked.
set -e
F=/private/tmp/flows-z-perf
SC="${1:?usage: run-swift.sh <scenario.json> [trace-dir]}"
case "$SC" in "$F"/*.json) ;; *) echo "refused: scenario must be $F/*.json" >&2; exit 1;; esac
[ -f "$SC" ] || { echo "no $SC" >&2; exit 1; }
APP="$F/LoupeFlows.app"
[ -d "$APP" ] || ditto /Users/extypen/loupe-flows-cache/LoupeFlows.app "$APP"
ID=dev.extypen.loupe.flows.flows-z-perf
P="$APP/Contents/Info.plist"
if [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$P")" != "$ID" ]; then
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $ID" "$P"
  codesign --force --deep -s - "$APP" 2>/dev/null
fi
export LOUPE_STRESS="$SC" HOME="$F/home" GIT_CONFIG_NOSYSTEM=1
[ -n "$2" ] && export GIT_TRACE2_EVENT="$2"
[ "$LOUPE_STRESS" = "$SC" ] && [ "$HOME" = "$F/home" ] || { echo "refused: env not set" >&2; exit 1; }
[ -f "$F/swift.pid" ] && kill "$(cat "$F/swift.pid")" 2>/dev/null && sleep 1
"$APP/Contents/MacOS/LoupeFlows" -AppleLanguages '(en)' >"$F/swift.log" 2>&1 &
echo $! >"$F/swift.pid"
echo "swift pid $(cat "$F/swift.pid")"
