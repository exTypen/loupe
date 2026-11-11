#!/usr/bin/env python3
"""Groups a `leaks` report (run with MallocStackLogging) by leaked type and allocation stack, and resolves the Loupe
frames to source lines with atos (the .o files under .build/ carry the debug info).

    scripts/memtest/leaksummary.py <leaks-report.txt> [--binary <LoupeMemTest>] [--top 25]
"""
import argparse, collections, re, subprocess

ap = argparse.ArgumentParser()
ap.add_argument("report")
ap.add_argument("--binary", default=None, help="the LoupeMemTest executable the report came from (for atos)")
ap.add_argument("--top", type=int, default=25)
a = ap.parse_args()
text = open(a.report, errors="replace").read()
load = re.search(r"Load Address:\s+(0x[0-9a-f]+)", text)
total = re.search(r"(\d+) leaks? for (\d+) total leaked bytes", text)
print(total.group(0) if total else "no leak count in report")

# STACK OF 3 INSTANCES OF 'ROOT LEAK: <Foo>':  then frames, then ====
blocks = re.findall(r"STACK OF (\d+) INSTANCES? OF '([^']+)':\n(.*?)\n====", text, re.S)
groups = collections.Counter()
stacks = {}
for n, kind, frames in blocks:
    fr = [l.strip() for l in frames.split("\n") if l.strip()][::-1]   # allocation site first
    fr = [f for f in fr if "libsystem_malloc" not in f and "libobjc" not in f] or fr
    ours = [f for f in fr if " LoupeMemTest " in f or "Loupe" in f.split()[1:2]]
    key = (re.sub(r"0x[0-9a-f]+", "", kind), ours[0] if ours else (fr[0] if fr else "?"))
    groups[key] += int(n)
    stacks.setdefault(key, (fr, ours))


def resolve(frame):
    m = re.search(r"(0x[0-9a-f]+)\s", frame + " ")
    if not (a.binary and load and m):
        return ""
    r = subprocess.run(["atos", "-o", a.binary, "-l", load.group(1), m.group(1)], capture_output=True, text=True)
    loc = re.search(r"\(([^()]+\.swift:\d+)\)", r.stdout)
    return loc.group(1) if loc else ""


for (kind, top), n in groups.most_common(a.top):
    fr, ours = stacks[(kind, top)]
    print(f"\n{n:6d} x {kind}")
    for f in (ours[:4] if ours else fr[:4]):
        sym = re.sub(r"^\d+\s+\S+\s+0x[0-9a-f]+\s+", "", f)
        print(f"         {sym[:150]}  {resolve(f)}")
