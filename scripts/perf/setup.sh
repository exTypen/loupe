#!/bin/sh
# Prepares the perf runs in F (default /private/tmp/flows-z-perf; the scripts here hardcode it, change F in each if
# you use another folder): the flows fixture + mock forge on FLOWS_PORT, the "big" repo with a 150k-line diff, the
# scenarios (flows.json, big.json, wt.json), the prebuilt Swift copy and the cap probe.
set -e
F=${F:-/private/tmp/flows-z-perf}; HERE=$(cd "$(dirname "$0")" && pwd); G=/opt/homebrew/bin/git
FLOWS_PORT=${FLOWS_PORT:-8837} sh "$HERE/../flows/up.sh" "$F"
cp "$HERE"/*.sh "$HERE"/wt.py "$HERE"/cap.swift "$F/"
(cd "$F" && swiftc -O cap.swift -o cap 2>/dev/null)
[ -d "$F/LoupeFlows.app" ] || ditto /Users/extypen/loupe-flows-cache/LoupeFlows.app "$F/LoupeFlows.app"
if [ ! -d "$F/big" ]; then
  mkdir "$F/big" && cd "$F/big" && $G init -q -b main
  python3 -c '
open("large.txt","w").write("".join(f"line {k:06d}: the quick brown fox jumps over the lazy dog {k * 31 % 997}\n" for k in range(150_000)))
open("README.md","w").write("# big\n")'
  $G -c user.name=A -c user.email=a@x add -A && $G -c user.name=A -c user.email=a@x commit -qm base
  python3 -c '
# huge.txt: a new 150k-line file, intent-to-add (a 150k-line unstaged diff through git diff)
open("huge.txt","w").write("".join(f"fn item_{k:06d}() -> u64 {{ let x = {k}; x * {k % 97} + {k % 13} }} // generated\n" for k in range(150_000)))
# large.txt: the memtest shape, every 1000th line edited
l = open("large.txt").readlines()
for k in range(0, len(l), 1000): l[k] = l[k].rstrip("\n") + "  // edited\n"
open("large.txt","w").writelines(l)
open("README.md","w").write("# big\nedited\n")'
  $G add -N huge.txt
fi
python3 - "$F" <<'PY'
import json, sys
F = sys.argv[1]; s = json.load(open(f"{F}/scenario.json"))
s["repos"] = s["repos"] + [f"{F}/big"]
json.dump(s, open(f"{F}/flows.json", "w"), indent=1)
json.dump(dict(s, big=f"{F}/big", bigFileWorktree=f"{F}/big", bigFile="huge.txt", minifiedFile="large.txt", steps=[]), open(f"{F}/big.json", "w"), indent=1)
json.dump(dict(s, steps=["worktrees"], iterations=3), open(f"{F}/wt.json", "w"), indent=1)
PY
echo "ready: $F"
