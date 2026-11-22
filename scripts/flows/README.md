# Flows fixture: one deterministic, human-scale backend for the Swift and GPUI Loupe

The old Swift Loupe and the GPUI port can be driven through the same user flows (docs/gpui/FLOWS.md) on the same
data. No real repos or forge are involved. The memtest harness (scripts/memtest/) is stress-sized; this one is the
realistic version.

| File | What it does |
| --- | --- |
| `mkflows.py <out> [--port 8790]` | Builds the fixture. It is deterministic: fixed authors and dates, so every run makes the same SHAs. |
| `mockforge-flows.py <out> [options]` | Mock Forgejo API v1, git smart HTTP (fetch and push), and the LFS API, all over the fixture. |
| `up.sh <out> [--reset] [forge options]` | Builds the fixture if it is missing, starts the forge in the background, waits for it, and prints the env. |
| `down.sh <out>` | Stops the forge and the Swift copy. The fixture stays. |
| `run-swift.sh <out> [--rebuild]` | Builds `LoupeFlows.app` from `~/loupe` HEAD and launches it on the fixture. |

## Start, stop, reset

```sh
scripts/flows/up.sh /tmp/flows             # build the fixture (~40 s the first time) and start the forge on :8790
scripts/flows/run-swift.sh /tmp/flows      # the Swift app (builds once per ~/loupe HEAD)
scripts/flows/down.sh /tmp/flows           # stop both
scripts/flows/up.sh /tmp/flows --reset     # wipe and rebuild repos, forge data and home, then start
FLOWS_PORT=8800 scripts/flows/up.sh /tmp/f2 # another port, chosen at generation
```

The forge keeps PRs, reviews, comments, CI and locks in memory, so restarting it resets them. Merges and pushes
change the served repos, and only `--reset` undoes those.

### What an app needs (GPUI included)

```sh
export LOUPE_STRESS=<out>/scenario.json   # the scenario below
export HOME=<out>/home                    # .gitconfig with user Alice Mock and the LFS filters
export GIT_CONFIG_NOSYSTEM=1
```

`scenario.json` is a `StressDriver.Scenario` (Sources/Loupe/StressDriver.swift). With `"steps": []` the Swift driver
does three things. It wipes the copy's defaults and writes one space, "Default", holding `repos`. It holds the account
`ForgejoAccount(baseURL: forge, username: user)` and `token` in memory, with no Keychain. It selects `big` once. A
port that reads the same file needs these fields:

```jsonc
{
  "forge": "http://127.0.0.1:8790", "token": "flows-token", "user": "alice",
  "repos": ["<out>/repos/app", "<out>/repos/lib", "<out>/repos/infra"],   // sidebar order, one space
  "worktrees": [...],                       // every worktree path (informational; git worktree list is the truth)
  "big": "<out>/repos/app",                 // initial selection
  // Required by the Swift decoder, unused with no steps:
  "conflict": "...", "submodule": "...", "stashes": "...", "bigFileWorktree": "...", "bigFile": "...",
  "minifiedFile": "...", "hugeCommit": "<sha>", "hugePR": 26, "prs": [22, ...],
  "iterations": 1, "pause": 0.5, "logSeconds": 5, "background": false, "steps": [],
  "fixture": {                              // roles, for test scripts (Swift ignores it)
    "home": "...", "forgeDir": "...",
    "worktrees": {"main", "clean", "dirty", "merge", "unpushed", "squashMerged", "fresh", "localOnly", "cherryPick",
                  "detached", "nested", "locked", "prunable", "lib", "rebase", "noAccount"},
    "takenPaths": [...],
    "pulls": {"acme/app": {"thisBranch", "dirtyWorktree", "conflictWorktree", "rebaseWorktree", "mediumPR", "fork",
                           "prDirTaken", "mine", "open", "closed"}, "acme/lib": {"open", "closed"}}
  }
}
```

