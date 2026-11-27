#!/usr/bin/env python3
"""(d') git processes per worktree-switching round: wt.py gpui|swift. The StressDriver "worktrees" step (selects every
worktree of every repo, 0.25 s apart, then opens and closes the ⌘K switcher), 3 iterations. Round 1 visits each
worktree for the first time; rounds 2 and 3 revisit them. Counts GIT_TRACE2_EVENT files (one per git process)."""
import os, subprocess, sys, time, shutil, json, collections
F = "/private/tmp/flows-z-perf"; app = sys.argv[1]; T = f"{F}/trace-{app}-wt"
shutil.rmtree(T, ignore_errors=True); os.makedirs(T)
open(f"{F}/{app}.log", "w").close()
subprocess.run(["sh", f"{F}/run-{app}.sh", f"{F}/wt.json", T], check=True, stdout=subprocess.DEVNULL)
pid = int(open(f"{F}/{app}.pid").read())
seen, marks = 0, {}
while True:
    lines = open(f"{F}/{app}.log").read().splitlines()
    for l in lines[seen:]:
        if l.startswith("LOUPE_STRESS"):
            marks[l.split()[1] + " " + (l.split()[2] if "step" in l else "")] = (time.time(), set(os.listdir(T)))
    seen = len(lines)
    if any(k.startswith("finished") for k in marks): break
    time.sleep(0.05)
def argv(f):
    for l in open(f"{T}/{f}"):
        e = json.loads(l)
        if e.get("event") == "start": return " ".join(e["argv"][1:3])
    return "?"
for i in (1, 2, 3):
    b, e = marks[f"step-begin iter={i}"][1], marks[f"step-end iter={i}"][1]
    new = e - b
    print(f"{app} worktrees round{i} git={len(new)}", dict(collections.Counter(argv(f) for f in new).most_common(6)))
os.kill(pid, 15)
