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
| SH-20 | Row menu "Create Pull Request…" when the branch exists and a client is available | Shown when `pr_create::show_menu_item` (asked when the menu opens); emits `AppEvent::CreatePull` | fixed-swift-bug (see prcreate PC-06) |
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
| SH-39 | Merge shows the merge in History | History on `<remote>/<base>`, merge selected (`g-merged.png`) | same |
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
| KB-14 | Edit › Undo titled by the stack ("Undo Pull"), text fields keep theirs, beep with nothing to undo | The menu title follows the selected worktree; ⌘Z is bound with context `!Input`, so a focused text field keeps its own undo (`g-textundo.png`) and Edit › Undo clicked with a field focused goes to the field; elsewhere git undo (`g-undo.png`), NSBeep when empty | same |
| KB-15 | Redo ⇧⌘Z: text fields only | ⇧⌘Z bound `!Input`: text fields redo, elsewhere a beep | same |
| KB-16..20 | Commit ⌘↩, Back ⌘[, ⌃- ⌃⇧-, ⌥⌘↓↑, ⌥⌘←→ (customizable) | Same defaults and Swift "shortcuts" JSON (keys tests) | same |
| KB-21 | Sheet keys: Esc cancels, ↩ confirms | New Worktree, rename, dialogs: Esc closes, ↩ confirms | same |
| KB-22 | System items: About, Services, Hide, Hide Others, Quit; Cut/Copy/Paste/Select All; Enter Full Screen ⌃⌘F; Minimize ⌘M, Zoom; Help | Hide, Hide Others, Show All, Quit; the edit items; Enter Full Screen ⌃⌘F (it was bound to Zoom); Minimize, Zoom. No About, Services or Help menu | intentional (gpui has no About panel, Services or help book; Loupe has no help either) |
| RW-01 | Open panel, `addRepo`, moves the repo into this space, selects | Same (`AppModel::add_repo`) | same |
| RW-02 | Drop folders on the window | Same (`on_drop` ExternalPaths) | same |
| RW-03 | Remove from Loupe: no confirmation; selection was left on a forgotten path | Same item; `remove_repo` clears a selection that went with the repo | fixed-swift-bug (§14 App shell: `removeRepo` never clears the selection) |
| RW-04 | Prune: `worktree prune`, re-list, errors silent | Same; the prunable row went away (`gpui-prunable-menu.png`, `gpui-pruned.png`, real input); errors go to the Error alert | fixed-swift-bug (§14: prune errors swallowed) |
| RW-05 | Sheet: New / Existing branch, Name ("feature/my-change"), Based on (HEAD, initial commit, branches), Location + Choose…, red error, Cancel / Create; suggested `<repo>-<branch>`; origin/x → `-b x` | Same fields (`swift-sheet.png` vs `gpui-sheet.png`); created feature/demo → selected `app-feature-demo` (`gpui-wt-created.png`, real input). An existing "origin/x" with a local x now checks out x; the folder drops the remote name instead of every "origin-" | fixed-swift-bug (§14: origin/x with local x fails; "origin-" cut anywhere). The same fixes are in core now (`add_worktree_for_branch`, `remote_branch_local_name`, `suggest_worktree_path`), used by `AppModel::add_worktree`, which the sheet now calls (its own add_args are gone; the folder is `suggest_worktree_path` of the branch's local name, so other remotes drop too). Re-checked: Existing origin/feat/csv-export → `app-feat-csv-export` on feat/csv-export tracking origin, selected (`/tmp/flows-f-cleanup/g-wt-ready.png`, `g-wt-created.png`, real input) |
| RW-06 | 'Remove worktree “name”?', "The directory is deleted. The branch is kept.", Remove / Force Remove; Force passes one `--force` | Same dialog (`gpui-remove-confirm.png`); Remove deleted app-feature-demo and selected Root (`gpui-removed.png`, real input). Force passes `-f -f`, so locked worktrees go too (checked with git); the confirmation now calls `AppModel::remove_worktree` (core `remove_worktree`, `-f -f`). Re-checked on a locked worktree: Remove shows git's "cannot remove a locked working tree" in the Error alert, Force Remove deletes it, the branch stays and Root is selected (`/tmp/flows-f-cleanup/g-wt-remove-err.png`, `g-wt-removed.png`) | fixed-swift-bug (§14: Force Remove of a locked worktree always fails) |
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
| MS-06 | history, bigfile | bigfile: same (unstaged file selected through `select_file`, waits for its diff, every side, Full mode, then the view switch: `view-switch segment=0 of=2`, `segment=1`). history: waits for commits, `ShowCommit` of hugeCommit, waits for ≥ 5000 files (`HistoryView::file_paths`), selects every 40th of the first 400, Overview/Files through the view switch without its line (Swift sets the default), Full mode, sides, then All Branches and back (`HistoryView::set_scope`). On the flows fixture both print `timeout huge_commit` and the same step lines (40.1 s each) | same |
| MS-07 | prs, hugepr, mergepr | prs / hugepr: `OpenPull`, Full mode, view switch Overview → Conversation → Files (`of=3`), BackToList; pauses stand in for the list/files waits (the list is the Pulls view's state). mergepr: the forge merge, `AppModel::show_merged`, then checks git: GPUI `merged pr=22 ok=yes secs=0.4 tab=history scope=origin/main top=Merge_pull_request_'Invoice_search_index'_(#22)_from_feature/search-index_into_main error=none`; Swift `merged pr=23 ok=yes secs=0.5 tab=history scope=origin/main top=Merge_pull_request_'Billing_v2:…'_(#23)… error=none` (Swift first waited 90 s: `timeout pull_list`, the fixture has 38 PRs, not 100). ok= is "origin/<base>'s newest commit is the merge of the PR head" (Swift: History's selection is its first row) | same |
| MS-08 | checks, merge, create | merge: 20 conflicted files selected (each opens the merge editor), overlay dismissed; create: `CreatePull` for big's second worktree, 3×, dismissed; checks: waits for ≥ 100 PRs (`PullsView::list_count`), `ShowChecks` with the PR head, waits for the overlay's checks, opens the first whose id contains "job-01" (`ChecksOverlay::open_check_containing`), waits for its page, logSeconds. On the flows fixture both print `timeout pull_list` and `no-check-row` (its checks are "CI / build (pull_request)" …); with "build" the GPUI page opened, no marker | same |
| MS-09 | rebase, stashes, submodule | submodule: same. rebase: waits for > 30 commits, opens the planner from commit #25 (`HistoryView::open_planner_at`), closes it. stashes: Stashes scope, waits for ≥ 200, selects every 20th, back to Current. Flows fixture: GPUI and Swift print the same lines (rebase 2.4 s; stashes `timeout stashes`, 31.4 s) | same |
| MS-10 | LOUPE_STRESS_LOGSTACKS, LOUPE_STRESS_DUMP | Not ported: both hook AppKit (NSLog, NSView / accessibility trees) that the GPUI app doesn't have | intentional |
| MS-12 | View-switch press helper, "view-switch segment= of=" | `press_view_switch`: calls the shown tab's `DiffControls.view` callback, as the toolbar click does; same line | same |
| MS-11 | LOUPE_TRACE git timing on stderr | core `git.rs` TRACE | same |
| MS-13 | git binary order, GIT_OPTIONAL_LOCKS=0, GIT_TERMINAL_PROMPT=0, LC_ALL=C, cancellable reads, 2 s stderr cap | core `git.rs` | same |
| MS-14 | File-type icons | `ui::file_icon` (Changes rows show them, `gpui-launch.png`) | same |

## Known gaps in this pass

- The "Can't Undo" dialog (Revert Commit / Cancel, or OK) is implemented in `shell/mod.rs` (`undo_or_refusal`,
  `revert_refused`) and its revert is now recorded for ⌘Z (§14 History #6), but no refused undo was produced on the
  mock in this pass.
- A row's tooltip can show over its own open context menu (still in the final pass: gpui-component's context menu
  occludes the row, but a tooltip timer started by the hover before the right-click still fires).

## Final pass (merged app, branch gpui-z-shell)

Both apps on the merged `gpui` (5dde615 + this branch) and the prebuilt `LoupeFlows.app` (fde8251), side by side on the
flows mock (port 8831, `/tmp/flows-z-shell`). Shots are in `/tmp/flows-z-shell/shots/` (`g-*` GPUI, `s-*` Swift; `gpui-*` /
`swift-*` for key-driven pairs). Input was real (cliclick clicks, System Events keys, under `/tmp/loupe-gui.lock`).
Note for testers: on this machine's Turkish-QWERTY-PC layout `cliclick kp:return` / `kp:esc` reach neither app; send
keys with System Events `key code 36` / `53`. Idle CPU, GPUI, sidebar + PR list open and again with ⌘K open:
`0.0 0:01.36` → `0.0 0:01.36` and `0.0 0:01.41` → `0.0 0:01.41`, 10 s apart.

Fixed in this pass:
- **SH-13**: the repo header "+" showed only on hover; it is always shown, as in Swift (`g-sh24.png` vs `s-launch.png`).
- **KB-22**: the View menu had two "Enter Full Screen" items (ours plus the one AppKit adds to any "View" menu); ours is
  gone, ⌃⌘F stays bound. Menu dump of both apps now matches item for item except the documented About/Services/Help.
- **KB-02, KB-17 (all customizable shortcuts)**: bindings load with gpui key equivalents, so a shortcut sits on the key in
  that position on the current layout, as AppKit localizes the Swift menus: Settings reads ⌘Ö on Turkish in both apps.
  Before, ⌘[ could only be typed as ⌥⌘8 on Turkish, which selected the 8th worktree (`gpui-kb17.png` before; after:
  ⌘ğ in an open PR went back to the list and kept the selection, `g-kb17.png`). Swift itself ignores ⌘ğ and ⌥⌘8 there
  (`s-kb17.png`, `s-kb17b.png`): its Back to List is a button shortcut AppKit doesn't localize.
- **SH-06, SH-25, SH-27, RW-06, KB-21**: Return/Esc on the Git/Error alert and Esc on the sidebar's confirmation dialogs
  did nothing (nothing inside them had focus); they now dismiss as NSAlert and confirmationDialog do
  (`g-sh25.png` → `g-sh25-ret.png`).
- **SH-04**: with nothing selected the title bar was empty; it reads "Loupe" as Swift's window title does (`s-sh24.png`).
- **MS-04**: a LOUPE_STRESS run now opens the window at the screen's visible frame (Swift: setFrame(visibleFrame)), at
  0,39 1800×1069 like LoupeFlows (0,39 1800×1068). It used to open centered and then grow, so the sidebar footer and the
  commit box sat under the Dock and real clicks there hit the Dock.
- **QS-06 / RW-07**: picking a prunable worktree ("gone") in ⌘K selected its missing folder; it re-lists instead, like a
  sidebar click (`g-qs-gone.png`: selection stays on Root).
- shared (code area, CB-02 / FT-02): the Full-mode tree's rows didn't span the pane, so status icons followed the name
  instead of sitting at the trailing edge as in Swift (`g-full.png` vs `s-full.png`).

| ID | Final pass (merged app) |
|---|---|
| SH-01 | same: launch, restored sidebar, Changes on app (`g-launch.png`, `s-launch.png`) |
| SH-02, SH-03 | same (code; ⌘W is Close Window in both menus) |
| SH-04 | fixed (title "Loupe"); same text/button (`g-sh24.png`, `s-sh24.png`). GPUI also writes "No repositories" in an empty page: intentional (the filter's empty state) |
| SH-05 | same: branch/path button, selection per row (`g-click.png`) |
| SH-06, SH-25 | fixed keys; same text: pull on diverged main and push rejected show git's stderr (`g-sh25.png`/`s-sh25.png`, `g-sh31.png`/`s-sh31.png`) |
| SH-07, SH-08 | same (hosts; owned by ci / pr_create) |
| SH-09 | same (`g-qs01.png`, `s-qs01.png`) |
| SH-10 | same + footer filter (intentional, as before) |
| SH-11 | same rows, badges, Merged, lock; GPUI "missing" on the prunable row (fixed-swift-bug, as before) |
| SH-12 | same: collapsed app shows 39 in both (`g-sh12.png`, `s-sh12.png`) |
| SH-13 | fixed (always-visible "+"); tooltip "New Worktree in lib…" (`g-rw05.png`) |
| SH-14 | same: dragged infra above lib (`g-sh14.png`) |
| SH-15 | same (code; not re-dragged) |
| SH-16 | same items and order (`g-sh16.png`, `s-sh16.png`) |
| SH-17, SH-18, SH-20 | same items incl. Create Pull Request… and Remove Worktree… (`g-sh17.png`, `s-sh17.png`) |
| SH-19 | same tooltips. Still different: a tooltip can show over its own open context menu (`g-sh26menu.png`), see gaps |
| SH-21 | same |
| SH-22 | same: dot click back to Default, its selection restored (`g-sh22.png`) |
| SH-23 | intentional (commit on release), not driven (no trackpad gestures from cliclick) |
| SH-24 | same: "Space 2", empty page, empty state (`g-sh24.png`, `s-sh24.png`) |
| SH-26 | same: Rename Space dialog prefilled, Return renamed to "Work" (`g-sh26.png`, `g-sh26b.png`) |
| SH-27 | same text, Delete moved back (`g-sh27.png`/`s-sh27.png`, `g-sh27b.png`/`s-sh27b.png`) |
| SH-28 | same (code) |
| SH-29, SH-30, SH-31 | same: Pull on lib (ff) then Edit menu "Undo Pull" and ⌘Z → "Undone: Pull", lib back to behind 2 (`g-kb14.png`) |
| SH-32..35 | same (toolbar segments in every shot) |
| SH-36 | same (`g-kb14.png`) |
| SH-37 | same: a worktree added and removed by the other app appeared/vanished without refresh (`s-rw07.png`, `s-rw07b.png`) |
| SH-38, SH-39 | same (code; SH-39 owned by history) |
| KB-01 | same (hidden, no updater) |
| KB-02 | fixed: ⌘Ö on Turkish in both menus |
| KB-03 | same (⌘O) |
| KB-04 | same (`gpui-kb04.png`, `swift-kb04.png`) |
| KB-05..07 | same (`gpui-kb04b.png`, `gpui-kb07.png`, `swift-kb07.png`) |
| KB-08 | same (⌘K) |
| KB-09..13 | same menu items and keys (menu dump); ⇧⌘V marked the PR file viewed in both (`gpui-kb11.png`, `swift-kb11.png`) |
| KB-14, KB-15 | same (`g-kb14.png`) |
| KB-16..20 | same defaults; positional on non-US layouts now (see KB-17 above) |
| KB-17 | fixed (see above) |
| KB-21 | fixed for alerts/dialogs; sheets same (New Worktree: Return created lib-zdemo, `g-rw05c.png`) |
| KB-22 | fixed duplicate Enter Full Screen; About/Services/Help intentional as before |
| RW-01, RW-02 | same (code; the panel opens on ⌘O) |
| RW-03, RW-04 | fixed-swift-bug as before |
| RW-05 | same sheet (`g-rw05.png`, `s-rw05.png`); created lib-zdemo on branch zdemo and selected it (`g-rw05c.png`) |
| RW-06 | same dialog; Remove deleted the folder, kept the branch, selected lib Root (`g-rw06.png`, `g-rw06b.png`) |
| RW-07 | same; prunable handling fixed-swift-bug, now also in ⌘K |
| RW-08, RW-09 | same (code) |
| QS-01..05, QS-07 | same (`g-qs01.png`, `g-qs03.png`, `gpui-qs04.png`/`swift-qs04.png`: ⌘K ↓ ↑ ↩ toggled back to Root in both) |
| QS-06 | same (`g-qs06.png`, `s-qs06.png`: "rout" ↩ opened router); fixed for prunable rows |
| MS-01 | same as before: `--api /repos/acme/app/pulls/1` printed PR #1's JSON |
| MS-02 | same: `--lens repos/app HEAD~3 HEAD` answered in 16 ms |
| MS-03, MS-04 | fixed window origin (see above); `window 1800x1069 t=1.94 … finished` |
| MS-05 | same: steps worktrees 5.6 s, tabsonly 2.1 s, `unknown-step bogus` |
| MS-06..09, MS-11..14 | same (code and the previous pass; not re-run) |
| MS-10 | intentional (AppKit-only hooks) |
| MS-15, MS-16 | same (see changes.md) |
| FT-01..09, UP-01..04 | see changes.md and settings.md |
