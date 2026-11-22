#!/bin/sh
# Starts the flows environment: scripts/flows/up.sh <out> [--reset] [mockforge-flows.py options...]
# Builds the fixture if <out>/scenario.json is missing (or with --reset), starts the mock forge in the background,
# waits for it, and prints what an app needs to load the same repos and account. FLOWS_PORT picks the port at
# generation (default 8790); after that the port comes from the fixture.
set -e
DIR=$(cd "$(dirname "$0")" && pwd)
OUT="${1:?usage: up.sh <out> [--reset] [mockforge options]}"; shift
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd -P)
if [ "$1" = --reset ]; then shift; "$DIR/down.sh" "$OUT" >/dev/null 2>&1 || true; rm -f "$OUT/scenario.json"; fi
[ -f "$OUT/scenario.json" ] || python3 "$DIR/mkflows.py" "$OUT" --port "${FLOWS_PORT:-8790}"
URL=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["forge"])' "$OUT/scenario.json")
PORT=${URL##*:}
if curl -sf -m 2 "$URL/_mock" >/dev/null 2>&1; then
  echo "mock forge already running on $URL"
elif curl -s -m 2 -o /dev/null "$URL/" 2>/dev/null; then
  echo "something else answers on $URL: stop it or regenerate with another FLOWS_PORT" >&2; exit 1
else
  python3 "$DIR/mockforge-flows.py" "$OUT" --port "$PORT" "$@" >"$OUT/forge.log" 2>&1 &
  echo $! >"$OUT/forge.pid"
  i=0; until curl -sf -m 1 "$URL/_mock" >/dev/null 2>&1; do i=$((i + 1)); [ $i -gt 50 ] && { cat "$OUT/forge.log" >&2; exit 1; }; sleep 0.2; done
  echo "mock forge on $URL (pid $(cat "$OUT/forge.pid"), log $OUT/forge.log)"
fi
cat <<MSG

scenario: $OUT/scenario.json
Environment for an app (the Swift copy via run-swift.sh, or the GPUI app):
  export LOUPE_STRESS=$OUT/scenario.json
  export HOME=$OUT/home
  export GIT_CONFIG_NOSYSTEM=1
Account: $URL, user alice, token flows-token (from scenario.json).
MSG
