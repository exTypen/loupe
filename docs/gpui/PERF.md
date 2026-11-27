# GPUI vs Swift: performance

Measured 2026-11-27 on one MacBook Pro (built-in Retina display, 120 Hz, 1800×1169 pt), the same data for both apps.
GPUI is the release build of branch `gpui` at 8bf456f (`cargo build --release -p loupe`), the commit that ships.
Swift is the prebuilt `LoupeFlows.app` from `~/loupe-flows-cache` (Swift Loupe at fde8251). Every row was run at
least 3 times and the table gives the median. The individual runs are listed under each method. Other agents' apps
were running on the machine at the same time, so the CPU numbers are what each process used itself (`ps` time), not
a share of the machine.

## Results

| Measurement | Target | Swift | GPUI | GPUI meets it |
| --- | --- | --- | --- | --- |
| (a) Click a 150k-line diff → first full frame | < 1 s | 0.204 s | **0.164 s** | yes |
| (b) Scrolling it: frames shown per second (display is 120 Hz) | display rate | 106.7 fps (p50 8.4 ms, p95 16.8 ms) | **116.7 fps** (p50 8.3 ms, p95 8.9 ms) | yes |
| (b′) Scroll start: first scrolled frame after the first wheel event | — | 52 ms (29–58) | 63 ms (45–80) | (not a target) |
| (c) Idle CPU over 60 s, Changes | ~0 | 0.00 s (0 %) | 0.01 s (0.01 %) | yes |
| (c) Idle CPU over 60 s, History | ~0 | 0.01 s (0.01 %) | 0.00 s (0 %) | yes |
| (c) Idle CPU over 60 s, open PR #22 | ~0 | 0.05 s (0.08 %) | 0.02 s (0.03 %) | yes |
| (c) git processes during those idle minutes (9 per app) | 0 | 0 | 0 | yes |
| (d) git processes during 10 tab switches | 0 | 0 | 0 | yes |
| (d) CPU used by those 10 switches | — | 1.29 s | **0.15 s** | — |
| (d′) git processes per round of switching all 15 worktrees, after the first round | nothing unasked | 13 | 14 | yes |
| (d′) …the first round (each worktree's first visit) | — | 6 | 14 | see note |
| (e) RSS after opening the 150k-line diff | — | 228 MB | 193 MB | — |
| (e) phys_footprint at the same moment | — | 156 MB | 231 MB | see note |

Previous run (5dde615, same method): (a) Swift 0.279 / GPUI 0.160 s; (b) 90.7 / 116.7 fps; (b′) 34–42 / 57–67 ms;
(c) Changes 0.00 / 0.01, History 0.00 / 0.02, open PR 0.22 / 0.04 s; (d) 1.32 / 0.14 s; (d′) first round 6 / 27,
later 13 / 13; (e) RSS 230 / 193 MB, footprint 155 / 218 MB.

GPUI meets every target. Since the previous run the first worktree round fell from 27 to 14 git processes (the
Create-PR offer reads the merged check's count, see the last section), and keyboard tab switching works: ⌘2, ⌘1,
⌘2, ⌘3, ⌘1 with no click showed History, Changes, History, Pull Requests, Changes (screenshots checked). This pass
changed no app code.

Notes:
- (b) Swift's rate varies between sessions: 71–77 fps and 87–115 fps in the previous run, 106–107 fps in this one,
  and it rendered at 60 Hz (p50 16.4 ms) in the 1 s scroll-start runs. GPUI stayed at 116–117 fps in every run.
- (b′) Both apps take one long frame when a scroll starts, and both vary a lot run to run. Medians of 6 GPUI runs
  (53 / 48 / 45 / 73 / 80 / 73 ms) and 8 Swift runs (29 / 34 / 58 / 50 / 56 / 56 / 43 / 54 ms), two sessions each. A
  sample(1) (1 ms) over 6 scroll starts showed the main thread busy ~550 ms of 9 s, all ordinary frame work
  (prepaint, taffy layout, glyph painting), nothing hot and no Loupe frame. The extra time is in GPUI's frame
  pipeline: an idle window pauses its `CAMetalDisplayLink` (`gpui-pre-macos` `display_link.rs`, `MetalFrames`), so
  the first wheel event first resumes the link and then waits for its next update and the drawable's presentation.
  That is framework code with no cheap, safe app-side lever, so nothing was changed. The sustained rate is not
  affected.
- (d′) The steady rounds: 7–8 × `diff` (the selected file of each dirty worktree), the conflict view's `ls-files -u`
  and 3 × `show :N:`, and 2 × `config --get`, all asked for by what's shown, the same set in both apps (GPUI's 8th
  `diff` is the `big` repo's selected file). GPUI's first round is now the same as its steady rounds. Swift's first
  round has no `diff` because it loads the selected-file diffs during launch.
- (e) The footprint gap is the window's drawables. `vmmap --summary` right after the click: GPUI IOSurface 120 MB
  resident (13 regions) against Swift's CoreAnimation 38.6 MB; malloc is about the same (GPUI MALLOC_LARGE+SMALL
  ~90 MB, Swift ~77 MB + 22 MB empty). GPUI's `CAMetalLayer` keeps up to 3 full-window drawables
  (`setMaximumDrawableCount(3)` in `gpui-pre-apple` `metal_renderer.rs`), each 3600×2338×4 B ≈ 34 MB at this size.
  Going to 2 would save ~34 MB, but the layer has `allowsNextDrawableTimeout(false)`, so with 2 drawables a frame
  can block the main thread waiting for one. That is a framework change with a frame-pacing risk, so it was not made.
- (c) In the first idle run another agent's keyboard input reached the windows (GPUI's History window got commit
  selections and a tab switch: 2.57 s and 20 git processes; the Swift copy quit). Both apps' idle runs were redone
  and only the clean runs are counted.

