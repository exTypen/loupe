#!/bin/sh
# Runs each step group of the stress scenario on its own (no MallocStackLogging, so slopes are the app's own growth),
# one after another, and prints each run's summary: which steps make the footprint grow.
#   scripts/memtest/bisect.sh <memtest-dir> [iterations=10] [groups...]
# Extra measure.py flags in $BISECT_ARGS (e.g. --set minifiedFile='"large.txt"'). Default groups: worktrees,tabs  history,rebase  bigfile  prs  hugepr  checks  merge  create,stashes,submodule
set -e
M="${1:?usage: bisect.sh <memtest-dir> [iterations] [groups...]}"; N="${2:-10}"
shift; [ $# -gt 0 ] && shift
[ $# -eq 0 ] && set -- worktrees,tabs history,rebase bigfile prs hugepr checks merge create,stashes,submodule
HERE="$(dirname "$0")"
for g in "$@"; do
  python3 "$HERE/measure.py" "$M" --iterations "$N" --steps "$g" --no-msl --leaks-at "" --max-footprint 12000 $BISECT_ARGS --tag "bisect-$(echo "$g" | tr , +)" | sed -n '/^run /,/^top heap/p'
done
