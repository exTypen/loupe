#!/bin/sh
# Bursts of writes into a git-ignored folder of the test repo every 1.5 s (the pattern nix-direnv and builds made in
# the real repo: ~1 event batch a second) for $1 seconds.
B="${2:?usage: churn.sh <seconds> <memtest-dir>}"/data/big/churn; end=$(( $(date +%s) + $1 )); i=0
while [ $(date +%s) -lt $end ]; do for k in 1 2 3; do echo $i > "$B/f$k"; done; i=$((i+1)); sleep 1.5; done
