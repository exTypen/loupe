//! LOUPE_STRESS=<scenario.json>: the test mode every flow is checked in, and the memory/CPU stress driver
//! (StressDriver.swift, scripts/memtest). The app then:
//! - uses its own settings domain, wiped at launch, and never the real `dev.extypen.loupe` domain or the Keychain.
//!   The domain is `dev.extypen.loupe.flows-gpui.<folder>`, after the scenario file's folder ([`domain_for`]), so
//!   parallel copies on different fixtures don't share their tab or Full mode;
//! - opens the scenario's repos in the default space (the `big` one selected) with an in-memory account for the
//!   mock forge (`forge`, `user`, `token`), and talks plain http to it (ureq allows it);
//! - runs the scenario's steps ([`run`]) and prints `LOUPE_STRESS <event> … t=<secs>` lines on stdout, the protocol
//!   scripts/memtest/measure.py reads. With `"steps": []` (the flows fixture) it waits for the worktrees, sizes the
//!   window to the screen, prints ready / iter / finished and then leaves the app alone.
use std::time::{Duration, Instant};

use gpui_kit::{App, AsyncApp, Entity, Window};
use loupe_core::model::{DEFAULT_SPACE, Space};
use loupe_forge::{MergeStyle, settings};
use serde::Deserialize;

use loupe_core::history::Scope;

use super::{AppEvent, AppModel, Tab};
use crate::diff::DiffSide;
use crate::views::{ci::ChecksOverlay, history::HistoryView, pulls::PullsView};

pub const DOMAIN: &str = "dev.extypen.loupe.flows-gpui";

/// The settings domain of the scenario at `path`: [`DOMAIN`] plus its folder's name, cut to the characters a domain
/// takes (run-swift.sh names its LoupeFlows copy's bundle id the same way).
/// ponytail: two fixtures in same-named folders (/a/flows, /b/flows) share one; add a path hash if that happens.
pub fn domain_for(path: &str) -> String {
    let dir = std::path::Path::new(path).parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug: String = dir.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    if slug.is_empty() { DOMAIN.into() } else { format!("{DOMAIN}.{slug}") }
}

/// StressDriver.Scenario. Missing fields default (the flows fixture and older scenarios leave some out); extra ones
/// (`fixture`, `worktrees`) are ignored.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Scenario {
    pub forge: String,
    pub token: String,
    pub user: String,
    pub repos: Vec<String>,
    pub big: Option<String>,
    pub conflict: String,
    pub submodule: String,
    pub stashes: String,
    pub big_file_worktree: String,
    pub big_file: String,
    pub minified_file: String,
    pub huge_commit: String,
    #[serde(rename = "hugePR")]
    pub huge_pr: u64,
    pub prs: Vec<u64>,
    pub iterations: usize,
    pub pause: f64,
    pub log_seconds: f64,
    pub background: bool,
    pub steps: Vec<String>,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            forge: String::new(), token: String::new(), user: String::new(), repos: vec![], big: None, conflict: String::new(),
            submodule: String::new(), stashes: String::new(), big_file_worktree: String::new(), big_file: String::new(),
            minified_file: String::new(), huge_commit: String::new(), huge_pr: 0, prs: vec![], iterations: 1, pause: 0.5,
            log_seconds: 5., background: false, steps: vec![],
        }
    }
}

/// Reads the scenario named by LOUPE_STRESS and prepares its settings domain. Exits 2 on an unreadable file, as the
/// Swift app did. Must run before anything reads settings.
pub fn from_env() -> Option<Scenario> {
    let path = std::env::var("LOUPE_STRESS").ok().filter(|p| !p.is_empty())?;
    let s: Scenario = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|d| serde_json::from_slice(&d).map_err(|e| e.to_string())) {
        Ok(s) => s,
        Err(e) => { eprintln!("LOUPE_STRESS: can't read {path}: {e}"); std::process::exit(2) }
    };
    let domain = domain_for(&path);
    if !settings::use_fresh_domain(&domain) || settings::shared_domain() != domain {
        eprintln!("LOUPE_STRESS refused: settings were read before the scenario picked its domain");
        std::process::exit(2)
    }
    let mut p = settings::shared();
    let selection = s.big.clone().or_else(|| s.repos.first().cloned());
    let spaces = vec![Space { id: DEFAULT_SPACE.into(), name: "Default".into(), repos: s.repos.clone(), selection }];
    p.set("spaces", Some(plist::Value::Data(serde_json::to_vec(&spaces).unwrap_or_default())));
    p.set_string("tab", "changes");
    p.set_bool("prFullMode", false);
    p.set_string("diffSide", "diff");
    p.set_bool("lfsMissingBannerDismissed", true);
    Some(s)
}

