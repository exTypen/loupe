#!/usr/bin/env python3
"""Runs Loupe's memory stress scenario on the test copy and measures it.

    scripts/memtest/measure.py <memtest-dir> [--iterations 30] [--steps a,b,...] [--leaks-at 1,10] [--tag name]
                               [--pause 0.6] [--no-msl] [--port 8765]

<memtest-dir> is where mkrepos.py and mkcopy.sh wrote (scenario.json, home/, LoupeMemTest.app). Starts the mock forge
if nothing answers on the port. Launches LoupeMemTest with MallocStackLogging=1 and LOUPE_STRESS, HOME and
CFFIXED_USER_HOME pointing into <memtest-dir> (its caches, saved state and git config stay there), then:
  - samples phys_footprint (footprint) and RSS (ps) every 2 s, and at "ready" and after every iteration;
  - runs `leaks` at ready, at --leaks-at iterations and at the end (with allocation stacks), plus a memgraph at ready
    and at the end; `heap -s` and `vmmap --summary` at ready and at the end;
  - writes <memtest-dir>/runs/<tag>/{iterations.csv,samples.csv,summary.txt,heap-growth.txt,leaks-*.txt,...}.
Only ever signals the LoupeMemTest process it started itself.
"""
import argparse, csv, json, os, re, signal, subprocess, sys, threading, time, urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("dir")
ap.add_argument("--iterations", type=int, default=30)
ap.add_argument("--steps", default=None)
ap.add_argument("--leaks-at", default="1,10")
ap.add_argument("--tag", default=None)
ap.add_argument("--pause", type=float, default=None)
ap.add_argument("--port", type=int, default=8765)
ap.add_argument("--no-msl", action="store_true", help="without MallocStackLogging (less overhead, no leak stacks)")
ap.add_argument("--max-footprint", type=float, default=16000, help="MB: stop early (with the end measurements) past this, so the machine never swaps")
ap.add_argument("--set", action="append", default=[], metavar="KEY=JSON", help="override a scenario key, e.g. minifiedFile='\"large.txt\"'")
ap.add_argument("--timeout", type=float, default=6 * 3600)
a = ap.parse_args()

M = os.path.abspath(a.dir)
APP = os.path.join(M, "LoupeMemTest.app/Contents/MacOS/LoupeMemTest")
tag = a.tag or time.strftime("%Y%m%d-%H%M%S")
OUT = os.path.join(M, "runs", tag)
os.makedirs(OUT, exist_ok=True)
HERE = os.path.dirname(os.path.abspath(__file__))
leaks_at = {int(x) for x in a.leaks_at.split(",") if x}


def say(msg):
    line = f"[{time.strftime('%H:%M:%S')}] {msg}"
    print(line, flush=True)
    with open(os.path.join(OUT, "measure.log"), "a") as f:
        f.write(line + "\n")


# --- mock forge ------------------------------------------------------------------------------------------------------
sc = json.load(open(os.path.join(M, "scenario.json")))
forge_proc = None
try:
    urllib.request.urlopen(f"http://127.0.0.1:{a.port}/api/v1/version", timeout=2).read()
except Exception:
    say("starting mock forge")
    forge_proc = subprocess.Popen([sys.executable, os.path.join(HERE, "mockforge.py"), sc["big"], "--port", str(a.port)],
                                  stdout=open(os.path.join(OUT, "forge.log"), "w"), stderr=subprocess.STDOUT)
    for _ in range(50):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{a.port}/api/v1/version", timeout=1).read(); break
        except Exception:
            time.sleep(0.2)

sc["iterations"] = a.iterations
if a.steps:
    sc["steps"] = a.steps.split(",")
if a.pause is not None:
    sc["pause"] = a.pause
for kv in a.set:
    k, v = kv.split("=", 1)
    sc[k] = json.loads(v)
scen = os.path.join(OUT, "scenario.json")
json.dump(sc, open(scen, "w"), indent=1)

# --- launch ----------------------------------------------------------------------------------------------------------
home = os.path.join(M, "home")
env = dict(os.environ, LOUPE_STRESS=scen, HOME=home, CFFIXED_USER_HOME=home)
if not a.no_msl:
    env["MallocStackLogging"] = "1"
say(f"launching {APP} (steps {','.join(sc['steps'])}, {a.iterations} iterations, MSL {'off' if a.no_msl else 'on'})")
app = subprocess.Popen([APP, "-AppleLanguages", "(en)", "-AppleLocale", "en_US"], env=env,
                       stdout=subprocess.PIPE, stderr=open(os.path.join(OUT, "app.stderr"), "w"), text=True, bufsize=1)
