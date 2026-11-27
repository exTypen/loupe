# View agents: ownership, entry points and shared APIs

Read `PORT.md` first; this file adds what wave 2 needs. The app runs on real models now (branch `gpui-integrate`):
sidebar, toolbar, branch picker, fetch/pull/push, undo + toast, the error alert and a minimal Changes view all work
against loupe-core. Each view agent owns only the files listed for it below.

## Frozen shared files

`app/src/lib.rs`, `main.rs`, `theme.rs`, `state/`, `shell/`, `ui/`, `diff/` and the crates are shared. View agents
don't edit them. If you need something there, write a local version in your own module and put the request in your
report.

## Running and testing

```sh
FLOWS_PORT=8797 sh ~/loupe-gpui-mock/scripts/flows/up.sh /tmp/<you>-flows     # fixture + mock forge (once)
F=/tmp/<you>-flows; LOUPE_STRESS=$F/scenario.json HOME=$F/home GIT_CONFIG_NOSYSTEM=1 target/debug/loupe &
swift /tmp/winid.swift loupe   # → screencapture -o -l <id> shot.png; cliclick to click/type
sh ~/loupe-gpui-mock/scripts/flows/down.sh /tmp/<you>-flows                     # stop the forge; kill the app
```

Pick your own port and output folder, so you don't share a forge or fixture with another agent. In scenario mode
(`state/scenario.rs`) the app uses its own settings domain `dev.extypen.loupe.flows-gpui.<fixture folder>` (the scenario file's folder, so parallel fixtures don't share one), wiped at launch. It never
touches the real domain or the Keychain. It opens the scenario's repos with an in-memory account
(`forge`/`user`/`token`) and talks plain http to the mock. Idle CPU must stay at 0: check with
`ps -o %cpu=,time= -p <pid>` twice, 10 s apart.

## Shared APIs

### State (`app/src/state/`)

