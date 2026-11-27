# Parity: PR create (PC-01..20, SH-08, SH-20)

Both apps ran on the flows mock (`FLOWS_PORT=8806`, `/tmp/flows-prcreate`). Screenshots are in
`/tmp/flows-prcreate/shots/` (`swift-*` = LoupeFlows, `gpui-*` = GPUI; `gpui-v2-*` are from the final build).
The GPUI sheet was opened with `LOUPE_FLOWS_CREATE_PULL=<worktree>` (scenario mode only), because the three
entry points live in frozen or other owners' files (see "Requests" below). The offer rule behind them was checked
on the fixture with a temporary probe that printed `offer()` / `show_menu_item()` for every worktree.

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| PC-01 | Banner never shows: its `.task` hangs off an empty `Group`, so the check never runs (spike is 1 ahead, no PR, still no banner: `swift-pc01-spike.png`, `swift-pc01-fresh.png`, `swift-pc01-router.png`) | `pr_create::banner(&model, cx)` renders icon, "N commit(s) on <b> that isn't/aren't on <target>", tooltip, "Create Pull Request…", ×; 8 % accent band + divider. Rule verified by probe: offered for spike (1), cherry (1), fresh (2), scratch (2); not for router/billing/search/sessions (open PR), merged, v0.3 (detached), main. Not on screen yet: `views/changes.rs` doesn't call it | fixed-swift-bug (new finding: the Swift banner never shows). On screen above Changes: `g-spike.png` "1 commit on spike/sqlite-fts that isn't on main" |
| PC-02 | n/a (banner never shows) | Button emits `AppEvent::CreatePull`, opening the sheet | same (host wired in Changes) |
| PC-03 | n/a (banner never shows); dismissal permanent | × appends "<path>\t<branch>" to `prBannerDismissed` (Swift key). Opening the sheet for that branch removes the line again (`with_line` unit test) | same (host wired); undismiss is fixed-swift-bug (§14 PullCreate.swift:403) |
| PC-04 | Re-check on `.branchPushed` / sheet close | Offer is cached per "headOID refsVersion branch client merged"; a push bumps `refs_version`; every sheet close calls `offer::invalidate(path)`; a key change cancels the in-flight check (`Cancel`) | same (host wired); fixed-swift-bug for the cache key after the in-sheet push (§14 PullCreate.swift:255) |
| PC-05 | "+" toolbar button; checks only the loaded list and ignores `isMerged` (`swift-pc05-list.png`) | `pr_create::can_create(&model, cx)`: same rule as the banner (forge open-PR lookup, merged excluded), checked once per HEAD/refs key | fixed-swift-bug (§14 PullRequestsView.swift:67). "+" after the filter, tooltip "Create a pull request for spike/sqlite-fts", opens the sheet (`g-pulls.png`, `g-plus.png`) |
| PC-06 | Menu item shows for any branch with a client, even with an open PR or nothing ahead (`swift-sh20-menu.png`, `swift-sh20-menu-afterpr.png`) | `pr_create::show_menu_item(&model, cx)`: shown while unknown (menus build synchronously), hidden once known to have nothing to create or an open PR. Probe: true only for spike/cherry/fresh/scratch | fixed-swift-bug (§14 App.swift:371-374). Asked when the menu opens: spike shows it (`g-wtmenu.png`), merged doesn't (`g-wtmenu2.png`) |
| SH-08 | `.sheet(item:)` hosts PullCreateSheet | `pr_create::install` subscribes to `CreatePull` and `present`s the sheet over a backdrop; Esc/Cancel `dismiss` | same |
| SH-20 | Divider + "Create Pull Request…" in the worktree menu (`swift-sh20-menu.png`) | See PC-06 | same (see PC-06) |
| PC-07 | "New Pull Request", 560 wide, spinner while loading; spike: branch + "pushed first", Into main, title = subject, template body, 1 commit (`swift-pc07-spike.png`) | Same fields and values in a card with a header strip; spinner while loading (`gpui-v2-pc07-spike.png`, `gpui-pc07-fresh.png`). No-client and detached errors as in Swift | same |
| PC-08 | Menu picker of bases; change reloads commits/title/body; stale answers dropped (`swift-pc08-picker.png`, `swift-pc08-webhooks.png`) | Select with the same list; change reloads in the background, stale results dropped by base compare, task replaced on each change (`gpui-pc08-picker.png`, `gpui-pc08-webhooks.png`, `gpui-pc08-ghpages.png`: 284 commits, capped list). An unfetched base shows "<remote>/<base> hasn't been fetched." + Fetch instead of a silent "No commits…" (not in the fixture, so not exercised live) | fixed-swift-bug (§14 PullCreate.swift:207-209) |
| PC-09 | One commit: subject; else humanized branch ("Router cleanup"); follows base until edited | Same (`loupe_forge::pull_create::title/humanize`, auto value tracked in `PullCreate`) | same |
| PC-10 | Template, else single commit body; follows base until edited; "Description" placeholder | Same; Textarea placeholder "Description" | same |
| PC-11 | Sparkles when a harness exists; spinner while writing; reply always overwrites an edited body | Same button/tooltip/spinner, disabled with no commits or while working. Body is replaced only while unedited (or empty). Not run live (it calls the real AI CLI) | fixed-swift-bug (§14 PullCreate.swift:243) |
| PC-12 | Draft toggle with forge-specific tooltip; GitHub `draft:true`, else "WIP: " prefix (`swift-pc12-draft.png`) | Checkbox with the same tooltips; `client::new_pull` builds the same body (`gpui-pc12-draft.png`) | same |
| PC-13 | "N commits", sha7 + subject rows, tooltip subject+body, "and N more", empty text (`swift-pc13-router.png`) | Same (`gpui-pc13-router.png`) | same |
| PC-14 | Branch mono, middle-truncated, tooltip; "pushed first" when `merge-base --is-ancestor` fails | Same (end-truncated: gpui has no middle truncation) | same |
| PC-15 | "Push and Create" / "Create Pull Request", disabled while loading/working/empty base/no commits/blank title; spinner beside it | Same labels and rule (`PullCreate::can_create`); Return in Title creates; dimmed when disabled | same |
| PC-16 | `push --set-upstream` with the space's credentials, then refresh; rejection shows git's stderr in red, label stays (`swift-pc16-pushing.png`, `swift-pc16-rejected.png`) | Same push and env (`app.git_auth`); rejection shows the same stderr, label stays (`gpui-pc16-rejected.png`). Success also bumps `refs_version` so the PR-tab/banner offer key moves. Live: spike pushed, `origin/spike/sqlite-fts` = HEAD 88a03b7 | same (plus the §14 PullCreate.swift:255 refs bump) |
| PC-17 | POST /pulls, sheet closes, worktree selected, Pull Requests tab, new PR #39 opened (`swift-pc17-after.png`). The flows mock decodes fine here (the old memtest mock in §14 didn't) | Same POST body (forge log), sheet closes, `AppEvent::OpenPull{worktree, number}` selects spike and switches to Pull Requests (`gpui-v2-pc17-after.png`). The PR detail itself is the Pulls owner's view (a stub on this branch) | same |
| PC-18 | At load or on 409/422: red "A pull request for this branch already exists." + "#N title" + "Open Pull Request" (`swift-pc18-load.png`, `swift-pc18-create.png`) | Same section and texts (`gpui-pc18-load.png`, `gpui-pc18-create.png`); Open emits `OpenPull` (`gpui-pc18-open.png`) | same |
| PC-19 | Red "Forgejo 422: branch feat/webhooks does not exist" (`swift-pc19-error.png`); Gitea also labelled Forgejo | Same text (`gpui-pc19-error.png`); error cleared at each create. Text isn't selectable (gpui text) Gitea servers now read "Gitea 422: …": `Client::server_name` asks GET /version once per server (test `gitea_servers_are_named_gitea`; the mock is Forgejo, so the shot is unchanged) | fixed-swift-bug (§14 Forgejo.swift:208) |
| PC-20 | Cancel/Esc close; an in-flight push/create still navigates later (`swift-pc20-cancel.png`, `swift-pc20-esc.png`) | Cancel/Esc close and re-check the offer; the sheet's task is dropped with it, so a create that lands later no longer navigates (a push already started still finishes) (`gpui-pc20-esc.png`) | fixed-swift-bug (§14 PullCreate.swift:363) |

