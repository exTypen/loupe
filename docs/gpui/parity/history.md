# History parity: Swift (LoupeFlows) vs GPUI

Mock: `FLOWS_PORT=8802 up.sh /tmp/flows-history`. Screenshots are in `/tmp/flows-history/shots/`: `g-*` (GPUI, this
session), `s-*` (Swift, this session), and `gpui-*` / `swift-*` (taken by the previous agent on the same fixture
before it was cut off; the code those flows run is unchanged since).

How it was checked: both apps side by side for the log, row content, graph, badges, scope menu, stashes, search,
detail header, diff header buttons and Markdown preview. The GPUI app alone for the rewrites, undo, the planner and
SH-39 (merge → History). Swift's rows there come from History.swift / InteractiveRebase.swift / Undo.swift and
FLOWS.md. Driving the Swift copy is unreliable while other agents run LoupeFlows: every copy shares the
`dev.extypen.loupe.flows` defaults, so another agent's tab switch or ⌥⌘F flips this window too (`s-now.png` shows my
window moved to Changes/Full by someone else). One stray click hit Stage All because of that. The fixture was
unstaged again right away.

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| HI-01 | Split list 340 / detail; search + scope menu; first commit auto-selected; `log -n 100`, `for-each-ref`, `rev-list --first-parent` (`s-hi01.png`) | Same layout (resizable 260–420, default 340), same three git calls, top commit selected (`g-hi01.png`) | same |
| HI-02 | `.task(id: "<head> <refsVersion>")`; same key → no git | Key `head_oid refs_version`; same key on return → no git process; leaving cancels loads | same |
| HI-03 | 26pt avatar (forge or one-letter initials), HEAD dot + semibold, short sha · author · relative date, refs line, tooltip | Same rows; avatars resolved one at a time and cached under `authorAvatars`; initials one letter on HSB(h, .45, .75) like Swift's Avatar | same |
| HI-04 | Canvas lanes 11pt, node at 21, accent main line, cycling colours, cap 12, none when filtered/stashes | Painted per row with the same geometry and palette; laid out off the main thread; re-laid per page (`g-sh39-merged.png`, `gpui-hi04-all.png`) | same |
| HI-05 | Capsule badges (HEAD→branch accent, tag orange, remote cloud, local green, other accent), "+N" with tooltip | Same kinds, icons and tints; selected row white on 20 % white | intentional: shows up to 3 badges then "+N" instead of measuring with ViewThatFits (`ponytail:` note in `ref_badges`) |
| HI-06 | Last row's onAppear loads the next 100; spinner row; `exhausted` below 100 | Last row's first draw loads the next page with `--skip=<count>`; spinner row; graph laid out again; stops when exhausted | same |
| HI-07 | ↑/↓ in the list, ⌥↓/⌥↑ move commit, clamped | ↑/↓ on the focused list, NextFile/PreviousFile (⌥↓/⌥↑) move the commit, clamped, scrolled into view (`g-hi07-keys.png`) | same |
| HI-08 | ⌘R always reloads (with unsubmitted search text) | `AppEvent::Refresh` reloads with the *applied* search only | fixed-swift-bug (§14 History #2) |
| HI-09 | Menu: Current, All, Local ▸, Remote ▸, divider, Stashes; label truncated to 17+…; tooltip "Which history to show: …" (`s-hi09-crop.png`) | Same items, order, truncation, icons and tooltip; branch scope falls back to Current when the branch is gone (`g-hi09-crop.png`) | same |
| HI-10 | "Search stashes"; stash subjects, no badges/graph; Drop… / Apply / Pop; header "merge of" with dead parent links (`s-hi10-stashes.png`) | Same list and buttons; no parent links on a stash (`g-hi10-stashes.png`) | fixed-swift-bug (§14 History #4) |
| HI-11 | Search on Return, cleared field reloads; `-i --grep --author`; "No Matches" | Same: typing does nothing until Return (`g-hi11-typed.png`), then `-i --grep=s --author=s` (`g-hi11-search.png`) | same. Note: git ANDs `--grep` and `--author`, so "carol" finds nothing in either app, as FLOWS.md HI-11 and §14 History #14 now say. A Swift bug, left as is for parity |
| HI-12 | Cancel previous detail; `show -s %B` + `show --diff-merges=first-parent`; stash `stash show -p -u`; file kept if present | Same calls through `core::history::detail` with a `Cancel`; spinner in the header; first file or the kept path | same |
| HI-13 | Subject, Markdown body (≤120pt), avatar · author · date · copy sha · parent/merge-of links · N files +/− | Same; parent link selects a loaded parent, tooltip "Parent not loaded yet — scroll the list to load more" otherwise | same |
| HI-14 | Files: tree (180–300) + DiffHeader (Blame, File History, Preview for .md) + diff / Binary / LFS / No Content; Overview switch; Compact/Full; Before/After whole file | Files tree, diff with Binary/LFS/"No Content Changes", Full and Before/After whole-file loads cached per "sha path" for the shown commit; Blame (at the commit's revision), File History (list on the left, picked commit's change with "Back to the Change") and Markdown Preview with the header kept (`g-hi14-header.png`, `g-hi14-blame.png`, `g-hi14-filehistory.png`, `g-hi14-preview.png`) | same; Overview switch: `ChangeOverview` of first parent → commit, not for stashes, "commitOverview" kept (`g-hiov.png`); a click opens that file at its line. File History logs from the commit, not HEAD: fixed-swift-bug (§14 History #11) |
| HI-15 | Header: stash Drop…/Apply/Pop; "Cherry-pick onto <b>" when not in HEAD; else "Revert" | Same buttons and tooltips (`g-sh39-merged.png` shows Cherry-pick on origin/main) | fixed-swift-bug (§14 History #10): header says "Revert Commit" like the menu |
| HI-16 | Menu: Cherry-pick/Revert Commit, rebase section (current, no search), Copy SHA, Copy Subject, New Worktree from Here…, Open in Browser; stash: Pop, Apply, Drop…, Copy Message | Same items and order (`gpui-rb02-menu.png`) | same |
| HI-17 | `cherry-pick [-m 1]`; undo "Cherry-pick of <short>"; conflicts → Changes | `core::history::cherry_pick` through `perform` (busy, error alert, undo); a stop on conflicts switches to Changes | same |
| HI-18 | `revert --no-edit [-m 1]`; undo "Revert of …"; conflicts → Changes | Same through `perform` | same |
| HI-19 | Apply/Pop/Drop; Drop… confirm "Drop stash@{n}?" / "Drop Stash"; toasts for Pop/Drop | Same commands, same dialog text, toast with Undo; list reloads (`gpui-hi19-drop.png`, `gpui-hi19-dropped.png`) | same |
| HI-20 | Sheet New/Existing branch, Name, Based on (HEAD, sha10, branches), Location + Choose…, inline error | "New Worktree from Here…" emits `AppEvent::NewWorktree { repo, base }`; the shell's sheet opens based on the commit (`g-newwt.png`: Based on d213fb3f11) | same |
| HI-21 | Fetch `<remote> <base>`, scope `<remote>/<base>`, merge selected | `ShowBranch` → `remote_op("Fetching main")` → scope origin/main, top commit (the merge of PR #22, merged on the mock) selected, graph drawn (`g-sh39-merged.png`) | same |
| HI-22 | Blank list on first load (no spinner); No Commits / No Matches / No Stashes; "Git" alert | Spinner on first load; same empty states; failures go to the worktree's "Git" alert; cancellations are silent | fixed-swift-bug (§14 History #12) |
| SH-39 | PR merge → fetch base, History on `<remote>/<base>`, merge selected | Pulls fetches the base and emits `ShowBranch`; History shows `<remote>/<base>` with the merge selected, without a second fetch (`g-merged.png`: #38 merged, origin/main, merge selected) | same |
| RB-01 | `rev-list --first-parent --parents -n 1000 HEAD` until a non-2-id line | Same, plus it records the root so the commit above it can meld into it (`--root`) | same |
| RB-02 | Reword…, Edit, Squash/Fixup into Parent, Move Up/Down, Drop…, Interactive Rebase…; enabled below a merge, failing later | Same items (`gpui-rb02-menu.png`), but enabled only for commits a rebase can reach | fixed-swift-bug (§14 History #5) |
| RB-03 | Drag: top/bottom 30 % above/below, middle squash (⌥ fixup); accent line / bordered "Squash"/"Fixup" | Same zones, indicator and labels; non-rewritable rows don't drag (`gpui-rb03-onto.png`, `gpui-rb03-after.png`) | same |
| RB-04 | Reword sheet; Return inserts a newline; only a click submits | Same sheet; ⌘↩ submits, Return is a newline (`gpui-rb04-sheet.png`) | fixed-swift-bug (§14 History #9) |
| RB-05 | `edit <sha>`; stops → Changes banner | Same todo line; a stop switches to Changes (the banner is the changes owner's) | same |
| RB-06 | Range from the parent; squash joins messages, fixup keeps the parent's | Same through `rebase::with_action` | same |
| RB-07 | Move Up swaps with the row above; Move Down with the parent | Same (`rebase::move_item` with Above/Below) | same |
| RB-08 | "Rewrite a pushed commit?" … "Rewrite Anyway"; drop-only "Drop “subj”?" | Same titles, messages and buttons (`g-rb08-pushed.png`, `gpui-rb09-drop.png`) | same |
| RB-09 | `drop <sha>`, always confirmed | Same | same |
| RB-10 | Planner sheet, per-row action, pushed cloud, editors for Reword/Squash, warning footer, Start skips both confirmations | Same sheet (`g-rb10-planner.png`); Start skips the pushed confirmation (the footer lists them), but drops are still confirmed, multi-drops included | fixed-swift-bug (§14 History #7) |
| RB-11 | Autostash rebase with a temp todo; undo `.reset(label, from: head)`; refused while an operation runs | `rebase::execute` through `perform`; one at a time; failure → alert + reload; stop → Changes (`gpui-rb11-dropped.png`). The dead `Rewrites` record isn't ported | fixed-swift-bug (§14 History #3) |
| RB-12 | Range errors as a "Git" alert | Same core errors → the worktree's "Git" alert | same |
| RB-13 | Conflict → Changes banner, Continue/Abort | A stop switches to Changes; the banner, Continue and Abort belong to the changes owner | same |
| UN-01 | ⌘Z undoes the newest entry, toast "Undone: <label>", reloads | ⌘Z (shell, or History's handler) runs `undo_last`; toast "Undone: Drop" (`gpui-un01.png`, `gpui-un05.png`) | same |
| UN-02 | Redo only in text fields | Nothing in History; text inputs keep their own redo | same |
| UN-03 | Toast with Undo after Pop/Drop, "Undone: …" after an undo, 6 s | The shell's toast, same texts (`gpui-un03-undone.png`) | same |
| UN-04 | "Can't Undo … already on <remote>"; Revert Commit / Cancel; that revert records no undo | Same alert from History's ⌘Z (`undo_offering_revert`); its Revert Commit goes through `perform` and records an undo | fixed-swift-bug (§14 History #6) |
| UN-05 | `reset --keep <before>`; labels "Undo Reorder Commits" etc. | Core's undo recipes; labels from the action | same |
| UN-06 | Continue records "Rebase"/"Cherry-pick"/… from orig-head | Core (`finish_operation`), driven from the Changes banner | same |
| UN-07 | Stash apply/pop/drop snapshot and restore, refusal if changed | Core's stash undo recipes | same |
| UN-08 | 50 entries, pruned on head change, memory only | Core's `UndoStack` | same |

Performance: idle CPU with History open is 0.0 % (`ps` twice, 10 s apart: 0:00.39 → 0:00.39 on main, and on
app-router 0:00.98 → 0:01.02). Nothing polls. Leaving the tab or the worktree cancels the list, detail, whole-file
and Blame/File History/Markdown loads. Whole files are cached per "sha path" for the commit showing only, and the
list per worktree keyed by "HEAD refsVersion".

## Final pass (merged app, branch gpui-z-history)

Both apps on one mock (port 8834, `/tmp/flows-z-history`), real clicks and keys (CGEvent clicks; a cliclick
down/up pair does not reach gpui buttons), screenshots in `/tmp/flows-z-history/shots/` (`g-*` GPUI, `s-*` Swift).

| ID | Final pass (merged app) |
|---|---|
| HI-01 | same: list, scope menu, top commit selected (`g-hist.png` / `s-hist2.png`) |
| HI-02, HI-06, HI-08, HI-12, HI-13, HI-20, HI-22 | same as the table above; no regression seen where views meet |
| HI-03..05 | same rows, graph, badges, "+N" (`g-hi01.png`, `g-hi09-all.png`, `s-hi09-all-later.png`) |
| HI-07 | same |
| HI-09 | same items and order (`c-g-hi09-menu.png` vs `c-s-hi09-menu.png`, `g-menu2.png` vs `s-menu.png`) |
| HI-10 | same; Changes' "3 stashes →" opens History on Stashes in both (`g-hi10.png` / `s-hi10.png`) |
| HI-11 | same: "readme" + Return → No Matches in both (git ANDs --grep/--author); clearing reloads (`g-hi11-search.png`, `s-hi11-search.png`, `g-hi11-cleared.png`) |
| HI-14 | same; Overview of a commit (`g-hi14-overview.png`) |
| HI-15, HI-17 | same: All Branches → "Cherry-pick onto main" → conflict → Changes with the "Cherry-picking …" banner in both (`g-hi17-abort.png`, `s-hi17b.png`) |
| HI-16 | **fixed**: the row's hover tooltip covered the menu's first items (Revert, Reword…, Edit) (`g-rb02.png` before, `g-rb02-notip.png` after) |
| HI-18 | same: Revert Commit makes `Revert "…"`, ⌘Z → "Undone: Revert of fd12ba38" (`g-hi18-after.png`, `g-un01.png`) |
| HI-19 | **fixed**: after Drop Stash the first ⌘Z did nothing (the dialog left nothing focused, so no key binding fired); dialogs now hand focus back to History. Drop → ⌘Z → 3 stashes again (`g-hi19-drop.png`, `g-un07b.png`) |
| HI-21 | same: merge of #33 (stress `mergepr`): `ok=yes tab=history scope=origin/main`, merge on top and selected (`g-hi21-history.png`) |
| RB-01, RB-02 | same items and enablement (`g-rb02-notip.png` vs `s-rb02.png`; Swift's menu shot caught after it closed) |
| RB-07, UN-05 | same: Move Up on WIP reordered (git log), ⌘Z → "Undone: Move Commit Up" (`g-un05.png`) |
| RB-08 | same: "Rewrite 2 pushed commits?" … Rewrite Anyway / Cancel; Cancel leaves HEAD (`g-rb08.png`) |
| RB-03..06, RB-09..13 | not re-driven in the merged app; the code paths are unchanged since the per-view pass |
| UN-01, UN-03 | same toasts: "Undone: …", "Drop Stash  Undo" (`g-un07.png`) |
| UN-06 | same: Continue after resolving records "Cherry-pick"; ⌘Z → "Undone: Cherry-pick" (`g-un06.png`) |
| UN-07 | fixed with HI-19 (focus) |
| UN-02, UN-04, UN-08 | unchanged since the per-view pass |

Idle CPU with History open (app, origin/main after the merge): `0.0 0:00.93` twice, 10 s apart.

## Final pass, remaining IDs (merged app, branch gpui-z-hist2)

Both apps side by side on one mock (port 8861, `/tmp/flows-z-hist2`, `scen-x.json` = scenario.json plus the extra
conflict repos), real CGEvent clicks/drags and System Events keys, git calls read from GIT_TRACE2_EVENT of each app.
Screenshots in `/tmp/flows-z-hist2/shots/` (`g-*` GPUI, `s-*` Swift, `*-c.png` crops). Every HI/RB/UN ID now has a
final-pass result (this table plus the one above).

| ID | Final pass (merged app) |
|---|---|
| HI-02 | same: first visit runs log/for-each-ref/rev-list/show ×2 in both; Changes → History again runs no git in either (`g-hi02-b.png` / `s-hi02-b.png`) |
| HI-06 | same: scrolling to the end loads `--skip=100` then `--skip=200` in both, graph continues (`g-hi06-crop.png` / `s-hi06-crop.png`) |
| HI-08 | fixed-swift-bug (§14 History #2): "readme" typed without Return, ⌘R: Swift's log carries `-i --grep=readme --author=readme`, GPUI's reload has no search (`g-hi08.png` / `s-hi08.png`) |
| HI-12 | same: click a row → `show -s --format=%B` + `show --diff-merges=first-parent <sha>` in both, nothing else (`g-hi12.png` / `s-hi12.png`) |
| HI-13 | same: merge row shows "merge of" with two links, root shows no parent section, copy button puts the full SHA on the pasteboard, parent link selects the parent (`g-hi13-merge-c.png`, `g-hi13-root-c.png`, `g-hi13-copy-crop.png` vs `s-*`) |
| HI-20 | same: "New Worktree from Here…" → sheet based on the commit (sha10), name typed, Create → `git worktree add -b`, new worktree selected in the sidebar; both made the worktree at the clicked commit (`g-hi20-sheet.png`, `g-hi20-created.png` / `s-hi20-sheet.png`, `s-hi20-created.png`) |
| HI-22 | same: lib + Stashes → "No Stashes" in both (`g-hi22-nostash-c.png` / `s-hi22-nostash-c.png`). Swift's app History on Stashes showed "0 files / No File Changes" after coming back to the worktree (`s-rb12.png`), GPUI reloads the files: fixed-swift-bug (§14 History #1) |
| RB-03 | same on drop: dragging "remove the dead /legacy route" below "table-driven routes" in GPUI ran the reorder (`rebase -i … 8906108`) and stopped on the expected conflict (`g-rb03-drop.png`). Swift's List drag could not be started with synthetic CGEvent or cliclick drags (`s-rb03-drop.png`), so Swift's reaction was taken from the same reorder through Move Down (identical todo, identical conflict) |
| RB-04 | **fixed**: (1) the caret started at the top of the message, so typing went before the subject; it now starts after the message, as in Swift (`g-rb04-sheet2-c.png` vs `s-rb04-sheet.png`); (2) ⌘Z right after the sheet closed did nothing (focus stayed on the dropped editor) and the HEAD row kept a stale "HEAD" badge: both fixed (see UN-05). Reword itself same: `tests: cover the route table (v2)` + body in both; ⌘↩ vs click: fixed-swift-bug (§14 History #9) |
| RB-05 | same: Edit (Stop Here) → Changes, blue "Rebasing … Stopped to edit this commit" banner, Abort Rebase restores the branch in both (`g-rb05.png` / `s-rb05.png`) |
| RB-06 | same: Squash into Parent on the `squash!` commit, no sheet; both made "router: table-driven routes\n\nsquash! router: table-driven routes", ⌘Z restores 764f66e |
| RB-09 | same: dialog "Drop “tests: cover the route table”?" with the reflog text, Drop Commit; both dropped it; ⌘Z → "Undone: Drop" (`g-rb09-dialog.png`, `g-rb09-undo-c.png` / `s-*`) |
| RB-10 | same planner: "7 commits onto f8e95f13…", per-row action menus, cloud on pushed rows, orange "Rewrites 1 pushed commit: f8e95f1", Start Rebase; Squash on the bottom row → "has no older commit left to squash into", Start disabled (`g-rb10.png`, `g-rb10-sqd-c.png` / `s-rb10.png`, `s-rb10-pick-c.png`); drops still confirmed in GPUI: fixed-swift-bug (§14 History #7) |
| RB-11 | same: Move Down with an unstaged edit in main.rs: both rebased with `--autostash`, the edit came back, no stash left behind; ⌘Z restored 764f66e with the edit kept |
| RB-12 | fixed-swift-bug (§14 History #5): on the commit below "Merge pull request 'Batch invoice totals' (#20)", Swift enables Reword… and fails after the sheet with the "Git" alert "The commits to rewrite include the merge 568bf2bf … (git rebase -i --rebase-merges)" (`s-rb12-alert.png`); GPUI disables the rewrite items there (`g-rb12-c.png`) |
| RB-13 | **fixed**: two Move Downs (legacy below table-driven) stop on `UU src/server/router.rs` in both, app on Changes with the orange "Rebasing …" banner and the editor (`g-rb13b.png` / `s-rb13.png`). Clicking Abort Rebase in the GPUI editor's banner also hit the Changes banner's Abort underneath (two `rebase --abort`, the second failing on index.lock → "Git" alert); overlays now occlude the mouse. Use Your Commit + Continue finished the rebase in both, ⌘Z → "Undone: Rebase" |
| UN-02 | same: ⇧⌘Z in History does nothing in either |
| UN-04 | same: Revert Commit, push, ⌘Z → "Can't Undo" "… is already on origin/feature/fresh. Undoing it would rewrite pushed history. Revert it instead…" with Revert Commit / Cancel in both (`g-un04.png` / `s-un04.png`); its revert records an undo in GPUI: fixed-swift-bug (§14 History #6) |
| UN-05 | **fixed** with RB-04: ⌘Z right after Reword/the planner now undoes ("Undone: Reword", `g-un05-reword3-c.png`) as Swift does (`s-un05-reword.png`); Squash into Parent, Drop, Move Down, autostash Move Down also undone in both |
| UN-06 | same: Continue after the RB-13 conflict, ⌘Z → "Undone: Rebase" in both (`s-un06-c.png`) |
| UN-07 | same: Pop of the spike stash → toast "Pop Stash  Undo", ⌘Z → "Undone: Pop Stash", 3 stashes and the same two changed files again (`g-un07-pop-c.png`, `g-un07-undo-c.png` / `s-un07-pop-c.png`). Popping the changelog stash fails in both with git's "would be overwritten" alert (config/app.yaml) |
| UN-08 | same: a commit made in a terminal on fresh, then ⌘Z: the entry was pruned in both, nothing happens, HEAD stays (`g-un08.png` / `s-un08.png`) |

Swift-only reload detail seen on the way: Swift's History selection at the GPUI-equivalent spot can end on another
row after a rewrite; not a parity item.
