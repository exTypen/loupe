#!/bin/sh
# (a) open the 150k-line diff, (b) scroll it, (e) RSS after open — one fresh launch per run.
#   measure-open.sh gpui|swift <runs>
# Each run: launch on big.json (README.md is selected at launch), wait for "finished" + 4 s, then under the GUI lock:
# frontmost, click huge.txt and record the diff area (cap click), RSS/footprint 3 s later, then 3 s of pixel scrolling
# (15 px every 1/240 s, so every 120 Hz refresh has new input) over the diff (cap scroll). Coordinates are window points of each app's layout at 1800 pt wide.
set -e
F=/private/tmp/flows-z-perf; cd $F
APP=$1; RUNS=${2:-3}
if [ $APP = gpui ]; then ROW="400 86"; CROP="640 130 1080 900"; SCROLLAT="1100 600"
else ROW="345 103"; CROP="570 130 1080 900"; SCROLLAT="1100 600"; fi
for r in $(seq $RUNS); do
  sh run-$APP.sh $F/big.json >/dev/null
  P=$(cat $APP.pid)
  i=0; until grep -q finished $APP.log 2>/dev/null; do i=$((i+1)); [ $i -gt 120 ] && { echo "no finished"; exit 1; }; sleep 0.5; done
  sleep 4
  lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.4; ./cap click $P $ROW 3 $CROP" | sed "s/^/$APP run$r /"
  echo "$APP run$r rss_mb=$(( $(ps -o rss= -p $P) / 1024 )) $(footprint -p $P --noCategories 2>/dev/null | grep -o 'phys_footprint: [0-9.]* [KMG]B' | head -1)"
  lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.4; EVERY_US=4166 ./cap scroll $P $SCROLLAT 3 15 $CROP" | sed "s/^/$APP run$r /"
  kill $P; sleep 1
done