## How each number was taken

Everything runs in one folder through the scripts in `scripts/perf/`: this run used `/private/tmp/flows-z-perf2`
(mock forge on port 8852), the previous one `/private/tmp/flows-z-perf` (8837). The scripts hardcode the latter;
`sed` them for another folder and set `FLOWS_PORT`. Run
`scripts/perf/setup.sh` once. It starts the flows fixture and mock forge, builds the `big` repo and the scenarios,
copies the Swift app and compiles the probe. Every script launches the apps only through `run-gpui.sh` /
`run-swift.sh`. Those set and check `LOUPE_STRESS=<scenario under the folder>` and `HOME=<folder>/home`, so neither
app touches the real settings or Keychain. Real input goes through `cliclick` or CGEvent, always under
`lockf /tmp/loupe-gui.lock`. `front.sh <pid>` brings that process to the front through System Events.

### Data

- `big` (`<folder>/big`) is a throwaway repo. `huge.txt` is a new 150,000-line file added with
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
- (a) GPUI 0.144 / 0.164 / 0.167 s (one more run missed the click and was redone). Swift 0.204 / 0.269 / 0.190 s.
  Swift's first change came at about 0.1 s, a partly drawn diff. GPUI's first change was the full diff.
- (b) GPUI 116.0 / 116.7 / 116.7 fps, p95 8.9 / 8.9 / 10.1 ms. Swift 107.3 / 106.7 / 106.0 fps, p95 16.8 / 16.9 /
  16.8 ms.
- (e) RSS: GPUI 193 / 193 / 194 MB, Swift 227 / 228 / 230 MB. Footprint: GPUI 231 / 232 / 231 MB, Swift 156 / 154 /
  156 MB.

(b′) is `start.sh gpui|swift`: a fresh launch on `big.json`, `cap click` on `huge.txt`, then `SETTLE_MS=300 EVERY_US=4166
cap scroll … 1 15` over the diff. The first scrolled frame is at `first-frame + max` (the hover frame, then the long
gap), and the latency is that minus the 300 ms settle.

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
- Changes: GPUI 0 / 0.12 / 0.01. Swift 0 / 0 / 0.
- History: GPUI 0.02 / 0 / 0. Swift 0.05 / 0.01 / 0.
- Open PR (`measure-pr.sh` for GPUI, `measure-switch.sh` for Swift): GPUI 0.02 / 0.02 / 0.03. Swift 0.05 / 0.04 / 0.05.

Every window had 0 git processes.

### (d) tab switches: `measure-switch.sh gpui|swift`

The app runs on `flows.json` with a trace dir. The warm-up clicks History, Pull Requests and Changes once each, so the
first History log and the first PR list load don't count, and then waits 8 s. Then come 3 rounds of 10 clicks, 1 s
apart: H R C H R C H R C H. Each round counts the git processes and the CPU, and a screenshot after the 10th click
checks that History is shown.

Runs:
- git processes: GPUI 0 / 0 / 0. Swift 0 / 0 / 0.
- CPU: GPUI 0.12 / 0.15 / 0.15 s. Swift 1.29 / 1.29 / 1.29 s.

### (d′) worktree switches: `wt.py gpui|swift`

The app runs on `wt.json`, so the StressDriver `worktrees` step runs 3 times in both apps. The step selects every
worktree of every repo, 0.25 s apart, and then opens and closes the ⌘K switcher. `wt.py` follows the app's
`LOUPE_STRESS step-begin/step-end` lines and counts the new trace files per round, grouped by git command.

Runs (rounds 1 / 2 / 3, three launches each):
- GPUI 13 / 14 / 13, 14 / 14 / 14, 14 / 14 / 14.
- Swift 6 / 13 / 13, 6 / 13 / 13, 6 / 13 / 13.

## Found on the way (not performance)

- GPUI (5dde615): after ⌘2 (History), ⌘1/⌘3 did nothing until something in the window was clicked. Fixed at 6835e7e
  (a tab switch moves focus into the shown view); verified at 8bf456f with keyboard-only ⌘2/⌘1/⌘2/⌘3/⌘1.
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

