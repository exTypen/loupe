# Parity: Changes (CH, CM, SS, FI, DV in Changes, LF, SM, FT, MS-15..16)

Both apps ran on the same flows fixture (`FLOWS_PORT=8801 up.sh /tmp/flows-changes`): the Swift copy
(`LoupeFlows.app`, prebuilt) and the GPUI build of branch `gpui-v-changes`. Screenshots are in
`/tmp/flows-changes/shots/` (`swift-*.png` / `gpui-*.png`). "Shot" means the flow was driven in both apps and compared on
screen; "code" means it was compared against the Swift source line by line and exercised headless or through the model.
Rows marked "shell" or "renderer" run in code another owner holds; they were checked in the Changes tab and recorded
here too.

Idle CPU with the Changes tab open on `app-billing` (29 changes, diff shown): `ps -o %cpu=,time=` read `0.0 0:00.35`
twice, 10 s apart. Nothing polls: every load is a keyed `Task` (blame by diff uid, History list by file, picked commit
by sha+path+whole, submodule pane by entry+status version, LFS images by both oids, Markdown by diff uid, Overview by
head+status version, Full tree by status version+change count, Full file by path+status version), and the `Task` is
dropped when its key moves or another worktree is selected.

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| CH-01 | Toolbar Fetch, `fetch --all --prune` with auth, refs bump, worktrees relisted | shell toolbar → `WorktreeModel::fetch`, same commands and bump | same (shell) |
| CH-02 | Pull ff-only, undo "Pull" | `pull --ff-only` via `remote_op`, undo recorded by core | same (shell) |
| CH-03 | Push `--set-upstream <remote> HEAD` | same args, remote from `detect_remote` | same (shell) |
| CH-04 | Branch title + popover, Local/Remote, filter | shell/branches.rs popover, same sections | same (shell) |
| CH-05 | Switch, other-worktree jump, inline error | shell/branches.rs → `switch_branch`; Return with an empty filter does nothing (the picker stays open, the branch stays): `g-branch.png`, `spike/sqlite-fts` before and after | fixed-swift-bug (§14: Return with nothing typed switched to the newest other branch) |
| CH-06 | Changes / History / Pull Requests picker, global `tab` | same picker, `tab` key; shots `swift-main.png`, `gpui-start.png` | same (shell) |
| CH-07 | "Git" alert, OK clears; ⌘R refreshes | `state.error` in the shell's alert; ⌘R → `refresh()` | same (shell) |
| CH-08 | Banners (operation / LFS missing / submodule) over a 220–380 split, list + stashes link + commit box | same order and split (`changesListWidth`). The operation banner is `merge::banner`; with no operation or conflict, `pr_create::banner` takes its slot (`g-sessions.png`: merge banner on sessions; `g-spike.png`: PR offer on spike) | same |
| CH-09 | Overview/Files toggle; Overview of HEAD→worktree, rich cards; a click selects the file, the line is dropped | same toggle and key; the code view's `ChangeOverview` (rich cards, `g-ov.png`); a click opens the file through the code browser at its line (`g-ovclick.png`); the last report stays on screen while the next is read | fixed-swift-bug (§14 Lenses 4 and 5: line dropped, spinner flash) |
| CH-10 | Compact/Full toggle, ⌥⌘F, Full opens the list's file, back selects the tab's file | same; shots `swift-discard-confirm.png` (Swift in Full), `gpui-full.png` | same |
| CH-11 | Conflicts / Staged ("Unstage All") / Changes ("Stage All") with counts; Changes always shown | same sections and buttons; shots `swift-billing.png`, `gpui-billing.png` | same |
| CH-12 | Folder rows, chain compaction, per-section fold "<section>/<dir>", not persisted | same (`tree::rows`), chain shown whole when it fits; Swift head-truncates | same |
| CH-13 | Icon, name, lock badge, StatusIcon, path tooltip | same; conflicted uses Lucide `square-asterisk` (no exclamation-square in the set) | same |
| CH-14 | "Working Tree Clean" + "No Changes", controls hidden, Stash… disabled | same text and states (code) | same |
| CH-15 | Click / ⌘ / ⇧ / ⌘A; diff follows the newly added row | `list::shown_for` (unit test `the_diff_follows_the_row_just_added`); ⌘A over a virtual list can't hang | same |
| CH-16 | ↑/↓ and ⌥↓/⌥↑ clamp through `listed` | same (`move_file`), list scrolls to the row | same |
| CH-17 | Selection moves W↔S, intersected; diff kept when text unchanged | core `apply_status` + `set_diff` keep it; seen after commit/undo (`gpui-commit-done.png`) | same |
| CH-18 | Context menu items in order | same items/order, Git LFS submenu; shots `swift-ctxmenu.png`, `gpui-ctxmenu.png`. Stage/Discard aren't offered for conflicted files | fixed-swift-bug (§14 Changes: conflicted files staged / discarded) |
| CH-19 | Reveal in Finder / Open / Copy Path(s) | `open -R`, `open_with_system`, newline-joined paths | same |
| CH-20 | Context Blame / File History set the panel | `open_panel` sets (not toggles) | same |
| CH-21 | Submodule pane, LFS object view, lock badges | same; shots `swift-submodule.png` / `gpui-submodule.png`, `swift-lfs.png` / `gpui-lfs.png`. Lock badges also show in a worktree that refreshed while a sibling asked the server: `WorktreeModel::load_lfs_locks` hands its answer to the repository's other worktrees (i-core: billing's hero.psd and banner.png locked, `/tmp/flows-i-core/g-billing.png`) | same |
| CH-22 | Stage `add -A`, undo "Stage x", no toast | `WorktreeModel::stage` (core recipe) | same |
| CH-23 | Space stages the unstaged part / unstages all-staged; conflicted gets staged | same, but conflicted files are left out | fixed-swift-bug (§14 Changes: conflicted files can be staged with markers) |
| CH-24 | Stage All (unstaged, no conflicts) | same | same |
| CH-25 | Unstage All | same | same |
| CH-26 | Unstage `restore --staged` (+origPath) | core `unstage` | same |
| CH-27 | Unborn: `rm --cached -r -q` | core `unstage` | same |
| CH-28 | Header Stage File / Discard… / Unstage File | same pills; shots `gpui-billing.png`, `gpui-sessions.png` | same |
| CH-29 | Stage/Unstage Hunk buttons; none for untracked | same (`hunk_actions`); none in Before/After | fixed-swift-bug (§14 Diff: Stage/Discard in Before/After acted on the whole file) |
| CH-30 | Stage/Unstage/Discard lines from the diff menu | renderer menu → `apply_lines` (stale-diff guard in the model) | same (renderer) |
| CH-31 | ⌘Z "Undo Stage", toast "Undone: …" | shell ⌘Z → `undo`; toast "Undone: Apply Stash" (`g-undo.png`). In a text field ⌘Z undoes the typing and leaves git alone (`g-textundo.png`: typed in History's search, ⌘Z cleared it, router.rs still applied): Loupe's ⌘Z/⇧⌘Z are bound `!Input`, and Edit › Undo clicked with a field focused goes to the field | same |
| CH-32 | "Discard changes?" dialog, "Discard N file(s)", toast with Undo | same sheet text and button; shots `gpui-discard-confirm.png` | same |
| CH-33 | ⌫ discards the unstaged selection; conflicted included | same, conflicted and submodules left out | fixed-swift-bug (§14 Changes: discard offered for conflicted files) |
| CH-34 | Discard… in header / Full pane | same | same |
| CH-35 | Untracked to Trash + undo | core `discard` | same |
| CH-36 | Hunk Discard → NSAlert "Discard these changes?" | same text in the same in-app sheet as file discard; shot `gpui-hunk-confirm.png` | fixed-swift-bug (§14 Changes: two confirmation styles) |
| CH-37 | Discard lines → same alert | same sheet (`Confirm::Lines`) | same |
| CH-38 | Toast 6 s, Undo button, refusals | shell toast via `show_undo_toast` | same (shell) |
| CH-39 | Pane states: submodule, diff, spinner, No Changes, No File Selected, binary, LFS, Markdown, conflict | same states; a rename/chmod-only change shows "No Content Changes", an empty new file "Empty File". A conflicted file shows a "Conflicted" pane with "Open Merge Editor" (and double-click opens it): the merge editor is an overlay here | fixed-swift-bug (§14 Diff: blank Changes pane); conflict entry intentional (merge editor is an overlay, MG owner) |
| CH-40 | Header: icon, old→new, path, +N −N, Diff/Before/After, Blame/History, Preview, actions | same order and conditions | same |
| CH-41 | Full tree: ls-files, unchanged dimmed, marks, lock badges, reload on status/count | the code browser's tree (`CodeBrowser::tree`): ls-files, unchanged dimmed, marks, lock badges; folds once to what changed, then stays as the user left it; an empty worktree says "No Files" (`g-ovclick.png`) | fixed-swift-bug (§14 Lenses 2 and 3) |
| CH-42 | Tabs, header, "No File Open", Empty/Binary, ⌘W, reload per status | the code browser in the Blame / File History panel: tabs, header with back/forward, outline, Blame and File History toggles, Discard / Stage File, ⌘W closes the tab; the open file stays on screen while it reloads (`g-ovclick.png`, `g-fullblame.png`). LFS image thumbnails show in Compact only | fixed-swift-bug (§14 Lenses 5: spinner flash) |
| CM-01 | Summary, ✨, Description 56 pt, Amend, Stash…, Commit | same layout; shots `swift-main.png`, `gpui-start.png` | same |
| CM-02 | One message; summary newlines → spaces; Return at start of Description eaten | same join/split (unit test); the Return is kept | fixed-swift-bug (§14 Changes: Return at the start of the Description) |
| CM-03 | Commit / Commit N / Amend, ⌘↩, Return in Summary; undo "Commit" | same; committed "Parity commit" with ⌘↩ and undid it, message back (`gpui-commit-done.png`, `gpui-undo-commit.png`) | same |
| CM-04 | Disabled without summary or staged; not while busy (double commit) | also disabled while git runs | fixed-swift-bug (§14 Changes: commit can run twice) |
| CM-05 | Amend fills the last message into an empty box | `set_amend` | same |
| CM-06 | `commit --amend`, undo "Amend" | core | same |
| CM-07 | Edit stop banner text, then Amend + Continue | banner text from `stopped_to_edit` | same |
| CM-08 | ✨ when a tool is found, Write/Rewrite help, spinner while running | same (one scan off the main thread) | same |
| CM-09 | Generates from the chosen diff; overwrites text typed meanwhile; no cancel | forge `ai::message` with the same diff choice; the reply is dropped if the message was edited meanwhile; it keeps running across a worktree switch and lands in its own worktree | fixed-swift-bug (§14 Changes: AI overwrites text typed meanwhile) |
| CM-10 | Per-tool argv/stdin, PATH, NO_COLOR | forge `ai` (ported table) | same (forge crate) |
| CM-11 | Errors in the "Git" alert, 120 s timeout | `set_error` with forge's messages | same |
| CM-12 | Reply cleaning | forge `ai` | same (forge crate) |
| CM-13 | Settings keys `aiHarness`, `aiStyle`, `aiInstructions`, `aiModel.<id>` | read through `ai::selected(settings)` | same |
| SS-01 | Stash… popover: message, "Include untracked" (kept), Stash | same; Esc closes it; shot `gpui-stash-popover.png` | same |
| SS-02 | `stash push [-u] [-m]`, no undo | same command (core `stash`), no undo | same |
| SS-03 | Context Stash N Files, `-u` only with an untracked file | same | same |
| SS-04 | "N stashes" link → History, Stashes scope | link emits `AppEvent::ShowStashes`: History on the Stashes scope, newest stash selected (`g-ss04.png`, Swift `s-ss04.png`) | same |
| SS-05 | Stash list in History | History owner's list (`loupe_core::history::stashes`, `stash_diff`) | intentional (History view owns it, as in Swift) |
| SS-06 | Apply with undo; conflicts → Changes | History's Apply calls `WorktreeModel::stash_op(Apply, …)`: recorded, ⌘Z gives "Undone: Apply Stash" (`g-apply.png`, `g-undo.png`); `Some(true)` on conflicts switches to Changes | same |
| SS-07 | Pop with undo + toast | History's Pop calls `stash_op(Pop, …)` | same |
| SS-08 | Drop… confirm, undo + toast | History's Drop… asks `stash::drop_question`, then `stash_op(Drop, …)` | same |
| SS-09 | Copy stash message | History's row menu | intentional (History view) |
| SS-10 | Conflicted apply/pop: "Conflicts" banner, no Abort/Continue, merge editor | `stash_op` treats it as a stop, not an error; banner shows "Conflicts" without buttons when no operation | same |
| FI-01 | Blame toggle, `blame --porcelain`, reload on diff uid; deleted file: silent nothing | same gutter ("854a291 Alice Mock · 10 mo", "Not committed"), keyed by uid; no Blame/History for an unstaged deleted file. Shot `gpui-blame.png` | fixed-swift-bug (§14 Changes: blame silently does nothing for deleted files) |
| FI-02 | History panel open/close | same header (History, name, ×); shot `gpui-history.png` | same |
| FI-03 | `log --follow`, rename caption, "No History" | core `file_log`, same caption | same |
| FI-04 | Pick → bar (sha, subject, author · date, Back to the Change), commit diff | same; shot `gpui-history-pick.png` | same |
| FI-05 | Resize 260–700, `filePanelWidth`, double-click 340 | same | same |
| FI-06 | Compact + Full, tracked only, not for submodules/conflicts | same | same |
| DV-01 | Diff load, cancelled on reselect, kept when text unchanged | `WorktreeModel::load_diff` (Cancel + keep) | same |
| DV-02 | Spinner / No Changes / No File Selected | same | same |
| DV-03 | Header bar | Changes' own header (see CH-40) | same |
| DV-04 | Stage/Discard/Unstage actions | same | same |
| DV-05 | Line row layout | renderer, shots `gpui-start.png` vs `swift-main.png` | same (renderer) |
| DV-06 | Hunk header + buttons; discard alert | renderer buttons, Changes sheet | same |
| DV-07 | "\ No newline" meta rows | renderer | same (renderer) |
| DV-08 | Tabs → 4 spaces, 1500-char cut | renderer | same (renderer) |
| DV-09 | CRLF | renderer | same (renderer) |
| DV-10 | Intra-line | renderer; seen on main.go and app.yaml | same (renderer) |
| DV-11 | Syntax | `adapt::highlighter` in the code theme | same (renderer) |
| DV-12 | Binary File | renderer "Binary File" | same (renderer) |
| DV-13 | Non-LFS image → Binary File | same | same |
| DV-14 | Rename header old → new; blank table in Changes | header old → new; "No Content Changes" instead of the blank table | fixed-swift-bug (§14 Diff: blank Changes pane) |
| DV-15 | Large diffs open at once | virtual renderer | same (renderer) |
| DV-16 | Diff/Before/After, only with + and −, `diffSide`, reload whole | same control and key; hunk actions off in Before/After | fixed-swift-bug (§14 Diff: Stage/Discard in Before/After) |
| DV-17 | Scroll anchoring | renderer `set_file` | same (renderer) |
| DV-18 | Compact/Full | see CH-10 | same |
| DV-19 | Change ruler in Full | `set_change_ruler(true)` on the Full tab and a picked commit in Full | same |
| DV-20 | No split mode | Changes forces `Layout::Unified` | same |
| DV-21 | n / p | renderer keys | same (renderer) |
| DV-22 | v does nothing in Changes | no `on_mark_viewed` in Changes | same |
| DV-23 | ⌥↓ / ⌥↑ | see CH-16 | same |
| DV-24 | Click/drag selection | renderer | same (renderer) |
| DV-25 | Double/triple click | renderer | same (renderer) |
| DV-26 | ⌘A in the diff | renderer | same (renderer) |
| DV-27 | Copy | renderer | same (renderer) |
| DV-28 | Focus redraw | renderer | same (renderer) |
| DV-29 | ⌘-click go to definition | the Compact diff's `on_navigate` goes to the worktree's `CodeBrowser::navigate`; ⌘-click on `fetch_user` in router.rs opened db.rs at line 24 in place (`g-cmdclick.png`); a click on the list row, even the selected one, ends the peek (`g-endpeek.png`) | same |
| DV-30 | Diff context menu | renderer + Changes' hunk actions | same (renderer) |
| DV-31 | Reveal a line | Overview fact → `reveal` once the file's diff is in | same |
| DV-32 | ⌘F anywhere in the window | ⌘F opens find on the shown diff even with the list focused; shot `gpui-find.png` ("1 of 7") | same |
| DV-33 | ⌘G / ⇧⌘G / Esc | renderer | same (renderer) |
| DV-34 | Inline comments | not in Changes | same (n/a in Changes) |
| DV-35 | Add Comment | not in Changes | same (n/a in Changes) |
| DV-36 | Delete draft | not in Changes | same (n/a in Changes) |
| DV-37 | Blame gutter | see FI-01 | same |
| DV-38 | File History panel | see FI-02..05 | same |
| DV-39 | Editor preset | settings owner; diff follows `apply_code_theme` | same (settings) |
| DV-40 | Code themes | `adapt::apply_code_theme` | same (settings) |
| DV-41 | Font, size, line height | settings owner | same (settings) |
| DV-42 | Settings preview | settings owner | same (settings) |
| DV-43 | Diff colors | theme/renderer | same (renderer) |
| DV-44 | Markdown Preview (`markdownPreview`), reads the file from disk | same checkbox and key; hides Before/After; shot `gpui-sessions.png` (README.md) | same |
| DV-45 | Resize clamp | renderer | same (renderer) |
| LF-01 | Detection from .gitattributes / lfs/objects | core `LfsState::check` | same |
| LF-02 | "git-lfs isn't installed" banner, Copy Command, Dismiss (global) | same text, `lfsMissingBannerDismissed` | same |
| LF-03 | LFS object view Before → After, sizes, oids | same; shots `swift-lfs.png`, `gpui-lfs.png`. The headline has the drive icon (LFS.swift:242 `externaldrive`; `/tmp/flows-i-core/g-lf03.png`) | same |
| LF-04 | Thumbnails ≤ 720 px, "Not downloaded" | same in Compact; also in Full mode: Changes resolves the objects once and hands them to the browser (`CodeBrowser::set_images`), shown while that file is its open tab (`/tmp/flows-f-cleanup/g-lf04-full.png`: banner.png before/after in Full) | same |
| LF-05 | Locks once a minute per repository | core cache; the worktree that asked hands the answer to its siblings (`WorktreeModel::load_lfs_locks`; Changes' own copy of that hand-off is gone). Badges on hero.psd / banner.png in app-billing (`/tmp/flows-f-cleanup/g-billing.png`) | same |
| LF-06 | Lock badge with "Locked by <owner> (Git LFS)" | same; shot `gpui-billing-locks.png` vs `swift-billing.png` | same |
| LF-07 | Track *.ext / Track This File | same submenu | same |
| LF-08 | Lock/Unlock N Files, reload at once | same (`lfs_lock` then `load_locks`) | same |
| LF-09 | File History pick and PR full file show raw pointer text | File History pick shows the pointer as text too | same |
| SM-01 | Submodule pane: range, caption, N new commits, subjects | same; shots `swift-submodule.png`, `gpui-submodule.png` | same |
| SM-02 | Open Submodule (disabled when not checked out) | same, `add_repo` | same |
| SM-03 | Update to Recorded Commit, stranded-commits alert | same text in the in-app sheet | same |
| SM-04 | Stage Pointer / Unstage, prominence by behind | same | same |
| SM-05 | Menu items, Delete ignores submodules | same | same |
| SM-06 | "A submodule isn't initialized" banner, Not Now, Initialize | same; shots `swift-billing.png`, `gpui-billing.png` | same |
| SM-07 | Plain text diff outside Changes | not Changes' | same (n/a) |
| FT-01 | Folders first, natural order, chain compaction | core `file_tree` (unit test `filters_and_folds`) | same |
| FT-02 | Row look, 10 pt indent with guides | same | same |
| FT-03 | Click folds a folder | same | same |
| FT-04 | Tree menu: Expand/Collapse, Expand All, Collapse All | same in the Full tree | same |
| FT-05 | Single select, ⌘A swallowed | same in the Full tree | same |
| FT-06 | Hosts | Changes Compact + Full here; `tree::rows`/`dir_row`/`file_row_frame` are pub for History and PRs | same |
| FT-07 | PR filtering | `tree::rows(.., filter)` opens every folder while filtering | same |
| FT-08 | File icons | `ui::file_icon` | same |
| FT-09 | Splitter, `changesListWidth`, double-click reset | same (Changes default 300) | same |
| MS-15 | StatusIcon table | `tree::status_icon` (same kinds, colors, tooltips) | same |
| MS-16 | Porcelain v2 parsing | core `git::status` | same |

## Requests

- Done (i-core): `WorktreeModel::load_lfs_locks` hands its answer to the repository's other worktrees, so
  `ChangesView::share_lfs_locks` is now redundant and can go.
- Done by the views integrator: `AppEvent::ShowStashes` (SS-04), History's stash buttons through `stash_op`, ⌘Z in a
  text field (CH-31), the code browser and ChangeOverview (CH-09, CH-41/42, DV-29), the merge and PR banners (CH-08).
  GPUI shots of that run: `/tmp/flows-i-views/shots/`.

### Final pass (merged app): FT-01..09, MS-15..16

Driven in both apps on the flows mock (`/tmp/flows-z-shell/shots/`, see shell.md's final pass for the setup).
- FT-01, FT-02, FT-08, MS-15: same order, compaction, icons and status icons in Changes Compact on app-billing
  (`g-ft01.png`, `s-ft01.png`). Full mode: fixed (shared, views/code.rs) — the tree rows didn't span the pane, so
  status icons followed the name; now trailing as in Swift (`g-full.png`, `s-full.png`). GPUI also writes the LFS lock
  owner after the lock ("bob", "carol"); Swift shows the lock only (left to the code owner).
- FT-03: clicking "db" folded it in both (`g-ft03.png`, `s-ft03.png`).
- FT-04: Full-tree menu on "assets": Collapse / Expand All / Collapse All in both (`g-ft04.png`, `s-ft04.png`).
- FT-05, FT-06: same (PR #26/#32 Files tree with done/total per folder and "N of M files viewed", `g-pr26.png`, `s-pr26.png`).
- FT-07: "sql" filtered PR #26's tree to db/ and dev/test_data with folders open (`g-ft07.png`; Swift `s-ft07.png` on #32).
- FT-09, MS-16: same (code; not re-dragged).
