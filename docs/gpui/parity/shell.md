# Shell parity: SH, KB, RW, QS, MS-01..14

Both apps ran side by side on the flows mock (port 8809, `/tmp/flows-shell`): the prebuilt Swift `LoupeFlows.app` and
`target/debug/loupe` from branch `gpui-v-shell`. Screenshots are in `/tmp/flows-shell/shots/`. GPUI shots marked
"(real input)" were driven with cliclick under `/tmp/loupe-gui.lock`; other rows were checked against the Swift source
(the spec) and the GPUI code, plus unit tests where they say so. Swift's native context menus and alerts are separate
windows that `screencapture -l` misses, so their Swift side comes from App.swift and FLOWS.md rather than a shot.

Idle CPU, GPUI, with the sidebar, a sheet and the switcher used, then left alone: `ps -o %cpu=,time=` read
`0.0 0:02.24` twice 10 s apart (and `0.0 0:00.78` twice after relaunch). The page slide is a gpui spring that stops when it
settles, and its old page is dropped by one 450 ms timer. Nothing polls.

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| SH-01 | One "Loupe" window, 1440×900, min 860×560; restores spaces, selection, recent, collapsed; lists worktrees, loads tokens off-main | Same window size and minimum, the same keys restored, worktrees listed and statuses loaded at launch (`swift-launch.png`, `gpui-launch.png`) | same |
| SH-02 | Reactivating with no visible window orders the window front | `cx.activate(true)` at launch; gpui reopens and fronts its window when reactivated | same |
| SH-03 | Closing the window quits | `on_window_closed` quits when no window is left; ⌘W → CloseWindow | same |
| SH-04 | "No Worktree Selected", the description, "Open Repository…" button | Same text and button in the app's style (`gpui-new-space.png`: an empty space shows it) | same |
| SH-05 | Detail per selection; title is branch / "detached HEAD" / empty, subtitle ~path; selection releases the old model's heavy state; recent ≤ 30 | Branch button shows the same two lines; `AppModel::select` releases and restores heavy state, keeps 30 recent (`gpui-switcher-open.png`) | same |
| SH-06 | App-level failures in an alert titled "Error", OK clears | Same card, now titled "Error" for `app.error` (it said "Git" for both before) | same |
| SH-07 | Checks overlay host | `AppModel::overlay` rendered over the shell; owned by ci | same (host) |
| SH-08 | PR create sheet host | The same overlay host; owned by pr_create | same (host) |
| SH-09 | ⌘K sheet | Palette over the window (`gpui-switcher.png`, real input) | same |
| SH-10 | Space title (more than one space), repo sections, footer: open repo, dots, "+" | Same parts; the GPUI footer also holds the worktree filter field | intentional (the filter is the GPUI shell's quick sidebar filter, kept) |
| SH-11 | Row icon, title ("Root" / prefix stripped), branch or "detached @ sha", Merged, ↑N ↓N, change count, lock, tooltip "path\nbranch" | Same parts and tooltip; plus a faint ⌥⌘N hint (`gpui-launch.png`) | same |
| SH-12 | Header click collapses / expands, persisted in `collapsed`; collapsed header shows the summed change count | Same, persisted under `collapsed` | same |
| SH-13 | Header "+" (help "New Worktree in <repo>…") opens AddWorktreeSheet | Same tooltip; opens the New Worktree sheet (`gpui-sheet.png`, real input) | same |
| SH-14 | Drag a header onto another to put it before; 2 pt accent line above the target; only repos of this page's space | `on_drag`/`on_drop` with a 2 px blue line on the target, `can_drop` limited to the page's repos; dragged "infra" above "lib" (`gpui-reorder.png`, real input) | same |
| SH-15 | Drop a header onto a space dot: `moveRepos`, selection jumps to the current space's first repo | Same; dragged "app" onto Space 2's dot, selection went to lib (`gpui-drop-dot.png`, real input) | same |
| SH-16 | Header menu: New Worktree…, Reveal in Finder, Prune Stale Worktrees, Move to Space ▸ (more than one space), divider, Remove from Loupe | Same items and order, Move to Space as a submenu (`gpui-repo-menu.png`, real input). Prune errors now show | fixed-swift-bug (§14 App shell: "errors from Prune Stale Worktrees are swallowed") |
| SH-17 | Row menu: Reveal in Finder, Open in Terminal / Zed / VS Code, Copy Path | Same (`gpui-wt-menu.png`, real input) | same |
| SH-18 | "Remove Worktree…" (not on the main worktree) opens RW-06 | Same, hidden on main (`gpui-wt-menu.png`, real input) | same |
| SH-19 | Tooltips: row, header "+", dot (space name), "Open Repository…", "New Space" | Same tooltips (`gpui-new-space.png` shows "New Space") | same |
| SH-20 | Row menu "Create Pull Request…" when the branch exists and a client is available | Same condition (`client_for` on the model's remote); emits `AppEvent::CreatePull` (`gpui-wt-menu.png`) | same |
| SH-21 | Rows highlight; headers can't be selected | Same | same |
| SH-22 | Dots switch space (each keeps its selection); pages slide 0.28 s; current dot filled | Same; the pages spring (k=500, critically damped) from the old space | same |
| SH-23 | Two-finger swipe: pages follow, 0.35× rubber band, spring to nearest or flick > 0.6 pages/s; first 4 pt decide the axis; wheels go to the list | Same rules (`on_swipe`, `swipe_step` unit test); the list uses `restrict_scroll_to_axis` so a horizontal swipe never scrolls it. Not driven with a real trackpad here (cliclick has no gestures). The switch commits on release and the pages spring from where they were, instead of committing at rest | intentional (commit on release: same visible result, no display link) |
| SH-24 | "+" adds "Space N" and switches to it | Same (`gpui-new-space.png`, real input) | same |
| SH-25 | Worktree git errors in an alert titled "Git" | Same (pull over local changes in billing showed git's message, real input) | same |
| SH-26 | Space title / dot menu › Rename…: alert with prefilled field, Rename / Cancel; blank ignored | Same dialog; Return renames, Esc cancels (`gpui-rename.png`, `gpui-renamed.png`, real input) | same |
| SH-27 | Delete…: confirmation with the Swift message; disabled with one space | Same text and disabled state; `AppModel::delete_space` | same |
| SH-28 | Selecting a worktree of another space slides the sidebar there | `AppState::select` switches space; `follow_space` slides the pages | same |
| SH-29 | Fetch: spins while fetching, all three disabled while busy, `fetch --all --prune` with auth env | Busy label shows, buttons dim and ignore clicks; same command | same |
| SH-30 | Pull `--ff-only`, undoable ("Undo Pull") | Same; Edit menu read "Undo Pull" after a pull and ⌘Z showed "Undone: Pull" (`gpui-undo-toast.png`, real input) | same |
| SH-31 | Push `--set-upstream <remote> HEAD` | Same | same |
| SH-32 | Branch button opens BranchPicker | Same (shell/branches.rs) | same |
| SH-33 | Changes / History / Pull Requests picker, ⌘1–3 | Same (`gpui-cmd2.png`, real input) | same |
| SH-34 | Compact / Full picker while a diff shows | Same | same |
| SH-35 | Overview / Files / Conversation picker from `DiffControls.view` | Same, labelled segments | same |
| SH-36 | Bottom toast for 6 s, Undo link | Same (`gpui-undo-toast.png`) | same |
| SH-37 | File events refresh about 0.4 s later, filtered | Core Watcher filter + 250 ms debounce (WorktreeModel) | same |
| SH-38 | ⌘R refreshes status and posts refresh; no re-list | Same | same |
| SH-39 | Merge shows the merge in History | Owned by history / pulls | same (not shell) |
| KB-01 | "Check for Updates…" hidden when Sparkle isn't configured | Not in the menu: this build has no updater | same (as an unconfigured Swift build) |
| KB-02 | Loupe › Settings… ⌘, | Same item and key | same |
| KB-03 | File › Open Repository… ⌘O, no ⌘N | Same | same |
| KB-04 | View › Toggle Sidebar ⇧⌘S | Same (`gpui-toggle-sidebar.png`, real input) | same |
| KB-05..07 | View › Changes ⌘1, History ⌘2, Pull Requests ⌘3 | Same (`gpui-cmd2.png`) | same |
| KB-08 | View › Go to Worktree… ⌘K | Same | same |
| KB-09..10 | Review › Next / Previous File ⌥↓ ⌥↑ | Same items and keys; views handle the actions | same |
| KB-11 | Mark Viewed & Next ⇧⌘V | Same | same |
| KB-12 | Compact / Full Mode ⌥⌘F, flips even with no diff | Same | same |
| KB-13 | Refresh ⌘R | Same | same |
| KB-14 | Edit › Undo titled by the stack ("Undo Pull"), text fields keep theirs, beep with nothing to undo | Same: the menu title follows the selected worktree (read via System Events: "Undo Pull", then "Undo"); inputs bind ⌘Z themselves; NSBeep when empty | same |
| KB-15 | Redo ⇧⌘Z: text fields only | Same; inputs bind ⇧⌘Z, elsewhere a beep | same |
| KB-16..20 | Commit ⌘↩, Back ⌘[, ⌃- ⌃⇧-, ⌥⌘↓↑, ⌥⌘←→ (customizable) | Same defaults and Swift "shortcuts" JSON (keys tests) | same |
| KB-21 | Sheet keys: Esc cancels, ↩ confirms | New Worktree, rename, dialogs: Esc closes, ↩ confirms | same |
| KB-22 | System items: About, Services, Hide, Hide Others, Quit; Cut/Copy/Paste/Select All; Enter Full Screen ⌃⌘F; Minimize ⌘M, Zoom; Help | Hide, Hide Others, Show All, Quit; the edit items; Enter Full Screen ⌃⌘F (it was bound to Zoom); Minimize, Zoom. No About, Services or Help menu | intentional (gpui has no About panel, Services or help book; Loupe has no help either) |
| RW-01 | Open panel, `addRepo`, moves the repo into this space, selects | Same (`AppModel::add_repo`) | same |
| RW-02 | Drop folders on the window | Same (`on_drop` ExternalPaths) | same |
| RW-03 | Remove from Loupe: no confirmation; selection was left on a forgotten path | Same item; `remove_repo` clears a selection that went with the repo | fixed-swift-bug (§14 App shell: `removeRepo` never clears the selection) |
| RW-04 | Prune: `worktree prune`, re-list, errors silent | Same; the prunable row went away (`gpui-prunable-menu.png`, `gpui-pruned.png`, real input); errors go to the Error alert | fixed-swift-bug (§14: prune errors swallowed) |
| RW-05 | Sheet: New / Existing branch, Name ("feature/my-change"), Based on (HEAD, initial commit, branches), Location + Choose…, red error, Cancel / Create; suggested `<repo>-<branch>`; origin/x → `-b x` | Same fields (`swift-sheet.png` vs `gpui-sheet.png`); created feature/demo → selected `app-feature-demo` (`gpui-wt-created.png`, real input). An existing "origin/x" with a local x now checks out x; the folder drops the remote name instead of every "origin-" | fixed-swift-bug (§14: origin/x with local x fails; "origin-" cut anywhere). The same fixes are in core now (`add_worktree_for_branch`, `remote_branch_local_name`, `suggest_worktree_path`), used by `AppModel::add_worktree` (test `worktrees_for_remote_branches_and_locked_removal`) |
| RW-06 | 'Remove worktree “name”?', "The directory is deleted. The branch is kept.", Remove / Force Remove; Force passes one `--force` | Same dialog (`gpui-remove-confirm.png`); Remove deleted app-feature-demo and selected Root (`gpui-removed.png`, real input). Force passes `-f -f`, so locked worktrees go too (checked with git); core `remove_worktree` and `AppModel::remove_worktree` pass `-f -f` too (same test) | fixed-swift-bug (§14: Force Remove of a locked worktree always fails) |
| RW-07 | Worktree added/removed outside: re-listed; `rm -rf` leaves a prunable row that runs git in a missing folder | Re-listed the same way. A prunable row shows dimmed with "missing" and a tooltip, can't be selected, and offers Prune; a row whose folder vanished since the last listing re-lists instead of selecting. `refresh_worktrees` builds no model (watcher + git status) for a prunable worktree whose folder is gone (`gone` in the fixture; idle CPU 0.0 twice 10 s apart) | fixed-swift-bug (§14: `isPrunable` parsed, never used) |
| RW-08 | PR checkout into `<repo>-pr-N` | `AppModel::checkout_pull` now only reuses this repository's `pr/N` worktree, like the pulls view's `checkout_pull_in_repo` | fixed-swift-bug (§14 PR: Model.swift:559 searched every repo) |
| RW-09 | Every repo listed at launch; models per worktree | Same | same |
| QS-01 | 600×380 sheet, "Go to worktree…" field | Same size and placeholder (`gpui-switcher.png`) | same |
| QS-02 | Repo name (130 wide) + worktree row; collapsed repos listed | Same; no model built per row | fixed-swift-bug (§14 quick switcher #18: a model per row) |
| QS-03 | Every token a substring of "repo branch folder"; highlight resets to 0 | Same (switcher test) | same |
| QS-04 | MRU order, row 1 preselected when row 0 is current; only the current space | Same order and preselection (`gpui-switcher.png`); every space is listed, the current one first | fixed-swift-bug (§14 quick switcher #1: other spaces unreachable) |
| QS-05 | ↑/↓ move, clamped, list scrolls | Same | same |
| QS-06 | ↩ or click opens: expands the repo, selects, closes | Same (↩ opened billing, `gpui-switcher-open.png`, real input) | same |
| QS-07 | Esc closes, no change | Same | same |
| MS-01 | `--api <path>` prints a forge GET and exits; first account only, always `<base>/api/v1` with `token` auth; no account → "no account/token", exit 1 (LoupeFlows, no accounts: exactly that) | `main.rs` `api()`: the current space's account (else the first) through `Client::raw`, so GitHub gets api.github.com or /api/v3 and its own Accept; the raw body on stdout, exit 0; same "no account/token" exit 1. In a LOUPE_STRESS run the scenario's account answers: `--api /repos/acme/app/pulls/1` on the flows mock printed PR #1's JSON, no window | fixed-swift-bug (§14 App.swift:54 `--api`) |
| MS-02 | `--lens …` prints lens reports and exits | `main.rs` prints `code::overview::lens_cli` and exits before the app starts. On the fixture, `repos/app HEAD~3 HEAD` and `repos/app-billing main worktree` gave byte-identical reports in both apps (the `(N ms)` line aside). Swift's `--lens repos/app-billing HEAD~40 HEAD` hung at 0% CPU (main thread blocked on its semaphore); GPUI answered in 19 ms | same |
| MS-03 | LOUPE_STRESS bootstrap, own defaults domain, in-memory account | `state/scenario.rs`: own domain `dev.extypen.loupe.flows-gpui.<fixture folder>` (`domain_for`; LoupeFlows copies get `dev.extypen.loupe.flows.<fixture folder>` from run-swift.sh), wiped at launch; in-memory account, the scenario's repos; unreadable file → "LOUPE_STRESS: can't read …", exit 2 (checked) | same |
| MS-04 | Output protocol: wait for worktrees, 3× pause, window to the visible frame, "window WxH", ready, step-begin/step-end secs=, iter, finished, timeout <what>, unknown-step | `scenario::run`: the same lines and order on stdout. Steps [] on the fixture: Swift `window 1800x1066 t=1.83, ready, iter iter=1 t=3.39, finished`; GPUI `window 1800x1066 t=1.89, ready, iter iter=1 t=3.4, finished`; `bogus` → `unknown-step bogus`. Idle after `finished`: 0.0% CPU, 0.01 s over 10 s. `background` leaves the app inactive (main.rs skips `activate`); gpui can't order a window back or move it, so only the size follows the screen | same |
| MS-05 | worktrees, tabs, tabsonly, idle | Same steps through `AppModel::select` / `set_tab`; ⌘K is the shell's GoToWorktree action and Esc sent to the window (stress run: worktrees 5.4 s, tabsonly 2.1 s, tabs 8.3 s) | same |
| MS-06 | history, bigfile | bigfile: same (unstaged file selected through `select_file`, waits for its diff, every side, Full mode, then the view switch: `view-switch segment=0 of=2`, `segment=1`). history: `ShowCommit` of hugeCommit, Full mode, sides; the file count, the commitOverview default and the All Branches scope are History's own state, so it pauses instead of waiting and prints `unsupported history-scope` | intentional (History exposes no scope/overview API to state/; requested from views) |
| MS-07 | prs, hugepr, mergepr | prs / hugepr: `OpenPull`, Full mode, view switch Overview → Conversation → Files (`of=3`), BackToList; pauses stand in for the list/files waits (the list is the Pulls view's state). mergepr: the forge merge, `AppModel::show_merged`, then checks git: GPUI `merged pr=22 ok=yes secs=0.4 tab=history scope=origin/main top=Merge_pull_request_'Invoice_search_index'_(#22)_from_feature/search-index_into_main error=none`; Swift `merged pr=23 ok=yes secs=0.5 tab=history scope=origin/main top=Merge_pull_request_'Billing_v2:…'_(#23)… error=none` (Swift first waited 90 s: `timeout pull_list`, the fixture has 38 PRs, not 100). ok= is "origin/<base>'s newest commit is the merge of the PR head" (Swift: History's selection is its first row) | same |
| MS-08 | checks, merge, create | merge: 20 conflicted files selected (each opens the merge editor), overlay dismissed; create: `CreatePull` for big's second worktree, 3×, dismissed; checks: `ShowChecks` with the PR head, waits for the overlay, logSeconds; the job row is the overlay's own state, so it prints `no-check-row` | intentional (checks: no row-click API in views/ci; requested) |
| MS-09 | rebase, stashes, submodule | submodule: same. rebase and stashes select the worktree's History and print `unsupported rebase-planner` / `unsupported stashes-scope` (planner and scope are History's state) | intentional (requested from views) |
| MS-10 | LOUPE_STRESS_LOGSTACKS, LOUPE_STRESS_DUMP | Not ported: both hook AppKit (NSLog, NSView / accessibility trees) that the GPUI app doesn't have | intentional |
| MS-12 | View-switch press helper, "view-switch segment= of=" | `press_view_switch`: calls the shown tab's `DiffControls.view` callback, as the toolbar click does; same line | same |
| MS-11 | LOUPE_TRACE git timing on stderr | core `git.rs` TRACE | same |
| MS-13 | git binary order, GIT_OPTIONAL_LOCKS=0, GIT_TERMINAL_PROMPT=0, LC_ALL=C, cancellable reads, 2 s stderr cap | core `git.rs` | same |
| MS-14 | File-type icons | `ui::file_icon` (Changes rows show them, `gpui-launch.png`) | same |

## Known gaps in this pass

- The "Can't Undo" dialog (Revert Commit / Cancel, or OK) is implemented in `shell/mod.rs` (`undo_or_refusal`,
  `revert_refused`) and its revert is now recorded for ⌘Z (§14 History #6), but no refused undo was produced on the
  mock in this pass.
- A row's tooltip can show over its own open context menu.
