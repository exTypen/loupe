#!/bin/sh
# A real change every 3 s: an untracked file in a folder git doesn't ignore, then removed again.
B="${2:?usage: realchange.sh <seconds> <memtest-dir>}"/data/big; end=$(( $(date +%s) + $1 ))
while [ $(date +%s) -lt $end ]; do echo x > "$B/loupe-watch-test.txt"; sleep 1.5; rm -f "$B/loupe-watch-test.txt"; sleep 1.5; done
