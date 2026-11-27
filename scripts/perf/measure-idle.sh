#!/bin/sh
# (c) idle CPU and git processes, 3 windows of 60 s each, on History and then on Changes: measure-idle.sh gpui|swift
# Launch on flows.json (app main selected) with GIT_TRACE2_EVENT=<dir> (one file per git process); tabs are switched by
# real clicks on the toolbar segments (window points) under the GUI lock, and a screenshot checks each state.
set -e
F=/private/tmp/flows-z-perf; cd $F
APP=$1
if [ $APP = gpui ]; then C="853 21"; H="921 21"; else C="922 26"; H="1030 26"; fi
T=$F/trace-$APP-idle; rm -rf $T; mkdir -p $T
sh run-$APP.sh $F/flows.json $T >/dev/null; P=$(cat $APP.pid)
i=0; until grep -q finished $APP.log 2>/dev/null; do i=$((i+1)); [ $i -gt 120 ] && { echo "no finished"; exit 1; }; sleep 0.5; done
sleep 6
set -- $(./cap list $P); WX=$2; WY=$3
at() { echo "cliclick c:$(( WX + ${1% *} )),$(( WY + ${1#* } )); sleep $2;"; }
cpu() { ps -o time= -p $P | awk -F: '{ print $1 * 60 + $2 }'; }
gits() { ls $T | wc -l | tr -d ' '; }
idle() {
  sleep 10
  for k in 1 2 3; do g0=$(gits); c0=$(cpu); sleep 60; g1=$(gits); c1=$(cpu)
    echo "$APP idle-$1 run$k cpu=$(echo "$c1 - $c0" | bc)s pct=$(echo "scale=2; ($c1 - $c0) * 100 / 60" | bc)% git=$((g1 - g0))"; done
}
lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.5; $(at "$H" 3) ./cap shot $P $F/idle-history-$APP.png"
idle history
lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.5; $(at "$C" 3) ./cap shot $P $F/idle-changes-$APP.png"
idle changes
kill $P
