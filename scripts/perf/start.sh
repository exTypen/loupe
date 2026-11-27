#!/bin/sh
# (b′) scroll start latency, 3 fresh launches: start.sh gpui|swift. Opens huge.txt, then scrolls with SETTLE_MS=300; the
# first scrolled frame is at first-frame + max, latency = that - 0.300 s. Run 1 also saves vmmap --summary (e).
set -e
F=/private/tmp/flows-z-perf; cd $F
APP=$1
if [ $APP = gpui ]; then ROW="400 86"; CROP="640 130 1080 900"; else ROW="345 103"; CROP="570 130 1080 900"; fi
for r in 1 2 3; do
  sh run-$APP.sh $F/big.json >/dev/null; P=$(cat $APP.pid)
  i=0; until grep -q finished $APP.log 2>/dev/null; do i=$((i+1)); [ $i -gt 120 ] && exit 1; sleep 0.5; done; sleep 4
  lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.4; ./cap click $P $ROW 3 $CROP" | sed "s/^/$APP start$r /"
  [ $r = 1 ] && vmmap --summary $P > vmmap-$APP.txt 2>&1 || true
  lockf -k -t 900 /tmp/loupe-gui.lock sh -c "sh front.sh $P; sleep 0.4; SETTLE_MS=300 EVERY_US=4166 ./cap scroll $P 1100 600 1 15 $CROP" | sed "s/^/$APP start$r /"
  kill $P; sleep 1
done