pid = app.pid
say(f"pid {pid}")


def footprint():
    r = subprocess.run(["footprint", "-p", str(pid), "--noCategories", "-f", "bytes"], capture_output=True, text=True)
    fp = re.search(r"phys_footprint: (\d+)", r.stdout)
    pk = re.search(r"phys_footprint_peak: (\d+)", r.stdout)
    rss = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    mb = lambda x: round(int(x) / 1048576, 1) if x else None
    return mb(fp.group(1) if fp else None), mb(pk.group(1) if pk else None), round(int(rss) / 1024, 1) if rss else None


state = {"step": "launch", "iter": 0, "done": False}
samples = open(os.path.join(OUT, "samples.csv"), "w", newline="")
sw = csv.writer(samples)
sw.writerow(["t", "iter", "step", "footprint_mb", "peak_mb", "rss_mb"])
t0 = time.time()
paused = threading.Event()   # set while leaks/heap hold the process


def sampler():
    while not state["done"] and app.poll() is None:
        if not paused.is_set():
            fp, pk, rss = footprint()
            if fp is not None:
                sw.writerow([round(time.time() - t0, 1), state["iter"], state["step"], fp, pk, rss]); samples.flush()
        time.sleep(2)


threading.Thread(target=sampler, daemon=True).start()


def tool(name, args_, outfile, timeout=900):
    paused.set()
    t = time.time()
    try:
        r = subprocess.run(args_, capture_output=True, text=True, timeout=timeout)
        open(os.path.join(OUT, outfile), "w").write(r.stdout + ("\n--- stderr ---\n" + r.stderr if r.stderr.strip() else ""))
        return r.stdout
    except subprocess.TimeoutExpired:
        say(f"{name} timed out"); return ""
    finally:
        paused.clear()
        say(f"  {name} → {outfile} ({time.time() - t:.0f} s)")


def leaks(label, graph=False):
    if graph:   # the graph alone holds the process once; the report is then read from the file, offline
        g = os.path.join(OUT, label + ".memgraph")
        tool("memgraph", ["leaks", str(pid), f"--outputGraph={g}"], f"memgraph-{label}.txt")
        out = tool("leaks (offline)", ["leaks", g], f"leaks-{label}.txt")
    else:
        out = tool("leaks", ["leaks", str(pid)], f"leaks-{label}.txt")
    m = re.search(r"(\d+) leaks? for (\d+) total leaked bytes", out)
    return (int(m.group(1)), int(m.group(2))) if m else (None, None)


rows = []
leak_rows = []
iter_file = open(os.path.join(OUT, "iterations.csv"), "w", newline="")
iw = csv.writer(iter_file)
iw.writerow(["iter", "t", "footprint_mb", "peak_mb", "rss_mb", "iter_secs"])
last_iter_t = None
deadline = time.time() + a.timeout
crashed = False
for line in app.stdout:
    line = line.rstrip("\n")
    if not line.startswith("LOUPE_STRESS"):
        continue
    with open(os.path.join(OUT, "app.stdout"), "a") as f:
        f.write(line + "\n")
    ev = line.split()[1]
    if ev == "step-begin":
        state["step"] = re.search(r"step=(\S+)", line).group(1)
    elif ev in ("timeout", "missing", "no-view-switch", "no-check-row", "unknown-step"):
        say(f"  app: {line}")
    elif ev == "ready":
        state["step"] = "ready"
        time.sleep(3)
        fp, pk, rss = footprint()
        rows.append((0, round(time.time() - t0, 1), fp, pk, rss, None)); iw.writerow(rows[-1]); iter_file.flush()
        say(f"ready: footprint {fp} MB, rss {rss} MB")
        n, b = leaks("start", graph=True); leak_rows.append(("start", n, b))
        tool("heap", ["heap", "-s", str(pid)], "heap-start.txt")
        tool("vmmap", ["vmmap", "--summary", str(pid)], "vmmap-start.txt")
        last_iter_t = time.time()
    elif ev == "iter":
        i = int(re.search(r"iter=(\d+)", line).group(1))
        state["iter"] = i; state["step"] = "rest"
        fp, pk, rss = footprint()
        secs = round(time.time() - last_iter_t, 1) if last_iter_t else None
        last_iter_t = time.time()
        rows.append((i, round(time.time() - t0, 1), fp, pk, rss, secs)); iw.writerow(rows[-1]); iter_file.flush()
        say(f"iter {i}: footprint {fp} MB (peak {pk}), rss {rss} MB, {secs} s")
        if i in leaks_at and i != a.iterations:
            n, b = leaks(f"iter{i}"); leak_rows.append((f"iter {i}", n, b))
            last_iter_t = time.time()
        if fp and fp > a.max_footprint:
            say(f"footprint {fp} MB is past --max-footprint {a.max_footprint:.0f}: stopping after iteration {i}")
            ev = "finished"
    if ev == "finished":
        say("finished: final measurements")
        n, b = leaks("end", graph=True); leak_rows.append(("end", n, b))
        tool("heap", ["heap", "-s", str(pid)], "heap-end.txt")
        tool("heap-diff", ["heap", "-s", f"--diffFrom={os.path.join(OUT, 'start.memgraph')}", os.path.join(OUT, "end.memgraph")], "heap-diff.txt")
        tool("vmmap", ["vmmap", "--summary", str(pid)], "vmmap-end.txt")
        break
    if time.time() > deadline:
        say("timeout"); break
