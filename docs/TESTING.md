# Testing

Everything here runs in-process and headless. It uses GPUI's test platform with the Metal offscreen renderer. No
window opens, nothing takes focus, and no event reaches the desktop, so it can run while you work. Nothing touches
your real settings (the `dev.extypen.loupe` defaults domain) or the Keychain.

| What | Where | Run |
|---|---|---|
| Unit tests | `crates/*/src`, `app/src` | `cargo test --workspace` |
| Interaction tests (no pixels) | `app/tests/headless.rs` | `cargo test -p loupe --test headless` |
| End-to-end tests with screenshots | `app/tests/e2e/` | `cargo test -p loupe --test e2e` |
| Performance benches | `bench/benches/loupe.rs` | `scripts/bench.sh compare <version>` |

## End-to-end tests

`app/tests/e2e/main.rs` builds the app the way `main.rs` does: theme, shortcuts, `AppModel`, and the `Shell` window.
It builds them in a `HeadlessAppContext` that renders with real Metal. Each test starts from fresh fixtures:

- **Repositories** (`fixture.rs`). They are built with git under `target/tmp/e2e/home`, which is also HOME for
  the app and for git. Every commit has a fixed author and date, so SHAs and screenshots don't change between runs.
  - `~/app`: five commits, two uncommitted hunks and an untracked file.
  - `~/app-feature`: a worktree on `feature`, which is PR #7.
  - `~/app-merge`: a worktree stopped in a conflicted merge.
- **Forge** (`forge.rs`). A mock Forgejo on 127.0.0.1:47291, inside the test process. It serves the PR list, the PR
  with its diff and comments, and 60 commit statuses in two pages, the case behind the 0.10.2 pagination fix. It logs
  every request so tests can check them.
- **Isolation**:
  - Settings go to `target/tmp/e2e/settings.plist` through `settings::use_file`.
  - The account is the scenario's in-memory one, so the Keychain is never read.
  - The app clock is frozen with `time::freeze`, so "3 days ago" reads the same on every run.
  - TZ is UTC.

Each test drives the UI through `gpui_kit::test::TestWindowExt`: key presses, clicks on element ids, and typed text.
It then checks the model and the repository (`git diff --cached`, `git log`, the mock's comments). Last, it compares
key states with `app/tests/baselines/*.png` (`shot.rs`). A pixel matches when every channel is within ±2, and a shot
passes when 99% of its pixels match.

```sh
cargo test -p loupe --test e2e                    # all 9 flows, about 10 s
cargo test -p loupe --test e2e -- merge pull      # only tests whose names contain "merge" or "pull"
UPDATE_BASELINE=1 cargo test -p loupe --test e2e  # rewrite the baselines (look at each PNG before committing it)
```

When a test fails, it writes `<name>.actual.png` and `<name>.diff.png` to `target/visual/`. The diff image shows
mismatched pixels in red over the dimmed actual image.

A control the tests click has to be observable: give it an id and end its builder chain with `.test_support()`.
That call does nothing outside test builds. Rows that end in `.context_menu(..)` can't take it, so tests reach them
with the keyboard (⌥⌘1…9) or through the model.

The baselines are tied to the fonts of the Mac that recorded them. On another macOS version, rewrite them and check
them by eye.

## Performance benches

`bench/` (`loupe-bench`) is a separate workspace package. That keeps `test-support` out of its dependency graph, and
it depends on gpui-pre at exactly the version gpui-kit uses. Each bench is a `#[gpui::bench]` on `BenchAppContext`
with the Metal headless renderer and GPUI's production threading. The repositories are generated once per run under
`target/tmp/bench`:

| Bench | One iteration | Counted |
|---|---|---|
| `diff_open_150k` | Parse a 150,000-line diff, build its DiffView with highlighting, wait for the first paint | main-thread instructions |
| `diff_scroll_150k` | One 60 px wheel event over that diff, then the frame it causes | main-thread instructions |
| `tab_switch` | Switch to the next tab (Changes, History, Pull Requests) and draw a frame | main-thread instructions |
| `history_open_20k` | A new window on a 20,000-commit repository's History, until the log is listed and drawn | process instructions |
| `changes_open_5k` | A new window on a worktree with 5,000 changed files, until they're listed and drawn | process instructions |
| `merge_editor_large` | Open the merge editor on a 20,000-line file with 500 conflicts, until it's loaded and drawn | process instructions |

`diff_open_150k` also reports the process's peak phys_footprint after one big diff is opened and drawn.

```sh
scripts/bench.sh record [label]    # writes bench/baselines/<label>.json (label: app/Cargo.toml's version)
scripts/bench.sh compare [label]   # prints a table against it; exits 1 past +5% instructions, +10% time, +10% memory
```

The script runs one bench at a time, with a 10-minute limit each, in the `release-fast` profile. A full run takes
about 70 s. Instruction counts repeat within about 1%, so they are the real gate. Wall time is noisier. History and
Changes are git-bound and can move 10–20% between runs on a busy machine, so read a wall-time flag on those two
before believing it. `scripts/release.sh` records the new version's baseline after building. Commit
`bench/baselines/<version>.json` with the release.

To run one bench by hand:

```sh
BENCH_MEASUREMENT=foreground-instructions cargo bench --profile release-fast -p loupe-bench -- --exact diff_scroll_150k
```

These numbers don't compare directly with [PERFORMANCE.md](PERFORMANCE.md), which timed the bundled app on a real
display. The benches leave out git's own diff of the file, the display link and the three window drawables. They
include parsing, layout, highlighting, drawing and GPU submission. Use them to compare one commit with another.
