#!/bin/sh
# The only way this agent launches the GPUI app: run-gpui.sh <scenario.json under /private/tmp/flows-z-perf> [trace-dir]
# Sets and checks LOUPE_STRESS and HOME, so the app never sees the real settings domain or Keychain.
set -e
F=/private/tmp/flows-z-perf
SC="${1:?usage: run-gpui.sh <scenario.json> [trace-dir]}"
case "$SC" in "$F"/*.json) ;; *) echo "refused: scenario must be $F/*.json" >&2; exit 1;; esac
[ -f "$SC" ] || { echo "no $SC" >&2; exit 1; }
BIN=/Users/extypen/loupe-gpui-z-perf/target/release/loupe
export LOUPE_STRESS="$SC" HOME="$F/home" GIT_CONFIG_NOSYSTEM=1 LOUPE_FLOWS_AI="$F/home/.local/bin/claude"
[ -n "$2" ] && export GIT_TRACE2_EVENT="$2"
[ "$LOUPE_STRESS" = "$SC" ] && [ "$HOME" = "$F/home" ] || { echo "refused: env not set" >&2; exit 1; }
[ -f "$F/gpui.pid" ] && kill "$(cat "$F/gpui.pid")" 2>/dev/null && sleep 1
cd /Users/extypen/loupe-gpui-z-perf
"$BIN" >"$F/gpui.log" 2>"$F/gpui.err" &
echo $! >"$F/gpui.pid"
echo "gpui pid $(cat "$F/gpui.pid")"
