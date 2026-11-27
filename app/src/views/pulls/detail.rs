//! One open pull request (PullModel.swift's PullRequestModel and PullRequestsView.swift's detail): the header, the
//! Files tab (tree with viewed state, the diff with review comments and drafts, Full mode's whole files, tabs and code
//! navigation), Overview and Conversation, the review popover, merge and checkout.
//!
//! Rules kept from the Swift app's last commits: it loads on showing unless loaded in the last 30 s, then every 60 s
//! while shown, and on activation, ⌘R and after a review. One load at a time; a request while one runs runs once more
//! after it. An unchanged diff isn't parsed again, so a poll keeps scroll, selection and highlighting. Hidden, it
//! doesn't poll, and its loads are cancelled.
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
    uniform_list,
    component::{
        Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
        tooltip::Tooltip,
    },
};
use loupe_core::diff as core;
use loupe_core::file_tree::{self, TreeRow};
use crate::views::ci::{self, ChecksModel};
use loupe_forge::pull::{self, DiffSource, LocalCheckout, PullFiles, ReviewerState};
use loupe_forge::{Cancel, Client, DraftComment, IssueComment, MergeStyle, PrCommit, PullRequest, RemoteRepo, Review, ReviewComment, ReviewEvent, User};

use super::{avatar::avatar, overview};
use crate::diff::{self as rdiff, CodeAction, DiffSide, DiffView};
use crate::shell::keys::{BackToList, CloseWindow, MarkViewed, NavigateBack, NavigateForward, NextFile, PreviousFile};
use crate::state::{AppModel, DiffControls, Tab, ViewMode, adapt};
use crate::ui::tabs::{self, CodeNav, NavHit, NavResults, TabEvent};
use crate::{theme, ui};

pub enum DetailEvent { Back, Merged { base: String } }

pub struct Composer { pub input: Entity<TextareaState>, pub sending: bool, pub clear: bool, _sub: Subscription }

struct ReviewPopover { input: Entity<TextareaState>, event: ReviewEvent, sending: bool, _sub: Subscription }

