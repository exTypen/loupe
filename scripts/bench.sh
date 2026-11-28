#!/bin/bash
# Performance regression benches (bench/benches/loupe.rs): headless, in-process, nothing on screen.
#
#   scripts/bench.sh record [label]    run every bench, write bench/baselines/<label>.json
#   scripts/bench.sh compare [label]   run every bench, print a table against that baseline, exit 1 past a threshold
#
# label defaults to the version in app/Cargo.toml. Thresholds: +5% instructions, +10% wall time, +10% memory.
# Benches that run on the main thread (diff open/scroll, tab switch) count the main thread's instructions
# (BENCH_MEASUREMENT=foreground-instructions); the ones whose work is git and background parsing (History, Changes,
# merge editor) count the whole process's. Instructions are the gate that matters; wall time is noisier (a busy
# machine moves it), so its threshold is wider.
#
# Cutting a release: scripts/release.sh runs `scripts/bench.sh record <new version>` after the build, so the
# release's numbers are what the next one compares against (`scripts/bench.sh compare <that version>`). Commit
# bench/baselines/<version>.json with the release. By hand: `scripts/bench.sh record 0.10.3`.
set -euo pipefail
cd "$(dirname "$0")/.."

mode=${1:-}
label=${2:-$(sed -n 's/^version = "\(.*\)"/\1/p' app/Cargo.toml | head -1)}
case "$mode" in record|compare) ;; *) echo "usage: scripts/bench.sh record|compare [label]" >&2; exit 2 ;; esac
mkdir -p bench/baselines
baseline="bench/baselines/$label.json"
if [ "$mode" = compare ] && [ ! -f "$baseline" ]; then echo "no baseline $baseline (scripts/bench.sh record $label)" >&2; exit 2; fi

# name and measurement, one at a time, each with a 10 minute limit.
benches=(
  "diff_open_150k foreground-instructions"
  "diff_scroll_150k foreground-instructions"
  "tab_switch foreground-instructions"
  "history_open_20k instructions"
  "changes_open_5k instructions"
  "merge_editor_large instructions"
)

cargo bench --profile release-fast -p loupe-bench --no-run 2>&1 | tail -1
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
for b in "${benches[@]}"; do
  set -- $b
  echo "running $1 ($2)…" >&2
  if ! BENCH_MEASUREMENT=$2 perl -e 'alarm shift; exec @ARGV' 600 \
      cargo bench --profile release-fast -p loupe-bench -- --exact "$1" > "$out/$1.log" 2>&1; then
    tail -20 "$out/$1.log" >&2
    echo "$1 failed or timed out" >&2
    exit 1
  fi
done

python3 - "$mode" "$label" "$baseline" "$out" "${benches[@]}" <<'EOF'
import json, re, subprocess, sys
mode, label, baseline, out, benches = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]
UNITS = {"s": 1e9, "ms": 1e6, "µs": 1e3, "us": 1e3, "ns": 1}
results = {}
for b in benches:
    name, measurement = b.split()
    log = open(f"{out}/{name}.log", encoding="utf-8").read()
    est = json.load(open(f"target/criterion/{name}/new/estimates.json"))
    r = {"measurement": measurement, "instructions": round(est["median"]["point_estimate"])}
    m = re.search(r"wall time per iteration: median ([\d.]+) (\S+)", log)
    if m: r["wall_ns"] = round(float(m.group(1)) * UNITS[m.group(2)])
    m = re.search(r"LOUPE_BENCH peak_phys_footprint=(\d+)", log)
    if m: r["peak_phys_footprint"] = int(m.group(1))
    results[name] = r
rev = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
machine = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip()
doc = {"label": label, "commit": rev, "machine": machine, "benches": results}

if mode == "record":
    json.dump(doc, open(baseline, "w"), indent=2)
    open(baseline, "a").write("\n")
    print(f"wrote {baseline}")

base = json.load(open(baseline))["benches"] if mode == "compare" else results
LIMITS = {"instructions": 0.05, "wall_ns": 0.10, "peak_phys_footprint": 0.10}
def fmt(k, v):
    if v is None: return "-"
    if k == "instructions": return f"{v / 1e6:.1f} M"
    if k == "wall_ns": return f"{v / 1e6:.2f} ms"
    return f"{v / 2**20:.0f} MB"
failed = []
print(f"\n{'bench':<20} {'metric':<20} {'baseline':>12} {'now':>12} {'change':>8}")
for name, r in results.items():
    for k in ("instructions", "wall_ns", "peak_phys_footprint"):
        if k not in r: continue
        old = base.get(name, {}).get(k)
        change = (r[k] - old) / old if old else 0.0
        flag = ""
        if old and change > LIMITS[k]:
            flag = "  REGRESSED"
            failed.append(f"{name} {k} {change:+.1%}")
        metric = k if k != "instructions" else ("fg instructions" if r["measurement"].startswith("foreground") else "instructions")
        print(f"{name:<20} {metric:<20} {fmt(k, old):>12} {fmt(k, r[k]):>12} {change:>+8.1%}{flag}")
if mode == "compare":
    if failed:
        print(f"\nbeyond the thresholds (+5% instructions, +10% time, +10% memory) vs {label}: " + "; ".join(failed))
        sys.exit(1)
    print(f"\nwithin the thresholds vs {label}")
EOF
