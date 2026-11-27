# CI parity: Swift LoupeFlows vs GPUI (`app/src/views/ci.rs`)

Both apps ran on the flows mock (port 8804, `/tmp/flows-ci`, Forgejo `17.2.0+gitea-1.22.0`). Swift: Pull Requests →
PR → checks badge. GPUI: at the time the overlay was opened through a scenario-mode hook emitting `AppEvent::ShowChecks`; the hook is
gone now that the PR header shows `ci::badge` (CI-01, verified on #38 in `/tmp/flows-i-views/shots/g-pr.png`). Screenshots are in `/tmp/flows-ci/shots/` (`s-*` Swift, `g-*` GPUI, `t-*` thumbnails).

Fixture PRs used: #26 (failing + external), #28 (mixed: every status kind), #23 (running, growing log), #25 (queued),
#29 (success, 16.36 MB e2e log), #30 (no CI), #31 (404 log), #35 (60 matrix statuses, paged), #37 (all skipped).

Performance checks (GPUI): idle CPU with the overlay open on #28 (5 s cadence, one check running) and with a finished
job's detail open: `ps -o %cpu=,time=` twice 10 s apart → 0.0 %, CPU time unchanged. After "Done" a fault on every
`/status` and `/actions` path saw 0 requests in 35 s (nothing polls while hidden). Detail and overlay tasks are owned
by their views and their forge `Cancel` is fired on drop, so leaving cancels requests in flight.

| ID | Swift: render / reaction | GPUI: render / reaction | status |
|---|---|---|---|
| SH-07 | `ChecksOverlay` ZStack over the split view, 0.15 s fade | `ci::install` presents `ChecksOverlay` through `AppModel::present` over the whole window (sidebar included); no fade | intentional (no animations rule: a 0.15 s fade is the only difference) |
| CI-01 | PR header badge: failing/running/queued/passed counts + "checks", static running dot, tooltip | `ci::badge` in the PR header's second row ("4 checks" on #38, `g-pr.png`); a click opens the overlay | same |
| CI-02 | Badge click → centred card min(w−64,1500)×(h−64), radius 14, 35 % black backdrop (s2-28-list.png) | Same geometry, radius 14, `theme::backdrop()`; opened via `ShowChecks` (g2-28-list.png). Stays over tab/worktree switches because it is an app overlay | same |
| CI-03 | Every PR reload sets `checks.sha` and fires `refresh()` | the PR detail holds `ChecksModel::shared(app, worktree, head sha, Some(n))` while it shows and calls `refresh()` when it shows and after every load; hiding drops it (cancels its request) | same |
| CI-04 | GET /version once per server, Forgejo ≥2 / "+gitea-" / "forgejo" → Forgejo; failure → Forgejo, no writes, not cached | `loupe_forge::ci::CiFeatures::detect` + per-base-URL cache, same table and fallback (unit test `detects_ci_features_from_version`); on the mock both show Re-run Job / Re-run All / Cancel and no Re-run Failed | same |
| CI-05 | Paged `/commits/{sha}/status`, 50 per page until total_count | Same paging; #35 shows all 60 matrix rows in both (g-35-list.png) | same |
| CI-06 | GitHub status + check-runs mapped to rows | Ported in `loupe_forge` (`github_check_runs_jobs_and_log` test); the mock has no GitHub mode | same (unit-tested only; mock can't serve GitHub) |
| CI-07 | Runs + jobs, finished runs not refetched, status → job by index/id, attempt, name fallback | Same linking (`matches_checks_to_jobs` test); #26/#28 rows show the same runs_on, "needs build", attempt captions as Swift | same |
| CI-08 | failure/error/warning → Failing, cancelled job → Skipped, "running"/"started" → Running | Same classes; #28 lists identical sections and counts (5/1/1/3/2) in both apps | same |
| CI-09 | Done / Esc / backdrop close; Esc on detail goes back first; poll task cancelled | Done, Esc and a backdrop click close; Esc on the detail returns to the list first (g2-esc-back.png, g-esc-close.png); the poll `Task` is dropped with the view (0 requests after close, verified with faults) | same |
| CI-10 | Icon (octagon/orange dot/seal), headline, "P of T successful · sha", filter, "Updated …" ticking at 1 Hz, refresh, Done. Orange dot pulses forever | Same row. Running dot is static; "Updated …" is recomputed on each poll render, not by a 1 Hz timer. Zero checks show a dashed circle and "No checks" (g-30-list.png) instead of a green seal and "All checks passed" | fixed-swift-bug (§14 CI.swift:628 endless pulse + CI.swift:639 1 Hz TimelineView; §14 CI.swift:625-631 zero checks read "All checks passed") |
| CI-11 | Error (orange triangle) wins over notice (info, 2 lines, tooltip); 403 on runs → "Forgejo won't list Actions runs…" | Same texts and precedence; a 403 fault on `/actions/runs` shows the notice and rows fall back to "external" (g2-28-403-notice.png) | same |
| CI-12 | "Filter checks" field, case-insensitive substring on context, empty sections hidden, resets on close | Same (g-filter.png); filter state lives in the overlay and is gone when it closes | same |
| CI-13 | Sections "<Title> · n"; row icon, name + workflow, caption (description, runs_on, attempt, needs, external, relative time), chevron, tooltip | Same layout and captions (g2-28-list.png vs s2-28-list.png). Relative time is English ("1 second ago") where Swift follows the system locale ("1 saniye önce") | same |
| CI-14 | Spinner on first load; "No Checks" / "Nothing has reported a status for this commit." | "Loading checks…" text (no spinner animation) on first load; identical "No Checks" empty state (g-30-list.png) | intentional (no endless animation while loading) |
| CI-15 | Refresh then sleep 30 s if all done else 5 s; zero checks poll every 5 s forever | 5 s while anything runs, 30 s once settled; zero checks count as settled (30 s). Verified: #28 with a running status polled every ~5 s | fixed-swift-bug (§14 CI.swift:161-165 empty commit polled every 5 s forever) |
| CI-16 | Refresh button runs an extra refresh, no de-duplication | Refresh button joins the refresh in flight (one at a time for badge, poll, button and PR reloads) | fixed-swift-bug (§14 CI.swift:75-101 overlapping refreshes) |
| CI-17 | Row click opens detail; identity context + job.attempt; duplicate contexts share an id | Same, but each row gets its own key (context, `#2` for duplicates) so duplicates open their own page; a new attempt rebuilds the page (g-rerun-after.png shows attempt 2 with a fresh log) | fixed-swift-bug (§14 CI.swift:9, 585, 607 duplicate context ids) |
| CI-18 | "‹ Checks" back (⌘[), state icon, context, actions, Copy Log, "Open in Forgejo", description, action error | Same header (g2-28-test-detail.png vs s-ci-detail-test.png). Back, Esc and ⌘[ (`BackToList`) return to the list; the overlay holds focus so ⌘[ never reaches the PR detail behind it | fixed-swift-bug (§14 CI.swift:718 ⌘[ bound twice) |
| CI-19 | Job “name”, runs_on, attempt N>1, duration first start → last stop | Same line ("Job “test” · docker · 2m 0s"; Swift "2m 3s" for the mock's clock at that moment) | same |
| CI-20 | Steps from the per-job GET, max 150 pt, icon, name (tertiary if skipped), duration | Same strip and data (g2-28-test-detail.png) | same |
| CI-21 | Log GET with `Range: bytes=-16777216` on Forgejo, offset from Content-Range, problems expanded when failing | Same request and offset (`JobLog::poll_request`/`apply`); problems disclosure starts open on a failing job | same |
| CI-22 | Spinner + "Waiting for a runner…" / GitHub text / "Waiting for the job's log…", retry 5 s, give up after 6 tries with "<Forge> has no log for this job." | Same texts, cadence and give-up (test `gives_up_waiting_for_a_log_after_the_grace`; #31 `flaky` and #25 queued, g-25-list.png / g-31-list.png); a static clock icon instead of a spinner | intentional (no endless spinner during a wait that can last minutes) |
| CI-23 | Every 2 s: job GET + `Range: bytes=<offset>-`, partial lines held back, follow only from the bottom | Same requests and hold-back; the list follows the tail only when it was at the bottom (g-23-tail-a.png → g-23-tail-b.png, steps and duration update) | same |
| CI-24 | First poll that sees the job done does a final read, flushes, stops | Same (test `tails_a_running_job_then_stops_when_it_ends`; g-23-done.png); no task left after that (CPU 0) | same |
| CI-25 | Timestamps/BOM/CRLF stripped, `##[…]` styles, ⭐/✅, problem patterns red; ANSI shown raw, lone CR not handled | Same styling (g2-28-test-detail.png vs s-ci-detail-test.png). ANSI escapes are stripped (and a line coloured red by them counts as a problem), a lone CR keeps what a terminal ends on ("Downloading 100%" vs Swift's three lines) | fixed-swift-bug (§14 CI.swift:366-403 no ANSI / lone-CR handling) |
| CI-26 | Click "▸ title" toggles ▸/▾, groups start folded, split groups stay one | Same (g-fold-open.png; 16 MB #29 "Run e2e" group unfolds at once, g2-29-e2e-unfold.png); folded lines are left out of a virtual list | same |
| CI-27 | "N problem line(s) — click to jump", list ≤200, click unfolds, scrolls, selects the line | Same list and counts ("5 problem line(s)" on #26/#28 test in both); click unfolds the group, centres and highlights the line (g-problem-jump.png) | same |
| CI-28 | "Showing the last 16 MB of the log." + "Load Full Log" | Banner reads "The first X MB of the log aren't shown." + "Load Full Log" (test `loads_earlier_output_in_front`). On the mock both apps show no banner for #29: its log is 16.36 MB, under the 16 MiB tail (g2-29-e2e-16mb.png) | fixed-swift-bug (§14 CI.swift:778 banner always says 16 MB even after a trim) |
| CI-29 | Live tail over 24M chars drops the oldest quarter; Load Full Log reloads all | Same trim and reload (test `live_tail_drops_its_oldest_output`); not reachable on the mock (a running log is ~1 MB) | same (unit-tested) |
| CI-30 | Copy Log copies the formatted text incl. folded bodies, no toast | Same text (`copy_text`: ▸/▾ headers, folded bodies included, escapes stripped), no toast. ⌘C copies selected lines | same |
| CI-31 | "Open in <Forge>" / "Open Details", relative URL gets the base URL | Same buttons and URL resolution (`Check::web_url`), opens with `cx.open_url` | same |
| CI-32 | Re-run Job when job and run are done; spinner label, buttons disabled, refresh after | Same condition; label becomes "Working…" and buttons dim while busy (g-rerun-busy.png); after success the page rebuilds on attempt 2 and tails (g-rerun-after.png) | intentional (static "Working…" label instead of a spinner) |
| CI-33 | Re-run Failed Jobs shown whenever the run is failing, even while it still runs | Shown only once the run is done (never on Forgejo; test `rerun_failed_waits_for_the_run_to_finish`) | fixed-swift-bug (§14 CIForges.swift:57 offered on a running run) |
| CI-34 | Re-run All Jobs when the run is done | Same (#26 test detail shows it in both) | same |
| CI-35 | Cancel Run while the run moves, no confirmation | Same; on #23/#25 Cancel stops the run and the page shows the cancelled state (g-cancel-after.png) | same |
| CI-36 | Red action error: GitHub 403/404 text, "<Forge> refused this (403)…", else raw | Same texts (`CiFeatures::explain`, unit-tested in `offers_rerun_and_cancel_by_state`); other errors show the raw message | same |
| CI-37 | GitHub check-run summary (Markdown) and annotations | Ported (summary via `TextView::markdown`, annotations fetched off-thread); the mock has no GitHub mode | same (not exercisable on the mock) |
| CI-38 | "External Status" / "Job Not Loaded" empty page with "Open Details" | Same titles, texts and button (g-external.png vs s-external.png, g-external-codecov.png) | same |
| CI-39 | GitHub log 302 to blob host, Authorization dropped cross-host | `loupe_forge` client follows redirects with `RedirectAuthHeaders::SameHost` (Authorization dropped cross-host); the mock has no GitHub mode | same (unit-tested only) |

## Final pass (merged app, branch gpui-z-forge)

Both apps on the merged build, mock on :8836 (Forgejo 17.2.0+gitea-1.22.0), shots in `/tmp/flows-z-forge/shots/`
(`g-*` GPUI, `s-*` Swift). The overlay is now opened only through the PR header badge, in both apps.

| ID | Final result | Shots |
|---|---|---|
| SH-07 | overlay hosted by `ci::install`; **fixed**: it covered the toolbar (centred in the whole window). Swift's lives in the content view under the toolbar, so the tabs stay usable; it now starts under the 42 pt title bar and is the content height − 64 | `g-ci26.png`, `s-ci26.png` |
| CI-01 | #26 "⊗2 ◌1 ✓5 checks", #39 "✓ 4 checks", #30 "◌ Checks" in both | `g-pr26.png`, `s-pr26.png`, `g-30.png` |
| CI-02 | badge click opens the card | `g-ci26.png`, `s-ci26.png` |
| CI-03 | badge updates with each PR load; **fixed**: a PR opened by number right after Create refreshed with an empty sha (`GET …/commits//status` 404 in forge.log); refresh now waits for the head | `g-pc17.png` |
| CI-04 | Re-run Job / Re-run All / Cancel shown, no Re-run Failed, in both | `g-ci18-test.png`, `s-ci18-test.png` |
| CI-05..08 | #26: identical sections, counts (2/1/5), captions and classes | `g-ci26.png`, `s-ci26.png` |
| CI-09 | backdrop click closes in both; Esc after clicking the header goes back to the list in both; Esc with focus in the log view does nothing in either | `g-ci09-back2.png`, `s-ci09-back2.png`, `g-ci09-bd.png`, `s-ci09-bd.png` |
| CI-10 | "2 failing · 1 queued", "5 of 8 successful · e86169cc" in both | `g-ci26.png` |
| CI-11 | no notice on the mock (unchanged row) | — |
| CI-12 | "lint" filters to one row in both. "LINT" matches nothing in Swift on a Turkish-locale Mac (`I` lowercases to `ı`), one row in GPUI: fixed-swift-bug (new finding, not in §14) | `g-ci12.png`, `s-ci12.png`, `s-ci12b.png` |
| CI-13 | same rows; **fixed**: GPUI said "0 seconds ago" where Swift's named relative time says "now" ("şimdi") | `g-ci09-back2.png`, `s-ci02.png` |
| CI-14 | #30: "No checks" header + "No Checks" empty state (recorded fix) | `g-ci14.png` |
| CI-15/16 | 30 s settled cadence: 0.03 s CPU in 10 s with the overlay open | — |
| CI-17/18 | row opens the detail; header, actions, description | `g-ci18-test.png`, `s-ci18-test.png` |
| CI-19/20 | Job “test” · docker · duration; 5 steps with icons | same |
| CI-21/25 | same log, colours and problem list (5); lone CR difference is the recorded fix | same |
| CI-26 | "▸ Set up job" unfolds to "Job test on docker" in both | `g-ci26-fold.png`, `s-ci26-fold.png` |
| CI-27 | click a problem: group unfolds, line highlighted, in both | `g-ci27.png`, `s-ci27.png` |
| CI-28/29 | unchanged (unit-tested) | — |
| CI-30/31 | Copy Log / Open in Forgejo present in both | `g-ci18-test.png` |
| CI-32 | Re-run Job → attempt 2 running, page rebuilt, live tail; Re-run All → attempt 4 | `g-ci32-after.png`, `g-ci35-cancel.png` |
| CI-23/24 | tail grows every 2 s and stops when the job ends | `g-ci23-tail.png` |
| CI-33/34 | as above | — |
| CI-35/36 | Cancel on a run that had just finished: "Forgejo 409: run is not running" in red; buttons switch to Re-run after the next poll | `g-ci35-cancel.png`, `g-ci36-later.png` |
| CI-37/39 | GitHub-only, unit-tested (unchanged) | — |
| CI-38 | security/snyk: "External Status" + "Open Details" in both | `g-ci38.png`, `s-ci-test-a4.png` |

Idle CPU with the overlay open on #30 (settled): `0.0 0:03.88` → `0.0 0:03.91` over 10 s.