/// Starts the scenario's run once the main window is open; nothing without LOUPE_STRESS.
pub fn run(app: &Entity<AppModel>, cx: &mut App) {
    let Some(s) = app.read(cx).scenario.clone() else { return };
    let big = s.big.clone().or_else(|| s.repos.first().cloned()).unwrap_or_default();
    let r = Run { app: app.clone(), big, s, start: Instant::now() };
    cx.spawn(async move |cx| r.run(cx).await).detach();
}

struct Run {
    app: Entity<AppModel>,
    s: Scenario,
    big: String,
    start: Instant,
}

/// Spaces as underscores: every protocol field is one word.
fn word(s: &str) -> String { s.replace(' ', "_") }

impl Run {
    fn mark(&self, event: &str) {
        use std::io::Write as _;
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "LOUPE_STRESS {event} t={:.2}", self.start.elapsed().as_secs_f64());
        let _ = o.flush();
    }

    async fn pause(&self, cx: &mut AsyncApp, factor: f64) {
        cx.background_executor().timer(Duration::from_secs_f64((self.s.pause * factor).max(0.))).await;
    }

    /// Checks `cond` every 100 ms for up to `timeout` s; says so when it gave up. Only while a step waits: nothing
    /// runs once the run has finished.
    async fn wait(&self, cx: &mut AsyncApp, what: &str, timeout: f64, cond: impl Fn(&AppModel, &App) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs_f64(timeout);
        while !self.app.read_with(cx, |a, cx| cond(a, cx)) {
            if Instant::now() > end { self.mark(&format!("timeout {}", word(what))); return false }
            cx.background_executor().timer(Duration::from_millis(100)).await;
        }
        true
    }

    async fn select(&self, cx: &mut AsyncApp, path: &str, tab: Tab) {
        self.app.update(cx, |a, cx| { a.select(Some(path.into()), cx); a.set_tab(tab, cx) });
        self.pause(cx, 1.).await;
    }

    fn with_window<R>(&self, cx: &mut AsyncApp, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R> {
        let w = cx.update(|cx| cx.windows().first().copied())?;
        w.update(cx, |_, window, cx| f(window, cx)).ok()
    }

    /// The shell's History (1) or Pull Requests (2) view.
    fn view<V: 'static>(&self, cx: &mut AsyncApp, i: usize) -> Option<Entity<V>> {
        let w = cx.update(|cx| cx.windows().first().copied())?.downcast::<crate::shell::Shell>()?;
        w.read_with(cx, |s, _| s.views[i].clone().downcast::<V>().ok()).ok().flatten()
    }

    fn history_view(&self, cx: &mut AsyncApp) -> Option<Entity<HistoryView>> { self.view(cx, 1) }

    /// Waits until `f` holds for History.
    async fn wait_history(&self, cx: &mut AsyncApp, what: &str, timeout: f64, f: impl Fn(&HistoryView, &App) -> bool) -> bool {
        let Some(h) = self.history_view(cx) else { return false };
        self.wait(cx, what, timeout, move |_, cx| f(h.read(cx), cx)).await
    }

    fn set_scope(&self, cx: &mut AsyncApp, scope: Scope) {
        if let Some(h) = self.history_view(cx) { self.with_window(cx, |w, cx| h.update(cx, |h, cx| h.set_scope(scope, w, cx))); }
    }

    async fn run(self, cx: &mut AsyncApp) {
        let repos = self.s.repos.clone();
        self.wait(cx, "worktrees", 60., move |a, _| repos.iter().all(|r| a.worktrees.contains_key(r))).await;
        self.pause(cx, 3.).await;
        // As big as the screen (Swift: setFrame(visibleFrame)): a narrow toolbar clips its controls.
        // ponytail: gpui can't move a window, so only the size follows the visible frame; the origin stays.
        self.with_window(cx, |w, cx| if let Some(d) = w.display(cx) { w.resize(d.visible_bounds().size) });
        cx.background_executor().timer(Duration::from_millis(200)).await; // the new frame arrives asynchronously
        let size = self.with_window(cx, |w, _| w.bounds().size);
        if let Some(sz) = size { self.mark(&format!("window {:.0}x{:.0}", f32::from(sz.width), f32::from(sz.height))) }
        // `background` (behind the owner's windows, never key): main.rs doesn't activate the app.
        self.mark("ready");
        for i in 1..=self.s.iterations.max(1) {
            for step in &self.s.steps {
                let t = Instant::now();
                self.mark(&format!("step-begin iter={i} step={step}"));
                self.perform(cx, step).await;
                self.mark(&format!("step-end iter={i} step={step} secs={:.1}", t.elapsed().as_secs_f64()));
            }
            // The same resting state after every iteration, so samples compare.
            self.select(cx, &self.big, Tab::Changes).await;
            self.pause(cx, 2.).await;
            self.mark(&format!("iter iter={i}"));
        }
        self.mark("finished");
    }

    async fn perform(&self, cx: &mut AsyncApp, step: &str) {
        match step {
            "worktrees" => self.worktrees(cx).await,
            "tabs" => self.tabs(cx).await,
            "tabsonly" => self.tabs_only(cx).await,
            "mergepr" => self.merge_pr(cx).await,
            "idle" => self.pause(cx, 30.).await, // nothing driven: background work only (file events, polls)
            "history" => self.history(cx).await,
            "bigfile" => self.big_file(cx).await,
            "prs" => self.prs(cx).await,
            "hugepr" => self.huge_pr(cx).await,
            "checks" => self.checks(cx).await,
            "merge" => self.merge(cx).await,
            "create" => self.create(cx).await,
            "rebase" => self.rebase(cx).await,
            "stashes" => self.stashes(cx).await,
            "submodule" => self.submodule(cx).await,
            _ => self.mark(&format!("unknown-step {step}")),
        }
    }

    /// Every worktree of every repo, then ⌘K.
    async fn worktrees(&self, cx: &mut AsyncApp) {
        for repo in &self.s.repos {
            let paths: Vec<String> = self.app.read_with(cx, |a, _| a.worktrees_of(repo).iter().map(|w| w.path.clone()).collect());
            for p in paths { self.app.update(cx, |a, cx| a.select(Some(p), cx)); self.pause(cx, 0.5).await }
        }
        self.with_window(cx, |w, cx| w.dispatch_action(Box::new(crate::shell::keys::GoToWorktree), cx));
        self.pause(cx, 2.).await;
        // The switcher closes on Escape: a key sent to the window, never through the system event stream.
        self.key(cx, "escape");
        self.pause(cx, 1.).await;
    }

    fn key(&self, cx: &mut AsyncApp, k: &'static str) {
        if let Ok(k) = gpui_kit::Keystroke::parse(k) { self.with_window(cx, |w, cx| w.dispatch_keystroke(k, cx)); }
    }

    async fn tabs(&self, cx: &mut AsyncApp) {
        for wt in [&self.big, &self.s.conflict] {
            for t in [Tab::Changes, Tab::History, Tab::Pulls, Tab::Changes] { self.select(cx, wt, t).await; self.pause(cx, 1.).await }
        }
    }

    /// Only the view switch, on one worktree: what a person flipping Changes / History / Pull Requests does.
    async fn tabs_only(&self, cx: &mut AsyncApp) {
        if self.app.read_with(cx, |a, _| a.selection() != Some(self.big.as_str())) { self.select(cx, &self.big, Tab::Changes).await }
        for t in [Tab::History, Tab::Pulls, Tab::Changes] { self.app.update(cx, |a, cx| a.set_tab(t, cx)); self.pause(cx, 1.).await }
    }

    async fn sides(&self, cx: &mut AsyncApp, each: f64) {
        for side in [DiffSide::Old, DiffSide::New, DiffSide::Diff] {
            self.app.update(cx, |a, cx| { let l = a.layout; a.set_diff_side(side, l, cx) });
            self.pause(cx, each).await;
        }
    }

    async fn full(&self, cx: &mut AsyncApp, on: bool, then: f64) {
        self.app.update(cx, |a, cx| a.set_full_mode(on, cx));
        self.pause(cx, then).await;
    }

    /// The big commit: Overview, Compact/Full, Diff/Before/After.
    async fn history(&self, cx: &mut AsyncApp) {
        self.select(cx, &self.big, Tab::History).await;
        let (worktree, sha) = (self.big.clone(), self.s.huge_commit.clone());
        self.wait_history(cx, "history", 30., |h, cx| !h.commit_shas(cx).is_empty()).await;
        self.app.update(cx, |a, cx| a.emit(AppEvent::ShowCommit { worktree, sha }, cx));
        self.wait_history(cx, "huge commit", 30., |h, cx| h.file_paths(cx).len() >= 5000).await;
        self.pause(cx, 2.).await;
        if let Some(h) = self.history_view(cx) {
            let paths = h.read_with(cx, |h, cx| h.file_paths(cx));
            for p in paths.into_iter().take(400).step_by(40) {
                self.with_window(cx, |w, cx| h.update(cx, |h, cx| h.select_file(p, w, cx)));
                self.pause(cx, 0.3).await;
            }
        }
        // Swift sets "commitOverview" itself: the switch without its "view-switch" line.
        if self.view_switch(cx, 0) { self.pause(cx, 5.).await; self.view_switch(cx, 1); self.pause(cx, 1.).await }
        self.full(cx, true, 2.).await;
        self.full(cx, false, 1.).await;
        self.sides(cx, 1.5).await;
        self.set_scope(cx, Scope::All);
        self.wait_history(cx, "all branches", 30., |h, cx| !h.commit_shas(cx).is_empty()).await;
        self.pause(cx, 2.).await;
        self.set_scope(cx, Scope::Current);
        self.wait_history(cx, "current", 30., |h, cx| !h.commit_shas(cx).is_empty()).await;
    }

    /// The big file and the minified one in Changes, in every mode; then the Overview of the worktree.
    async fn big_file(&self, cx: &mut AsyncApp) {
        let wt = self.s.big_file_worktree.clone();
        self.select(cx, &wt, Tab::Changes).await;
        let Some(m) = self.app.read_with(cx, |a, _| a.model(&wt)) else { return self.mark(&format!("missing {wt}")) };
        let m2 = m.clone();
        self.wait(cx, "status", 30., move |_, cx| !m2.read(cx).state.files().is_empty()).await;
        for path in [&self.s.big_file, &self.s.minified_file] {
            let id = m.read_with(cx, |m, _| m.state.files().iter().find(|f| &f.path == path && !f.staged).map(|f| f.id()));
            let Some(id) = id else { self.mark(&format!("missing {path}")); continue };
            m.update(cx, |m, cx| m.select_file(Some(id), cx));
            let (m2, p) = (m.clone(), path.clone());
            let shows = move |_: &AppModel, cx: &App| m2.read(cx).diff.as_ref().is_some_and(|d| d.new_path.as_ref().or(d.old_path.as_ref()) == Some(&p));
            self.wait(cx, &format!("diff {path}"), 60., shows).await;
            self.pause(cx, 2.).await;
            self.sides(cx, 2.).await;
            self.full(cx, true, 3.).await;
            self.full(cx, false, 2.).await;
        }
        if self.press_view_switch(cx, 0) { self.pause(cx, 6.).await; self.press_view_switch(cx, 1); self.pause(cx, 1.).await } else { self.mark("no-view-switch changes") }
    }

    /// Big's Pull Requests tab, once its list has at least 100 PRs.
    async fn open_pulls(&self, cx: &mut AsyncApp) {
        self.select(cx, &self.big, Tab::Pulls).await;
        let Some(p) = self.view::<PullsView>(cx, 2) else { return };
        self.wait(cx, "pull list", 90., move |_, cx| p.read(cx).list_count() >= 100).await;
    }

    async fn open_pull(&self, cx: &mut AsyncApp, number: u64, settle: f64) {
        let worktree = self.big.clone();
        self.app.update(cx, |a, cx| a.emit(AppEvent::OpenPull { worktree, number }, cx));
        self.pause(cx, settle).await;
    }

    /// Back to the list (Esc in an open PR).
    async fn back(&self, cx: &mut AsyncApp, then: f64) {
        self.with_window(cx, |w, cx| w.dispatch_action(Box::new(crate::shell::keys::BackToList), cx));
        self.pause(cx, then).await;
    }

    async fn prs(&self, cx: &mut AsyncApp) {
        self.open_pulls(cx).await;
        for &n in &self.s.prs { self.open_pull(cx, n, 4.).await; self.back(cx, 0.5).await }
    }

    /// The forge client for big's remote, and the remote.
    fn client(&self, cx: &mut AsyncApp) -> Option<(loupe_forge::Client, loupe_forge::RemoteRepo)> {
        self.app.read_with(cx, |a, cx| {
            let r = a.model(&self.big)?.read(cx).remote.clone()?;
            Some((a.client_for(&r, &self.big)?, r))
        })
    }

    /// What the Merge button does once confirmed: merge on the forge, leave the PR, show the base branch's History
    /// with the merge on top (`AppModel::show_merged`). Prints what it ended up showing.
    async fn merge_pr(&self, cx: &mut AsyncApp) {
        self.open_pulls(cx).await;
        let Some((client, remote)) = self.client(cx) else { return self.mark("merge-no-open-pr") };
        let (c, r) = (client.clone(), remote.clone());
        let open = cx.background_executor().spawn(async move { c.pulls(&r, "open", 1) }).await.unwrap_or_default();
        let Some(pr) = self.s.prs.iter().find_map(|&k| open.iter().find(|p| p.number as u64 == k).cloned()) else { return self.mark("merge-no-open-pr") };
        let n = pr.number;
        self.open_pull(cx, n as u64, 4.).await;
        let t = Instant::now();
        let r = remote.clone();
        if let Err(e) = cx.background_executor().spawn(async move { client.merge(&r, n, MergeStyle::Merge) }).await {
            return self.mark(&format!("merge-failed {}", word(&e.user_message().unwrap_or_default())));
        }
        self.back(cx, 0.).await;
        let Some(m) = self.app.read_with(cx, |a, _| a.model(&self.big)) else { return };
        let (refs, remote_name) = m.read_with(cx, |m, _| (m.state.refs_version, m.state.remote_name.clone()));
        let base = pr.base.ref_.clone();
        self.app.update(cx, |a, cx| a.show_merged(&self.big, &base, cx));
        let m2 = m.clone();
        let fetched = move |a: &AppModel, cx: &App| { let s = &m2.read(cx).state; a.tab == Tab::History && s.refs_version != refs && s.busy.is_none() };
        let mut ok = self.wait(cx, "merged history", 30., fetched).await;
        let branch = format!("{remote_name}/{base}");
        let (dir, b) = (self.big.clone(), branch.clone());
        let (second, top) = cx.background_executor().spawn(async move {
            let g = |args: &[&str]| loupe_core::git::string(&dir, args).map(|s| s.trim().to_string()).unwrap_or_default();
            (g(&["rev-parse", &format!("{b}^2")]), g(&["log", "-1", "--format=%s", &b]))
        }).await;
        ok &= second == pr.head.sha; // the merge commit, with the PR's head as its second parent, is the branch's newest
        let (tab, error) = self.app.read_with(cx, |a, cx| (a.tab.key(), m.read(cx).state.error.clone()));
        let top = if top.is_empty() { "-".into() } else { top };
        self.mark(&format!("merged pr={n} ok={} secs={:.1} tab={tab} scope={} top={} error={}", if ok { "yes" } else { "no" },
            t.elapsed().as_secs_f64(), word(&branch), word(&top), word(error.as_deref().unwrap_or("none"))));
        self.pause(cx, 3.).await;
    }

    /// The huge PR: files, Full mode, Overview, Conversation, Files.
    async fn huge_pr(&self, cx: &mut AsyncApp) {
        self.open_pulls(cx).await;
        // ponytail: the PR's files and comments are the detail view's state; a pause stands in for Swift's wait.
        self.open_pull(cx, self.s.huge_pr, 8.).await;
        self.full(cx, true, 4.).await;
        self.full(cx, false, 1.).await;
        if self.press_view_switch(cx, 0) {
            self.pause(cx, 8.).await;
            self.press_view_switch(cx, 2);
            self.pause(cx, 4.).await;
            self.press_view_switch(cx, 1);
            self.pause(cx, 1.).await;
        } else { self.mark("no-view-switch pr") }
        self.back(cx, 1.).await;
    }

    /// The checks overlay of a PR (AppEvent::ShowChecks with its head), watched for logSeconds.
    async fn checks(&self, cx: &mut AsyncApp) {
        self.open_pulls(cx).await;
        let n = self.s.prs.first().copied().unwrap_or(self.s.huge_pr);
        self.open_pull(cx, n, 2.).await;
        let Some((client, remote)) = self.client(cx) else { return self.mark("no-check-row") };
        let Ok(pr) = cx.background_executor().spawn(async move { client.pull(&remote, n as i64) }).await else { return self.mark("no-check-row") };
        let worktree = self.big.clone();
        self.app.update(cx, |a, cx| a.emit(AppEvent::ShowChecks { worktree, sha: pr.head.sha, pull: Some(n) }, cx));
        let overlay = |a: &AppModel| a.overlay.clone().and_then(|o| o.downcast::<ChecksOverlay>().ok());
        self.wait(cx, "checks", 30., move |a, cx| overlay(a).is_some_and(|o| o.read(cx).check_count(cx) > 0)).await;
        self.pause(cx, 2.).await;
        match self.app.read_with(cx, |a, _| overlay(a)) {
            Some(o) if o.update(cx, |o, cx| o.open_check_containing("job-01", cx)) => {
                self.wait(cx, "job log", 20., move |_, cx| !o.read(cx).showing_list()).await;
                self.pause(cx, self.s.log_seconds / self.s.pause.max(0.01)).await;
            }
            _ => self.mark("no-check-row"),
        }
        self.app.update(cx, |a, cx| a.dismiss(cx));
        self.pause(cx, 1.).await;
        self.back(cx, 1.).await;
    }

    /// Selects 20 of the conflicted files (each opens the merge editor) and leaves.
    async fn merge(&self, cx: &mut AsyncApp) {
        let wt = self.s.conflict.clone();
        self.select(cx, &wt, Tab::Changes).await;
        let Some(m) = self.app.read_with(cx, |a, _| a.model(&wt)) else { return };
        let m2 = m.clone();
        self.wait(cx, "conflicts", 30., move |_, cx| !m2.read(cx).state.conflicted().is_empty()).await;
        let ids: Vec<String> = m.read_with(cx, |m, _| m.state.conflicted().iter().map(|f| f.id()).collect());
        for k in 0..ids.len().min(20) {
            let id = ids[(k * 7) % ids.len()].clone();
            m.update(cx, |m, cx| m.select_file(Some(id), cx));
            self.pause(cx, 0.8).await;
        }
        self.app.update(cx, |a, cx| a.dismiss(cx));
        self.select(cx, &self.big, Tab::Changes).await;
    }

    async fn create(&self, cx: &mut AsyncApp) {
        let wt = self.app.read_with(cx, |a, _| a.worktrees_of(&self.big).get(1).map(|w| w.path.clone())).unwrap_or_else(|| self.big.clone());
        self.select(cx, &wt, Tab::Changes).await;
        self.app.update(cx, |a, cx| a.emit(AppEvent::CreatePull { worktree: wt }, cx));
        self.pause(cx, 3.).await;
        self.app.update(cx, |a, cx| a.dismiss(cx));
        self.pause(cx, 1.).await;
    }

    /// The rebase planner from commit #25, then closed.
    async fn rebase(&self, cx: &mut AsyncApp) {
        self.select(cx, &self.big, Tab::History).await;
        if !self.wait_history(cx, "history", 30., |h, cx| h.commit_shas(cx).len() > 30).await { return }
        let Some(h) = self.history_view(cx) else { return };
        self.with_window(cx, |w, cx| h.update(cx, |h, cx| h.open_planner_at(25, w, cx)));
        self.pause(cx, 2.5).await;
        self.app.update(cx, |a, cx| a.dismiss(cx));
        self.pause(cx, 1.).await;
    }

    /// The Stashes scope: every 20th stash, then Current again.
    async fn stashes(&self, cx: &mut AsyncApp) {
        let wt = self.s.stashes.clone();
        self.select(cx, &wt, Tab::History).await;
        self.set_scope(cx, Scope::Stashes);
        self.wait_history(cx, "stashes", 30., |h, cx| h.commit_shas(cx).len() >= 200).await;
        let shas = self.history_view(cx).map(|h| h.read_with(cx, |h, cx| h.commit_shas(cx))).unwrap_or_default();
        for sha in shas.into_iter().step_by(20) {
            let worktree = wt.clone();
            self.app.update(cx, |a, cx| a.emit(AppEvent::ShowCommit { worktree, sha }, cx));
            self.pause(cx, 0.5).await;
        }
        self.set_scope(cx, Scope::Current);
        self.pause(cx, 1.).await;
    }

    async fn submodule(&self, cx: &mut AsyncApp) {
        let wt = self.s.submodule.clone();
        self.select(cx, &wt, Tab::Changes).await;
        let Some(m) = self.app.read_with(cx, |a, _| a.model(&wt)) else { return };
        let m2 = m.clone();
        self.wait(cx, "submodule status", 30., move |_, cx| !m2.read(cx).state.files().is_empty()).await;
        let ids: Vec<String> = m.read_with(cx, |m, _| m.state.files().iter().map(|f| f.id()).collect());
        for id in ids { m.update(cx, |m, cx| m.select_file(Some(id), cx)); self.pause(cx, 1.).await }
    }

    /// MS-12: the toolbar's Overview / Files (/ Conversation) switch of the shown tab, as a click would (its
    /// DiffControls callback). Prints "view-switch segment= of=".
    fn press_view_switch(&self, cx: &mut AsyncApp, segment: usize) -> bool {
        let Some(of) = self.app.read_with(cx, |a, _| a.controls().view.as_ref().map(|v| v.0.len())) else { return false };
        if segment >= of { return false }
        self.mark(&format!("view-switch segment={segment} of={of}"));
        self.view_switch(cx, segment)
    }

    fn view_switch(&self, cx: &mut AsyncApp, segment: usize) -> bool {
        let Some((modes, _, set)) = self.app.read_with(cx, |a, _| a.controls().view.clone()) else { return false };
        let Some(&mode) = modes.get(segment) else { return false };
        self.with_window(cx, |w, cx| set(mode, w, cx)).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_stress_driver_format_and_tolerates_extra_or_missing_fields() {
        let s: Scenario = serde_json::from_str(r#"{"forge":"http://127.0.0.1:8765","token":"t","user":"alice",
            "repos":["/a","/b"],"big":"/a","steps":["worktrees","mergepr"],"iterations":50,"hugePR":1,"bigFileWorktree":"/w",
            "logSeconds":2,"prs":[3,4],"fixture":{"x":1}}"#).unwrap();
        assert_eq!((s.forge.as_str(), s.user.as_str(), s.repos.len(), s.big.as_deref()), ("http://127.0.0.1:8765", "alice", 2, Some("/a")));
        assert_eq!((s.huge_pr, s.iterations, s.big_file_worktree.as_str(), s.log_seconds, s.prs.len(), s.steps.len()), (1, 50, "/w", 2., 2, 2));
        let s: Scenario = serde_json::from_str(r#"{"repos":["/x"]}"#).unwrap();
        assert!(s.token.is_empty() && s.big.is_none() && s.steps.is_empty());
        assert_eq!((s.iterations, s.pause), (1, 0.5));
    }

    #[test]
    fn each_fixture_folder_has_its_own_domain() {
        assert_eq!(domain_for("/private/tmp/flows-i-cli/scenario.json"), "dev.extypen.loupe.flows-gpui.flows-i-cli");
        assert_eq!(domain_for("/tmp/my flows_2/s.json"), "dev.extypen.loupe.flows-gpui.my-flows-2");
        assert_eq!(domain_for("s.json"), DOMAIN);
    }

    #[test]
    fn protocol_words_have_no_spaces() { assert_eq!(word("merged history"), "merged_history") }
}