pub struct PullDetail {
    pub app: Entity<AppModel>,
    pub worktree: String,
    pub pr: PullRequest,
    pub client: Client,
    pub repo: RemoteRepo,
    pub local: LocalCheckout,
    pub reviews: Vec<Review>,
    pub conversation: Vec<IssueComment>,
    pub commits: Vec<PrCommit>,
    pub comments: Vec<ReviewComment>,
    /// Bumped whenever the timeline's inputs change: the conversation rebuilds only then.
    pub data_version: u64,
    diffs: Vec<Arc<core::FileDiff>>,
    index: HashMap<String, usize>,
    source: DiffSource,
    files: PullFiles,
    pub drafts: Vec<DraftComment>,
    pub loading: bool,
    reload_queued: bool,
    load: Option<(Cancel, Task<()>)>,
    pub error: Option<String>,
    /// The last load failure: shown once as an alert, then as a mark in the header until a load works again.
    load_error: Option<String>,
    pub last_loaded: Option<Instant>,
    /// The header's checks (CI-01): ci's shared model for this worktree, held while the PR shows.
    checks: Option<(Entity<ChecksModel>, Subscription)>,
    pub tab: ViewMode,
    collapsed: HashSet<String>,
    full_collapsed: HashSet<String>,
    rows: Option<Rc<Vec<TreeRow>>>,
    filter: Entity<InputState>,
    adapted: Option<(u64, Arc<rdiff::FileDiff>)>,
    diff_view: Option<Entity<DiffView>>,
    pending_reveal: Option<(String, u32)>,
    tree: Vec<String>,
    tree_sha: Option<String>,
    tree_error: Option<String>,
    tree_dirs: HashSet<String>,
    collapse_applied: bool,
    _tree: Option<Task<()>>,
    full: HashMap<String, Result<Arc<rdiff::FileDiff>, String>>,
    full_sha: Option<String>,
    full_loading: HashSet<String>,
    nav: CodeNav,
    nav_results: Option<NavResults>,
    nav_busy: Option<String>,
    nav_task: Option<(Arc<AtomicBool>, Task<()>)>,
    outlines: HashMap<String, tabs::Outline>,
    review: Option<ReviewPopover>,
    badge_open: Option<ReviewerState>,
    merge_confirm: Option<MergeStyle>,
    pub composer: Composer,
    pub overview: overview::State,
    pub conv_scroll: ScrollHandle,
    pub conv_at_bottom: bool,
    poll: Option<Task<()>>,
    shown: bool,
    was_full: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DetailEvent> for PullDetail {}

fn draft_key(id: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

/// Kinds as Swift's DiffLine.Kind raw values, for the viewed fingerprint.
fn kind_raw(k: core::LineKind) -> u8 {
    use loupe_forge::pull::line_kind::*;
    match k { core::LineKind::Context => CONTEXT, core::LineKind::Added => ADDED, core::LineKind::Removed => REMOVED, core::LineKind::Hunk => HUNK, core::LineKind::Meta => META }
}

/// The local `git` the PR views run (fetches, diffs, ls-tree, show).
fn git(dir: &str, args: &[&str], env: &[(String, String)], cancel: Option<&Cancel>) -> loupe_forge::Result<Vec<u8>> {
    loupe_forge::git::run(args, dir, env, cancel)
}

pub fn badge(f: &core::FileDiff) -> (&'static str, gpui_kit::Hsla) {
    match (&f.old_path, &f.new_path) {
        (None, _) => ("A", theme::green()),
        (_, None) => ("D", theme::red()),
        (Some(a), Some(b)) if a != b => ("R", theme::purple()),
        _ => ("M", theme::blue()),
    }
}

impl PullDetail {
    pub fn new(app: Entity<AppModel>, worktree: String, pr: PullRequest, client: Client, repo: RemoteRepo, local: LocalCheckout,
               window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (viewed, drafts) = { let p = loupe_forge::settings::shared(); (p.viewed(&repo, pr.number), p.drafts(&repo, pr.number)) };
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let composer_input = cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment"));
        let subs = vec![
            cx.subscribe(&filter, |d, input, e: &InputEvent, cx| if matches!(e, InputEvent::Change) {
                let q = input.read(cx).value().to_string();
                d.files.set_filter(&q);
                d.rows = None;
                cx.notify();
            }),
            // The toolbar's Compact / Full and Diff / Before / After are app-wide: follow them.
            cx.observe(&app, |d, app, cx| {
                let full = app.read(cx).full_mode;
                if full != d.was_full { d.was_full = full; d.full_mode_changed(full, cx) }
                cx.notify();
            }),
        ];
        let csub = cx.subscribe(&composer_input, |d, _, e: &InputEvent, cx| match e {
            InputEvent::PressEnter { secondary: true, .. } => d.post_comment(cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let was_full = app.read(cx).full_mode;
        let d = PullDetail {
            app, worktree, pr, client, repo, local, reviews: vec![], conversation: vec![], commits: vec![], comments: vec![],
            data_version: 0, diffs: vec![], index: HashMap::new(), source: DiffSource::default(), files: PullFiles::new(viewed),
            drafts, loading: false, reload_queued: false, load: None, error: None, load_error: None, last_loaded: None, checks: None,
            tab: ViewMode::Files, collapsed: HashSet::new(), full_collapsed: HashSet::new(), rows: None, filter,
            adapted: None, diff_view: None, pending_reveal: None, tree: vec![], tree_sha: None, tree_error: None,
            tree_dirs: HashSet::new(), collapse_applied: false, _tree: None, full: HashMap::new(), full_sha: None,
            full_loading: HashSet::new(), nav: CodeNav::default(), nav_results: None, nav_busy: None, nav_task: None,
            outlines: HashMap::new(), review: None, badge_open: None, merge_confirm: None,
            composer: Composer { input: composer_input, sending: false, clear: false, _sub: csub }, overview: overview::State::default(),
            conv_scroll: ScrollHandle::new(), conv_at_bottom: false, poll: None, shown: false, focus: cx.focus_handle(),
            _subs: subs, was_full,
        };
        d.focus.focus(window, cx);
        d
    }

    fn full_mode(&self, cx: &App) -> bool { self.app.read(cx).full_mode }
    /// Before / After show the file itself: whole, as in Full mode.
    fn whole(&self, cx: &App) -> bool { let a = self.app.read(cx); a.full_mode || a.diff_side != DiffSide::Diff }
    pub fn file(&self, path: &str) -> Option<&Arc<core::FileDiff>> { self.index.get(path).map(|&i| &self.diffs[i]) }
    pub fn files(&self) -> &[Arc<core::FileDiff>] { &self.diffs }
    fn selected(&self) -> Option<String> { self.files.selected.clone() }

    // MARK: Showing, polling, loading

    /// The detail is on screen: load unless that happened in the last 30 s, then every 60 s.
    pub fn show(&mut self, cx: &mut Context<Self>) {
        if self.shown { return }
        self.shown = true;
        let fresh = self.last_loaded.is_some_and(|t| t.elapsed() < pull::RELOAD_AFTER);
        self.poll = Some(cx.spawn(async move |this, cx| {
            if fresh { cx.background_executor().timer(pull::POLL).await }
            loop {
                if this.update(cx, |d, cx| d.load(cx)).is_err() { return }
                cx.background_executor().timer(pull::POLL).await;
            }
        }));
        self.publish_controls(cx);
        self.ensure_full(cx);
        self.refresh_checks(cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) { self.focus.focus(window, cx) }

    /// Off screen (back to the list, another tab or worktree, a merge): no polling, and running work stops.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.shown = false;
        self.poll = None;
        if let Some((c, _)) = self.load.take() { c.cancel() }
        self.loading = false;
        self.reload_queued = false;
        if let Some((c, _)) = self.nav_task.take() { c.store(true, std::sync::atomic::Ordering::Relaxed) }
        self.nav_busy = None;
        // Dropping the model cancels its request in flight (unless the overlay still holds it).
        self.checks = None;
        self._tree = None;
        self.full_loading.clear();
        self.overview.cancel();
        cx.notify();
    }

    /// The view switch and Compact / Full in the toolbar (Compact / Full only on Files).
    pub fn publish_controls(&self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let set: Rc<dyn Fn(ViewMode, &mut Window, &mut App)> = Rc::new(move |m, w, cx| { weak.update(cx, |d, cx| d.set_tab(m, w, cx)).ok(); });
        let c = DiffControls { full: self.tab == ViewMode::Files, view: Some((vec![ViewMode::Overview, ViewMode::Files, ViewMode::Conversation], self.tab, set)) };
        let app = self.app.clone();
        cx.defer(move |cx| app.update(cx, |a, cx| a.set_controls(Tab::Pulls, c, cx)));
    }

    pub fn set_tab(&mut self, m: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == m { return }
        self.tab = m;
        if m == ViewMode::Conversation { self.conv_at_bottom = false }
        self.publish_controls(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// One at a time: asked again while running, it runs once more after.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading { self.reload_queued = true; return }
        self.loading = true;
        cx.notify();
        let cancel = Cancel::new();
        let client = self.client.with_cancel(&cancel);
        let (repo, n, local, mut source) = (self.repo.clone(), self.pr.number, self.local.clone(), self.source.clone());
        let task = cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                let d = pull::load_detail(&client, &repo, n)?;
                let diff = source.fetch(&client, &repo, &d.pr, &local)?;
                // Parsed and fingerprinted here, off the main thread; files in tree order so next/prev follows the tree.
                let parsed = diff.map(|data| {
                    let mut fs = core::parse_bytes(&data);
                    fs.sort_by(|a, b| file_tree::tree_order(a.path(), b.path()));
                    let fps: Vec<(String, u64)> = fs.iter().map(|f| (f.path().to_string(), pull::fingerprint(f.lines.iter().map(|l| (f.text(l), kind_raw(l.kind)))))).collect();
                    (fs.into_iter().map(Arc::new).collect::<Vec<_>>(), fps)
                });
                Ok::<_, loupe_forge::Error>((d, parsed, source))
            }).await;
            this.update(cx, |d, cx| {
                d.load = None;
                d.loading = false;
                d.apply_load(r, cx);
                if std::mem::take(&mut d.reload_queued) && d.shown { d.load(cx) }
                cx.notify();
            }).ok();
        });
        self.load = Some((cancel, task));
    }

    fn apply_load(&mut self, r: loupe_forge::Result<(pull::PullDetail, Option<(Vec<Arc<core::FileDiff>>, Vec<(String, u64)>)>, DiffSource)>, cx: &mut Context<Self>) {
        let (d, parsed, source) = match r {
            Ok(x) => x,
            Err(e) => {
                // A poll failing again with the same message doesn't raise the alert again (the Swift app did, every
                // minute while offline); the header keeps a mark until a load works.
                if let Some(m) = e.user_message() && self.load_error.as_ref() != Some(&m) {
                    self.error = Some(m.clone());
                    self.load_error = Some(m);
                }
                return;
            }
        };
        self.source = source;
        let head_moved = d.pr.head.sha != self.pr.head.sha;
        let changed = d.pr != self.pr || d.reviews != self.reviews || d.conversation != self.conversation
            || d.comments != self.comments || d.commits.as_ref().is_some_and(|c| *c != self.commits);
        if changed { self.data_version += 1 }
        self.pr = d.pr;
        self.reviews = d.reviews;
        self.conversation = d.conversation;
        if let Some(c) = d.commits { self.commits = c }
        self.comments = d.comments;
        if let Some((fs, fps)) = parsed {
            self.index = fs.iter().enumerate().map(|(i, f)| (f.path().to_string(), i)).collect();
            self.diffs = fs;
            self.files.set_files(fps);
            self.adapted = None;
            self.rows = None;
        } else if self.files.selected.is_none() {
            self.files.selected = self.diffs.iter().map(|f| f.path()).find(|p| !self.files.is_viewed(p))
                .or(self.diffs.first().map(|f| f.path())).map(str::to_string);
        }
        self.apply_default_collapse();
        self.load_error = None;
        self.last_loaded = Some(Instant::now());
        if head_moved { self.full.clear(); self.outlines.clear() }
        self.refresh_checks(cx);
        self.ensure_full(cx);
    }

    /// CI-01 / CI-03: when the badge appears and after every load, the shared checks model follows the head and
    /// refreshes (joining one in flight).
    fn refresh_checks(&mut self, cx: &mut Context<Self>) {
        if !self.shown { return }
        let m = ChecksModel::shared(&self.app, &self.worktree, &self.pr.head.sha, Some(self.pr.number as u64), cx);
        if self.checks.as_ref().is_none_or(|(c, _)| *c != m) { self.checks = Some((m.clone(), cx.observe(&m, |_, _, cx| cx.notify()))) }
        m.update(cx, |m, cx| { m.refresh(cx); });
    }

    // MARK: Files and viewed

    fn persist_viewed(&self) {
        loupe_forge::settings::shared().set_viewed(&self.repo, self.pr.number, &self.files.viewed);
    }

    fn persist_drafts(&mut self) {
        let mut p = loupe_forge::settings::shared();
        p.set_drafts(&self.repo, self.pr.number, &self.drafts);
        // The head the drafts were written on (FLOWS.md §14: they keep their line numbers after a new push, and the
        // Swift app gave no warning). Its own key, beside the Swift-compatible drafts JSON.
        let key = format!("draftsHead:{}#{}", self.repo.key(), self.pr.number);
        if self.drafts.is_empty() { p.set(&key, None) } else if p.string(&key).is_none() { p.set_string(&key, &self.pr.head.sha) }
    }

    /// Whether the pending comments were written on an earlier head than the PR's current one.
    fn drafts_stale(&self) -> bool {
        let key = format!("draftsHead:{}#{}", self.repo.key(), self.pr.number);
        !self.drafts.is_empty() && loupe_forge::settings::shared().string(&key).is_some_and(|s| s != self.pr.head.sha)
    }

    pub fn is_viewed(&self, path: &str) -> bool { self.files.is_viewed(path) }

    pub fn toggle_viewed(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.files.toggle_viewed(path) { self.persist_viewed(); cx.notify() }
    }

    fn toggle_viewed_dir(&mut self, dir: &str, cx: &mut Context<Self>) {
        if self.files.toggle_viewed_dir(dir) { self.persist_viewed(); cx.notify() }
    }

    pub fn mark_viewed_and_advance(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.selected() else { return };
        if self.file(&cur).is_none() { return } // an unchanged file (Full mode)
        if self.files.mark_viewed_and_advance() { self.persist_viewed() }
        let next = self.selected();
        if next.as_deref() != Some(&cur) && let Some(p) = next { self.select(p, cx) }
        cx.notify();
    }

    fn move_by(&mut self, d: isize, cx: &mut Context<Self>) {
        self.files.move_by(d);
        if let Some(p) = self.selected() { self.select(p, cx) }
    }

    /// Shows `path`. In Full mode it opens as a tab next to the current one.
    pub fn select(&mut self, path: String, cx: &mut Context<Self>) {
        if self.full_mode(cx) && !self.nav.tabs.contains(&path) {
            let at = self.nav.selected.as_ref().and_then(|s| self.nav.tabs.iter().position(|t| t == s)).map_or(self.nav.tabs.len(), |i| i + 1);
            self.nav.tabs.insert(at.min(self.nav.tabs.len()), path.clone());
        }
        self.nav.selected = Some(path.clone());
        self.files.selected = Some(path);
        self.ensure_full(cx);
        cx.notify();
    }

    fn full_mode_changed(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            if let Some(p) = self.selected() && !self.nav.tabs.contains(&p) { self.nav.tabs.push(p.clone()); self.nav.selected = Some(p) }
        } else {
            // Back to Compact on a file this PR doesn't change: show the first change instead.
            if self.selected().is_none_or(|p| self.file(&p).is_none()) { self.files.selected = self.diffs.first().map(|f| f.path().to_string()) }
            self.nav_results = None;
        }
        self.rows = None;
        self.publish_controls(cx);
        self.ensure_full(cx);
    }

    /// From the Overview or Conversation: show a file (and a line, when known). A file this PR doesn't change needs
    /// Full mode.
    pub fn reveal(&mut self, path: &str, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_tab(ViewMode::Files, window, cx);
        if self.file(path).is_none() && !self.full_mode(cx) {
            let app = self.app.clone();
            app.update(cx, |a, cx| a.set_full_mode(true, cx));
            self.was_full = true;
            self.full_mode_changed(true, cx);
        }
        match line {
            Some(l) => {
                self.nav.open(path, l);
                self.files.selected = Some(path.into());
                self.pending_reveal = Some((path.into(), l as u32));
                self.ensure_full(cx);
            }
            None => self.select(path.into(), cx),
        }
        cx.notify();
    }

    // MARK: Full mode

    /// What Full mode (or one side shown) needs that isn't here yet: the tree at the head, the shown file whole.
    pub fn ensure_full(&mut self, cx: &mut Context<Self>) {
        if !self.shown { return }
        if self.full_mode(cx) && self.tree_sha.as_deref() != Some(&self.pr.head.sha) && self._tree.is_none() { self.load_tree(cx) }
        if self.whole(cx) && let Some(p) = self.selected() { self.load_full(p, cx) }
    }

    fn load_tree(&mut self, cx: &mut Context<Self>) {
        let (pr, local) = (self.pr.clone(), self.local.clone());
        let changed: Vec<String> = self.diffs.iter().map(|f| f.path().to_string()).collect();
        self._tree = Some(cx.spawn(async move |this, cx| {
            let sha = pr.head.sha.clone();
            let r = cx.background_executor().spawn(async move {
                pull::fetch_head(&pr, &local, None)?;
                let out = git(&local.dir, &["ls-tree", "-r", "-z", "--name-only", &pr.head.sha], &[], None)?;
                let mut set: HashSet<String> = out.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
                set.extend(changed); // deleted files stay reachable
                let mut paths: Vec<String> = set.into_iter().collect();
                file_tree::sort_paths(&mut paths);
                let dirs: HashSet<String> = file_tree::all_dirs(&paths).into_iter().collect();
                Ok::<_, loupe_forge::Error>((paths, dirs))
            }).await;
            this.update(cx, |d, cx| {
                d._tree = None;
                match r {
                    Ok((paths, dirs)) => { d.tree = paths; d.tree_dirs = dirs; d.tree_sha = Some(sha); d.tree_error = None; d.rows = None; d.apply_default_collapse() }
                    Err(e) => d.tree_error = e.user_message().or(d.tree_error.take()),
                }
                cx.notify();
            }).ok();
        }));
    }

    /// Once both the tree and the PR's files are known: fold every directory this PR doesn't touch.
    fn apply_default_collapse(&mut self) {
        if self.collapse_applied || self.tree_dirs.is_empty() || self.diffs.is_empty() { return }
        self.collapse_applied = true;
        let mut open = HashSet::new();
        for f in &self.diffs {
            let p = f.path();
            for (i, _) in p.match_indices('/') { open.insert(p[..i].to_string()); }
        }
        self.full_collapsed = self.tree_dirs.difference(&open).cloned().collect();
        self.rows = None;
    }

    /// The whole file at the head: a changed file as a full-context diff, the rest as plain lines.
    fn load_full(&mut self, path: String, cx: &mut Context<Self>) {
        if self.full_sha.as_deref() != Some(&self.pr.head.sha) { self.full.clear(); self.full_sha = Some(self.pr.head.sha.clone()) }
        if self.full.contains_key(&path) || self.full_loading.contains(&path) { return }
        self.full_loading.insert(path.clone());
        let (pr, local, changed) = (self.pr.clone(), self.local.clone(), self.file(&path).cloned());
        let p = path.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                pull::fetch_head(&pr, &local, None)?;
                let f = match changed {
                    // Binary, or deleted: the PR's diff already shows everything.
                    Some(c) if c.is_binary || c.new_path.is_none() => (*c).clone(),
                    Some(c) => {
                        let mut paths: Vec<&str> = [c.old_path.as_deref(), c.new_path.as_deref()].into_iter().flatten().collect();
                        paths.dedup();
                        let range = format!("{}...{}", pr.base.sha, pr.head.sha);
                        let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-M", "-U1000000", &range, "--"];
                        args.extend(paths);
                        let data = git(&local.dir, &args, &[], None)?;
                        core::whole_file(core::parse_bytes(&data).into_iter().next()).unwrap_or((*c).clone())
                    }
                    None => core::plain_file(&p, &git(&local.dir, &["show", &format!("{}:{p}", pr.head.sha)], &[], None)?),
                };
                Ok::<_, loupe_forge::Error>(Arc::new(adapt::file_diff(&f)))
            }).await;
            this.update(cx, |d, cx| {
                d.full_loading.remove(&path);
                match r {
                    Ok(f) => { d.load_outline(&path, &f, cx); d.full.insert(path, Ok(f)); }
                    // The Swift app span forever here (FLOWS.md §14); the pane says why and offers Retry.
                    Err(e) => if let Some(m) = e.user_message() { d.full.insert(path, Err(m)); },
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    fn load_outline(&mut self, path: &str, f: &Arc<rdiff::FileDiff>, cx: &mut Context<Self>) {
        if f.is_binary { return }
        let text: String = (0..f.lines.len()).filter(|&i| !matches!(f.lines[i].kind, rdiff::LineKind::Removed | rdiff::LineKind::Meta | rdiff::LineKind::Hunk))
            .map(|i| f.line_text(i)).collect::<Vec<_>>().join("\n");
        let p = path.to_string();
        cx.spawn(async move |this, cx| {
            let pp = p.clone();
            let o = cx.background_executor().spawn(async move {
                let cancel = AtomicBool::new(false);
                match loupe_code::index::outline_of(&Arc::from(text.as_str()), &pp, &cancel) {
                    loupe_code::index::Outline::TooLarge => tabs::Outline::TooLarge,
                    loupe_code::index::Outline::Symbols(s) => tabs::Outline::Symbols(s.iter().map(|s| tabs::OutlineSymbol {
                        name: s.name.clone(), kind: format!("{:?}", s.kind).to_lowercase(), line: s.line }).collect()),
                }
            }).await;
            this.update(cx, |d, cx| { d.outlines.insert(p, o); cx.notify() }).ok();
        }).detach();
    }

    /// ⌘-click / Go to Definition / Find References on `word` in `path`, at the PR head.
    fn navigate(&mut self, word: String, action: CodeAction, from: String, cx: &mut Context<Self>) {
        if let Some((c, _)) = self.nav_task.take() { c.store(true, std::sync::atomic::Ordering::Relaxed) }
        self.nav_busy = Some(word.clone());
        cx.notify();
        let cancel = Arc::new(AtomicBool::new(false));
        let (pr, local, c2) = (self.pr.clone(), self.local.clone(), cancel.clone());
        let task = cx.spawn(async move |this, cx| {
            let w = word.clone();
            let r = cx.background_executor().spawn(async move {
                pull::fetch_head(&pr, &local, None)?;
                let head = Some(pr.head.sha.as_str());
                let defs = if action == CodeAction::Definition { loupe_code::index::definitions(&w, head, &local.dir, &from, &c2) } else { vec![] };
                if !defs.is_empty() { return Ok(Err(defs)) }
                Ok::<_, loupe_forge::Error>(Ok(loupe_code::index::references(&w, head, &local.dir, loupe_code::index::REFERENCE_LIMIT + 1)))
            }).await;
            this.update(cx, |d, cx| {
                d.nav_task = None;
                d.nav_busy = None;
                match r {
                    Ok(Err(defs)) if defs.len() == 1 => { d.nav_results = None; let (p, l) = (defs[0].path.clone(), defs[0].line); d.open_at(p, l, cx) }
                    Ok(Err(defs)) => d.nav_results = Some(NavResults::Definitions(word, defs.into_iter().map(|s| NavHit { path: s.path, line: s.line, text: s.name }).collect())),
                    Ok(Ok(refs)) => d.nav_results = Some(NavResults::References(word, refs.into_iter().map(|m| NavHit { path: m.path, line: m.line, text: m.text }).collect(), action == CodeAction::Definition)),
                    Err(e) => if let Some(m) = e.user_message() { d.error = Some(m) },
                }
                cx.notify();
            }).ok();
        });
        self.nav_task = Some((cancel, task));
    }

    fn open_at(&mut self, path: String, line: usize, cx: &mut Context<Self>) {
        self.nav.open(&path, line);
        self.files.selected = Some(path.clone());
        self.pending_reveal = Some((path, line as u32));
        self.ensure_full(cx);
        cx.notify();
    }

    fn tab_event(&mut self, e: TabEvent, cx: &mut Context<Self>) {
        match e {
            TabEvent::Select(p) => { self.nav.selected = Some(p.clone()); self.files.selected = Some(p) }
            TabEvent::Close(p) => { self.nav.close_tab(&p); self.files.selected = self.nav.selected.clone() }
            TabEvent::CloseOthers(p) => { self.nav.close_other_tabs(&p); self.files.selected = Some(p) }
            TabEvent::CloseAll => { self.nav.close_all(); self.files.selected = None }
            TabEvent::Open(loc) => return self.open_at(loc.path, loc.line, cx),
            TabEvent::Back => { self.nav.go_back(); self.after_jump() }
            TabEvent::Forward => { self.nav.go_forward(); self.after_jump() }
            TabEvent::DismissResults => self.nav_results = None,
        }
        self.ensure_full(cx);
        cx.notify();
    }

    fn after_jump(&mut self) {
        self.files.selected = self.nav.selected.clone();
        if let (Some(p), Some(l)) = (self.nav.selected.clone(), self.nav.line) { self.pending_reveal = Some((p, l as u32)) }
    }

    // MARK: Review, comments, merge

    fn add_draft(&mut self, path: String, new: Option<u32>, old: Option<u32>, body: String, cx: &mut Context<Self>) {
        if body.trim().is_empty() { return }
        self.drafts.push(DraftComment::new(path, new.map(i64::from), old.map(i64::from), body));
        self.persist_drafts();
        cx.notify();
    }

    fn delete_draft(&mut self, key: u64, cx: &mut Context<Self>) {
        self.drafts.retain(|d| draft_key(&d.id) != key);
        self.persist_drafts();
        cx.notify();
    }

    fn open_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_some() { self.review = None; return cx.notify() }
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment"));
        let sub = cx.subscribe(&input, |d, _, e: &InputEvent, cx| match e {
            InputEvent::PressEnter { secondary: true, .. } => d.submit_review(cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.review = Some(ReviewPopover { input, event: ReviewEvent::Comment, sending: false, _sub: sub });
        cx.notify();
    }

    fn can_submit(&self, cx: &App) -> bool {
        let Some(r) = &self.review else { return false };
        // Trimmed: a body of only spaces isn't a review (the Swift app sent it, FLOWS.md §14).
        let empty = r.input.read(cx).value().trim().is_empty();
        !r.sending && (r.event == ReviewEvent::Approve || !empty || !self.drafts.is_empty())
    }

    fn submit_review(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) { return }
        let Some(r) = &mut self.review else { return };
        r.sending = true;
        let (event, body) = (r.event, r.input.read(cx).value().to_string());
        let (client, repo, n, sha, drafts) = (self.client.clone(), self.repo.clone(), self.pr.number, self.pr.head.sha.clone(), self.drafts.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { client.submit_review(&repo, n, event, &body, &sha, &drafts) }).await;
            this.update(cx, |d, cx| {
                match r {
                    Ok(()) => { d.drafts.clear(); d.persist_drafts(); d.review = None; d.load(cx) }
                    Err(e) => { if let Some(r) = &mut d.review { r.sending = false } if let Some(m) = e.user_message() { d.error = Some(m) } }
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    pub fn post_comment(&mut self, cx: &mut Context<Self>) {
        let body = self.composer.input.read(cx).value().to_string();
        if self.composer.sending || body.trim().is_empty() { return }
        self.composer.sending = true;
        cx.notify();
        let (client, repo, n) = (self.client.clone(), self.repo.clone(), self.pr.number);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                client.add_issue_comment(&repo, n, &body)?;
                client.issue_comments(&repo, n)
            }).await;
            this.update(cx, |d, cx| {
                d.composer.sending = false;
                match r {
                    // Cleared only once it's posted: the Swift app cleared it on failure too, losing the text.
                    Ok(c) => {
                        d.composer.clear = true;
                        if c != d.conversation { d.conversation = c; d.data_version += 1; d.conv_at_bottom = false }
                    }
                    Err(e) => if let Some(m) = e.user_message() { d.error = Some(m) },
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    fn merge(&mut self, style: MergeStyle, cx: &mut Context<Self>) {
        self.merge_confirm = None;
        let (client, repo, n, base) = (self.client.clone(), self.repo.clone(), self.pr.number, self.pr.base.ref_.clone());
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { client.merge(&repo, n, style) }).await;
            this.update(cx, |d, cx| match r {
                Ok(()) => cx.emit(DetailEvent::Merged { base }),
                Err(e) => { if let Some(m) = e.user_message() { d.error = Some(m) } cx.notify() }
            }).ok();
        }).detach();
        cx.notify();
    }

    pub fn reviewer_states(&self) -> Vec<(String, ReviewerState)> { pull::reviewer_states(&self.reviews, &self.pr, self.client.github) }

    /// Everyone the pull request knows, by login, for avatars.
    pub fn users(&self) -> HashMap<String, User> {
        let mut u: HashMap<String, User> = HashMap::new();
        let all = std::iter::once(&self.pr.user).chain(self.pr.requested_reviewers.iter().flatten())
            .chain(self.reviews.iter().filter_map(|r| r.user.as_ref())).chain(self.conversation.iter().filter_map(|c| c.user.as_ref()))
            .chain(self.comments.iter().filter_map(|c| c.user.as_ref())).chain(self.commits.iter().filter_map(|c| c.author.as_ref()));
        for x in all { if u.get(&x.login).is_none_or(|y| y.avatar_url.is_none()) { u.insert(x.login.clone(), x.clone()); } }
        u
    }

    fn comment_count(&self, path: &str) -> usize {
        self.comments.iter().filter(|c| c.path == path).count() + self.drafts.iter().filter(|d| d.path == path).count()
    }

    fn inline_comments(&self, path: &str) -> Vec<rdiff::InlineComment> {
        let m = pull::inline_comments(&self.comments, &self.drafts);
        m.get(path).into_iter().flatten().map(|c| rdiff::InlineComment {
            author: c.author.clone(), body: c.body.clone(),
            new_line: c.new_line.map(|n| n as u32), old_line: c.old_line.map(|n| n as u32),
            draft_id: c.draft_id.as_deref().map(draft_key),
        }).collect()
    }
}

// MARK: - Rendering

impl PullDetail {
    fn header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity_id();
        let pr = self.pr.clone();
        let w = cx.weak_entity();
        let open = pr.state == "open";
        let url = pr.html_url.clone();
        let (u1, u2) = (url.clone(), url);
        let number = Button::new("pr-number").ghost().small().label(format!("#{} ⌄", pr.number)).tooltip("Open in Browser or Copy Link")
            .dropdown_menu(move |menu, _, _| {
                let (a, b) = (u1.clone(), u2.clone());
                menu.item(PopupMenuItem::new("Open in Browser").on_click(move |_, _, cx| cx.open_url(&a)))
                    .item(PopupMenuItem::new("Copy Link").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(b.clone()))))
            });
        let w_merge = w.clone();
        let blocked = pr.mergeable == Some(false);
        let merge_tip: SharedString = if blocked { format!("Can't be merged: conflicts with {}, or a branch protection rule isn't met", pr.base.ref_).into() } else { format!("Merge into {}", pr.base.ref_).into() };
        let merge = Button::new("pr-merge").small().label("Merge").icon(gpui_kit::component::Icon::empty().path("icons/git-merge.svg"))
            .disabled(blocked).tooltip(merge_tip)
            .dropdown_menu(move |mut menu, _, _| {
                for (s, t) in [(MergeStyle::Merge, "Merge"), (MergeStyle::Rebase, "Rebase"), (MergeStyle::Squash, "Squash")] {
                    let w = w_merge.clone();
                    menu = menu.item(PopupMenuItem::new(t).on_click(move |_, _, cx| { w.update(cx, |d, cx| { d.merge_confirm = Some(s); cx.notify() }).ok(); }));
                }
                menu
            });
        let review_label = if self.drafts.is_empty() { "Review".to_string() } else { format!("Review ({})", self.drafts.len()) };
        let (wt, n) = (self.worktree.clone(), pr.number as u64);
        let app = self.app.clone();
        let states = self.reviewer_states();
        let back = cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Back));
        let load_error = self.load_error.clone();
        div().flex().flex_col().gap_1p5().px_3p5().py_2p5().flex_none().bg(theme::sidebar()).border_b_1().border_color(theme::border())
            .child(div().flex().items_center().gap_2().min_w_0()
                .child(div().id("pr-back").flex().flex_none().items_center().gap_1().cursor_pointer().text_color(theme::blue())
                    .hover(|s| s.text_color(theme::text())).on_click(back)
                    .child(ui::icon("chevron-left").text_color(theme::blue())).child("Pull Requests"))
                .child(div().id("pr-title").min_w_0().truncate().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(pr.title.clone())
                    .tooltip({ let t: SharedString = pr.title.clone().into(); move |w, cx| Tooltip::new(t.clone()).build(w, cx) }))
                .child(number)
                .child(div().flex_1())
                .child(div().flex().flex_none().items_center().gap_2()
                    // Only while loading: nothing animates otherwise.
                    .child(div().size(px(16.)).flex_none().when(self.loading, |d| d.child(ui::icon("loader").text_color(theme::text_faint()))))
                    .when_some(load_error, |d, e| d.child(div().id("pr-load-error").child(ui::icon("triangle-alert").text_color(theme::accent()))
                        .tooltip(move |w, cx| Tooltip::new(format!("Couldn't refresh: {e}")).build(w, cx))))
                    .child(Button::new("pr-checkout").small().label("Check Out").icon(gpui_kit::component::Icon::empty().path("icons/git-branch.svg"))
                        .tooltip("Check Out in New Worktree")
                        .on_click(move |_, _, cx| app.update(cx, |a, cx| a.checkout_pull(&wt, n, cx))))
                    .when(open, |d| d.child(merge))
                    .child(Button::new("pr-review").small().primary().label(review_label)
                        .on_click(cx.listener(|d, _, w, cx| d.open_review(w, cx))))))
            .child(div().flex().items_center().gap_2p5().min_w_0().text_size(px(12.5))
                .child(avatar(&pr.user, 18., me, cx))
                .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(pr.user.login.clone()))
                .child(div().id("pr-refs").min_w_0().truncate().font_family(theme::CODE_FONT).text_size(px(12.)).text_color(theme::text_muted())
                    .child(format!("{} → {}", pr.head.ref_, pr.base.ref_)))
                .when_some(pr.additions.zip(pr.deletions), |d, (a, r)| d
                    .child(div().flex_none().text_color(theme::green()).child(format!("+{a}")))
                    .child(div().flex_none().text_color(theme::red()).child(format!("−{r}"))))
                .children([(ReviewerState::Approved, "approved", "circle-check", theme::green()),
                           (ReviewerState::ChangesRequested, "requested changes", "circle-alert", theme::red()),
                           (ReviewerState::Pending, "pending", "circle", theme::accent())].into_iter().filter_map(|(s, label, icon, color)| {
                    let n = states.iter().filter(|(_, x)| *x == s).count();
                    (n > 0).then(|| {
                        let on = self.badge_open == Some(s);
                        div().id(SharedString::from(format!("badge-{label}"))).flex().flex_none().items_center().gap_1().cursor_pointer()
                            .text_color(if on { theme::text() } else { theme::text_muted() }).hover(|st| st.text_color(theme::text()))
                            .on_click(cx.listener(move |d, _, _, cx| { d.badge_open = if d.badge_open == Some(s) { None } else { Some(s) }; cx.notify() }))
                            .child(ui::icon(icon).text_color(color)).child(format!("{n} {label}"))
                    })
                }))
                .when_some(self.checks.as_ref().map(|c| c.0.clone()), |d, m| d.child(ci::badge(&m, cx)))
                .child(div().flex_1()))
            .into_any_element()
    }

    /// Who's behind a review badge.
    fn badge_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.badge_open?;
        let title = match s { ReviewerState::Approved => "Approved", ReviewerState::ChangesRequested => "Requested changes", ReviewerState::Pending => "Pending" };
        let users = self.users();
        let me = cx.entity_id();
        let names: Vec<User> = self.reviewer_states().into_iter().filter(|(_, x)| *x == s).map(|(l, _)| users.get(&l).cloned().unwrap_or(User { login: l, avatar_url: None })).collect();
        Some(popover_frame(cx.listener(|d, _, _, cx| { d.badge_open = None; cx.notify() }), px(84.), None, Some(px(300.)),
            div().w(px(220.)).py_2().flex().flex_col()
                .child(div().px_3().pb_1().font_weight(FontWeight::SEMIBOLD).child(title))
                .children(names.iter().map(|u| div().flex().items_center().gap_2().px_3().py_1().text_size(px(12.))
                    .child(avatar(u, 20., me, cx)).child(div().truncate().child(u.login.clone()))))
                .into_any_element()))
    }