All paths are real paths (`/tmp` resolves to `/private/tmp`), because git reports real paths and selection compares
them.

## The fixture

Authors: alice (the account), bob, carol, dave, erin, frank, grace, `renovate[bot]`, and xavier. xavier has no forge
account, so his avatar shows initials. Dates run from 2026-01 to 2026-11-21.

### acme/app: `repos/app` and 11 linked worktrees

- **History:** 284 commits on main and 463 across all refs, by 9 authors. It has 14 `--no-ff` merges
  ("Merge pull request 'x' (#n)"), tags v0.1.0 to v0.6.0 and v1.0.0 (annotated) plus v1.0.0-rc.1 (lightweight), and
  Markdown commit bodies. Other features of the history:
  - a rename (`docs/overview.md` → `docs/intro.md`, with 5 edits) for file history and blame;
  - a gitlink bump;
  - LFS files changed over time, with the object of one old version missing locally (the forge has it);
  - a 20k-line generated TS file;
  - a 100 KB one-line minified JS file.
- **Code, one or more files per language, with cross-file calls for go-to-definition:**
  - Rust (`src/server`): `fetch_user` defined once;
  - Swift (`ios/App`);
  - Go (`services/billing`, with tabs);
  - TS and TSX (`web/src`), with `formatAmount` defined twice on the payments PR;
  - JS;
  - Python (`tools`);
  - C and H (`native`), C++ and HPP;
  - Java (`android`).
- **Other files:**
  - SQL: `db/schema.sql`, `db/migrations/0001..0006`, seeds in `dev/test_data`;
  - YAML: CI workflow, `config/app.yaml`, compose;
  - TOML, JSON, `.env`, `.properties`;
  - manifests and lock files: Cargo.toml/lock, package.json and package-lock, go.mod, pyproject, requirements-dev;
  - Markdown;
  - PNG and SVG, a binary `.woff2`, LFS `*.psd` and `assets/lfs/**` (PNG and `.bin`);
  - a submodule `vendor/acme-lib` → acme/lib;
  - both PR templates, Dockerfile, Makefile, an empty file;
  - natural-sort names (`step1/2/10/Step3`), a deep single-child folder chain, two `index.ts`.
- **Remote:** `origin` = the mock forge. `mirror` = `https://git.example.invalid/...` is a second remote with no
  account. `origin/HEAD` is set, and `origin/gh-pages` is an unrelated branch with 120 commits.

