#!/bin/sh
# (d) git processes and CPU during 10 tab switches (3 rounds), then (c) idle with an open PR (#22), 3 x 60 s.
#   measure-switch.sh gpui|swift
# Tabs are switched by real clicks on the toolbar's Changes / History / Pull Requests segments (window points),
# under the GUI lock. GIT_TRACE2_EVENT=<dir>: one file per git process.
set -e
F=/private/tmp/flows-z-perf; cd $F
APP=$1
if [ $APP = gpui ]; then C="853 21"; H="921 21"; R="1003 21"; PR="459 363"
else C="922 26"; H="1030 26"; R="1136 26"; PR="390 343"; fi
T=$F/trace-$APP-sw; rm -rf $T; mkdir -p $T
sh run-$APP.sh $F/flows.json $T >/dev/null
P=$(cat $APP.pid)
i=0; until grep -q finished $APP.log 2>/dev/null; do i=$((i+1)); [ $i -gt 120 ] && { echo "no finished"; exit 1; }; sleep 0.5; done
sleep 8
set -- $(./cap list $P); WX=$2; WY=$3
at() { echo "cliclick c:$(( WX + ${1% *} )),$(( WY + ${1#* } )); sleep $2;"; }
cpu() { ps -o time= -p $P | awk -F: '{ print $1 * 60 + $2 }'; }
gits() { ls $T | wc -l | tr -d ' '; }
lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.5; $(at "$H" 2) $(at "$R" 2) $(at "$C" 1)"
sleep 8
for k in 1 2 3; do
  g0=$(gits); c0=$(cpu)
  lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.5; $(at "$H" 1) $(at "$R" 1) $(at "$C" 1) $(at "$H" 1) $(at "$R" 1) $(at "$C" 1) $(at "$H" 1) $(at "$R" 1) $(at "$C" 1) $(at "$H" 1) ./cap shot $P $F/switch-$APP-$k.png; $(at "$C" 0.5)"
  g1=$(gits); c1=$(cpu)
  echo "$APP switches10 round$k git=$((g1 - g0)) cpu=$(echo "$c1 - $c0" | bc)s"
  [ $((g1 - g0)) -gt 0 ] && ls -t $T | head -$((g1 - g0)) | while read f; do grep -o '"argv":\[[^]]*\]' "$T/$f" | head -1; done | sed "s/^/  /"
  sleep 5
done
lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.5; $(at "$R" 3) $(at "$PR" 3) ./cap shot $P $F/pr-$APP.png"
sleep 10
for k in 1 2 3; do g0=$(gits); c0=$(cpu); sleep 60; g1=$(gits); c1=$(cpu)
  echo "$APP idle-pr run$k cpu=$(echo "$c1 - $c0" | bc)s pct=$(echo "scale=2; ($c1 - $c0) * 100 / 60" | bc)% git=$((g1 - g0))"; done
echo "$APP rss_mb=$(( $(ps -o rss= -p $P) / 1024 ))"
kill $P
