#!/bin/sh
# (c) idle with an open PR, GPUI (measure-switch.sh's PR click missed once on GPUI; this one checks it with a screenshot): launch with a trace dir, click Pull Requests then PR #22, verify, 3 x 60 s.
set -e
F=/private/tmp/flows-z-perf; cd $F
T=$F/trace-gpui-pr; rm -rf $T; mkdir -p $T
sh run-gpui.sh $F/flows.json $T >/dev/null; P=$(cat gpui.pid)
until grep -q finished gpui.log; do sleep 0.5; done; sleep 6
set -- $(./cap list $P); WX=$2; WY=$3
lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.5; cliclick c:$((WX+1003)),$((WY+21)); sleep 3; cliclick c:$((WX+459)),$((WY+363)); sleep 3; ./cap shot $P $F/pr-gpui.png"
cpu() { ps -o time= -p $P | awk -F: '{ print $1 * 60 + $2 }'; }; gits() { ls $T | wc -l | tr -d ' '; }
sleep 10
for k in 1 2 3; do g0=$(gits); c0=$(cpu); sleep 60; g1=$(gits); c1=$(cpu)
  echo "gpui idle-pr run$k cpu=$(echo "$c1 - $c0" | bc)s pct=$(echo "scale=2; ($c1 - $c0) * 100 / 60" | bc)% git=$((g1 - g0))"; done
echo "gpui rss_mb=$(( $(ps -o rss= -p $P) / 1024 ))"
kill $P