    fn review_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.review.as_ref()?;
        let can = self.can_submit(cx);
        let pending = if self.drafts.is_empty() { "No pending comments".to_string() } else { format!("{} pending comment(s)", self.drafts.len()) };
        let stale = self.drafts_stale();
        let radio = |id: &'static str, label: &'static str, ev: ReviewEvent, cx: &mut Context<Self>| {
            let on = r.event == ev;
            div().id(id).flex().items_center().gap_1p5().cursor_pointer()
                .on_click(cx.listener(move |d, _, _, cx| { if let Some(r) = &mut d.review { r.event = ev } cx.notify() }))
                .child(ui::icon(if on { "circle-dot" } else { "circle" }).text_color(if on { theme::blue() } else { theme::text_muted() }))
                .child(label)
        };
        Some(popover_frame(cx.listener(|d, _, _, cx| { d.review = None; cx.notify() }), px(46.), Some(px(14.)), None,
            div().id("review-popover").w(px(428.)).p_3p5().flex().flex_col().gap_2p5()
                .on_key_down(cx.listener(|d, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" { d.review = None; cx.notify() }))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Finish your review"))
                .child(div().h(px(120.)).px_2().py_1().rounded(px(6.)).bg(theme::raised()).border_1().border_color(theme::border())
                    .child(Textarea::new(&r.input).appearance(false).h_full()))
                .child(div().flex().flex_col().gap_1()
                    .child(radio("ev-comment", "Comment", ReviewEvent::Comment, cx))
                    .child(radio("ev-approve", "Approve", ReviewEvent::Approve, cx))
                    .child(radio("ev-request", "Request Changes", ReviewEvent::RequestChanges, cx)))
                .when(stale, |d| d.child(div().flex().items_center().gap_1p5().text_size(px(12.)).text_color(theme::accent())
                    .child(ui::icon("triangle-alert").text_color(theme::accent()))
                    .child("Pending comments were written on an earlier commit: their lines may have moved.")))
                .child(div().flex().items_center().gap_2()
                    .child(div().flex_1().text_size(px(12.)).text_color(theme::text_muted()).child(pending))
                    .child(ui::pill("review-cancel", "Cancel").on_click(cx.listener(|d, _, _, cx| { d.review = None; cx.notify() })))
                    .child(ui::pill_primary("review-submit", if r.sending { "Submitting…" } else { "Submit Review  ⌘↩" }, theme::blue())
                        .when(!can, |d| d.opacity(0.4))
                        .when(can, |d| d.on_click(cx.listener(|d, _, _, cx| d.submit_review(cx))))))
                .into_any_element()))
    }

    fn merge_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.merge_confirm?;
        let name = match s { MergeStyle::Merge => "Merge", MergeStyle::Rebase => "Rebase", MergeStyle::Squash => "Squash" };
        Some(modal(format!("{name} #{} into {}?", self.pr.number, self.pr.base.ref_), None,
            div().flex().justify_end().gap_2()
                .child(ui::pill("merge-cancel", "Cancel").on_click(cx.listener(|d, _, _, cx| { d.merge_confirm = None; cx.notify() })))
                .child(ui::pill_primary("merge-ok", name, theme::blue()).on_click(cx.listener(move |d, _, _, cx| d.merge(s, cx))))
                .into_any_element()))
    }

    /// The detail's alert, titled by the forge it talks to (the Swift app said "Forgejo" on GitHub too).
    fn error_alert(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.error.clone()?;
        Some(modal(self.client.server_name().into(), Some(e),
            div().flex().justify_end().child(ui::pill_primary("pr-alert-ok", "OK", theme::blue())
                .on_click(cx.listener(|d, _, _, cx| { d.error = None; cx.notify() }))).into_any_element()))
    }

    // MARK: Files tab

    fn tree_rows(&mut self, cx: &App) -> Rc<Vec<TreeRow>> {
        if let Some(r) = &self.rows { return r.clone() }
        let full = self.full_mode(cx);
        let q = self.files.filter().to_lowercase();
        let paths: Vec<String> = if full {
            self.tree.iter().filter(|p| q.is_empty() || p.to_lowercase().contains(&q)).cloned().collect()
        } else {
            self.files.filtered().map(str::to_string).collect()
        };
        // While a filter is set every folder shows open.
        let none = HashSet::new();
        let collapsed = if !q.is_empty() { &none } else if full { &self.full_collapsed } else { &self.collapsed };
        let rows = Rc::new(file_tree::rows(&paths, collapsed));
        self.rows = Some(rows.clone());
        rows
    }

    fn toggle_dir(&mut self, dir: &str, cx: &mut Context<Self>) {
        if !self.files.filter().is_empty() { return }
        let set = if self.full_mode(cx) { &mut self.full_collapsed } else { &mut self.collapsed };
        if !set.remove(dir) { set.insert(dir.to_string()); }
        self.rows = None;
        cx.notify();
    }

    fn set_all_collapsed(&mut self, collapse: bool, cx: &mut Context<Self>) {
        let full = self.full_mode(cx);
        let dirs: HashSet<String> = if collapse {
            let paths: Vec<String> = if full { self.tree.clone() } else { self.diffs.iter().map(|f| f.path().to_string()).collect() };
            file_tree::all_dirs(&paths).into_iter().collect()
        } else { HashSet::new() };
        if full { self.full_collapsed = dirs } else { self.collapsed = dirs }
        self.rows = None;
        cx.notify();
    }

    fn tree_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let full = self.full_mode(cx);
        let total = self.diffs.len();
        let viewed = self.files.viewed_count();
        let frac = viewed as f32 / total.max(1) as f32;
        let body = if full && self.tree.is_empty() {
            match &self.tree_error {
                Some(e) => ui::empty_state("triangle-alert", "Can't Load the Repository", e.clone()).into_any_element(),
                None => ui::empty_state("loader", "Fetching the pull request…", "").into_any_element(),
            }
        } else {
            let rows = self.tree_rows(cx);
            let this = cx.entity();
            uniform_list("pr-tree", rows.len(), move |range, _, cx| {
                range.map(|i| this.update(cx, |d, cx| d.tree_row(&rows[i], cx))).collect::<Vec<_>>()
            }).flex_1().into_any_element()
        };
        div().flex().flex_col().size_full().bg(theme::sidebar())
            .child(div().p_2().child(div().h(px(28.)).px_2().flex().items_center().rounded(px(7.)).bg(theme::raised())
                .border_1().border_color(theme::border()).child(Input::new(&self.filter).appearance(false))))
            .child(div().px_2p5().pb_1p5().flex().flex_col().gap_1()
                .child(div().h(px(4.)).w_full().rounded_full().bg(theme::raised())
                    .child(div().h_full().rounded_full().bg(theme::blue()).w(gpui_kit::relative(frac))))
                .child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(format!("{viewed} of {total} files viewed"))))
            .child(div().flex().flex_col().flex_1().min_h_0().px_1().child(body))
            .into_any_element()
    }

    fn tree_row(&mut self, r: &TreeRow, cx: &mut Context<Self>) -> AnyElement {
        let indent = px(r.depth as f32 * 10. + 4.);
        let id = SharedString::from(format!("t-{}", r.id));
        let w = cx.weak_entity();
        if let Some(dir) = r.dir_path.clone() {
            let full = self.full_mode(cx);
            let open = !(if full { &self.full_collapsed } else { &self.collapsed }).contains(&dir) || !self.files.filter().is_empty();
            let (done, total) = self.files.viewed_progress(&dir);
            let (d1, d2) = (dir.clone(), dir.clone());
            let (w1, w2) = (w.clone(), w);
            return ui::list_row(id, false).w_full().h(px(24.)).pl(indent).gap_1()
                .on_click(cx.listener(move |d, _, _, cx| d.toggle_dir(&d1, cx)))
                .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(11.)).text_color(theme::text_faint()))
                .child(ui::icon(if open { "folder-open" } else { "folder" }))
                .child(div().flex_1().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().child(r.name.clone()))
                .when(total > 0, |d| d.child(div().text_size(px(11.)).font_family(theme::CODE_FONT)
                    .text_color(if done == total { theme::blue() } else { theme::text_faint() }).child(format!("{done}/{total}"))))
                .context_menu(move |menu, _, _| {
                    let (a, b, c, e) = (w1.clone(), w1.clone(), w2.clone(), w2.clone());
                    let (da, db) = (d2.clone(), d2.clone());
                    let all = total > 0 && done == total;
                    menu.item(PopupMenuItem::new(if open { "Collapse" } else { "Expand" }).on_click(move |_, _, cx| { a.update(cx, |d, cx| d.toggle_dir(&da, cx)).ok(); }))
                        .item(PopupMenuItem::new(if all { "Mark Folder as Not Viewed" } else { "Mark Folder as Viewed" })
                            .on_click(move |_, _, cx| { b.update(cx, |d, cx| d.toggle_viewed_dir(&db, cx)).ok(); }))
                        .separator()
                        .item(PopupMenuItem::new("Expand All").on_click(move |_, _, cx| { c.update(cx, |d, cx| d.set_all_collapsed(false, cx)).ok(); }))
                        .item(PopupMenuItem::new("Collapse All").on_click(move |_, _, cx| { e.update(cx, |d, cx| d.set_all_collapsed(true, cx)).ok(); }))
                })
                .into_any_element();
        }
        let path = r.file_path.clone().unwrap_or_default();
        let sel = self.selected().as_deref() == Some(path.as_str());
        let p = path.clone();
        let (w1, w2) = (w.clone(), w);
        let menu = move |menu: gpui_kit::component::menu::PopupMenu, _: &mut Window, _: &mut Context<gpui_kit::component::menu::PopupMenu>| {
            let (a, b) = (w1.clone(), w2.clone());
            menu.item(PopupMenuItem::new("Expand All").on_click(move |_, _, cx| { a.update(cx, |d, cx| d.set_all_collapsed(false, cx)).ok(); }))
                .item(PopupMenuItem::new("Collapse All").on_click(move |_, _, cx| { b.update(cx, |d, cx| d.set_all_collapsed(true, cx)).ok(); }))
        };
        let row = ui::list_row(id, sel).w_full().h(px(24.)).pl(indent + px(15.)).gap_1p5()
            .on_click(cx.listener(move |d, _, _, cx| d.select(p.clone(), cx)))
            .child(ui::file_icon(&path));
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        match self.file(&path).cloned() {
            Some(f) => {
                let (letter, color) = badge(&f);
                let comments = self.comment_count(&path);
                row.when(self.is_viewed(&path), |d| d.opacity(0.5))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .map(|d| if f.is_binary { d.child(div().text_size(px(11.)).text_color(theme::text_faint()).child("binary")) } else {
                        d.child(div().flex().flex_none().gap_1().font_family(theme::CODE_FONT).text_size(px(11.))
                            .child(div().text_color(theme::green()).child(format!("+{}", f.additions)))
                            .child(div().text_color(theme::red()).child(format!("−{}", f.deletions))))
                    })
                    .when(comments > 0, |d| d.child(div().flex().flex_none().items_center().gap_0p5().text_size(px(11.)).text_color(theme::text_muted())
                        .child(ui::icon("message-square").size(px(11.))).child(comments.to_string())))
                    .child(div().w(px(14.)).flex_none().text_center().rounded(px(3.)).bg(color.opacity(0.18)).text_color(color)
                        .text_size(px(10.)).font_weight(FontWeight::BOLD).child(letter))
                    .context_menu(menu).into_any_element()
            }
            // Full mode: a file this PR doesn't touch.
            None => {
                let tip: SharedString = path.clone().into();
                row.child(div().flex_1().min_w_0().truncate().text_color(theme::text_muted()).child(name))
                    .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx)).context_menu(menu).into_any_element()
            }
        }
    }

    fn make_diff_view(&self, file: Arc<rdiff::FileDiff>, cx: &mut Context<Self>) -> Entity<DiffView> {
        let w = cx.weak_entity();
        let (side, layout) = { let a = self.app.read(cx); (a.diff_side, a.layout) };
        let app = self.app.clone();
        cx.new(|cx| {
            let mut d = DiffView::new(file, cx);
            d.set_highlighter(Some(adapt::highlighter()), cx);
            d.set_side(side, cx);
            d.set_layout(layout, cx);
            let (w1, w2, w3, w4, w5, w6) = (w.clone(), w.clone(), w.clone(), w.clone(), w.clone(), w);
            d.on_add_comment(move |new, old, body, _, cx| { w1.update(cx, |p, cx| if let Some(path) = p.selected() { p.add_draft(path, new, old, body, cx) }).ok(); });
            d.on_delete_draft(move |id, _, cx| { w2.update(cx, |p, cx| p.delete_draft(id, cx)).ok(); });
            d.can_comment(move |l| w3.upgrade().is_some_and(|_| l.kind != rdiff::LineKind::Meta) && COMMENTABLE.with(|c| c.borrow().as_ref().is_none_or(|(new, old)| {
                if l.kind == rdiff::LineKind::Removed { l.old_line.is_some_and(|n| old.contains(&n)) } else { l.new_line.is_some_and(|n| new.contains(&n)) }
            })));
            d.on_mark_viewed(move |_, _, cx| { w4.update(cx, |p, cx| p.mark_viewed_and_advance(cx)).ok(); });
            d.on_navigate(move |word, action, _, cx| { w5.update(cx, |p, cx| if p.full_mode(cx) && let Some(from) = p.selected() { p.navigate(word, action, from, cx) }).ok(); });
            d.on_side_change(move |side, layout, _, cx| app.update(cx, |a, cx| a.set_diff_side(side, layout, cx)));
            d.header_actions(move |window, cx| w6.upgrade().map(|p| p.update(cx, |p, cx| p.file_header_actions(window, cx))).unwrap_or_else(|| div().into_any_element()));
            d
        })
    }

    /// The file header's extras: Full mode's back / forward / outline, then the Viewed checkbox or "Unchanged".
    fn file_header_actions(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(path) = self.selected() else { return div().into_any_element() };
        let changed = self.file(&path).is_some();
        let whole = self.whole(cx);
        let w = cx.weak_entity();
        let on: tabs::OnTabEvent = Rc::new(move |e, _, cx| { w.update(cx, |d, cx| d.tab_event(e, cx)).ok(); });
        let outline = self.outlines.get(&path).cloned().unwrap_or(tabs::Outline::None);
        let viewed = self.is_viewed(&path);
        let p = path.clone();
        div().flex().items_center().gap_2()
            .when(whole && self.full_mode(cx), |d| d.child(tabs::nav_controls(&self.nav, &path, &outline, self.nav_busy.as_deref(), on)))
            .map(|d| if changed {
                d.child(div().id("viewed").flex().items_center().gap_1().cursor_pointer().text_size(px(12.)).text_color(theme::text_muted())
                    .on_click(cx.listener(move |d, _, _, cx| d.toggle_viewed(&p, cx)))
                    .child(ui::icon(if viewed { "square-check" } else { "square" }).size(px(13.)).text_color(if viewed { theme::blue() } else { theme::text_muted() }))
                    .child("Viewed"))
            } else {
                d.child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Unchanged"))
            })
            .into_any_element()
    }

    /// The file pane: the diff (Compact) or the whole file (Full, Before / After), tabs and results in Full mode.
    fn file_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let full = self.full_mode(cx);
        let whole = self.whole(cx);
        let sel = self.selected();
        let shown: Result<Option<Arc<rdiff::FileDiff>>, String> = match &sel {
            Some(p) if whole => match self.full.get(p) {
                Some(Ok(f)) => Ok(Some(f.clone())),
                Some(Err(e)) => Err(e.clone()),
                None => Ok(None),
            },
            Some(p) => match self.file(p).cloned() {
                Some(core) => Ok(Some(match &self.adapted {
                    Some((uid, f)) if *uid == core.uid => f.clone(),
                    _ => { let f = Arc::new(adapt::file_diff(&core)); self.adapted = Some((core.uid, f.clone())); f }
                })),
                None => Ok(None),
            },
            None => Ok(None),
        };
        let content = match shown {
            Ok(Some(file)) => {
                let path = file.path().to_string();
                // Review comments attach only to lines inside the PR's hunks: in a whole file, the rest can't take one.
                let commentable = self.file(sel.as_deref().unwrap_or("")).map(|c| {
                    let new: HashSet<u32> = c.lines.iter().filter(|l| l.kind != core::LineKind::Removed).filter_map(|l| l.new_line).collect();
                    let old: HashSet<u32> = c.lines.iter().filter(|l| l.kind == core::LineKind::Removed).filter_map(|l| l.old_line).collect();
                    (new, old)
                });
                let changed = commentable.is_some();
                COMMENTABLE.with(|c| *c.borrow_mut() = Some(commentable.unwrap_or_default()));
                let comments = if changed { self.inline_comments(sel.as_deref().unwrap_or("")) } else { vec![] };
                let (side, layout) = { let a = self.app.read(cx); (a.diff_side, a.layout) };
                let reveal = self.pending_reveal.take_if(|(p, _)| *p == path).map(|(_, l)| l);
                let d = match &self.diff_view {
                    Some(d) => d.clone(),
                    None => { let d = self.make_diff_view(file.clone(), cx); self.diff_view = Some(d.clone()); d }
                };
                d.update(cx, |d, cx| {
                    d.set_file(file, cx);
                    d.set_comments(comments, cx);
                    d.set_side(side, cx);
                    d.set_layout(layout, cx);
                    d.set_change_ruler(whole && changed, cx);
                    if let Some(l) = reveal { d.reveal(l, cx) }
                });
                div().flex_1().min_h_0().p_2().child(d).into_any_element()
            }
            Err(e) => {
                let p = sel.clone().unwrap_or_default();
                div().flex_1().child(ui::empty_state("triangle-alert", "Can't Load the File", e)
                    .child(ui::pill("full-retry", "Retry").on_click(cx.listener(move |d, _, _, cx| { d.full.remove(&p); d.ensure_full(cx); cx.notify() }))))
                    .into_any_element()
            }
            Ok(None) if sel.is_some() && whole => div().flex_1().child(ui::empty_state("loader", "", "")).into_any_element(),
            Ok(None) if self.loading && self.diffs.is_empty() => div().flex_1().child(ui::empty_state("loader", "Loading diff…", "")).into_any_element(),
            Ok(None) => div().flex_1().child(ui::empty_state("file-search", "No File Selected", "")).into_any_element(),
        };
        let w = cx.weak_entity();
        let on: tabs::OnTabEvent = Rc::new(move |e, _, cx| { w.update(cx, |d, cx| d.tab_event(e, cx)).ok(); });
        let diffs: HashMap<String, gpui_kit::Hsla> = self.diffs.iter().map(|f| (f.path().to_string(), badge(f).1)).collect();
        div().flex().flex_col().size_full().min_w(px(360.))
            .when(full && !self.nav.tabs.is_empty(), |d| d.child(tabs::tab_bar(&self.nav, move |p| diffs.get(p).copied(), on.clone())))
            .child(content)
            .when_some(self.nav_results.as_ref().filter(|_| full), |d, r| d.child(div().h(px(220.)).flex_none().border_t_1().border_color(theme::border()).child(tabs::results_panel(r, on))))
            .into_any_element()
    }

    fn files_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tree = self.tree_pane(cx);
        let pane = self.file_pane(cx);
        ui::h_resizable("pr-files-split")
            .child(ui::resizable_panel().size(px(300.)).size_range(px(250.)..px(460.)).child(tree))
            .child(ui::resizable_panel().child(pane))
            .into_any_element()
    }
}