| Worktree (`repos/…`) | Branch | State |
| --- | --- | --- |
| `app` | main | Ahead 2 and behind 5 (one more forge commit isn't fetched yet). Submodule moved ahead. `config/app.yaml` edit that conflicts with stash@{0}. 3 stashes: one plain, one with untracked files (`-u`), one staged+unstaged. Ignored `.direnv/` churn. |
| `app-search` | feature/search-index | Clean and **locked**. PR #22 ("This Branch"), plus fork PR #38 with the same head ref name. |
| `app-billing` | feature/billing-v2 | **Dirty**, behind origin by 1. Changes: staged, unstaged, partially staged (`main.go`), untracked (incl. a new dir, an SVG, a PNG, an empty file), deleted (staged and unstaged), staged rename, mode change (+x), type change (file → symlink), no newline at EOF, an LF→CRLF line, the 20k-line regenerated file, a changed PNG, a binary, LFS modified/added/deleted, a minified one-line change, and a migration (`0007_refunds.sql`) with a `schema.sql` diff. |
| `app-sessions` | feature/sessions | **Stopped merge** of origin/main. Conflicts: content (`db.rs`), add/add (`feature-flags.yaml`), delete/modify (`report.py`), binary (`icon.png`). One file merged cleanly. |
| `app-router` | feature/router-cleanup | 10 commits: 4 pushed, 6 not. Includes `fixup!`/`squash!`/`WIP`, two commits editing the same line (a reorder conflict), and a multi-line message. |
| `app-merged` | fix/readme-wording | Squash-merged on the forge (PR #21); its branch is gone there. Shows the **Merged** badge. |
| `app-fresh` | feature/fresh | Cut from main, no commits of its own. |
| `app-spike` | spike/sqlite-fts | Local only, never pushed (`needsPush`). `.fail-pre-commit` makes the repo's pre-commit hook fail here. |
| `app-cherry` | hotfix/pool | **Stopped cherry-pick** with a conflict. |
| `app-v0.3` | (detached at v0.3.0) | Detached HEAD. |
| `app/.worktree/scratch` | scratch | A linked worktree nested inside the main one (git-ignored). |
| `app-gone` | gone | Deleted with `rm -rf`, so **prunable**. |

Also on disk: `repos/app-existing` and `repos/app-pr-27`, two destination folders that are already taken.

### acme/lib: `repos/lib` and `repos/lib-ct`

A small Rust crate: 8 commits, and the submodule of app. `lib` is on main, **behind origin by 2 and
fast-forwardable**. Its remote sets `lfs.<url>.locksverify false`. `lib-ct` is **stopped at an `edit` step of an
interactive rebase** (`feature/ct-compare` onto main, with a fixup queued). PRs: #2 and #3 open, #1 merged, #4
closed.

### acme/infra: `repos/infra`

A third repo whose remote (`git.example.invalid`) has **no account**: no PRs and no CI ("No Remote"/no-account
flows). It has k8s multi-document YAML, RBAC, a Helm template with `{{ }}` and `|`/`>` scalars, and two CRDs, with
a commit adding the same field to both CRDs plus a type change. It also has an unstaged edit.

### Pull requests on acme/app (17 open, 21 closed or merged)

| # | Branch | By | Notable |
| --- | --- | --- | --- |
| 1–20 | merged feature branches / closed spikes | various | 14 merged (real merge commits in history), 6 closed unmerged; some branches deleted on the forge (`refs/pull/N/head` kept) |
| 21 | fix/readme-wording | alice | squash-merged, branch deleted |
| 22 | feature/search-index | alice | **This Branch** from `app-search`; approved by bob; CI success; checklist body |
| 23 | feature/billing-v2 | alice | **Other Worktrees**; CI **running** (grows ~60 s, then succeeds) |
| 24 | feature/sessions | alice | changes requested; CI failure; not mergeable |
| 25 | feature/router-cleanup | alice | **draft** (`WIP:`), CI **queued** (never starts; Cancel works) |
| 26 | feat/payments-api | bob | **The medium PR**: 52 files, 8 commits, labels, assignees, requested reviewers. Reviews: COMMENT with 4 inline comments (new and old side, one outdated), a reply review on the same lines, REQUEST_CHANGES with 3 inline comments and a SQL code block, a COMMENT reply, and APPROVED. 7 issue comments: code blocks, a quote, a task list, a same-author pair 10 min apart and another 40 min apart, and one using every Markdown construct. CI **failure** (`test` log with file:line errors) plus external statuses. Conflicts with main. |
| 27 | fix/login-timeout | carol | CI success; `repos/app-pr-27` already exists |
| 28 | feat/dark-mode | dave | draft; **mixed** statuses (see CI) |
| 29 | chore/deps-bump | `renovate[bot]` | branched 30 commits back; CI success + a finished **17 MB log** (`e2e`) |
| 30 | docs/api-reference | erin | **no CI at all** |
| 31 | perf/query-cache | bob | CI failure; `flaky` job whose log is always 404 |
| 32 | feat/csv-export | carol | approved |
| 33 | fix/null-invoice-total | dave | **empty body** |
| 34 | feat/webhooks | erin | CI queued |
| 35 | ci/matrix-builds | frank | 60 matrix jobs (statuses paged over 50), 2 running, 1 failing; `mergeable: null`; frank's avatar URL 404s |
| 36 | refactor/error-types | bob | PD-07 reviewer states: alice APPROVED then REQUEST_REVIEW, bob REQUEST_CHANGES, carol APPROVED+dismissed, dave APPROVED+stale; requested [erin, alice] |
| 37 | feat/i18n-strings | **Alice** (other case) | draft; all checks skipped |
| 38 | feature/search-index (fork) | frank | `head.repo_id` 99: same ref name as #22, from another repo |

### CI model

Each PR head sha has one Actions run (id ≠ `index_in_repo`). The run has jobs lint, build, test and web.
- Status `target_url` is `/acme/app/actions/runs/<index>/jobs/<n>`. `web` links to the run only, which exercises the
  name fallback.
- Job `html_url` is `…/jobs/<n>/attempt/<a>`. Older attempts stay in the jobs list.
- The per-job GET has steps. The list doesn't.

Profiles:
- success / failure / running / queued / skipped;
- `external`: codecov, snyk, `staged-ai-review/summary` with an external URL, and a preview status with an absolute
  URL;
- `mixed`: adds warning, error, a skipped status with a relative URL, pending "running…" with no URL, and a
  cancelled job.

A running job starts its clock the first time anything looks at it. It logs `--log-rate` lines/s for `--log-seconds`
(60), then succeeds. A new push to a PR branch starts a fresh running run, as does a PR created from the app.

Logs honour Range (206 + Content-Range, 416 when nothing is new, suffix ranges). `--ignore-range` serves them whole.
The failing log contains:
- a BOM and runner timestamps;
- `##[group]`, `[command]`, `##[error|warning|notice|debug]`;
- "error:", `FAILED`, `thread '…' panicked at`;
- file:line errors, 220 of them inside a folded group;
- ANSI colours, ⭐ and ✅, CRLF, and a lone CR.

Rerun and cancel work like Forgejo/Gitea. `/version` is `17.2.0+gitea-1.22.0`, so Loupe shows Re-run Job, Re-run
All and Cancel. Behaviour:
- A rerun needs the run finished and adds attempt N+1, which runs `--rerun-seconds` (30) and succeeds.
- Cancel needs the run moving.
- A request in the wrong state gets 409; success is 204.

## Endpoints

Every API path in the Swift sources (Forgejo.swift, CIForges.swift, PullModel.swift, PullCreate.swift, LFS.swift,
History.swift) is served. Under `/api/v1`, all need `Authorization: token flows-token` (a bad one gets 401).

| Method | Path |
| --- | --- |
| GET | `/version`, `/user` |
| GET | `/repos/{o}/{r}` (default_branch) |
| GET/POST | `/repos/{o}/{r}/pulls` (state, page/limit, newest first; create: 201 PR JSON, 409 "already exists", 422) |
| GET | `/repos/{o}/{r}/pulls/{n}`, `/pulls/{n}.diff` (`git diff -M base...head` of the served repo) |
| GET/POST | `/pulls/{n}/reviews` (POST: COMMENT/APPROVED/REQUEST_CHANGES with `new_position`/`old_position` comments; 422 for self-approve or reject) |
| GET | `/pulls/{n}/reviews/{id}/comments` (position/original_position, diff_hunk, commit_id) |
| GET | `/pulls/{n}/commits` (oldest first, full messages) |
| POST | `/pulls/{n}/merge` (`Do`: merge / squash / rebase, done for real in the bare repo; 405 if closed or conflicting) |
| GET/POST | `/issues/{n}/comments` |
| GET | `/commits/{sha}/status` (combined, paged with total_count), `/commits/{sha}/statuses` |
| GET | `/commits?sha=` (author user by email, for History avatars) |
| GET | `/actions/runs?head_sha=`, `/actions/runs/{id}`, `/actions/runs/{id}/jobs`, `/actions/jobs/{id}`, `/actions/jobs/{id}/logs` (Range) |
| POST | `/actions/jobs/{id}/rerun`, `/actions/runs/{id}/rerun`, `/actions/runs/{id}/rerun-failed-jobs`, `/actions/runs/{id}/cancel`, `/actions/runs/{rid}/jobs/{jid}/rerun` (Gitea) |
| GET/POST | `/{o}/{r}.git/info/refs`, `git-upload-pack`, `git-receive-pack` (git http-backend: clone, fetch, push; a push moves PR heads and `refs/pull/N/head`) |
| POST | `/{o}/{r}.git/info/lfs/objects/batch`; GET/PUT `/lfs-objects/{oid}` |
| GET/POST | `/{o}/{r}.git/info/lfs/locks`, `locks/verify`, `locks/{id}/unlock` (seeded: bob holds `hero.psd`, carol holds `assets/lfs/banner.png`) |
| GET | `/avatars/{login}.png` (`/avatars/missing/…` is a 404); simple HTML for web URLs (`/{o}/{r}/pulls/…`, `/actions/…`, `/preview/…`) |

Test controls exist for error variants and polling:
- `POST /_mock/fault {"method","path"(regex),"status","body","times"}`, cleared with `DELETE /_mock/fault`. Use it
  for 500/401/403/404/409/422, a failing `/commits`, `/actions/runs` 403, and a failing default-branch request.
- `POST /_mock/version {"version"}`: `"16.0.0"`, `"1.26.0"`, `"500"`, …
- `POST /_mock/pr {"repo","number",…}`: change a title between polls.
- `GET /_mock`: a state summary.

Forge options: `--more-open N`, `--more-closed N` (filler PRs for paging), `--ignore-range`, `--verbose`.

Unknown endpoints answer 404 and are logged to `<out>/forge.log`. POST bodies are logged there too, so review and
create payloads can be checked.

## FLOWS.md §15 coverage

✓ covered · ◐ partly · ✗ not covered (reason).

| §15 need | Fixture piece |
| --- | --- |
| SH-01, RW-09: several spaces, legacy defaults | ✗ StressDriver writes exactly one space and wipes the rest (Swift unchanged); `scenario.json` has one space |
| SH-11: main, `<repo>-` prefixed, detached, locked, ahead/behind, staged+unstaged same file, squash-merged, fresh branch, main on a contained branch | ✓ app, app-*, app-v0.3, app-search (locked), app (↑2↓5), app-billing `main.go`, app-merged, app-fresh, app |
| SH-12: collapsed repo with dirty worktrees | ✓ data (collapse app in the UI); ✗ no pre-collapsed default |
| SH-14/15/22–28: ≥3 repos in a space, ≥2–3 spaces, an empty one | ◐ 3 repos in one space; ✗ multiple spaces (see SH-01) |
| SH-20/29/30/31, ST-17, RW-08: HTTPS remote + account, a second remote without one, an upstream to pull | ✓ origin, `mirror`, origin/main ahead of main, lib behind 2 |
| SH-27, ST-10, ST-12/13: Keychain tokens | ✗ by design the test copy never touches the Keychain |
| SH-37: ignored `.direnv/`, nested `.worktree/x`, `.gitignore` change | ✓ app/.direnv, app/.worktree/scratch, app-billing `.gitignore` |
| RW-01: non-repo, linked worktree, submodule, subfolder | ✓ (any folder under `<out>`, app-*, vendor/acme-lib, app/src); ◐ "repo in another space" needs spaces |
| RW-05: origin/x without local x, origin/y with local y, taken path, checked-out branch | ✓ origin/feat/*, origin/feature/*, `app-existing`, any worktree branch |
| RW-06, RW-07: dirty and locked; removed and prunable | ✓ app-billing, app-search; app-gone (prunable); removing one via `git worktree remove` is a test step |
| ST-14: `/user`, 401 for a bad token, unreachable URL | ✓ /user, 401; unreachable = any closed port; ✗ GitHub-mode `/user` |
| ST-15, ST-03–06, CM-08–12, MG-17, PC-11: `gh` and AI CLIs on PATH | ✗ Loupe looks in /opt/homebrew/bin and the real home first (homeDirectoryForCurrentUser ignores `$HOME`), so a stub in `<out>/home` can't win; needs a PATH-isolated run |
| UP-01–04: appcast | ✗ updates are out of scope (the copy has no feed) |
| MS-01–09: memtest scenario | n/a: that is scripts/memtest |
| CH-01–03: ahead 2, behind fast-forward, diverged | ✓ app-router ↑6, lib ↓2 (ff), app main ↑2↓5 (diverged) |
| CH-04/05: local, remote-only, nested branch names, conflicting dirty file | ✓ many origin/feature/* and feat/*; app main's `config/app.yaml` vs stash |
| CH-08: LFS-missing + uninitialized submodule banners | ✓ linked worktrees have `vendor/acme-lib` uninitialized; banner.png old object missing |
| CH-09–17, 22–27, 32–35, 41/42, DV-01/02: modified, deleted, renamed, untracked, type change, executable, empty file, both-sides, nested dirs, clean worktree | ✓ app-billing (29 entries), app-search clean; ✗ repo with no commits (CH-27, CM-03 first commit, HI-01) |
| CH-14: clean on a branch, and detached | ✓ app-search, app-v0.3 |
| CH-21: submodule with new commits, LFS PNG locked by another user | ✓ app `vendor/acme-lib`; carol's lock on `assets/lfs/banner.png` |
| CH-29/30/36/37: 2+ mixed hunks, "\ No newline" | ✓ `schema.gen.ts`, `main.go`; `go.mod` |
| CM-03–06: pushed commit, failing pre-commit, stopped merge | ✓ app-search HEAD is pushed; `.fail-pre-commit` in app-spike; app-sessions |
| CM-07: interactive rebase at an `edit` stop | ✓ lib-ct |
| SS-01–10: 3 stashes, one with untracked, one conflicting | ✓ app |
| MG-01–03: merge, cherry-pick, revert, rebase with conflicts, all resolved, edit stop | ◐ merge (app-sessions), cherry-pick (app-cherry), edit stop (lib-ct); ✗ revert and rebase-with-conflicts stops (reproduce with app-router reorder) |
| MG-04–13: one file with 3 conflicts and clean regions; diff3 | ◐ `db.rs` has 2 conflicts and merged regions; ✗ 3-conflict file, diff3 variant |
| MG-16, MG-18, MG-21, MG-23–25: guitool, external edits, rename/delete, symlink, submodule, CRLF conflicts | ✗ |
| MG-19, MG-20, MG-22: add/add, delete/modify, binary | ✓ app-sessions (delete/modify in one direction only) |
| FI-01–06: 5+ commits with a rename, binary, md, deleted, untracked, added | ✓ `docs/intro.md`, `assets/icon.png`, app-billing |
| DV-05/06/21, DV-15/16–19: 3+ hunks, 1000+ lines, >5000 changed lines | ◐ `schema.gen.ts` (20k lines, ~120 changed lines in many hunks); ✗ >5000 changed lines, 150k-line file (memtest has both) |
| DV-07, DV-08, DV-09: no-EOL, tabs, long minified line, CRLF-only | ◐ `go.mod` (new side only), Go tabs, `vendor.min.js`, `.gitignore` |
| DV-10, DV-32: intra-line cases, accented text | ◐ natural edits only; ✗ the specific run shapes |
| DV-11, CB-18: one file per highlighted language | ◐ rs swift go ts tsx js py c h cpp hpp java sql yaml toml json md Dockerfile Makefile; ✗ justfile, .envrc, .mm, .pyi, .mts, .cjs |
| DV-12/13, DV-14: binary, untracked binary, PNG; 100% rename, chmod, empty, copy | ✓ woff2, `checkout.png`, icon.png, rename (with edits), +x, empty file; ✗ 100% rename, copy |
| DV-22/34–36, RV-05, RV-11: comments on new and old lines, outdated, drafts, diff_hunk | ✓ PR #26 (drafts are made in the app) |
| DV-29/30, PD-25, CB-09–12: definitions | ◐ `fetch_user` once, `formatAmount` twice; ✗ 2000-reference word, ":" path |
| LF-01–08: LFS PNG modified/added/deleted, missing object, lock server, locksverify false | ◐ all present; ✗ 64 MiB object, no-git-lfs machine |
| SM-01–07: submodule ahead and gitlink commit | ◐ ahead + bump commit; ✗ behind, dirty-only, staged pointer, 5 uninitialized |
| CO-01–22, LN-11–22: SQL migrations | ◐ `0008_payments.sql` (enum + ALTER TYPE, a 13-column table with PK/FK/DEFAULT/long CHECK/unnamed UNIQUE, ADD COLUMN ± IF NOT EXISTS, ALTER TYPE USING, SET DEFAULT, RENAME COLUMN, DROP COLUMN, DROP TABLE, CREATE [UNIQUE] INDEX CONCURRENTLY IF NOT EXISTS, a plpgsql function with RAISE … ERRCODE, a BEFORE UPDATE OF trigger, a materialized view, UPDATE without WHERE), `0003_payments_backfill.sql` (reused version), seed add/remove/edit, `0007_refunds.sql`; ✗ Diesel/Flyway layouts, DROP TABLE a,b, RENAME TO, policies, `DO $$`, nested comments, function >25 lines |
| LN-23: manifests | ◐ Cargo.toml (deps, dev, target, path) + lock with a duplicate crate, package.json (all 4 sections) + lock with nested node_modules, go.mod require block, requirements-dev, pyproject (PEP 621 + Poetry); PR #26 bumps major (tokio, vite), downgrades (axum, vitest), adds >10; ✗ Package.resolved, flake.lock |
| LN-24: config | ✓ JSON/YAML/TOML/.env/.properties, `api_token`, nested groups of 2 and 5 keys, list change (PR #26) |
| LN-25/26: k8s, Helm, CRDs | ◐ repos/infra (multi-doc, RBAC, Helm, `|`/`>`, CRDs with a shared new field + type change); ✗ Expo `app.json`, multi-version CRD, printer columns |
| LN-27 | ✗ |
| LN-28: TODO / `it.only` / `@ts-ignore` | ✓ `legacyFormat.ts`, `format.ts` (PR #26) |
| LN-01–05, LN-10: base moved on, conflicting and clean | ✓ PR #24 and #26 (conflicting), the others (clean) |
| CB-02–06, FT-01–09: nested folders, single-child chain, empty file, LFS pointer, two index.ts, natural sort | ✓; ✗ 3 MB file, empty repo |
| CB-16/17: Swift class, Go type/func/method, minified JS, `.sql` | ✓ |
| QS-01–07 | ◐ one space, 3 repos, detached, merged, `app-` prefixes |
| HI-03, HI-11: avatars, email without account, several authors | ✓ (`/commits?sha=` by email; xavier has none) |
| HI-04/05/06, HI-09: merges, >12 concurrent branches, tags + refs on one commit, `refs/pull/*`, >250 commits | ✓ |
| HI-10/19: stashes | ✓ |
| HI-12/14, HI-13: rename, mode, binary, LFS, md, delete, root commit; Markdown body | ✓ across history, the PR #26 branch and app-billing |
| HI-15/17/18, UN-04/07/08 | ◐ feature branches exist; ✗ purpose-built empty cherry-pick and conflicting-revert commits |
| HI-21, PD-30, SH-39: open PR merged server-side, served over git | ✓ real merge, squash and rebase in the bare repo; fetchable |
| RB-01–13: linear range, same-line reorder conflict, multi-line messages, pushed + unpushed | ✓ app-router; ✗ range with a merge, >1000 linear commits |
| PR-02/03: no remote, no account | ✓ repos/infra (host with no account); ✗ no-remote repo |
| PR-05, PR-09: >51 open, >100 closed, cache file | ✓ via `--more-open 40 --more-closed 90`; the cache file is the app's own |
| PR-06, PC-01: This Branch, Other Worktrees, fork with the same ref | ✓ #22, #23–25, #38 |
| PR-07, PR-08, PR-10: open/closed/merged/draft/comments, avatar variants, Alice vs alice | ✓ (grace has no avatar_url, frank's is a 404) |
| PR-13, PR-17/18, RV-10, PD-05 failure variants | ✓ via `/_mock/pr` and `/_mock/fault` |
| PD-07, RV-04: reviewer states, `renovate[bot]` | ✓ #36, #29 |
| PD-13/14/15, PD-16, PD-20/21: PR with binary, rename, delete, mode-only, LFS; second push; unchanged files; unfetchable head | ◐ #26 (binary, rename, delete); a second push is a test step (push from a worktree moves the PR); ✗ mode-only file in a PR, unfetchable head (use a fault on `git-upload-pack`) |
| PD-28: `pr/N` worktree exists, destination taken | ◐ `repos/app-pr-27` |
| PD-29/31: mergeable true/false/null, merge 405/409, draft | ✓ computed with `merge-tree`, #35 null, 405 on conflict, faults for others |
| RV-02/03/09: timing, empty body, all Markdown | ✓ #26, #33 |
| RV-12–14: drafts from a previous launch | ✗ drafts live in the app's defaults, which StressDriver wipes |
| RV-17: recorded review body, self-approve 422 | ✓ forge.log, 422 |
| FG-04, FG-07, CI-06, CI-39, PC-18 (GitHub 422): GitHub mode | ✗ StressDriver only makes Forgejo accounts; no GitHub-shaped API |
| CI-01/10/13/14: every status kind, no checks, only skipped | ✓ #28, #30, #37 |
| CI-04: version per scenario | ✓ `--version` or `/_mock/version` |
| CI-05: >50 statuses | ✓ #35 |
| CI-07: index ≠ id, attempts, name fallback | ✓ |
| CI-11: `/actions/runs` 403/404 | ✓ via faults |
| CI-15: done vs running cadence | ✓ |
| CI-20/23: steps on the per-job GET only | ✓ |
| CI-21–29: Range, growing log, >16 MB log, Range ignored, 404 logs | ✓ #23/#35, #29, `--ignore-range`, queued jobs and #31 `flaky` |
| CI-25–27: log content | ✓ (except a group split across two tail chunks, which happens only in the growing log) |
| CI-31/38: absolute, relative and no target_url, `staged-ai-review/x` | ✓ #26, #28 |
| CI-32–36: rerun and cancel endpoints, buttons | ✓ (403 variants via faults) |
| PC-01–06: ahead, needsPush, pushed, merged, detached, no account, existing PR + fork | ✓ app-router, app-spike, app-search, app-merged, app-v0.3, infra, #22/#38 |
| PC-07/10, PC-08: templates, default_branch, failing default branch; remote branches + gh-pages | ✓ (failing request via fault); ✗ default branch missing from refs/remotes |
| PC-16/17/18/19: push with token, non-ff rejection, 201 PR JSON, 409, 422/500 | ✓ app-billing push (behind → rejected), create, 409, faults |

## Verified (2026-11-22, Swift Loupe at fde8251)

`up.sh` and `run-swift.sh` were run, then the app was driven through System Events accessibility actions
(cua-driver is not installed on this machine). The shots show, in order:
1. the sidebar with all three repos and every worktree state;
2. Changes on app-billing;
3. History;
4. the PR list with This Branch, Other Worktrees and the fork;
5. PR #26 files with its CI badge and reviewer states;
6. its Conversation;
7. the checks overlay;
8. the failing `test` job log with problem lines and the Re-run buttons;
9. after Re-run Job: attempt 2 running, with Cancel Run.

The Swift app's PR list itself has no CI badges; CI shows in the PR header.