Performance: with the sheet open, `ps -o %cpu=,time=` read `0.0 0:00.32` twice, 10 s apart; with the app idle after
all offer checks, `0.0 0:00.30` twice. Nothing polls: the offer runs once per key; the sheet loads once and
reloads only on a base change, and closing it drops its tasks.

## Requests (frozen / other owners' files)

- Done by the views integrator: the sidebar menu item (SH-20), the Changes banner (PC-01..04) and the Pulls '+' (PC-05).
- Done (i-core): `loupe_forge::Client::is_gitea` / `server_name` label Gitea servers "Gitea" (PC-19).

## Final pass (merged app, branch gpui-z-forge)

Both apps on the merged build, mock on :8836, shots in `/tmp/flows-z-forge/shots/` (`g-*` GPUI, `s-*` Swift). Every
entry point is now wired in the merged app, so the `LOUPE_FLOWS_CREATE_PULL` hook was not needed.

| ID | Final result | Shots |
|---|---|---|
| PC-01 | same as above: GPUI banner "1 commit on spike/sqlite-fts that isn't on main" above Changes, under it the submodule banner; Swift still shows none (§14) | `g-pc01.png`, `s-pc01-spike.png` |
| PC-02 | banner button opens the sheet | `g-pc07.png` |
| PC-03 | × hides it and writes `<path>\tfeature/fresh` to `prBannerDismissed`; opening the sheet from the menu brings it back | `g-pc03.png`, `g-pc07-fresh.png` |
| PC-04 | after the in-sheet push and create the spike banner is gone (open PR #39) | `g-pc17.png` |
| PC-05 | **fixed**: the list button read as a bare "+"; Swift shows a bordered "+ Create Pull Request…". Now the same label (shared: `views/pulls.rs`). Shown on fresh, not on main | `s-pc05-fresh.png`, `g-pc05-fresh.png` |
| PC-06 | menu item opens the sheet without changing the selection (spike, fresh, scratch) | `g-pc06-menu.png`, `g-scr-menu.png`, `s-pc06-menu.png` |
| PC-07 | spike: Branch + "pushed first", Into main, title = subject, template body, 1 commit, "Push and Create" in both | `g-pc07.png`, `s-pc07.png` |
| PC-08 | picker lists the remote branches; picking feat/webhooks reloads; Esc with the menu open closes the menu only | `g-pc08.png`, `g-pc08-wh.png`, `g-pc08-esc.png` |
| PC-09 | one commit: subject; fresh (2 commits): "Fresh"; scratch: "Scratch" | `g-pc07-fresh.png` |
| PC-10 | template body in both | `g-pc07.png`, `s-pc07.png` |
| PC-11 | sparkles present (stub harness); not run (prior row stands) | `g-pc07.png` |
| PC-12 | Draft on a Forgejo server sent title "WIP: Fresh" (forge.log) | `g-pc19.png` (PR #40 "WIP: Fresh") |
| PC-13 | "1 commit"/"2 commits", sha7 + subject | `g-pc07.png`, `g-pc07-fresh.png` |
| PC-14 | "pushed first" for spike and fresh, absent for scratch | `g-scratch-sheet.png` |
| PC-15 | labels follow needsPush ("Push and Create" / "Create Pull Request") | `g-pc19.png` |
| PC-16 | spike pushed with the space credentials, then created | forge.log, `g-pc17.png` |
| PC-17 | POST body as Swift; sheet closes, spike selected, Pull Requests tab, PR #39 open. **fixed**: the PR opened by number asked `commits//status` (empty sha) before its JSON arrived; the checks model now waits for a head (`views/ci.rs`) | `g-pc17.png`, `s-pr39.png` |
| PC-18 | 409 fault on create: red "A pull request for this branch already exists." | `g-pc18.png` |
| PC-19 | 500 fault: "Forgejo 500: database is locked" | `g-pc19.png` |
| PC-20 | **fixed**: Esc did nothing once focus sat in the Title field or after a click on Draft/a button (the input and checkbox took Esc before the sheet's bubble handler). Esc is now captured for the whole sheet, except while the base menu is open (that Esc closes the menu, the next one the sheet). Separately, after any sheet or overlay closed nothing had focus, so the toolbar's gear/Fetch/Pull/Push dispatched into nothing (shared: `shell/mod.rs` gives the window its focus back) | `g-pc20-l.png`, `g-pc08-esc2.png` |
| SH-08 | sheet hosted over the window with a backdrop | `g-pc07.png` |
| SH-20 | see PC-06 | `g-pc06-menu.png` |

Idle CPU with the sheet open and after closing it: `0.0`, CPU time unchanged over 10 s.
