# Parity: PR create (PC-01..20, SH-08, SH-20)

Both apps ran on the flows mock (`FLOWS_PORT=8806`, `/tmp/flows-prcreate`). Screenshots are in
`/tmp/flows-prcreate/shots/` (`swift-*` = LoupeFlows, `gpui-*` = GPUI; `gpui-v2-*` are from the final build).
The GPUI sheet was opened with `LOUPE_FLOWS_CREATE_PULL=<worktree>` (scenario mode only), because the three
entry points live in frozen or other owners' files (see "Requests" below). The offer rule behind them was checked
on the fixture with a temporary probe that printed `offer()` / `show_menu_item()` for every worktree.

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| PC-01 | Banner never shows: its `.task` hangs off an empty `Group`, so the check never runs (spike is 1 ahead, no PR, still no banner: `swift-pc01-spike.png`, `swift-pc01-fresh.png`, `swift-pc01-router.png`) | `pr_create::banner(&model, cx)` renders icon, "N commit(s) on <b> that isn't/aren't on <target>", tooltip, "Create Pull Request…", ×; 8 % accent band + divider. Rule verified by probe: offered for spike (1), cherry (1), fresh (2), scratch (2); not for router/billing/search/sessions (open PR), merged, v0.3 (detached), main. Not on screen yet: `views/changes.rs` doesn't call it | missing (host: Changes owner must call `banner`; logic done, also fixes the never-shown Swift banner, a new finding not in §14) |
| PC-02 | n/a (banner never shows) | Button emits `AppEvent::CreatePull`, opening the sheet | missing (same host as PC-01) |
| PC-03 | n/a (banner never shows); dismissal permanent | × appends "<path>\t<branch>" to `prBannerDismissed` (Swift key). Opening the sheet for that branch removes the line again (`with_line` unit test) | missing (host); undismiss is fixed-swift-bug (§14 PullCreate.swift:403) |
| PC-04 | Re-check on `.branchPushed` / sheet close | Offer is cached per "headOID refsVersion branch client merged"; a push bumps `refs_version`; every sheet close calls `offer::invalidate(path)`; a key change cancels the in-flight check (`Cancel`) | missing (host for the banner); fixed-swift-bug for the cache key after the in-sheet push (§14 PullCreate.swift:255) |
| PC-05 | "+" toolbar button; checks only the loaded list and ignores `isMerged` (`swift-pc05-list.png`) | `pr_create::can_create(&model, cx)`: same rule as the banner (forge open-PR lookup, merged excluded), checked once per HEAD/refs key | missing (host: Pulls owner's toolbar); rule is fixed-swift-bug (§14 PullRequestsView.swift:67) |
| PC-06 | Menu item shows for any branch with a client, even with an open PR or nothing ahead (`swift-sh20-menu.png`, `swift-sh20-menu-afterpr.png`) | `pr_create::show_menu_item(&model, cx)`: shown while unknown (menus build synchronously), hidden once known to have nothing to create or an open PR. Probe: true only for spike/cherry/fresh/scratch | missing (host: shell sidebar menu is frozen); rule is fixed-swift-bug (§14 App.swift:371-374) |
| SH-08 | `.sheet(item:)` hosts PullCreateSheet | `pr_create::install` subscribes to `CreatePull` and `present`s the sheet over a backdrop; Esc/Cancel `dismiss` | same |
| SH-20 | Divider + "Create Pull Request…" in the worktree menu (`swift-sh20-menu.png`) | See PC-06 | missing (host: shell frozen; one-line request below) |
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
| PC-19 | Red "Forgejo 422: branch feat/webhooks does not exist" (`swift-pc19-error.png`); Gitea also labelled Forgejo | Same text (`gpui-pc19-error.png`); error cleared at each create. Text isn't selectable (gpui text) | same (Gitea label stays: it comes from `loupe_forge::Client`, a frozen crate) |
| PC-20 | Cancel/Esc close; an in-flight push/create still navigates later (`swift-pc20-cancel.png`, `swift-pc20-esc.png`) | Cancel/Esc close and re-check the offer; the sheet's task is dropped with it, so a create that lands later no longer navigates (a push already started still finishes) (`gpui-pc20-esc.png`) | fixed-swift-bug (§14 PullCreate.swift:363) |

Performance: with the sheet open, `ps -o %cpu=,time=` read `0.0 0:00.32` twice, 10 s apart; with the app idle after
all offer checks, `0.0 0:00.30` twice. Nothing polls: the offer runs once per key; the sheet loads once and
reloads only on a base change, and closing it drops its tasks.

## Requests (frozen / other owners' files)

- `shell/mod.rs` worktree row `context_menu`: append, when `pr_create::show_menu_item(&model, cx)` (model =
  `app.model(&path)`), `.separator().item(PopupMenuItem::new("Create Pull Request…").on_click(|_, _, cx|
  app.update(cx, |a, cx| a.emit(AppEvent::CreatePull { worktree: path }, cx))))` (SH-20, PC-06).
- `views/changes.rs`: above the list, unless the operation/conflict banner shows,
  `.children(pr_create::banner(&model, cx))` (PC-01..04).
- `views/pulls.rs` list toolbar: a "plus" icon button after the filter, shown when
  `pr_create::can_create(&model, cx)`, tooltip "Create a pull request for <branch>", emitting `CreatePull` (PC-05).
- `loupe_forge::Client::name()` says "Forgejo" for Gitea servers (§14 Forgejo.swift:208); a `gitea` flag would fix
  PC-19's label.
