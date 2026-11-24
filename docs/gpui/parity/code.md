# Code view parity (CO, LN, CB)

Swift = LoupeFlows (prebuilt at ~/loupe fde8251), GPUI = branch gpui-v-code, both on the flows fixture (port 8807, /tmp/flows-code).
GPUI was observed with the Changes wiring in the appendix applied (changes.rs belongs to the changes agent, so it isn't committed here).

Idle CPU with the Overview open: `ps -o %cpu=,time=` 0.0 / 0:01.28 then 0.1 / 0:01.28 10 s later. Nothing polls: the tree, open files and the
Overview reload only on statusVersion / head moves; leaving the worktree releases files, tree, lookups and outline parses (`release`).

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| CO-01 | View picker Overview/Files in Changes; Overview replaces the right pane, list stays; Compact/Full hidden (/tmp/flows-code/shots/s-ov.png) | Same picker (Overview / Files) via `set_controls`; Overview replaces the right pane, list stays, Compact/Full hidden (/tmp/flows-code/shots/g-ov2.png). History/PR inputs: `commit_input`, `pull_input` (hosts are stubs on this branch) | same |
| CO-02 | Spinner "Reading the change…"; re-shown on every status refresh (key has statusVersion) | Spinner on first read; a recompute for the same A→B keeps the old report up until the new one lands (migration edited while open: report went 4→5 tables, no spinner; /tmp/flows-code/shots/g-after.png). Cancelled task drops its result (task dropped) | fixed-swift-bug (§14 #5) |
| CO-03 | Files tile + other tiles when ≥2 other lenses; number, deltas, "N to read"; click scrolls to section | Same tiles, equal widths, deltas/"27 to read", hover chevron, tooltip = summary, click scrolls to the section (no 0.25 s animation) | same |
| CO-04 | Kind bar + chips; popover with SQL files and status symbols (/tmp/flows-code/shots/s-kindchip.png) | Same bar/chips; popover 400 wide with icon, name, dir, status; row click closes and opens (/tmp/flows-code/shots/g-kindchip.png) | same |
| CO-05 | "Nothing structural beyond the files…" when only Files/Follow-ups | Same text from `loupe_code::overview::NOTHING_STRUCTURAL`, same rule | same |
| CO-06 | Section header toggles body, chevron rotates, 0.15 s animation | Header toggles body, chevron right/down; not persisted, reset when the view is rebuilt; no animation | intentional (no animations: idle-CPU rule) |
| CO-07 | Root folder label, warnings capped at 6, Show/Open | Same: folder label (head-truncated), capped 6 "Show N More Warnings", Show/Hide CodeBlock 14 lines, Open → on_open(path,line) | same |
| CO-08 | Masonry 300–640 columns; header icon, name, status label (/tmp/flows-code/shots/s-ov.png) | Masonry 300–640 flex columns (fixed an overflow past the pane edge in the WIP); same header (/tmp/flows-code/shots/g-ov2.png) | same |
| CO-09 | Folded "5 unchanged columns", key/link icons, type, notes | Same ("5 unchanged columns", key icon, link icon w/ tooltip, "not null, default 0") | same |
| CO-10 | Extras: index/trigger/key/constraint icons, LineDiff / CodeBlock / inline | Same decisions (`ov::extra_body`), lucide icons list/zap/key-round/shield-check | same |
| CO-11 | "Definition" / "Definition before this change" disclosure, CodeBlock 30 | Same titles (`ov::table_sql_title`), CodeBlock 30 | same |
| CO-12 | Objects: status, name, type, Dropped/Redefined; Definition open when ≤25 lines | Same (`ov::object_open_by_default`, LineDiff for changed) | same |
| CO-13 | Data rows Insert into/Update/Delete from, N statements disclosure | Same (`ov::data_verb`, `data_summary`, CodeBlocks 10, red/green pairs) | same |
| CO-14 | Dependencies rows capped 10, major/downgrade orange | Same, "Show N More Packages" | same |
| CO-15 | Config rows; detail shown only without tag, so "new file · N keys" loses its count | Detail shown beside the tag (docker-compose.yml shows "deleted · had 4 keys") | fixed-swift-bug (§14 #13) |
| CO-16 | Code lens capped 6, "Show N More <section title>" | Capped 6, "Show N More Items" | fixed-swift-bug (§14 #12) |
| CO-17 | Follow-ups: line mono with tag orange, file:line, capped 8 | Same | same |
| CO-18 | CodeBlock dedent/trim, +/− tint, async highlight, Show All N Lines | Same dedent/trim/tint/link; highlight computed in render via `syntax::highlight_snippet` (snippets are ≤ a few dozen lines) | intentional (sync highlight of small snippets; no cache needed) |
| CO-19 | LineDiff LCS over trimmed lines | Same (`ov::line_diff`) | same |
| CO-20 | Changes: closes Overview, selects file, drops the line; unknown path only closes | Changes: closes Overview, selects the file and reveals+selects the line (refunds header → 0007_refunds.sql line 3, /tmp/flows-code/shots/g-openfact2.png); a path not in the list opens in the browser | fixed-swift-bug (§14 #4) |
| CO-21 | Branch row: N behind, touching text, Merges cleanly / N conflicting pill | Same text from `ov::branch_headline/branch_touching/branch_merge_label`; PR host (pulls agent) passes `pull_input` | same |
| CO-22 | Conflicting files / Commits disclosures capped 8; commit rows not clickable | Same; commit rows open the commit via `on_commit` when the host sets it | fixed-swift-bug (§14 #14) |
| LN-01 | Overview for uncommitted work; spinner on each refresh | As CO-01/CO-02: key headOID+statusVersion, A=HEAD or Empty when unborn, unstaged entry preferred (unit test `worktree_input_prefers_unstaged_and_marks_sides`) | fixed-swift-bug (§14 #5) |
| LN-02 | Commit Overview, A = first parent / empty, key "sha fileCount" | `commit_input(dir, sha, parent, changes)`, same key (test `inputs_key_like_swift`); History host on this branch is a stub | same |
| LN-03 | PR Overview, merge base vs head, key with target ref, Fetching/Can't Read states | `pull_input(...)` with the same key and target; fetch/merge-base/error UI belong to the pulls host | same |
| LN-04 | Tiles strip rules | Same rules (`ov::summary_tiles`, `tile_restates`, `files_to_read`) | same |
| LN-05 | Kind bar/chips/popover, >16 scrolls at 460 | Same (popover scrolls at 460 when >16) | same |
| LN-06 | Collapse section, local state | Same; no animation | intentional (no animations) |
| LN-07 | Open fact: Changes drops line; History/PR reveal | Changes reveals the line too (`CodeBrowser::open_from_overview`) | fixed-swift-bug (§14 #4) |
| LN-08 | Show All N Lines / LineDiff | Same | same |
| LN-09 | CappedRows limits 6/10/10/6/8/8/8 | Same limits; Code noun "Items" | fixed-swift-bug (§14 #12) |
| LN-10 | Branch row PR only | Same data via `lens::branch_status` (CLI parity: `--lens app origin/main origin/feature/search-index origin/main` identical) | same |
| LN-11 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-12 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-13 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-14 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-15 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-16 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-17 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-18 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-19 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-20 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-21 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-22 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-23 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-24 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-25 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-26 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-27 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-28 | Lens facts from ChangeLens/Lenses/SQLSchema/YAML | Same facts from loupe_code::lens (the crate's ported tests); text output identical to Swift `--lens` on 5 ranges of the fixture (billing worktree, root→HEAD, HEAD~60→HEAD, PR branch with upstream, HEAD~1→HEAD: `diff` empty, /tmp/flows-code/ls-*.txt vs lg-*.txt); drawn by overview.rs | same |
| LN-29 | `Loupe --lens <repo> <a> <b\|worktree> [up]` prints the report and exits | `code::overview::lens_cli(args) -> Option<String>` ports it (test `lens_cli_prints_reports`, output identical to Swift); main.rs is frozen so the flag isn't hooked up yet | missing (main.rs frozen; hook requested) |
| CB-01 | Compact/Full toggles global prFullMode; entering Full opens the selected file as a tab (/tmp/flows-code/shots/s-full.png) | Same: Full shows tree + browser, the list's file becomes the tab; leaving Full selects the browser's file in the list (/tmp/flows-code/shots/g-full.png) | same |
| CB-02 | ls-files tree, fold except changed ancestors; clean worktree re-folds on every refresh; empty repo spins forever | Same listing/fold; fold applied once status is known, so a clean worktree keeps what you expanded; failed/empty listing shows "No Files" | fixed-swift-bug (§14 #2, #3) |
| CB-03 | Tab inserted after current; header counts/Stage/Discard; reloads & flashes spinner each refresh | Same insert/header/Stage File/Discard…; a refresh reloads the shown file in place and swaps only if text changed (no spinner). Unchanged files show "+0 −0" because DiffView always draws counts | fixed-swift-bug (§14 #5) |
| CB-04 | PR Full tree from ls-tree <head>, changed files diffed base...head, unchanged via git show | `CodeBrowser::new_rev(.., base, head, changed)` / `set_rev`; same git calls (test `loads_changed_files_whole_and_others_plain`); Viewed checkbox through `header_actions`; pulls host is a stub here | same |
| CB-05 | Tabs: icon, name (+parent on duplicates), change dot, xmark on active/hover, accent line | Same via `ui::tabs::tab_bar` (db.rs / router.rs tabs, /tmp/flows-code/shots/g-p1s.png) | same |
| CB-06 | Close / Close Others / Close All / Copy Path | Same menu from `ui::tabs` | same |
| CB-07 | ⌘W closes the selected tab only in Full; nothing when none | Same: keystroke interceptor, only while the browser shows on its host tab | same |
| CB-08 | ⌘-hover underline + pointing hand in any file type | Same, drawn by the shared DiffView (diff/) | same |
| CB-09 | ⌘-click fetch_user: one definition → opens db.rs:24 (/tmp/flows-code/shots/s-cmdclick.png) | Same lookup (`loupe_code::index::definitions`), opens db.rs line 24 selected (/tmp/flows-code/shots/g-cmdclick2.png) | same |
| CB-10 | N definitions panel 220 high, rows stay, Esc closes | Same panel (`ui::tabs::results_panel`), Esc/xmark close | same |
| CB-11 | References fallback, 2000 cap; "2000+" for exactly 2000; paths with ':' misparsed | Same panel ("No definition found … 30 references to limit", /tmp/flows-code/shots/g-refs.png); NUL-separated `git grep -z` so ':' in paths parses (test `references_keep_colons_in_paths`). "2000+" for exactly 2000 remains: the label is in ui/tabs.rs (frozen) | fixed-swift-bug (§14 #10; #9 requested) |
| CB-12 | Context menu Go to Definition / Find References | Same items from DiffView → `on_navigate(word, CodeAction)` → `navigate` | same |
| CB-13 | Back/Forward only in Full; tree/tab clicks keep a stale reveal line | Back/Forward buttons + ⌃-/⌃⇧- also in Compact (⌃- after a peek returned to router.rs:89, /tmp/flows-code/shots/g-p2s.png); tree/tab clicks forget the line | fixed-swift-bug (§14 #6, #7) |
| CB-14 | Mini spinner + word while looking up | Same (`busy` word in nav controls), cleared on every exit | same |
| CB-15 | Compact jump to an unchanged file flips global Full mode (Swift switched to Full, tabs db.rs+router.rs, /tmp/flows-code/shots/s-cmdclick.png) | Changed target: list selects it and reveals the line; unchanged: the browser "peeks" in place, Compact stays, other tabs unaffected (/tmp/flows-code/shots/g-cmdclick2.png) | fixed-swift-bug (§14 #8) |
| CB-16 | Outline menu "name  kind · line", hidden without symbols | Same (`ui::tabs::nav_controls` outline), computed off-main, picking records Back | same |
| CB-17 | Too large/minified → dimmed icon; cache by path/len/hash, cancel on leave | Same via `loupe_code::index::outline_of` (shared cache, cancel flag); leaving the file sets cancel and drops the result | same |
| CB-18 | Languages/definition kinds per TagQueries | Same tag queries in loupe_code (crate tests) | same |

## Appendix: Changes wiring used for the GPUI runs

The changes owner embeds the browser like this (`browser_for`, `CodeBrowser::tree`, `results_panel`, `CodeBrowserEvent::Reveal`, `ChangeOverview` with `worktree_input`). Also call `end_peek` on every list click, not only when the selection changes.

```diff
diff --git a/app/src/views/changes.rs b/app/src/views/changes.rs
index ff5f85b..978ca0d 100644
--- a/app/src/views/changes.rs
+++ b/app/src/views/changes.rs
@@ -25,6 +25,13 @@ pub struct ChangesView {
     diff: Option<Entity<DiffView>>,
     message: Entity<TextareaState>,
     _subs: Vec<Subscription>,
+    // TEMP code-agent wiring (not committed)
+    browser: Option<(Entity<crate::views::code::CodeBrowser>, Subscription)>,
+    overview: Option<Entity<crate::views::code::ChangeOverview>>,
+    overview_on: bool,
+    pending_reveal: Option<(String, usize)>,
+    was_full: bool,
+    last_sel: Option<String>,
 }
 
 impl ChangesView {
@@ -41,7 +48,7 @@ impl ChangesView {
                 _ => {}
             }),
         ];
-        let mut v = Self { app, model: None, diff: None, message, _subs: subs };
+        let mut v = Self { app, model: None, diff: None, message, _subs: subs, browser: None, overview: None, overview_on: false, pending_reveal: None, was_full: false, last_sel: None };
         v.follow_selection(window, cx);
         v
     }
@@ -52,6 +59,15 @@ impl ChangesView {
         let m = self.app.read(cx).selected_model();
         if m.as_ref().map(|m| m.entity_id()) == self.model.as_ref().map(|x| x.0.entity_id()) { return }
         self.model = m.map(|m| (m.clone(), cx.observe_in(&m, window, |v, _, w, cx| v.sync(w, cx))));
+        self.overview = None; self.overview_on = false;
+        self.browser = self.model.as_ref().map(|(m, _)| {
+            let b = crate::views::code::browser_for(&self.app, m, window, cx);
+            let sub = cx.subscribe(&b, |v, _, e: &crate::views::code::CodeBrowserEvent, cx| {
+                let crate::views::code::CodeBrowserEvent::Reveal { path, line } = e;
+                v.pending_reveal = Some((path.clone(), *line)); v.apply_reveal(cx);
+            });
+            (b, sub)
+        });
         self.sync(window, cx);
     }
 
@@ -74,12 +90,58 @@ impl ChangesView {
             None => self.diff = None,
         }
         if self.message.read(cx).value().as_ref() != msg { self.message.update(cx, |i, cx| i.set_value(msg, window, cx)) }
-        self.app.update(cx, |a, cx| a.set_controls(Tab::Changes, DiffControls { full: has_file, view: None }, cx));
+        self.apply_reveal(cx);
+        let full = self.app.read(cx).full_mode;
+        let sel = m.read(cx).state.selected_file().map(|f| f.path.clone());
+        if let Some(b) = self.browser.as_ref().map(|b| b.0.clone()) {
+            if sel != self.last_sel { if self.last_sel.is_some() || sel.is_some() { if !full && let Some(p) = &sel { b.update(cx, |b, cx| b.follow(p, cx)) } } }
+            if full != self.was_full {
+                if full { if let Some(p) = &sel { b.update(cx, |b, cx| b.show(p, cx)) } }
+                else if let Some(p) = b.read(cx).nav.selected.clone() && let Some(c) = b.read(cx).change(&p, cx) { m.update(cx, |m, cx| m.select_file(Some(c.id()), cx)) }
+            }
+        }
+        if sel != self.last_sel && self.last_sel.is_some() { self.overview_on = false; self.overview = None }
+        self.last_sel = sel; self.was_full = full;
+        let no_files = m.read(cx).state.files().is_empty();
+        if self.overview_on && !no_files {
+            let input = crate::views::code::overview::worktree_input(m.read(cx));
+            match &self.overview {
+                Some(o) => o.update(cx, |o, cx| o.set_input(input, cx)),
+                None => {
+                    let b = self.browser.as_ref().map(|b| b.0.clone());
+                    let me = cx.entity().downgrade();
+                    let on_open: crate::views::code::overview::OnOpen = std::rc::Rc::new(move |p, l, _, cx| {
+                        me.update(cx, |v, cx| v.set_overview(false, cx)).ok();
+                        if let Some(b) = &b { b.update(cx, |b, cx| b.open_from_overview(&p, l, cx)) }
+                    });
+                    self.overview = Some(cx.new(|cx| crate::views::code::ChangeOverview::new(input, on_open, cx)));
+                }
+            }
+        }
+        let me = cx.entity().downgrade();
+        let view = (!no_files).then(|| (vec![crate::state::ViewMode::Overview, crate::state::ViewMode::Files],
+            if self.overview_on { crate::state::ViewMode::Overview } else { crate::state::ViewMode::Files },
+            std::rc::Rc::new(move |mode, _: &mut Window, cx: &mut gpui_kit::App| { me.update(cx, |v, cx| v.set_overview(mode == crate::state::ViewMode::Overview, cx)).ok(); }) as std::rc::Rc<dyn Fn(crate::state::ViewMode, &mut Window, &mut gpui_kit::App)>));
+        self.app.update(cx, |a, cx| a.set_controls(Tab::Changes, DiffControls { full: !no_files && !self.overview_on && (full || has_file), view }, cx));
+        cx.notify();
+    }
+
+    fn apply_reveal(&mut self, cx: &mut Context<Self>) {
+        let Some((p, l)) = self.pending_reveal.clone() else { return };
+        if let Some(d) = &self.diff && d.read(cx).file().path() == p { d.update(cx, |d, cx| d.reveal(l as u32, cx)); self.pending_reveal = None }
+    }
+
+    fn set_overview(&mut self, on: bool, cx: &mut Context<Self>) {
+        self.overview_on = on;
+        if !on { self.overview = None }
         cx.notify();
+        let m = self.selected();
+        if let Some(m) = m { m.update(cx, |_, cx| cx.notify()) }
     }
 
     fn make_diff_view(&self, file: Arc<crate::diff::FileDiff>, actions: Vec<HunkAction>, cx: &mut Context<Self>) -> Entity<DiffView> {
         let me = cx.entity().downgrade();
+        let me2 = me.clone();
         let (side, layout) = { let a = self.app.read(cx); (a.diff_side, a.layout) };
         let app = self.app.clone();
         cx.new(|cx| {
@@ -93,6 +155,12 @@ impl ChangesView {
                 if let Some(v) = me.upgrade() && let Some(m) = v.read(cx).selected() { m.update(cx, |m, cx| m.apply_lines(action, lines, cx).detach()) }
             });
             d.on_side_change(move |side, layout, _, cx| app.update(cx, |a, cx| a.set_diff_side(side, layout, cx)));
+            d.on_navigate(move |w, a, _, cx| {
+                if let Some(v) = me2.upgrade() && let Some(b) = v.read(cx).browser.as_ref().map(|b| b.0.clone()) {
+                    let from = v.read(cx).diff.as_ref().map(|d| d.read(cx).file().path().to_string()).unwrap_or_default();
+                    b.update(cx, |b, cx| b.navigate(w, a, from, cx))
+                }
+            });
             d
         })
     }
@@ -161,10 +229,20 @@ impl Render for ChangesView {
                     .child(ui::pill_primary("commit", if amend { "Amend" } else { "Commit" }, theme::blue())
                         .when(!can_commit, |d| d.opacity(0.4))
                         .when(can_commit, |d| d.on_click(move |_, _, cx| m2.update(cx, |m, cx| m.commit(cx).detach()))))));
-        let right = match &self.diff {
+        let full = self.app.read(cx).full_mode;
+        let browser = self.browser.as_ref().map(|b| b.0.clone());
+        let peek = browser.as_ref().is_some_and(|b| b.read(cx).peeking());
+        let right_code = if self.overview_on && let Some(o) = &self.overview { Some(div().flex_1().min_w_0().h_full().child(o.clone()).into_any_element()) }
+            else if (full || peek) && let Some(b) = &browser { Some(div().flex_1().min_w_0().h_full().child(b.clone()).into_any_element()) } else { None };
+        let left = if full && let Some(b) = &browser {
+            div().flex().flex_col().w(px(300.)).flex_none().h_full().border_r_1().border_color(theme::border())
+                .child(div().flex_1().min_h_0().child(crate::views::code::CodeBrowser::tree(b, cx))).into_any_element()
+        } else { left.into_any_element() };
+        let results = if !full && !peek && !self.overview_on { browser.as_ref().and_then(|b| crate::views::code::CodeBrowser::results_panel(b, cx)) } else { None };
+        let right = if let Some(r) = right_code { r } else { let r = match &self.diff {
             Some(d) => div().flex_1().min_w_0().h_full().child(d.clone()).into_any_element(),
             None => div().flex_1().child(ui::empty_state("file", if total == 0 { "" } else { "No File Selected" }, "")).into_any_element(),
-        };
+        }; div().flex_1().min_w_0().h_full().flex().flex_col().child(div().flex_1().min_h_0().flex().child(r)).children(results).into_any_element() };
         div().flex().size_full().font_weight(FontWeight::NORMAL)
             .on_action(cx.listener(|v, _: &Commit, _, cx| v.commit(cx)))
             .child(left)
```