state["done"] = True
if app.poll() is not None:
    crashed = True
    say(f"app exited by itself with {app.returncode}: see ~/Library/Logs/DiagnosticReports/LoupeMemTest-*.ips")
else:
    app.send_signal(signal.SIGTERM)
    try:
        app.wait(10)
    except subprocess.TimeoutExpired:
        app.kill()
if forge_proc:
    forge_proc.terminate()


# --- summary ---------------------------------------------------------------------------------------------------------
def parse_heap(path):
    out = {}
    try:
        for l in open(path):
            m = re.match(r"\s*(\d+)\s+(\d+)\s+([\d.]+)\s+(.+?)\s{2,}(\S.*)?$", l)
            if m:
                key = m.group(4).strip() + ("  [" + m.group(5).strip() + "]" if m.group(5) else "")
                c, b = out.get(key, (0, 0))
                out[key] = (c + int(m.group(1)), b + int(m.group(2)))
    except FileNotFoundError:
        pass
    return out


h0, h1 = parse_heap(os.path.join(OUT, "heap-start.txt")), parse_heap(os.path.join(OUT, "heap-end.txt"))
if not h1:
    say("no heap-end.txt (the run didn't finish): no heap growth table")
growth = sorted(((k, h1.get(k, (0, 0))[0] - h0.get(k, (0, 0))[0], h1.get(k, (0, 0))[1] - h0.get(k, (0, 0))[1]) for k in set(h0) | set(h1)),
                key=lambda x: -x[2])
with open(os.path.join(OUT, "heap-growth.txt"), "w") as f:
    f.write(f"{'Δcount':>10} {'Δbytes':>14}  class\n")
    for k, dc, db in growth[:80]:
        f.write(f"{dc:>10} {db:>14}  {k}\n")


def slope(pts):
    if len(pts) < 2:
        return None
    n = len(pts); mx = sum(x for x, _ in pts) / n; my = sum(y for _, y in pts) / n
    den = sum((x - mx) ** 2 for x, _ in pts)
    return sum((x - mx) * (y - my) for x, y in pts) / den if den else None


it = [r for r in rows if r[0] > 0 and r[2] is not None]
skip = max(1, len(it) // 5)
s_all = slope([(r[0], r[2]) for r in it])
s_steady = slope([(r[0], r[2]) for r in it[skip:]])
peak = max([r[3] for r in rows if r[3]] or [0])
lines = [f"run {tag}: steps {','.join(sc['steps'])}, {len(it)}/{a.iterations} iterations, MSL {'off' if a.no_msl else 'on'}" + (" — APP EXITED" if crashed else ""),
         f"launch (ready)      : {rows[0][2] if rows else '?'} MB footprint, {rows[0][4] if rows else '?'} MB rss",
         f"after iteration 1   : {it[0][2] if it else '?'} MB",
         f"after iteration {it[-1][0] if it else '?':<4}: {it[-1][2] if it else '?'} MB",
         f"peak                : {peak} MB (phys_footprint_peak)",
         f"slope, all iters    : {s_all:.2f} MB/iter" if s_all is not None else "slope: n/a",
         f"slope, steady state : {s_steady:.2f} MB/iter (iterations {it[skip][0]}..{it[-1][0]})" if s_steady is not None else "",
         f"mean iteration time : {sum(r[5] for r in it if r[5]) / max(1, len([r for r in it if r[5]])):.0f} s",
         "leaks: " + "; ".join(f"{l}: {n} leaks / {b} bytes" for l, n, b in leak_rows),
         "top heap growth (start → end):"] + [f"  {db / 1048576:+9.2f} MB {dc:+9d}  {k}" for k, dc, db in growth[:15]]
open(os.path.join(OUT, "summary.txt"), "w").write("\n".join(lines) + "\n")
print("\n".join(lines))
print(f"\noutput: {OUT}")
