#!/bin/sh
# Stops what up.sh started: scripts/flows/down.sh <out>. The fixture stays; up.sh --reset rebuilds it.
OUT="${1:?usage: down.sh <out>}"
for f in forge; do
  if [ -f "$OUT/$f.pid" ]; then kill "$(cat "$OUT/$f.pid")" 2>/dev/null && echo "stopped $f ($(cat "$OUT/$f.pid"))"; rm -f "$OUT/$f.pid"; fi
done
