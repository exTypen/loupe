# GPUI vs Swift: performance

Measured 2026-11-27 on one MacBook Pro (built-in Retina display, 120 Hz, 1800×1169 pt), the same data for both apps.
GPUI is the release build of branch `gpui` at 5dde615 (`cargo build --release -p loupe`). Swift is the prebuilt
`LoupeFlows.app` from `~/loupe-flows-cache` (Swift Loupe at fde8251). Every row was run at least 3 times and the
table gives the median. The individual runs are listed under each method. Other agents' apps were running on the
machine at the same time, so the CPU numbers are what each process used itself (`ps` time), not a share of the
machine.

## Results

| Measurement | Target | Swift | GPUI | GPUI meets it |
| --- | --- | --- | --- | --- |
| (a) Click a 150k-line diff → first full frame | < 1 s | 0.279 s | **0.160 s** | yes |
| (b) Scrolling it: frames shown per second (display is 120 Hz) | display rate | 90.7 fps (p50 8.5 ms, p95 16.7 ms) | **116.7 fps** (p50 8.3 ms, p95 9.0 ms) | yes |
| (b′) Scroll start: first scrolled frame after the first wheel event | — | 34–42 ms | 57–67 ms | (not a target) |
| (c) Idle CPU over 60 s, Changes | ~0 | 0.00 s (0 %) | 0.01 s (0.01 %) | yes |
| (c) Idle CPU over 60 s, History | ~0 | 0.00 s (0 %) | 0.02 s (0.03 %) | yes |
| (c) Idle CPU over 60 s, open PR #22 | ~0 | 0.22 s (0.36 %) | 0.04 s (0.06 %) | yes |
| (c) git processes during those idle minutes (9 per app) | 0 | 0 | 0 | yes |
| (d) git processes during 10 tab switches | 0 | 0 | 0 | yes |
| (d) CPU used by those 10 switches | — | 1.32 s | **0.14 s** | — |
| (d′) git processes per round of switching all 15 worktrees, after the first round | nothing unasked | 13 | 13 | yes |
| (d′) …the first round (each worktree's first visit) | — | 6 | 27 | see note |
| (e) RSS after opening the 150k-line diff | — | 230 MB | 193 MB | — |
| (e) phys_footprint at the same moment | — | 155 MB | 218 MB | — |

GPUI meets every target, so this pass changed no app code. What it adds is the harness (`scripts/perf/`) and this
file.

Notes:
- (b) Swift's rate varied between sessions: 71–77 fps in the first runs and 87–115 fps later. GPUI stayed at 116–118
  fps in every run. At 120 Hz wheel input (one event per refresh, which beats against vsync) the rates were GPUI 98
  fps and Swift 74 fps (medians of 3 and 4 runs).
- (b′) Both apps take one long frame when a scroll starts. With the pointer moved 300 ms before the first wheel event
  (`SETTLE_MS=300`), so that the hover frame is out of the way, the gap from the first event to the first scrolled
  frame was 57/67/63 ms for GPUI and 42/34/34 ms for Swift. The sustained rate is not affected. A sample(1) over 8
  scroll starts showed the main thread mostly idle and no hot Loupe frame, so nothing was changed. It's worth a look
  with Instruments if it can be felt.
- (d′) The steady rounds are the same in both apps: 7 × `diff` (the selected file of each dirty worktree), the
  conflict view's `ls-files -u` and 3 × `show :N:`, and 2 × `config --get`. All of them are asked for by what's
  shown. GPUI's first round adds 7 × `rev-parse origin/HEAD` and 6 × `rev-list --count origin/main..HEAD`. That is
  the "Create Pull Request" offer (`views/pr_create/offer.rs`), checked once per HEAD on a worktree's first visit, and
  the Swift app ran it during launch instead. Over the whole run (launch plus 3 rounds) the totals are Swift 300 and
  GPUI 314. The only difference is those 14 offer checks, which GPUI makes once per worktree for the banner, the
  toolbar button and the sidebar item together.
- (e) RSS is lower in GPUI, but phys_footprint is higher and varied from 191 to 323 MB across runs. The GPUI
  footprint includes Metal/IOSurface memory for the window's drawables and the glyph atlas. Neither is a target.
- (c) Swift's open-PR idle wasn't profiled. GPUI's only timers there are the PR poll (`pull::POLL`) and the
  once-a-minute relative-time refresh (`views/pulls.rs`).

## How each number was taken

Everything runs in `/private/tmp/flows-z-perf` (mock forge on port 8837) through the scripts in `scripts/perf/`. Run
`scripts/perf/setup.sh` once. It starts the flows fixture and mock forge, builds the `big` repo and the scenarios,
copies the Swift app and compiles the probe. Every script launches the apps only through `run-gpui.sh` /
`run-swift.sh`. Those set and check `LOUPE_STRESS=<scenario under the folder>` and `HOME=<folder>/home`, so neither
app touches the real settings or Keychain. Real input goes through `cliclick` or CGEvent, always under
`lockf /tmp/loupe-gui.lock`. `front.sh <pid>` brings that process to the front through System Events.

### Data

- `big` (`/private/tmp/flows-z-perf/big`) is a throwaway repo. `huge.txt` is a new 150,000-line file added with
  intent-to-add (`git add -N`), so it shows as an unstaged change with +150000 −0 and goes through the normal
  `git diff` path. `large.txt` is the memtest shape: a 150k-line file with every 1000th line edited. `README.md` has a
  one-line edit and is the file selected at launch.
- `big.json` is the flows scenario plus `big`, selected at launch, with `"steps": []`.
- `flows.json` is the flows fixture (`scripts/flows/`, 3 repos, 15 worktrees, 38 PRs on the mock forge) plus `big`,
  with `app` (main) selected at launch.
- `wt.json` is `flows.json` with `"steps": ["worktrees"], "iterations": 3`.

### The probe: `cap` (`scripts/perf/cap.swift`, `swiftc -O cap.swift -o cap`)

`cap` finds an app's largest on-screen window by PID. It records a crop of that window with ScreenCaptureKit
(`minimumFrameInterval` 1/240 s, only frames with status `.complete`). A frame counts as new only when some sampled
byte (green channel, every other row) moved by more than 12. Without that threshold the compositor's own 1–2 step
colour shift, about 1.5 s after a change, read as a late "last frame". Times are host-clock times of the frames,
taken relative to the moment the input was posted.
- `cap click <pid> x y secs cx cy cw ch` clicks window point (x, y) and records the crop. It prints the first and the
  last change. The last change is the first full frame of what the click opened.
- `cap scroll <pid> x y secs dy cx cy cw ch` moves the pointer to (x, y) and then posts a pixel scroll of dy every
  `EVERY_US` µs (default 8333) for `secs`. It prints the distinct frames, fps, the interval median, p95 and max,
  where the max happened, and how many gaps were over 33 ms. With `SETTLE_MS=n` it waits n ms between the pointer
  move and the first wheel event.

### (a), (b), (e): `measure-open.sh gpui|swift 3`

Each run is a fresh launch on `big.json`. The script waits for the `LOUPE_STRESS finished` line and then 4 s more.
Then, under the lock:
1. It brings the app to the front, clicks `huge.txt` in the file list and records the diff area for 3 s (`cap click`).
   For GPUI that's row (400, 86) and crop (640, 130, 1080×900). For Swift it's (345, 103) and (570, 130, 1080×900).
   The crop leaves out the scrollbar strip.
2. 3 s later it reads `ps -o rss=` and `footprint -p` (e).
3. It scrolls for 3 s at (1100, 600) with `EVERY_US=4166 dy=15`, i.e. 3600 pt/s and a new event before every 120 Hz
   refresh (`cap scroll`).

Runs:
- (a) GPUI 0.160 / 0.164 / 0.158 s. Swift 0.278 / 0.284 / 0.279 s. Swift's first change came at about 0.1 s, a
  partly drawn diff. GPUI's first change was the full diff. Earlier runs in the same session gave GPUI 0.136 / 0.154 /
  0.168 and Swift 0.275 / 0.289 / 0.270.
- (b) GPUI 116.7 / 116.7 / 117.0 fps, p95 9.0 / 9.0 / 8.9 ms. Swift 97.0 / 90.7 / 87.3 fps, p95 16.5 / 16.7 /
  16.7 ms.
- (e) RSS: GPUI 192 / 193 / 194 MB, Swift 230 / 230 / 230 MB. Footprint: GPUI 202 / 232 / 218 MB, Swift 155 ×3 MB.

A sample(1) of GPUI while it opened the diff showed `load_diff` (git diff plus parsing) at about 37 ms on a
background thread and nothing else of note.

### (c) idle: `measure-idle.sh` (Changes, History) and `measure-pr.sh` / `measure-switch.sh` (open PR)

The apps run on `flows.json` with `GIT_TRACE2_EVENT=<empty dir>`. Each git process writes one file into that dir, so
the file count is the number of git processes. The scripts switch tabs with a real click on the toolbar segment:
- GPUI: Changes (853, 21), History (921, 21), Pull Requests (1003, 21).
- Swift: Changes (922, 26), History (1030, 26), Pull Requests (1136, 26).

They take a screenshot to check each state, wait 10 s, and then take three 60 s windows. CPU is `ps -o time=` before
and after each window. The open PR is #22: click Pull Requests, then the #22 row, at (459, 363) in GPUI and (390, 343)
in Swift.

Runs, CPU seconds per 60 s:
- Changes: GPUI 0.01 / 0 / 0.01. Swift 0 / 0 / 0.
- History: GPUI 0 / 0.04 / 0.02. Swift 0 / 0.01 / 0.
- Open PR: GPUI 0.04 / 0.05 / 0.03. Swift 0.12 / 0.22 / 0.26.

Every window had 0 git processes.

### (d) tab switches: `measure-switch.sh gpui|swift`

The app runs on `flows.json` with a trace dir. The warm-up clicks History, Pull Requests and Changes once each, so the
first History log and the first PR list load don't count, and then waits 8 s. Then come 3 rounds of 10 clicks, 1 s
apart: H R C H R C H R C H. Each round counts the git processes and the CPU, and a screenshot after the 10th click
checks that History is shown.

Runs:
- git processes: GPUI 0 / 0 / 0. Swift 0 / 0 / 0.
- CPU: GPUI 0.14 / 0.14 / 0.14 s. Swift 1.31 / 1.33 / 1.32 s.

### (d′) worktree switches: `wt.py gpui|swift`

The app runs on `wt.json`, so the StressDriver `worktrees` step runs 3 times in both apps. The step selects every
worktree of every repo, 0.25 s apart, and then opens and closes the ⌘K switcher. `wt.py` follows the app's
`LOUPE_STRESS step-begin/step-end` lines and counts the new trace files per round, grouped by git command.

Runs:
- GPUI 27 / 13 / 13.
- Swift 6 / 13 / 13.

## Found on the way (not performance)

- `measure-switch.sh`'s final "open PR" click missed once in GPUI: the Pull Requests segment click after a Changes
  click 5 s earlier didn't switch. It didn't reproduce on its own, and `measure-pr.sh` checks the state with a
  screenshot.

## Create-PR offer without git (gpui-z-last)

The offer (`views/pr_create/offer.rs`) no longer runs `rev-parse origin/HEAD` and `rev-list --count` per worktree:
the status refresh's merged check already counts the branch's commits not on the default branch
(`git::merge_state`), and keeps them in `WorktreeState::ahead_of_default`. Measured with `wt.py` (flows fixture on
`/tmp/flows-z-last`, debug builds, GIT_TRACE2_EVENT file count), 3 runs each:
- before (gpui 3e5e52e): first round 26 / 26 / 26 (7 × rev-parse origin/HEAD, 6 × rev-list --count), then 13 / 13.
- after: first round 13 / 12 / 12, then 12–13, the same as the steady rounds. Whole run (launch + 3 rounds): 300 → 286.
- Swift on the same fixture: 6 / 12 / 11, 285 in all (it loads the 6–7 selected-file diffs at launch, so its first
  round has none; GPUI loads them on the first visit).