thread_local! {
    /// The shown file's commentable lines (new, old), read by the diff view's `can_comment` while it renders.
    static COMMENTABLE: std::cell::RefCell<Option<(HashSet<u32>, HashSet<u32>)>> = const { std::cell::RefCell::new(None) };
}

/// A floating panel under the header, closed by a click outside.
fn popover_frame(close: impl Fn(&gpui_kit::MouseDownEvent, &mut Window, &mut App) + 'static, top: gpui_kit::Pixels,
                 right: Option<gpui_kit::Pixels>, left: Option<gpui_kit::Pixels>, content: AnyElement) -> AnyElement {
    div().absolute().inset_0()
        .child(div().absolute().inset_0().on_mouse_down(gpui_kit::MouseButton::Left, close))
        .child(div().absolute().top(top).map(|d| match (right, left) { (Some(r), _) => d.right(r), (_, Some(l)) => d.left(l), _ => d })
            .occlude().rounded(px(8.)).bg(theme::raised()).border_1().border_color(theme::border_strong()).shadow_lg().child(content))
        .into_any_element()
}

/// A centred alert over the detail: a title, a message, the buttons.
fn modal(title: String, message: Option<String>, buttons: AnyElement) -> AnyElement {
    div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop()).occlude()
        .child(ui::card().w(px(420.)).p_4().gap_3().bg(theme::raised())
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .when_some(message, |d, m| d.child(div().id("modal-text").max_h(px(300.)).overflow_y_scroll().text_size(px(12.)).text_color(theme::text_muted()).child(m)))
            .child(buttons))
        .into_any_element()
}

