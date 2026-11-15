#!/usr/bin/env python3
"""CPU per stress iteration of the test copy: cpu.py <memtest-dir> <steps> <iterations>.
APP=<binary> runs another build (an older one, to compare); TRACE=<file> logs every git it starts (GIT_TRACE2_EVENT)."""
import json, os, subprocess, sys, time, urllib.request
M, steps, iters = sys.argv[1], sys.argv[2].split(","), int(sys.argv[3])
sc = json.load(open(f"{M}/scenario.json")); sc.update(steps=steps, iterations=iters, background=True, pause=0.6)
p = f"{M}/scenario-cpu.json"; json.dump(sc, open(p, "w"))
# The mock forge on PORT (default 8765). Whatever else answers there is never talked to: another program's server got
# this script's requests once (a merge POST included) because any answer used to count as the mock.
port = int(os.environ.get("PORT", "8765"))
sc["forge"] = f"http://127.0.0.1:{port}"
def answer():
    try: return urllib.request.urlopen(sc["forge"] + "/api/v1/version", timeout=1).read()
    except Exception as e: return b"" if not hasattr(e, "code") else b"other"
a = answer()
if a and b"loupe-memtest" not in a:
    sys.exit(f"port {port} is in use by another server, not the mock forge: run with PORT=<free port>")
if not a:
    subprocess.Popen([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "mockforge.py"), sc["big"], "--port", str(port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(50):
        if b"loupe-memtest" in answer(): break
        time.sleep(0.1)
    else: sys.exit("the mock forge didn't start")
subprocess.run(["git", "-C", sc["big"], "remote", "set-url", "origin", f"{sc['forge']}/acme/big.git"], check=True)
json.dump(sc, open(p, "w"))
home = f"{M}/home"
env = dict(os.environ, GIT_TRACE2_EVENT=os.environ.get("TRACE", ""), LOUPE_STRESS=p, HOME=home, CFFIXED_USER_HOME=home)
app = subprocess.Popen([os.environ.get("APP", f"{M}/LoupeMemTest.app/Contents/MacOS/LoupeMemTest"), "-AppleLanguages", "(en)"], env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
def cpu():
    out = subprocess.run(["ps", "-o", "time=", "-p", str(app.pid)], capture_output=True, text=True).stdout.strip()
    m, s = out.split(":"); return int(m) * 60 + float(s)
last = None
for line in app.stdout:
    if not line.startswith("LOUPE_STRESS"): continue
    if any(w in line for w in ("merge", "timeout", "error")): print(line.strip(), flush=True)
    if " ready" in line: last = cpu(); print("ready", round(last, 2), flush=True)
    elif " iter " in line or " iter iter=" in line:
        c = cpu(); print(line.split()[-1], "cpu", round(c - last, 2), "s", flush=True); last = c
    elif "finished" in line:
        time.sleep(10); c = cpu(); print("idle 10s after finish:", round(c - last, 2), "s", flush=True); break
app.terminate()
