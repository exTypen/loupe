# Pull Requests parity (PR, PD, RV, FG)

Both apps ran on the flows mock (`FLOWS_PORT=8803`, `/tmp/flows-pulls`). Screenshots are in `/tmp/flows-pulls/shots/`:
`s-*` is the Swift app (LoupeFlows) and `g-*` is GPUI. Where a Swift cell says "(spec)", that row was compared against
FLOWS.md §8 and the Swift source rather than driven in LoupeFlows. The mock only speaks Forgejo, so GitHub-only rows were
checked against the forge crate's code and its tests.

Status: same | fixed-swift-bug (§14 item) | intentional (why) | missing (why).

## List

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| PR-01 | Tab "Pull Requests" (⌘3); `tab` persisted (s-pr01.png) | Same tab, same key, `tab` saved by AppModel (g-pr01.png) | same |
| PR-02 | "No Remote", network.slash, the same text | `empty_state("unplug", "No Remote", same text)` when `remote_detected` and no remote | same |
| PR-03 | "Connect git.example.invalid" + Forgejo text + "Open Settings…" (s-pr03.png) | Same title, text and button, which dispatches OpenSettings (g-pr03.png) | same |
| PR-04 | Spinner until the remote and tokens are known | Spinner until `remote_detected && tokens_loaded` | same |
| PR-05 | Disk cache `pulls-<host>-<o>-<n>.json` shown at once, then page 1.., 30 s freshness | `PullList::restore_cache` (same file under `~/Library/Caches/dev.extypen.loupe…`), `should_load_on_show` uses 30 s | same |
| PR-06 | This Branch / Other Worktrees / All (s-pr06.png) | Same three sections, matched by head ref name (g-pr06.png) | same |
| PR-07 | Avatar, state icon, title, #N, author, Draft, head (accent when local) + branch icon, → base, comments | Same parts, Lucide icons (g-pr01.png, g-pr09.png) | same |
| PR-08 | 28 pt avatar, initials on a hashed hue while loading or on 404 | The same; frank (404) shows "F" (g-pr01.png) | same |
| PR-09 | Open/Closed/All; old list stays under the new picker value | The list clears on a state switch, so the old state's PRs can't pose as the new result (g-pr09.png) | fixed-swift-bug (§14 PR: "on a state switch, `pulls` isn't cleared") |
| PR-10 | "Opened by me" toggle, case-insensitive, `pullsMine` saved; disabled with no username | Same, with the same tooltips (g-pr10.png: #37 by "Alice" kept). With no username the filter doesn't hide anything | fixed-swift-bug (§14 PR: `pullsMine` true and `me` nil hides everything) |
| PR-11 | Filter on title, "#N", author (s-pr11.png) | Same match rules (g-pr11.png) | same |
| PR-12 | Paging only from rows in "All", so a last PR in another section stops paging | Pages whenever the end of the loaded rows is in view, whatever section it's in | fixed-swift-bug (§14 PR: `loadMore` hangs off "All" rows only) |
| PR-13 | Open list polled every 60 s, not while a PR is open; unchanged data doesn't re-render | Same 60 s poll; stopped while a PR is open, the tab is hidden or another worktree shows; `PullList::apply` keeps an unchanged list | same |
| PR-14 | Refresh button and ⌘R | The refresh button; ⌘R arrives as `AppEvent::Refresh` | same |
| PR-15 | Reload on app activation | `observe_window_activation` → refresh | same |
| PR-16 | "Updated 2 min. ago" redrawn every 5 s | "Updated N minutes ago", redrawn once a minute (the label only changes per minute) | intentional (idle CPU rule: no 5 s timer) |
| PR-17 | "No Pull Requests" / "No Pull Requests by You" | The same | same |
| PR-18 | "Couldn't Load" + message when empty; the error is invisible once PRs show | Same empty state (g-pr18.png: "Forgejo 401: token is required"); with PRs showing, a "Couldn't refresh" mark with the message as tooltip (g-pr18b.png after the fault is cleared, g-fg03.png) | fixed-swift-bug (§14 PR: "any list error is invisible once PRs are on screen") |
| PR-19 | "Create Pull Request…" when ahead of the target and no open PR, cached per head+refsVersion | The toolbar '+' uses `pr_create::can_create` (the forge's open PR for the branch, merged excluded), once per HEAD/refs (`g-pulls.png`) | fixed-swift-bug (see PC-05) |
| PR-20 | Click a row opens the detail; 2 recent + selected kept | `DetailCache` keeps the same set (g-pd01.png) | same |
| PR-21 | "‹ Pull Requests" / ⌘[ goes back and reloads at once | Same (g-pr21.png). The detail now takes the keyboard focus when it opens, so ⌘[ reaches it | same |
| PR-22 | Switching worktree drops opened PR models; viewed and drafts come back from defaults | Same: details dropped on a worktree switch, rebuilt from settings | same |

## Detail

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| PD-01 | Header row 1 + row 2 (s-pd01.png) | Same parts and order (g-pd01.png) | same |
| PD-02 | "#N ⌄" › Open in Browser | Dropdown › Open in Browser (`cx.open_url`) | same |
| PD-03 | "#N ⌄" › Copy Link | Dropdown › Copy Link (clipboard) | same |
| PD-04 | Overview / Files / Conversation in the toolbar; Files is the default; Compact/Full only on Files | `set_controls` with the same three modes; Compact/Full only on Files (g-pd01.png, g-rv01.png) | same |
| PD-05 | PR, reviews, comments, commits, then the diff; an unchanged diff isn't parsed again; first unviewed selected | `pull::load_detail` + `DiffSource::fetch` (hash skip), parsed off the main thread, the first unviewed file selected | same |
| PD-06 | Polls every 60 s while mounted; waits 60 s if loaded <30 s ago; offline raises the alert on every poll | Same cadence; a poll that fails with the same message doesn't raise the alert again, the header keeps a warning mark | fixed-swift-bug (§14 PR: "every 60 s poll raises the modal alert again") |
| PD-07 | Badges "N approved / requested changes / pending" with a popover | Same counts from `pull::reviewer_states`; the popover lists the users (g-pd07.png: PR #36 pending alice, erin) | same |
| PD-08 | ChecksBadge opens the checks overlay | `ci::badge` from the shared `ChecksModel` (see CI-01) | same |
| PD-09 | Overview: "Fetching the pull request…", then ChangeOverview | Same fetch and merge-base, then the code view's `ChangeOverview` (`pull_input` against the fetched base): branch row "6 commits behind main · Merges cleanly" (`g-prov.png`); branch-row commits open in History | same |
| PD-10 | Resizable split, tree 250–460 pt | `h_resizable`, 250–460 px, file pane min 360 | same |
| PD-11 | "Filter files": substring, folders forced open, folder clicks dead | Same | same |
| PD-12 | Progress bar + "X of Y files viewed" | Same (g-pd16.png, g-pd17b.png) | same |
| PD-13 | Folder counters, +a −d, A/D/R/M square, dim when viewed, comment bubble | Same (g-rv11.png) | same |
| PD-14 | Folder menu: Expand/Collapse, Mark Folder as (Not) Viewed, Expand All, Collapse All | Same items | same |
| PD-15 | DiffHeader with Viewed; Binary / No Content Changes; inline comments | DiffView with the Viewed checkbox in its header actions; inline comments | same |
| PD-16 | `viewed:<host>/<o>/<n>#N` = {path: FNV-1a as string} | Same key and format (checked in the defaults: `"db/schema.sql" = 6272669281002267718`) | same |
| PD-17 | ⇧⌘V / `v`: mark viewed, next unviewed | Same (g-pd17b.png: Payment.java dimmed, Session.java selected, "1 of 52") | same |
| PD-18 | ⌥↓ / ⌥↑ next/previous, clamped | Same (`PullFiles::move_by`) | same |
| PD-19 | Compact/Full writes global `prFullMode`; the pane follows only while Files is mounted | The detail observes the app's `full_mode`, so it follows from any tab | fixed-swift-bug (§14 PR, PLAUSIBLE: `pull.fullMode` sync gap) |
| PD-20 | Full: fetch head, `ls-tree`, untouched folders start collapsed (s-pd20.png) | Same (g-pd20.png). Fetches of the PR head are now serialized per repository and retried once inside `pull::fetch_head` (test `concurrent_fetch_heads_succeed`): Full mode's tree, file and Overview raced into "cannot lock ref" | same |
| PD-21 | Whole file via `git diff -U1000000 base...head` or `git show`; the spinner stays on failure | Same commands; a failure shows "Can't Load the File" with Retry (g-pd21.png) | fixed-swift-bug (§14 PR: "if `loadFull` fails, the file pane spins forever") |
| PD-22 | Tab bar, close, context menu with Copy Path | `ui::tabs::tab_bar` (shared) with the same menu | same |
| PD-23 | Reveal from Overview/Conversation, turns Full on for unchanged files | Same (`reveal`) | same |
| PD-24 | ⌘W closes a tab in Full on Files; elsewhere it closes the window and quits | Same in Full on Files, and with no tab it does nothing (g-pd24.png); elsewhere it propagates to the shell | same |
| PD-25 | ⌘-click / Go to Definition / Find References at head; results panel | Same via `loupe_code::index` at the head sha (menu in g-rv12a.png) | same |
| PD-26 | Back/Forward and outline in Full headers | `tabs::nav_controls` with the outline from `loupe_code::index::outline_of` | same |
| PD-27 | Before/After in Compact shows whole files with no tab bar | Same (`whole()` without tabs unless Full) | same |
| PD-28 | Check Out: select an existing `pr/N`, else fetch + `worktree add -B pr/N <repo>-pr-N` (any repo) | Same commands; only this repository's worktrees count (g-pd28.png: app-pr-27 created and selected). The button calls `AppModel::checkout_pull` (repo-scoped in state/ now; the pulls copy is gone) | fixed-swift-bug (§14 PR: Model.swift:559 searched every repo) |
| PD-29 | Merge menu Merge/Rebase/Squash, disabled when not mergeable, "<Style> #N into base?" | Same menu, tooltip and confirmation (g-pd29a.png, g-pd29.png); body `{"Do":"merge"}` in forge.log | same |
| PD-30 | After a merge: back, fetch base, History on `<remote>/<base>` with the merge selected | Leaves the PR, fetches `main`, emits `ShowBranch`; History shows origin/main with the merge selected (`g-merged.png`) | same |
| PD-31 | Merge rejected: "Forgejo 405: …" alert, stays on the PR | Same (g-pd31.png) | same |
| PD-32 | `cat-file -e`, then fetch `refs/pull/N/head` and the base | `pull::fetch_head`, one at a time per repository (git common dir), retried once; the view's mutex + retry wrapper is now redundant | same |
| PD-33 | Alert titled "Forgejo" (also on GitHub), OK clears | Titled by the server (`client.server_name()`), OK or Return clears; centred on the window (g-pd31.png, z-verify `ga1`/`ga2`) | fixed-swift-bug (§14 PR: "the detail alert is titled Forgejo for GitHub accounts too") |

## Conversation and review

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| RV-01 | Timeline max 960, rail, opens at the bottom, composer, sidebar from 1250 pt (s-rv01.png) | Same; opens scrolled to the end (g-rv01.png, g-rv01b.png); sidebar when wide (g-rv07.png) | same |
| RV-02 | Opened card first, date order, joined commits and same-author comments <30 min | Same (`conversation::build`, unit test) | same |
| RV-03 | Description card; no `created_at` means no description at all | The card is always there, undated when `created_at` is missing | fixed-swift-bug (§14 PR: Conversation.swift:162) |
| RV-04 | Comment/review cards, tags bot/Author, "earlier commit"; requested changes orange here, red in the header | Same cards (g-rv01b.png); requested changes red everywhere | fixed-swift-bug (§14 PR: red in the header, orange in the conversation) |
| RV-05 | Inline block: file header ↗, last 4 hunk lines, body; click reveals in Files | Same | same |
| RV-06 | Commit rows and day markers | Same (g-rv01b.png: "bob added 2 commits", "Today") | same |
| RV-07 | Sidebar reviewers, click scrolls to the review | Same (g-rv07.png) | same |
| RV-08 | Participants grid | Same | same |
| RV-09 | Markdown: headings, lists, tasks, quotes, code, tables, HTML stripped | Same blocks (unit test); bare URLs are links and an image shows its alt text | same |
| RV-10 | Post comment, ⌘↩; on failure the text is cleared anyway | Same request; the text stays when posting fails (g-rv10a.png: "Forgejo 500: boom", "Thanks" still typed), cleared after it posts (g-rv10b.png) | fixed-swift-bug (§14 PR: "the comment text is cleared even when posting fails") |
| RV-11 | Server comments under new/old lines in a blue box (s-rv11.png) | Same (g-rv11.png) | same |
| RV-12 | Right-click › Add Comment… popover "<path> · Line N", draft "You · Pending" | Same menu item; the editor opens inline under the line with the same title and ⌘↩ (g-rv12b.png); the draft row (g-rv12.png) | intentional (an inline editor instead of a transient popover, the diff renderer's comment UI) |
| RV-13 | Trash deletes a draft | "Delete" on the pending row | same |
| RV-14 | Drafts in `drafts:<host>/<o>/<n>#N`; no warning after a new push | Same key and JSON; the review popover warns when the drafts were written on an older head | fixed-swift-bug (§14 PR: drafts keep their old line numbers with no warning) |
| RV-15 | Bubble count = server comments + drafts | Same (g-rv12.png: Payment.java 1) | same |
| RV-16 | "Finish your review": text, Comment/Approve/Request Changes, "N pending comment(s)"; spaces-only body counts | Same popover (g-rv16.png); the body is trimmed | fixed-swift-bug (§14 PR: "the empty-body check isn't trimmed") |
| RV-17 | POST reviews `{body, event, commit_id, comments[{path, body, new_position, old_position}]}`; success clears drafts and reloads | Same body (forge.log: `"new_position":3,"old_position":null`), drafts cleared, the comment now shows as alice's (g-rv17.png) | same |

## Forge

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| FG-01 | `/api/v1`, `api.github.com`, `/api/v3`; `token` auth | `loupe_forge::Client` (same roots and headers) | same |
| FG-02 | "<Forgejo|GitHub> <status>: <message>"; cancellations suppressed | Same text (g-pr18.png, g-pd31.png); cancelled loads never set an error (`user_message()` is None) | same |
| FG-03 | 401 shows as "Forgejo 401: …", no re-auth prompt | Same (g-pr18.png) | same |
| FG-04 | No rate-limit handling, polling continues | Same | same |
| FG-05 | Offline: the list keeps its data, the detail alerts on every poll | The list keeps its data with a "Couldn't refresh" mark; the detail alerts once per distinct error | fixed-swift-bug (§14 PR: repeated alert; hidden list error) |
| FG-06 | Paging at 50; the open list stops at a short first page | Paging at 50; the open list keeps going past a server that caps pages below 50 (unit test `open_list_pages_past_a_server_capped_below_fifty`) | fixed-swift-bug (§14 PR: PullModel.swift:47-48 silently truncates) |
| FG-07 | GitHub diff too large → fetch head + local `git diff` | `DiffSource::fetch` in the forge crate does the same (not reachable on the Forgejo mock) | same |
| FG-08 | GitHub state mapping, `merged_at`, PUT merge, APPROVE | Forge crate mapping (its tests) | same |
| FG-09 | `GIT_CONFIG_COUNT=2` credential helper for fetches | `AppModel::git_auth` passed to every PR fetch | same |
| FG-10 | One load per state with one rerun; stale results dropped | Same for the list (`Load` + `again`) and the detail (`reload_queued`) | same |
| FG-11 | `Loupe --api <path>` prints a raw response | `main.rs` `--api`: the current space's account through `Client::raw` (GitHub's own API root); PR #1's JSON from the flows mock (see shell.md MS-01) | fixed-swift-bug (§14 `--api` always /api/v1) |

## Performance

- Idle with a PR open: `ps -o %cpu=,time=` read 0.0 / 0:01.28, 0.0 / 0:01.28 and 0.0 / 0:01.29, 10 s apart.
- Idle on the list: 0.0 / 0:00.56 twice, 10 s apart.
- Nothing polls while hidden: the list poll and the clock stop in `hide()` (another tab, another worktree, a PR open). The
  detail poll stops in `PullDetail::hide` (back, merge, tab or worktree switch), and loads are cancelled there.
- Nothing animates: the spinners exist only while loading, and the running-CI dot is static.
- Caches follow the Swift keys: list freshness 30 s, "Create Pull Request" per `head_oid refs_version`, the diff hash skip,
  and the conversation memo per data version.

## Final pass (merged app)

Branch `gpui-z-pulls` on the merged `gpui` (5dde615), both apps side by side on the flows mock (`FLOWS_PORT=8835`,
`/tmp/flows-z-pulls`). Screenshots are in `/tmp/flows-z-pulls/shots/` (`s-*` Swift LoupeFlows, `g-*` GPUI). Every row
above was re-read against the merged code; the ones below were driven by real clicks and keys in both apps.

| ID | Final pass: what both apps did | result |
|---|---|---|
| PR-01, PR-06, PR-07, PR-08 | Same tab, sections (Other Worktrees / All, This Branch on app-search), rows, avatars, F fallback (`s-pr01`, `g-pr01`, `g-pr21`) | same |
| PR-09, PR-11 | Closed list and the "Payments" filter match (`g-try`/`s-x`, `g-pr11`) | same |
| PR-14, PR-18 | **Fixed:** after a worktree switch dropped an open PR, coming back showed the list with the stale selection still set, and `load()` returned early, so Refresh and ⌘R did nothing (no request reached the forge). Now the worktree comes back on its PR, like Swift (`list.selected` survives). With a 401 fault the refresh shows "Couldn't refresh" (`g-pr18`) | fixed |
| PR-21 | Back button returns and reloads (`g-pr21`). ⌘[ can't be typed by cliclick on this Turkish layout (it sends ⌥8 → ⌥⌘8 = SelectWorktree8); the key path is unchanged | same |
| PR-22 | **Fixed** (see PR-14): billing → back to Root reopens #26 (`g-x` after the fix) | fixed |
| PR-02, PR-03 | infra: "Connect git.example.invalid" + Open Settings… (`s-pr03`, `g-pr03`) | same |
| PD-01, PD-04, PD-07 | Header, badges, Overview/Files/Conversation, Compact/Full only on Files (`s-pd01`, `g-pd01`) | same |
| PD-02, PD-03 | Same two items; GPUI now has Swift's compass and link icons (`s-pd02c`, `g-pd02c`) | fixed |
| PD-09 | Overview tiles, "6 commits behind main", Database section match (`s-pd09`, `g-pd09`). The Files tile's composition bar overflowed its card; fixed in 6835e7e, verified in z-verify below | same |
| PD-13 | **Fixed (shared):** the tree sorted `charge.go` before `charge_test.go`; Finder (`localizedStandardCompare`) puts punctuation before letters in ICU order, so Swift shows `charge_test.go` first. `loupe_core::file_tree::standard_compare` now follows ICU's punctuation order and Finder's lowercase-first tie-break (test `orders_names_like_finder`) | fixed |
| PD-14 | Same folder menu items (`s-pd14c`, `g-pd14c`) | same |
| PD-17 | `v` marks viewed and moves on once the diff has focus, in both (`s-pd17`, `g-pd17`); neither takes `v` before the detail is focused | same |
| PD-20, PD-22 | Full tree with untouched folders collapsed matches (`s-pd20`, `g-pd20`). **Fixed:** a PR opened while Full mode is on showed its first file with no tab; Swift's `selectedPath` didSet opens it as a tab | fixed |
| PD-28 | Check Out on #27 created `app-pr-27` on `pr/27` and selected it (`g-pd28`) | same |
| PD-29, PD-30 | Same "Merge #32 into main?" confirmation (`s-alert`, `g-pd29b`); GPUI merged (`{"Do":"merge"}` in forge.log), left the PR and showed origin/main in History with the merge selected (`g-pd30`) | same |
| RV-01 | **Fixed:** the timeline's rail (the line the avatars sit on) was missing; drawn per row now, so the sidebar's jump indices stay the entries (`g-rv01-final`) | fixed |
| RV-04, RV-06 | **Fixed:** dates read "2 weeks ago" where Swift says "3 weeks ago": Foundation's named relative format rounds (7–12 days "last week", 20 days "3 weeks ago", 30 days "last month", seconds under a minute). `pulls/time.rs::relative` follows it, thresholds read off `Date.RelativeFormatStyle` day by day | fixed |
| RV-09 | **Fixed:** Markdown tables stretched across the card; Swift's Grid sizes columns to content (`s-rv01`, `g-rv01-final`) | fixed |
| RV-10 | 500 fault: both alert "Forgejo 500: boom"; GPUI keeps "Thanks" (§14), then posts it (`s-rv10a`, `g-rv10a`, `g-rv10b`) | same / fixed-swift-bug |
| RV-12, RV-13, RV-15 | Right-click › Add Comment…, "You · Pending" draft, bubble count 1, "Review (1)" (`g-rv12a`, `g-rv12b`) | same / intentional (inline editor) |
| RV-16, RV-17 | "Finish your review" popover, ⌘↩ posts `{"event":"COMMENT","comments":[{"new_position":3,"old_position":null}]}` (`g-rv16`, `g-rv17`) | same |
| FG-02, FG-03 | "Forgejo 401: token is required" / "Forgejo 500: boom" texts | same |
| FG-11 | `loupe --api /repos/acme/app/pulls/1` printed PR #1's JSON from the mock | same |
| PR-04, PR-05, PR-10, PR-12, PR-13, PR-15–17, PR-19, PR-20, PD-05, PD-06, PD-08, PD-10–12, PD-15, PD-16, PD-18, PD-19, PD-21, PD-23–27, PD-31–33, RV-02, RV-03, RV-05, RV-07, RV-08, RV-11, RV-14, FG-01, FG-04–FG-10 | Not driven again: their code is unchanged by the merge (only `use` paths moved); the rows above stand | as above |

Idle CPU (merged app): a PR open, `ps -o %cpu=,time=` 0.0 / 0:01.39 twice, 10 s apart; on the list 0.0 / 0:01.52 →
0.0 / 0:01.54 (the 60 s open-list poll and the once-a-minute "Updated" label).

The PR detail's own alert centred on the detail pane; since 6835e7e it centres on the window (z-verify below); three copies of a relative-date
formatter exist (`pulls/time.rs`, `history/sheets.rs`, `changes/inspector.rs`), only the pulls one was changed.

## Verification against Swift (z-verify, after 6835e7e)

Both apps side by side on the flows mock (port 8851, `/tmp/flows-z-verify`), real clicks and System Events keys.
Shots in `/tmp/flows-z-verify/shots/` (`g*` GPUI, `s*` Swift).

| ID | Swift | GPUI | result |
|---|---|---|---|
| PD-09 | #26 Overview: the Files tile's bar fits its card, legend on three lines (`s11.png`) | the bar fits its card, legend on three lines (`g11.png`) | same |
| PD-33, RV-10 | 500 fault on `comments`: alert "Forgejo / Forgejo 500: mock fault" centred on the window; Return dismisses, Esc does nothing (single OK button) (`sa1`, `sa2`, `sa3`/`sa4`) | the same alert centred on the window (`a1.png`, `ga1.png`); Return didn't dismiss it (`a2.png`) | **fixed**: Return dismisses it (`ga2.png`), Esc too (harmless extra); the typed comment stays (fixed-swift-bug, as before) |
| PD-29 | Merge › Merge on #36: NSAlert "Merge #36 into main?" Cancel / Merge, horizontally centred and in the window's upper third (`sm1full.png`); Esc cancels (`sm2full.png`) | the same question and buttons centred on the window (`gm2.png`); Return and Esc did nothing | **fixed**: Esc cancels (`gm3.png`), Return merges and shows main's History with the merge (`gm4.png`). Still different: GPUI centres it vertically, NSAlert sits higher |
| PD-15 | #26 card.png: "Binary File" placeholder, header "+0 −0"; the header floats mid-pane above it (Swift layout quirk) (`sp2.png`) | header at the top "+0 −0", the same placeholder (`gp1.png`) | same (GPUI keeps the header at the top) |
| DV-32 / DV-33 (in a PR) | after ⌘1 / ⌘3 by keys: ⌘F opens find, ⌥↓ moves to the next file, plain ↓ does nothing (`sk6`, `sc4`, `sc8`) | the same (`gk6`, `gc7`, `r5.png`) | same |