impl Render for PullDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A closed popover or comment editor takes its focus with it; keys (⌘[, ⇧⌘V, ⌥↓) then reach nothing.
        if self.shown && window.focused(cx).is_none() { self.focus.focus(window, cx) }
        if std::mem::take(&mut self.composer.clear) { self.composer.input.update(cx, |i, cx| i.set_value("", window, cx)) }
        let header = self.header(cx);
        let body = match self.tab {
            ViewMode::Files => self.files_tab(cx),
            ViewMode::Conversation => super::conversation::render(self, window, cx),
            ViewMode::Overview => overview::render(self, cx),
        };
        let badge = self.badge_popover(cx);
        let review = self.review_popover(cx);
        let merge = self.merge_dialog(cx);
        let alert = self.error_alert(cx);
        div().id("pr-detail").track_focus(&self.focus).key_context("PullDetail").relative().flex().flex_col().size_full().bg(theme::canvas())
            .on_action(cx.listener(|d, _: &MarkViewed, _, cx| d.mark_viewed_and_advance(cx)))
            .on_action(cx.listener(|d, _: &NextFile, _, cx| d.move_by(1, cx)))
            .on_action(cx.listener(|d, _: &PreviousFile, _, cx| d.move_by(-1, cx)))
            .on_action(cx.listener(|d, _: &NavigateBack, _, cx| d.tab_event(TabEvent::Back, cx)))
            .on_action(cx.listener(|d, _: &NavigateForward, _, cx| d.tab_event(TabEvent::Forward, cx)))
            // ⌘[: with the checks overlay open over the PR the overlay takes it (FLOWS.md §14: it was ambiguous).
            .on_action(cx.listener(|d, _: &BackToList, _, cx| if d.app.read(cx).overlay.is_none() { cx.emit(DetailEvent::Back) } else { cx.propagate() }))
            // ⌘W closes the open tab in Full mode on Files, and with no tab does nothing: it never quits the app there.
            .on_action(cx.listener(|d, _: &CloseWindow, _, cx| {
                if d.tab == ViewMode::Files && d.full_mode(cx) {
                    if let Some(p) = d.nav.selected.clone() { d.tab_event(TabEvent::Close(p), cx) }
                } else { cx.propagate() }
            }))
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .children(badge).children(review).children(merge).children(alert)
    }
}