- **`AppModel`** (`Entity<AppModel>`, one per app; every view gets it in `new`). It derefs to core's `AppState`
  (`spaces`, `current_space`, `worktrees`, `selection`, `recent`, `collapsed`). Every `cx.notify()` saves the Swift
  keys `spaces`, `currentSpace`, `recent`, `collapsed`, `tab`, `prFullMode`, `diffSide` and `diffLayout`.
  - Reading: `selected_model() -> Option<Entity<WorktreeModel>>` (what every view shows), `model(path)`,
    `selected_worktree()`, `worktrees_of(repo)`, `status(path)`, `space_of(path)`.
  - Diff mode: `tab`, `full_mode`, `diff_side`, `layout`, `whole_files()`, `set_tab`, `set_full_mode`,
    `set_diff_side(side, layout)`. The last two reload the selected worktree's diff.
  - Toolbar: `set_controls(Tab, DiffControls { full, view: Some((modes, current, on_set)) })`. Call it when the
    controls change, not in `render`; it does nothing when they're unchanged.
  - Forge: `accounts`, `tokens`, `tokens_loaded`, `account_for(&RemoteRepo, worktree)`,
    `client_for(&RemoteRepo, worktree) -> Option<loupe_forge::Client>`, `git_auth(remote, worktree)` (the
    credential env for git), `set_account(Account, Option<token>)` and `remove_account(id)`, which write the Keychain
    except in scenario mode. Tokens are loaded once, off the main thread. `client_for` is None until they arrive, so
    observe the app and retry.
  - Repos and worktrees: `select(Some(path))`, `switch_space_to`, `add_space`, `delete_space`, `move_repos`,
    `move_repo`, `add_repo(path)`, `remove_repo`, `refresh_worktrees(repo) -> Task`, core `add_space_without_switch()` (Settings' "+"),
    `add_worktree(repo, path, branch, base, new_branch) -> Task<Result<(), String>>`, `remove_worktree(..)`,
    `checkout_pull(worktree, number)` (this repository's `pr/N` worktree, else a new one), `state::suggest_worktree_path`.
    Core (`loupe_core::model`) does the git side: `add_worktree_for_branch` (an existing "origin/x" checks out x or
    makes it), `remove_worktree` (`-f -f` when forced), `remote_branch_local_name(remotes, ref)`,
    `suggest_worktree_path` (drops a leading "origin/" only: pass the local name for other remotes).
  - Overlays and sheets: `present(AnyView)` shows a full-window view over everything; your view draws its own
    backdrop, and Esc or Close calls `dismiss()`. Only one shows at a time.
  - Event bus: `app.update(cx, |a, cx| a.emit(AppEvent::…, cx))`. `emit` first selects the worktree and switches to
    the tab that serves the event. The events:
    - `ShowCommit { worktree, sha }` and `ShowBranch { worktree, branch }`: History.
    - `OpenPull { worktree, number }`: Pulls.
    - `OpenMergeEditor { worktree, path }`: merge.
    - `ShowChecks { worktree, sha, pull }`: ci.
    - `CreatePull { worktree }`: pr_create.
    - `ShowStashes { worktree }`: History on the Stashes scope.
    - `NewWorktree { repo, base }`: the shell's New Worktree sheet, based on `base` ("" for HEAD).
    - `Toast { text, undoable }`: the shell shows it for 6 s. `undoable` adds "Undo", which runs ⌘Z.
    - `Refresh`: ⌘R. History, the PR list and an open PR reload.

    Listen with `cx.subscribe_in(&app, window, |v, _, e: &AppEvent, w, cx| …)`, or with `window.subscribe` in an
    `install` function.
  - Toast shortcut: `app.toast(text, undoable, cx)`. Errors: `app.error = Some(..)` for app-level failures. A
    worktree's failures go in its own `state.error`. Both show in the shell's "Git" alert.
- **`WorktreeModel`** (one per listed worktree; `app.selected_model()`): `state: loupe_core::model::WorktreeState`
  (branch, head_oid, ahead/behind, files/staged/unstaged/conflicted/listed, selection, commit_message, amend, busy,
  error, remote_name, refs_version, status_version, lfs, operation …), `remote: Option<RemoteRepo>`, `repo`,
  `diff: Option<Arc<diff::FileDiff>>` (the shown diff, replaced only when its text changed), `is_selected()`.
  - `perform(label, |path| -> Result<R, E>, cx) -> Task<Option<R>>`: runs a git action in the background with
    `busy` set, puts the failure in `error` (never a cancellation), shows the undo toast the action recorded, then
    refreshes. Use it for every git action you add. E implements `state::worktree::UserError` (core errors, forge
    errors, String).
  - Actions: `stage`, `unstage`, `discard` (confirm first), `apply_lines(HunkAction, lines)`, `commit`,
    `set_commit_message`, `set_amend`, `stash`, `finish_operation(abort)`, `resolve(path, side)`, `remote_op`,
    `fetch`, `pull`, `push`, `switch_branch`, `undo`, `undo_title`.
  - Status: `refresh()` (coalesced, one at a time), `select_file(id)`, `select_files(shown, ids)`, `load_diff()`.
    Refreshes come only from file events (250 ms debounce after core's Watcher filter), from actions and from ⌘R.
    Never poll.
  - Reload keys, following the last five Swift commits: reload History when `state.head_oid` or
    `state.refs_version` moved. Reload PR lists 30 s after the last load (`loupe_forge::pull::RELOAD_AFTER`). Cancel
    work a view no longer shows: drop its Task, or `Cancel` a core or forge read.
- **Background work**: `cx.spawn(async move |this, cx| { let r = cx.background_executor().spawn(async move { … })
  .await; this.update(cx, |v, cx| …) })`. Core and forge calls block, so never run them on the main thread. Never
  `read`/`update` the AppModel synchronously from inside a WorktreeModel method the app may be calling: defer it
  (`cx.defer`), or do it after an `.await`.
- **Shared helpers in the crates**: `loupe_forge::pull::fetch_head(pr, local, cancel)` serializes and retries per
  repository (call it directly); `loupe_code::index::references(name, rev, dir, limit)` is NUL-separated (ask for
  `REFERENCE_LIMIT + 1` so ui/tabs can tell "2000+"); `loupe_forge::ai::installed().unwrap_or_else(ai::scan)` finds the
  AI CLIs and honours LOUPE_FLOWS_AI in scenario runs; `WorktreeModel::load_lfs_locks` hands a repository's lock answer
  to its sibling worktrees.
- **Stress-driver hooks** (state/scenario.rs reaches the views through the window's `Shell::views`):
  `HistoryView::{set_scope, commit_shas, file_paths, select_file, open_planner_at}`, `PullsView::list_count`,
  `ChecksOverlay::{open_check_containing, showing_list, check_count}`; `CodeBrowser::set_images(path, old, new)`
  shows an LFS image's thumbnails while that file is open (LF-04 in Full mode).
- **`state::adapt`**: `file_diff(&core FileDiff) -> diff::FileDiff` keeps the same row indices, so a renderer line
  index is a core line index. `highlighter()` is the loupe_code tokenizer with separate old and new state.
  `apply_code_theme()` applies the `editorDarkTheme` setting to every diff. After changing the theme, call it and then
  `cx.refresh_windows()`.

### Diff renderer (`app/src/diff/`)

`cx.new(|cx| DiffView::new(Arc<FileDiff>, cx))`, then `set_file`, `set_hunk_actions`, `set_side`, `set_layout`,
`set_highlighter(Some(adapt::highlighter()))`, `set_comments`, `set_blame`, `set_change_ruler`, `set_images`,
`reveal`, `jump_change`, `open_find`, and the callbacks `on_hunk_action`, `on_add_comment`, `on_navigate`,
`on_mark_viewed`, `on_side_change` (route it to `app.set_diff_side`) and `header_actions`. See `views/changes.rs`
for a full wiring. `diff::set_token_colors` sets the syntax palette.

### UI (`app/src/ui/`) and the shell

- `ui::card`, `card_header`, `card_footer`, `pill`, `pill_primary`, `badge`, `badge_tinted`, `code_chip`, `kbd`,
  `dot`, `list_row`, `list_row_parts`, `section_header`, `code_block`, `code_line`, `empty_state`, `icon_button`,
  `segmented`, `toast`, `link_button`, `divider`, and `icon(lucide name)`.
- `ui::file_icon(path)`: a multicolour file-type image.
- Also in `ui`: `h_resizable` / `v_resizable`, `ui::palette::palette` (a picker frame), and `ui::tabs` (code tabs,
  nav controls, results panel).
- Colors come only from `theme.rs`. gpui-component is set to dark and restyled to it. Besides the palette it has
  the system hues Swift used: `teal()` (the merge editor's "both"), `orange()` (conflicts: tints, the operation banner,
  their icons), `brown()`, `indigo()`, `gray()` (lens tiles and kind bars).
- Keys: the `shell::keys` actions (`Commit`, `NextFile`, `PreviousFile`, `MarkViewed`, `BackToList`,
  `NavigateBack`/`NavigateForward`, `NextConflict`, `UseOurs`, …). Handle them with
  `.on_action(cx.listener(..))` on your view's root. The shell already handles OpenRepository, GoToWorktree,
  ToggleSidebar, the tab switches, FullMode, Fetch, Pull, Push, ShowBranches, Refresh (status plus
  `AppEvent::Refresh`), Undo, OpenSettings, SelectWorktree1–9 and the window actions.
- Navigation between views goes through `AppModel::set_tab` and the `AppEvent`s above. A view never holds another
  view.

## Owners

| Owner | Files | Entry point | FLOWS.md IDs |
|---|---|---|---|
| changes | `views/changes.rs` (+ new `views/changes/*.rs`) | `ChangesView::new(app, window, cx)`: the Changes tab | CH-01..42, CM-01..13, SS-01..10, FI-01..06, DV-01..45 (in Changes; renderer fixes go to the integrator), LF-01..09, SM-01..07, FT-01..09, MS-15..16 |
| history | `views/history.rs` (+ `views/history/*`) | `HistoryView::new`: the History tab; serves `ShowCommit`, `ShowBranch`, `Refresh` | HI-01..22, RB-01..13, UN-01..08 (the history side: undo labels, revert offer, refusal), SH-39 |
| pulls | `views/pulls.rs` (+ `views/pulls/*`) | `PullsView::new`: the Pull Requests tab; serves `OpenPull`, `Refresh`; emits `ShowChecks`, `ShowBranch` after a merge, and `app.checkout_pull` | PR-01..22, PD-01..33, RV-01..17, FG-01..11 |
| ci | `views/ci.rs` (+ `views/ci/*`) | `ci::install`: on `ShowChecks`, presents `ChecksOverlay` | CI-01..39, SH-07 |
| merge | `views/merge.rs` (+ `views/merge/*`) | `merge::install`: on `OpenMergeEditor`, presents `MergeEditor`. Changes owns the operation banner and emits the event | MG-01..26 |
| pr_create | `views/pr_create.rs` (+ `views/pr_create/*`) | `pr_create::install`: on `CreatePull`, presents `PullCreateSheet`; on success, `OpenPull`. Also the data for the Changes banner (`ahead_of_default`, `pr_banner_dismissed`) | PC-01..20, SH-08, SH-20 |
| code | `views/code.rs` (+ `views/code/*`) | `CodeBrowser::new(app, model, window, cx)`, embedded by Changes (Full mode, Overview) and by History/Pulls | CO-01..22, LN-01..29, CB-01..18 |
| settings | `views/settings.rs` (+ `views/settings/*`) | `settings::open(app, cx)` (⌘,): wire General (commit AI), Editor (themes → `adapt::apply_code_theme`) and Spaces › Accounts (`app.set_account` / `remove_account`, `loupe_forge::account::add_account` off the main thread) | ST-01..20, UP-01..04 |

The integrator did these, and they stay with the shell: SH-01..39 (except SH-07, SH-08, SH-20 and SH-39), KB-01..22,
RW-01..09 (except the AddWorktree sheet), QS-01..07, MS-01..14 and the ⌘Z dispatch.

Shared hooks a view must not re-implement: the toast (`AppEvent::Toast`), the error alert (`state.error` and
`app.error`), overlays (`present` and `dismiss`), tab switching (`set_tab`, or `emit`), the diff mode
(`set_full_mode` and `set_diff_side`), and refreshing (`WorktreeModel::refresh`; never a timer).

## Not wired yet (open for an owner, or a request to the integrator)

- The worktree context menu's "Remove Worktree…" and "Prune": `remove_worktree` exists, but nothing calls it yet.
- The "Undo <label>" Edit menu title (`WorktreeModel::undo_title`) and Redo.
- Space swipe, and drag and drop of repo headers.
- (Resolved) the toolbar Overview click: it was the harness (window off-screen), and a click from an inactive window
  works too (parity/shell.md, z-verify).
