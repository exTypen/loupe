# Merge editor and conflicts: Swift vs GPUI parity (MG-01..26)

Both apps ran on the flows mock (port 8805, `/tmp/flows-merge`). The flows fixture has only one stopped merge
(`app-sessions`) and one cherry-pick, so `scripts/flows/merge-fixture.sh` (mkflows.py runs it) adds one small repo per stopped
state under `<out>/repos/extra`: `merge-plain` (merge style, 3 conflicts in `notes.txt` with merged regions, a CRLF
file, a no-final-newline file, `merge.guitool` set to a stub tool), `merge-diff3` (diff3 style, `merge.tool=vimdiff`),
`rebase`, `cherry`, `revert`, `resolved`, `outside` (edited after the merge stopped) and `special` (add/add,
delete/modify both ways, added by us/them after a rename, both deleted, binary, symlink, submodule).

How it was checked:

- **GPUI:** `LOUPE_FLOWS_MERGE=<path>` selects that conflicted file (which opens the editor) and
  `LOUPE_FLOWS_MERGE_DO=<steps>` presses its buttons in turn, 700 ms apart (`ours`, `theirs`, `ot`, `to`, `base`,
  `restore`, `next`, `prev`, `unresolved`, `showbase`, `pick:<line>`, `picktheirs:<line>`, `type:<text>`, `suggest`,
  `accept`, `reset?`, `reset`, `tool`, `resolve`, `useours`, `usetheirs`, `keep`, `delete`, `sub:<stage>`, `abort`,
  `continue`, `close`, and `key:<keystroke>`, which goes through the window's key dispatch as a real key would).
  The result was read back from the file and from `git status`, and screenshotted.
- **Swift:** LoupeFlows (prebuilt at `~/loupe` fde8251) with StressDriver's `merge` step on each extra repo, which
  opens a conflicted file in the merge editor; screenshots of each. The screen was locked for the whole run
  (`loginwindow` frontmost), so no real mouse or keyboard input could reach either app: Swift's reactions below are
  read from MergeEditor.swift / Conflicts.swift / Model.swift (FLOWS.md §7) and its renders from the screenshots.
- `LOUPE_FLOWS_AI=<path>` (read by `loupe_forge::ai::scan` in scenario runs) points GPUI at the stub Claude mkflows.py plants in `<out>/home/.local/bin/claude`
  (scenario.json `fixture.ai`; up.sh prints the export; an `<out>/ai-empty` file makes it answer nothing), because `/opt/homebrew/bin/claude`
  is found first in both apps (and isn't logged in under the fixture's HOME).

Screenshots are in `/tmp/flows-merge/shots/` (`s-*` Swift, `g-*` GPUI).

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| MG-01 | Banner above Changes: orange triangle, "Merging 11b706e topic: rewrite the notes", "3 files with conflicts. Resolve each one, then continue.", orange 8% (s-merge-plain, s-cherry, s-revert, s-rebase) | `merge::banner(model)` renders the same icon, title, subtitle and tint (`theme::orange()`, `/tmp/flows-f-cleanup/g-sessions.png`); shown on top of the merge editor (g-merge-plain, g-cherry, g-revert, g-rebase). The Changes owner places it above the status list | same (above Changes: `g-sessions.png`, and on top of the merge editor) |
| MG-02 | "Abort Merge" / "Abort Cherry-Pick" / … → `<op> --abort` via perform("Aborting") | "Abort Merge" / "Abort Cherry-pick" / …, help "Go back to how things were before the <op>"; `finish_operation(true)`: cherry-pick aborted, worktree clean, no CHERRY_PICK_HEAD (verified on `cherry`) | fixed-swift-bug (§14 "Inconsistent casing for cherry-pick") |
| MG-03 | "Continue" prominent, disabled with "Resolve the conflicts first" while conflicts remain; `<op> --continue` with GIT_EDITOR=true | Same button, help and gating (disabled in every editor shot). `rebase`: all three files resolved, then Continue → "On branch topic, nothing to commit", rebase finished; `special`: last conflict resolved, Continue → merge commit "Merge branch 'topic'" | same |
| MG-04 | Selecting a file under Conflicts shows ConflictPane in the right pane, a spinner until loaded; ls-files -u, stage blobs, binary/symlink check, Merge3, focus first conflict | Selecting a conflicted file (the Opener follows the selection, or `AppEvent::OpenMergeEditor`) presents `MergeEditor` over the window; "Loading…" until `session::load` (same steps, off the main thread) returns; first conflict focused | intentional (an overlay, as VIEWS.md assigns: the editor is presented with `AppModel::present`, not embedded in Changes' right pane; Esc or × closes it, and it opens again when a conflicted file is next selected) |
| MG-05 | Header: triangle, path, nav, Use Your Branch / Use Incoming / Reset… / Merge Tool / Mark Resolved. Ours (blue dot) · Theirs (purple dot), detail = branch / subject; labels per op: merge Your Branch/Incoming, rebase Upstream "what you're rebasing onto"/Your Commit, cherry-pick Your Branch/Cherry-picked, revert Your Branch/Reverted "the code before …". Tints: ours blue, theirs purple, both teal, conflict orange, current darker | Same header, panes, labels and details for all four operations (g-merge-plain, g-rebase, g-cherry, g-revert vs s-*). Tints: blue / purple 13%, picked 32%, conflict `theme::orange()` 16%, current 30%, resolved green, "both" `theme::teal()` 12%; the header triangle and the conflict page icon are orange (`/tmp/flows-f-cleanup/g-merge.png`). The AI (Suggest) comes from `ai::installed().unwrap_or_else(ai::scan)`, which honours LOUPE_FLOWS_AI in scenario runs | same |
| MG-06 | "Base" toggle in the Ours header shows Base between Ours and Theirs; per session | Same toggle and help; Base pane with gray dot and "the common ancestor", changes grey, conflicts amber (g-pick-type) | same |
| MG-07 | ⌃ ⌄ (⌥⌘↑ / ⌥⌘↓), "Conflict X of N · K left" / green "all resolved"; wraps; scrolls result to ¼, panes to the chunk; caret moves the current conflict | Same controls and text; `key:alt-cmd-down` and `next`/`prev` steps move and wrap (g-takes "Conflict 3 of 3 ✓ all resolved"); panes scroll to the chunk; caret in a conflict makes it current | same |
| MG-08 | "Next Unresolved" while any is unresolved, next one with markers, wrapping | Same button, hidden once all resolved (g-takes); unit test `navigation_wraps_and_finds_unresolved` | same |
| MG-09 | Chunk bar "Result · Conflict N", Use Ours (⌥⌘←) / Use Theirs (⌥⌘→) / Ours, Then Theirs / Theirs, Then Ours / Use Base / Restore; written at once, undoable. Swift's chunk-built conflicts always carry a base, so the result shows "\|\|\|\|\|\|\| base" sections git never wrote (s-merge-plain) and Use Base is always enabled | Same buttons and shortcuts (`key:alt-cmd-right` took theirs); written at once; resolved region turns green. The result keeps git's own markers: no base section unless git wrote diff3 (then git's label, e.g. "\|\|\|\|\|\|\| 2a8dba0"), so the first write changes only what was resolved; Use Base disabled when there's no base (both added) | fixed-swift-bug (§14 "Merge editor adds base sections git did not write") |
| MG-10 | Click a line in a conflict of Ours/Theirs toggles its pick; first-picked side first; picks tinted; unpicking all restores markers; Base clicks ignored | Plain click (no drag, no selection) on Ours/Theirs picks the line through `Session::click`; same ordering, tints and restore (g-pick-type: picked "one" from Incoming); unit test `picking_lines_builds_the_conflict_from_them` | same (click itself not driven: screen locked; the hook's `pick:` runs the same path after the click handler) |
| MG-11 | Typing in the result: regions follow, 350 ms debounced atomic write keeping permissions; no final newline kept off | Same: `moved()` is Swift's `moved` (empty resolved regions take text typed into them), 350 ms debounce, temp file renamed over the original with its mode (`type:` step, file read back) | same |
| MG-12 | ⌘Z / ⇧⌘Z in the result undo typing and takes (NSTextView undo); elsewhere the app's undo | ⌘Z after "Use Your Branch" brought the markers back (3 → 2 → 3); typing "abc", ⌘Z, ⇧⌘Z → "abc" again. Outside the result ⌘Z stays the worktree undo | same |
| MG-13 | "Mark Resolved" prominent, disabled while markers are left; flush, `git add`, file leaves Conflicts, its staged entry selected | Same gating and help; after three takes → `M  notes.txt` staged, editor closed, Changes shows the staged notes.txt selected (g-resolved) | same |
| MG-14 | Header Use Ours/Theirs: drop pending write, checkout --ours/--theirs + add (git rm if that side deleted) | Same (`WorktreeModel::resolve`); `useours` on `link`, `usetheirs` on `logo.png`, `useours` on both-deleted `renamed.txt` (falls back to git rm) all staged | same |
| MG-15 | "Reset…" → dialog "Bring back the conflicts in notes.txt?" / "Your edits to the result are lost." / Reset File; checkout -m, then load twice | Same dialog (g-reset-dialog); after two takes, Reset File → 3 conflicts again with checkout -m's "ours"/"theirs" labels (g-reset-done); one load | fixed-swift-bug (§14 "Reset… loads twice") |
| MG-16 | "Merge Tool" enabled with merge.guitool (or a non-terminal merge.tool); `git mergetool --no-prompt [--gui]`, then reload | `merge-plain` (guitool stubtool): tool ran, notes.txt staged by mergetool, editor closed; `merge-diff3` (vimdiff): disabled with the "Set merge.tool…" help (g-diff3-tool) | same |
| MG-17 | "Suggest" (sparkles) when an AI tool is set up; spinner; popover "Suggested merge" 520×260, caption, Discard / Use Suggestion (Return); empty reply shows "(nothing)" and resolves the conflict to nothing | Same button, prompt (path, 20 lines around, base or "(not available)", ours, theirs), cleanup and card (g-suggest); Return or Use Suggestion replaced the conflict it was asked for (g-suggest-accept, "merged by AI: both changes"); Esc discards. An empty reply is an error alert "Claude Code suggested nothing for this conflict." (g-ai-empty) | fixed-swift-bug (§14 "An empty AI merge suggestion deletes the conflict"). Swift's popover itself not run: its real `/opt/homebrew/bin/claude` isn't logged in under the fixture HOME, the same "Not logged in" alert GPUI shows without the stub |
| MG-18 | File edited outside: footer "This file was changed outside Loupe, so only its remaining conflict markers are tracked."; reload on status change when file or stages differ. Swift's trimmed merge never matches a diff3 file either, so `merge-diff3` also opens in this mode (s-merge-diff3) | Same footer on `outside` (g-outside vs s-outside); appending to notes.txt while open reloaded it and showed the footer (g-outside-reload). For diff3 files GPUI also tries the untrimmed merge, which matches git: full editor, merged regions tinted, git's base sections kept (g-diff3-tool) | same; diff3: fixed-swift-bug (§14 "Merge editor adds base sections git did not write", same cause) |
| MG-19 | "added on both sides", one conflict, trimmed | `both.yaml`: "added on both sides", one conflict, Use Base disabled; theirs + Mark Resolved staged it (g-sp-both.yaml) | same (Use Base: fixed-swift-bug, §14) |
| MG-20 | Trash icon, "Deleted on Your Branch, changed on Incoming" / "Changed on Your Branch, deleted on Incoming", detail, survivor pane, Keep File / Delete File; header still has Use Ours / Use Theirs / Reset… (s-special) | Same icon, titles, detail, survivor pane and buttons (g-sp-delus.txt, g-sp-delthem.txt); Keep File kept delthem.txt, Delete File removed delus.txt. Header has no Use/Reset buttons | fixed-swift-bug (§14 "Duplicate buttons on special conflicts": Reset… can't bring a delete/modify back) |
| MG-21 | "Only on Your Branch" / "Only on Incoming", detail, Keep File / Delete File | Same (g-sp-renamed-ours.txt, g-sp-renamed-theirs.txt); keep and delete both resolved | same |
| MG-22 | "Binary file conflict", "Keep one side: Your Branch 13 bytes, Incoming 22 bytes.", Use buttons; header duplicates them | Same title, sizes and buttons (g-sp-logo.png); Use Incoming staged it; no duplicate header buttons | fixed-swift-bug (§14 duplicate buttons) |
| MG-23 | "Symbolic link conflict", "The two sides point the link at different targets. Keep one side." | Same (g-sp-link); Use Your Branch → link points at target-ours, staged | fixed-swift-bug (§14 duplicate buttons) |
| MG-24 | Shippingbox, "Submodule conflict", "Use Your Branch · 31d64714" / "Use Incoming · 57bd08d9", update-index --cacheinfo | Same (g-sp-lib); `sub:3` recorded lib at 57bd08d9 | same |
| MG-25 | CRLF kept per line, "=======\r" recognized; no final newline when neither side had one | `crlf.txt` opens in the full editor (not "changed outside") with git's CRLF markers kept (g-crlf); Use Your Branch wrote `a\r\nB ours\r\nc\r\n`; `nonl.txt` after Use Your Branch is `x\ny ours` with no newline | same |
| MG-26 | onDisappear flushes the pending write | Closing (×, Esc, another tab or worktree, the file resolved) flushes the pending write; release flushes too; the load task is dropped with the view | same |

Performance: with the editor open on `merge-plain`, `ps -o %cpu=,time=` read `0.0 0:00.34` twice, 10 s apart. Nothing
polls: the only timer is the 350 ms write debounce; reloads come from the worktree's status version, and the load and
AI tasks are dropped with the view.

## Final pass (merged app, branch gpui-z-history)

Same mock as the History final pass (port 8834, `/tmp/flows-z-history/shots/`). Real clicks and keys.

| ID | Final pass (merged app) |
|---|---|
| MG-01, MG-02, MG-03 | same: cherry-pick of 5e64fe0 onto app main stopped; banner "Cherry-picking 5e64fe0 billing: add ComputeToken" in both; Abort Cherry-pick cleaned it up; after resolving, Continue made the commit (`g-hi17-abort.png`, `s-hi17b.png`) |
| MG-04 | **fixed**: the panes were plain text; Swift highlights all four (`s-mg04.png`). They now use the code theme's tokens (loupe_code tokenizer, as the diff does), also after a take (`g-mg05-hl.png`, `g-mg09-take.png`) |
| MG-05 | **fixed**: a side with no lines in a conflict showed nothing in its pane; it now draws Swift's 2 pt bar where the conflict sits (Your Branch pane of invoice.go, `g-mg05-hl.png`) |
| MG-07, MG-08 | same: "Conflict 1 of 1 · 1 left" → "all resolved" |
| MG-09 | same: Use Cherry-picked resolved it and wrote the file at once; ⌥⌘→ took theirs on `crlf.txt` |
| MG-13 | same: Mark Resolved → `M  services/billing/invoice.go` staged |
| MG-25 | same: crlf.txt after ⌥⌘→ is `a\r\nB theirs\r\nc\r\n` |
| other MG | unchanged since the per-view pass |

Idle CPU with the editor open on `merge-plain`: `0.0 0:00.29` twice, 10 s apart.

## Final pass, remaining IDs (merged app, branch gpui-z-hist2)

Same mock and method as history.md's last table (port 8861, `/tmp/flows-z-hist2/shots/`, real clicks and keys in
both apps; `special` restored from a copy between the two apps). Every MG ID now has a final-pass result.

| ID | Final pass (merged app) |
|---|---|
| MG-04 | **fixed** (follow-up to the earlier pass): selecting a worktree whose selected file is conflicted showed GPUI's "Open Merge Editor" placeholder until the next status refresh; Swift shows the editor at once. The Opener now checks the selection on arrival (`g-mp.png` / `s-mp.png`) |
| MG-06 | **fixed**: Base toggle shows the Base pane "the common ancestor" between the sides in both (`g-mg06.png` / `s-mg06.png`); its on state is now the accent fill Swift's selected toggle has (it was a faint grey) |
| MG-10 | same: click "ONE ours" in Your Branch, then "two theirs" in Incoming → result `ONE ours` / `two theirs`, picked lines tinted, "· 2 left" (`g-mg10-c.png` / `s-mg10-c.png`) |
| MG-11 | same: typing "XY" at the end of the first result line wrote `headerXY` to disk after the debounce in both |
| MG-12 | same: ⌘Z → `header`, ⇧⌘Z → `headerXY` in both |
| MG-15 | same: "Bring back the conflicts in notes.txt?" / "Your edits to the result are lost." / Cancel, Reset File; Reset File → `header` and 3 conflict blocks again in both (`g-mg15.png` / `s-mg15.png`) |
| MG-16 | same: Merge Tool (stub guitool) → notes.txt staged, `notes.txt.orig` left untracked, Changes shows the staged diff in both (`g-mg16.png` / `s-mg16.png`); disabled on `outside` (no tool) in both |
| MG-17 | same render in GPUI (popover "Suggested merge", "merged by AI: both changes", caption, Discard / Use Suggestion, `g-mg17.png`); Swift can only reach the real `/opt/homebrew/bin/claude`, which answers "Not logged in · Please run /login" under the fixture HOME (`s-mg17.png`), as recorded in the per-view pass |
| MG-14 | same: header Use Your Branch/Use Your Commit on router.rs and on `special`'s both-deleted `renamed.txt` (git rm fallback) leave the same `git status` in both |
| MG-18 | same: `outside/notes.txt` opens with "header edited in vim" and the footer "This file was changed outside Loupe, so only its remaining conflict markers are tracked." in both (`g-mg18.png` / `s-mg18.png`) |
| MG-19 | same: both.yaml "added on both sides", one conflict, ⌥⌘→ + Mark Resolved → `M  both.yaml` = `flag: theirs`, staged entry selected in both; Use Base disabled in GPUI / base section shown in Swift: fixed-swift-bug (§14 "Merge editor adds base sections git did not write") |
| MG-20 | same: delthem "Changed on Your Branch, deleted on Incoming", delus the reverse, trash icon, survivor pane; Keep File / Delete File; header Use/Reset only in Swift: fixed-swift-bug (§14 duplicate buttons) (`g-sp-delthem-c.png` / `s-sp-delthem-c.png`) |
| MG-21 | same: "Only on Your Branch" / "Only on Incoming" with the rename text; Keep and Delete (`g-sp-rours-c.png` / `s-sp-rours-c.png`) |
| MG-22 | same: "Binary file conflict … Your Branch 13 bytes, Incoming 22 bytes.", Use Incoming staged logo.png (`g-sp-logo-c.png` / `s-sp-logo-c.png`) |
| MG-23 | same: "Symbolic link conflict", Use Your Branch → link → target-ours (`g-sp-link-c.png`) |
| MG-24 | same: "Submodule conflict", "Use Your Branch · 31d64714" / "Use Incoming · 57bd08d9"; Use Incoming recorded lib at 57bd08d9 in both (`g-sp-lib-c.png` / `s-sp-lib-c.png`) |
| MG-20..24 reactions | the same seven clicks in each app (Keep, Delete, Keep, Delete, Use Your Branch, Use Incoming ×2) left byte-identical `git status --short` (`st-g.txt` = `st-s.txt` in `/tmp/flows-z-hist2`) |
| MG-26 | same: typing and switching worktree or pressing Esc within 350 ms still wrote the edit (`headerW` / `elevenW` on disk) in both |

Not re-driven here: nothing; MG-01..05, 07..09, 13, 25 are in the table above.
