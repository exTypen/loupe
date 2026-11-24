//! One worktree's entity: status, the shown diff, every git action and the file watcher (Model.swift's WorktreeModel).
//!
//! Rules from the Swift app's last commits, kept here:
//! - no work while nothing changes: refreshes come only from file events that matter (core's Watcher filter), a
//!   250 ms debounce, actions and ⌘R; a refresh asked for while one runs runs once more after it, not twice;
//! - only the selected worktree loads its diff (`release_heavy_state` / `restore_heavy_state`);
//! - a cancelled read is never an error (`UserError::user_message` is None for it).
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::{Context, Task, WeakEntity};
use loupe_core::git::{self, Cancel, FileChange};
use loupe_core::model::{self as core_model, ConflictSide, GitOperation, HunkAction, StatusSnapshot, WorktreeState};
use loupe_core::undo::UndoStack;
use loupe_core::watch::Watcher;
use loupe_core::{lfs, Result};
use loupe_forge::RemoteRepo;

use super::{AppModel, Status, adapt};
use crate::diff;

/// A failure worth showing, or None for a cancellation.
pub trait UserError { fn user_message(&self) -> Option<String>; }
impl UserError for loupe_core::Error { fn user_message(&self) -> Option<String> { loupe_core::Error::user_message(self) } }
impl UserError for loupe_forge::Error { fn user_message(&self) -> Option<String> { loupe_forge::Error::user_message(self) } }
impl UserError for String { fn user_message(&self) -> Option<String> { Some(self.clone()) } }

pub struct WorktreeModel {
    /// loupe-core's state: branch, files (staged/unstaged/conflicted/listed), selection, commit box, busy, error, ….
    pub state: WorktreeState,
    pub repo: String,
    pub app: WeakEntity<AppModel>,
    /// The forge repository of the remote picked by `detect_remote` (`state.remote_name` is its name).
    pub remote: Option<RemoteRepo>,
    pub remote_detected: bool,
    /// The renderer's copy of `state.diff`: replaced only when the diff text changed, so the table keeps its scroll.
    pub diff: Option<Arc<diff::FileDiff>>,
    selected: bool,
    worktrees: Vec<String>,
    watcher: Option<Watcher>,
    _watch: Option<Task<()>>,
    debounce: Option<Task<()>>,
    refreshing: bool,
    refresh_again: bool,
    diff_load: Option<(Cancel, Task<()>)>,
    last_toast: u64,
}

impl WorktreeModel {
    pub(crate) fn new(path: String, repo: String, app: WeakEntity<AppModel>, worktrees: Vec<String>, selected: bool, cx: &mut Context<Self>) -> Self {
        let mut m = WorktreeModel {
            state: WorktreeState::new(PathBuf::from(&path)), repo, app, remote: None, remote_detected: false, diff: None,
            selected, worktrees, watcher: None, _watch: None, debounce: None, refreshing: false, refresh_again: false,
            diff_load: None, last_toast: 0,
        };
        m.detect_remote(cx);
        m.start_watching(cx);
        m.refresh(cx);
        m
    }

    pub fn path(&self) -> &Path { &self.state.path }
    pub fn path_str(&self) -> String { self.state.path.to_string_lossy().into_owned() }
    pub fn is_selected(&self) -> bool { self.selected }
    pub fn is_busy(&self) -> bool { self.state.busy.is_some() }
    pub fn clear_error(&mut self, cx: &mut Context<Self>) { if self.state.error.take().is_some() { cx.notify() } }
    pub fn set_error(&mut self, e: impl UserError, cx: &mut Context<Self>) {
        if let Some(m) = e.user_message() { self.state.error = Some(m); cx.notify() }
    }

    // MARK: Watching and refreshing

    fn start_watching(&mut self, cx: &mut Context<Self>) {
        let path = self.state.path.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        self._watch = Some(cx.spawn(async move |this, cx| {
            // Watcher::new blocks for one rev-parse (the git dir, which a linked worktree has outside its folder).
            let w = cx.background_executor().spawn(async move { Watcher::new(&path, move |c| { let _ = tx.unbounded_send(c); }) }).await;
            let Ok(w) = w else { return };
            if this.update(cx, |m, _| { w.set_worktrees(m.worktrees.clone()); m.watcher = Some(w) }).is_err() { return }
            // Ends when the model (and its watcher, the sender) goes away.
            while let Some(c) = rx.next().await {
                let alive = this.update(cx, |m, cx| {
                    if c.worktrees && let Some(app) = m.app.upgrade() {
                        let repo = m.repo.clone();
                        // Deferred: the app may be the one updating us right now.
                        cx.defer(move |cx| app.update(cx, |a, cx| a.refresh_worktrees(repo, cx).detach()));
                    }
                    if c.status { m.schedule_refresh(cx) }
                });
                if alive.is_err() { break }
            }
        }));
    }

    /// The repository's worktrees, so events inside nested ones are left to them.
    pub(crate) fn set_worktrees(&mut self, paths: Vec<String>) {
        if let Some(w) = &self.watcher { w.set_worktrees(paths.clone()) }
        self.worktrees = paths;
    }

    /// File events: FSEvents latency (150 ms) plus this 250 ms, so a burst of writes is one refresh.
    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            this.update(cx, |m, cx| m.refresh(cx)).ok();
        }));
    }

    /// Status, stash count, operation, LFS, submodules, merged state; then the selected file's diff. One at a time:
    /// asked again while running, it runs once more when done.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing { self.refresh_again = true; return }
        self.refreshing = true;
        let q = self.state.status_query();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { core_model::read_status(&q) }).await;
            this.update(cx, |m, cx| {
                m.refreshing = false;
                match r {
                    Ok(s) => m.apply_status(s, cx),
                    Err(e) => m.set_error(e, cx),
                }
                if std::mem::take(&mut m.refresh_again) { m.refresh(cx) }
            }).ok();
        }).detach();
    }

    fn apply_status(&mut self, s: StatusSnapshot, cx: &mut Context<Self>) {
        let locks_due = s.lfs.locks_due;
        let file = self.state.apply_status(s);
        if self.selected {
            match file { Some(_) => self.load_diff(cx), None => self.cancel_diff() }
        }
        if file.is_none() { self.diff = None }
        if locks_due { self.load_lfs_locks(cx) }
        let st = Status { changes: self.state.change_count(), ahead: self.state.ahead as usize, behind: self.state.behind as usize, merged: self.state.is_merged };
        let path = self.path_str();
        if let Some(app) = self.app.upgrade() { cx.defer(move |cx| app.update(cx, |a, cx| a.set_status(&path, st, cx))) }
        cx.notify();
    }

    /// LFS locks are asked once a minute per repository (LfsState decides when), off to the side. The answer goes to
    /// the repository's other worktrees too: one that refreshed while this request ran only got the placeholder
    /// (empty) answer, so its lock badges stayed off until the next minute.
    fn load_lfs_locks(&mut self, cx: &mut Context<Self>) {
        let path = self.state.path.clone();
        let key = self.state.lfs.key(&path);
        let (app, remote, p, repo) = (self.app.clone(), self.remote.clone(), self.path_str(), self.repo.clone());
        cx.spawn(async move |this, cx| {
            let env = app.read_with(cx, |a, _| a.git_auth(remote.as_ref(), &p)).unwrap_or_default();
            let k = key.clone();
            let ans = cx.background_executor().spawn(async move { lfs::load_locks(&path, &k, &env) }).await;
            this.update(cx, |m, cx| { m.state.lfs.apply_locks(ans.clone()); cx.notify() }).ok();
            let Some((locks, supported)) = ans else { return };
            let me = this.entity_id();
            let others = app.read_with(cx, |a, _| a.worktrees_of(&repo).iter().filter_map(|w| a.model(&w.path)).filter(|o| o.entity_id() != me).collect::<Vec<_>>()).unwrap_or_default();
            for o in others {
                o.update(cx, |w, cx| {
                    let l = &mut w.state.lfs;
                    if l.common_dir.as_deref() == Some(key.as_str()) && (l.locks != locks || l.locking_supported != supported) {
                        l.locks = locks.clone();
                        l.locking_supported = supported;
                        cx.notify();
                    }
                });
            }
        }).detach();
    }

    // MARK: Selection and the shown diff

    /// Shows one file's diff (the file list's plain click).
    pub fn select_file(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.state.selected_file_id == id && self.state.selected_file_ids.len() <= 1 { return }
        self.state.select(id);
        self.load_diff(cx);
        cx.notify();
    }

    /// The file list's multi-selection (⌘/⇧-click, ⌘A) and the file whose diff shows.
    pub fn select_files(&mut self, shown: Option<String>, ids: HashSet<String>, cx: &mut Context<Self>) {
        let reload = self.state.selected_file_id != shown;
        self.state.selected_file_id = shown;
        self.state.selected_file_ids = ids;
        if reload { self.load_diff(cx) }
        cx.notify();
    }

    fn cancel_diff(&mut self) {
        if let Some((c, _)) = self.diff_load.take() { c.cancel() }
    }

    /// (Re)loads the selected file's diff: whole file in Full mode or with one side shown. The previous load is
    /// cancelled; an unchanged diff keeps the shown one (and its scroll and selection).
    pub fn load_diff(&mut self, cx: &mut Context<Self>) {
        self.cancel_diff();
        let Some(f) = self.state.selected_file().cloned() else { self.diff = None; self.state.diff = None; cx.notify(); return };
        let cancel = Cancel::new();
        let (c, path, app) = (cancel.clone(), self.state.path.clone(), self.app.clone());
        let task = cx.spawn(async move |this, cx| {
            let whole = app.read_with(cx, |a, _| a.whole_files()).unwrap_or(false);
            let r = cx.background_executor().spawn(async move {
                let d = core_model::load_diff(&path, &f, whole, Some(&c))?;
                let shown = d.as_ref().map(adapt::file_diff);
                Result::Ok((d, shown))
            }).await;
            this.update(cx, |m, cx| {
                m.diff_load = None;
                match r {
                    Ok((d, shown)) => if m.state.set_diff(d) { m.diff = shown.map(Arc::new); cx.notify() },
                    Err(e) => m.set_error(e, cx),
                }
            }).ok();
        });
        self.diff_load = Some((cancel, task));
    }

    pub(crate) fn release_heavy_state(&mut self, cx: &mut Context<Self>) {
        self.selected = false;
        self.cancel_diff();
        self.state.release_heavy_state();
        self.diff = None;
        cx.notify();
    }

    pub(crate) fn restore_heavy_state(&mut self, cx: &mut Context<Self>) {
        self.selected = true;
        if self.diff.is_none() && self.state.selected_file().is_some() { self.load_diff(cx) }
    }

    // MARK: Remote

    /// Prefer a remote we have an account for, then "origin", then the first. Runs again when accounts or tokens
    /// change.
    pub fn detect_remote(&mut self, cx: &mut Context<Self>) {
        let path = self.state.path.clone();
        cx.spawn(async move |this, cx| {
            let list = cx.background_executor().spawn(async move { git::remotes(&path) }).await;
            let candidates: Vec<(String, RemoteRepo)> = list.into_iter().filter_map(|(n, u)| RemoteRepo::parse(&u).map(|r| (n, r))).collect();
            let Ok((app, p)) = this.update(cx, |m, _| (m.app.clone(), m.path_str())) else { return };
            let with_account = app.read_with(cx, |a, _| candidates.iter().position(|(_, r)| a.account_for(r, &p).is_some())).ok().flatten();
            let pick = with_account.or_else(|| candidates.iter().position(|(n, _)| n == "origin")).or((!candidates.is_empty()).then_some(0));
            this.update(cx, |m, cx| {
                let pick = pick.map(|i| candidates[i].clone());
                m.state.remote_name = pick.as_ref().map_or("origin".into(), |p| p.0.clone());
                m.remote = pick.map(|p| p.1);
                m.remote_detected = true;
                cx.notify();
            }).ok();
        }).detach();
    }

    // MARK: Actions

    /// Runs `op` (given the worktree path) on the background with `busy` set, puts a failure in `error` (never a
    /// cancellation), shows the undo toast an action recorded, then refreshes. The task gives `op`'s value, None
    /// when it failed.
    pub fn perform<R, E>(&mut self, label: &str, op: impl FnOnce(&Path) -> std::result::Result<R, E> + Send + 'static, cx: &mut Context<Self>) -> Task<Option<R>>
    where R: Send + 'static, E: UserError + Send + 'static {
        self.state.busy = Some(label.into());
        cx.notify();
        let path = self.state.path.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { op(&path) }).await;
            this.update(cx, |m, cx| {
                m.state.busy = None;
                let out = match r { Ok(v) => Some(v), Err(e) => { m.set_error(e, cx); None } };
                m.show_undo_toast(cx);
                m.refresh(cx);
                cx.notify();
                out
            }).ok().flatten()
        })
    }

    /// The toast the last action (or undo) left on the undo stack, once; gone after six seconds.
    fn show_undo_toast(&mut self, cx: &mut Context<Self>) {
        let stack = UndoStack::for_path(&self.state.path);
        let Some(t) = stack.toast().filter(|t| t.id != self.last_toast) else { return };
        self.last_toast = t.id;
        if let Some(app) = self.app.upgrade() {
            let (text, undoable) = (t.text.clone(), t.entry.is_some());
            cx.defer(move |cx| app.update(cx, |a, cx| a.toast(text, undoable, cx)));
        }
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            stack.clear_toast(t.id);
        }).detach();
    }

    pub fn stage(&mut self, fs: Vec<FileChange>, cx: &mut Context<Self>) -> Task<Option<()>> {
        self.perform("Staging", move |p| core_model::stage(p, &fs), cx)
    }
    pub fn unstage(&mut self, fs: Vec<FileChange>, cx: &mut Context<Self>) -> Task<Option<()>> {
        self.perform("Unstaging", move |p| core_model::unstage(p, &fs), cx)
    }
    /// Tracked changes are reverted, untracked files go to the Trash; the caller confirms first.
    pub fn discard(&mut self, fs: Vec<FileChange>, cx: &mut Context<Self>) -> Task<Option<()>> {
        self.perform("Discarding", move |p| core_model::discard(p, &fs), cx)
    }

    /// Stage, unstage or discard these lines (indices into the shown diff's lines, as DiffView's on_hunk_action
    /// gives them) of the selected file. Never on a stale diff: the pane may show the previous file's while the next
    /// loads.
    pub fn apply_lines(&mut self, action: HunkAction, lines: impl IntoIterator<Item = usize>, cx: &mut Context<Self>) -> Task<Option<()>> {
        let (Some(d), Some(f)) = (self.state.diff.clone(), self.state.selected_file().cloned()) else { return Task::ready(None) };
        let lines: HashSet<usize> = lines.into_iter().collect();
        let label = match action { HunkAction::Stage => "Staging", HunkAction::Unstage => "Unstaging", HunkAction::Discard => "Discarding" };
        self.perform(label, move |p| core_model::apply_lines(p, &d, &f, action, &lines), cx)
    }

    /// Commits what's staged with the commit box's message (hooks and signing run); clears the box on success.
    pub fn commit(&mut self, cx: &mut Context<Self>) -> Task<Option<()>> {
        let (msg, amend) = (self.state.commit_message.trim().to_string(), self.state.amend);
        if msg.is_empty() { return Task::ready(None) }
        let t = self.perform(if amend { "Amending" } else { "Committing" }, move |p| core_model::commit(p, &msg, amend), cx);
        cx.spawn(async move |this, cx| {
            let r = t.await;
            if r.is_some() { this.update(cx, |m, cx| { m.state.commit_message.clear(); m.state.amend = false; cx.notify() }).ok(); }
            r
        })
    }

    pub fn set_commit_message(&mut self, msg: String, cx: &mut Context<Self>) {
        if self.state.commit_message != msg { self.state.commit_message = msg; cx.notify() }
    }

    /// Amend on: the last commit's message goes into an empty box.
    pub fn set_amend(&mut self, on: bool, cx: &mut Context<Self>) {
        self.state.amend = on;
        cx.notify();
        if on && self.state.commit_message.trim().is_empty() {
            let path = self.state.path.clone();
            cx.spawn(async move |this, cx| {
                let m = cx.background_executor().spawn(async move { core_model::last_commit_message(&path) }).await;
                this.update(cx, |w, cx| if let Some(m) = m && w.state.amend && w.state.commit_message.is_empty() { w.state.commit_message = m; cx.notify() }).ok();
            }).detach();
        }
    }

    pub fn stash(&mut self, message: String, untracked: bool, paths: Vec<String>, cx: &mut Context<Self>) -> Task<Option<()>> {
        self.perform("Stashing", move |p| core_model::stash(p, &message, untracked, &paths), cx)
    }

    pub fn finish_operation(&mut self, abort: bool, cx: &mut Context<Self>) -> Task<Option<()>> {
        let Some(op) = self.state.operation else { return Task::ready(None) };
        self.perform(if abort { "Aborting" } else { "Continuing" }, move |p| core_model::finish_operation(p, op, abort), cx)
    }

    pub fn resolve(&mut self, file: String, side: Option<ConflictSide>, cx: &mut Context<Self>) -> Task<Option<()>> {
        self.perform("Resolving", move |p| core_model::resolve(p, &file, side), cx)
    }

    /// fetch / pull / push with the space's credentials; then remote refs moved (History reloads on its next
    /// showing) and the repository's worktrees are listed again.
    pub fn remote_op(&mut self, label: &str, args: Vec<String>, cx: &mut Context<Self>) -> Task<Option<()>> {
        let (app, remote, p, label) = (self.app.clone(), self.remote.clone(), self.path_str(), label.to_string());
        cx.spawn(async move |this, cx| {
            let env = app.read_with(cx, |a, _| a.git_auth(remote.as_ref(), &p)).unwrap_or_default();
            let t = this.update(cx, |m, cx| m.perform(&label, move |d| {
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                core_model::remote_op(d, &a, &env)
            }, cx)).ok()?;
            let r = t.await;
            let repo = this.update(cx, |m, cx| { m.state.bump_refs(); cx.notify(); m.repo.clone() }).ok()?;
            app.update(cx, |a, cx| a.refresh_worktrees(repo, cx)).ok()?.await;
            r
        })
    }

    pub fn fetch(&mut self, cx: &mut Context<Self>) -> Task<Option<()>> { self.remote_op("Fetching", vec!["fetch".into(), "--all".into(), "--prune".into()], cx) }
    pub fn pull(&mut self, cx: &mut Context<Self>) -> Task<Option<()>> { self.remote_op("Pulling", vec!["pull".into(), "--ff-only".into()], cx) }
    pub fn push(&mut self, cx: &mut Context<Self>) -> Task<Option<()>> {
        let remote = self.state.remote_name.clone();
        self.remote_op("Pushing", vec!["push".into(), "--set-upstream".into(), remote, "HEAD".into()], cx)
    }

    /// The branch picker: `git switch` (a remote "origin/x" makes a tracking x), recorded for Undo. A branch checked
    /// out in another worktree selects that worktree instead (`checked_out`: git::branches' map).
    pub fn switch_branch(&mut self, branch: String, remote: bool, checked_out: Option<String>, cx: &mut Context<Self>) -> Task<Option<()>> {
        let (app, repo) = (self.app.clone(), self.repo.clone());
        if let Some(wt) = checked_out {
            return cx.spawn(async move |_, cx| {
                let known = app.read_with(cx, |a, _| a.worktrees_of(&repo).iter().any(|w| w.path == wt)).ok()?;
                if !known { app.update(cx, |a, cx| a.refresh_worktrees(repo, cx)).ok()?.await }
                app.update(cx, |a, cx| a.select(Some(wt), cx)).ok()
            });
        }
        let (current, head) = (self.state.branch.clone(), self.state.head_oid.clone());
        let t = self.perform("Switching", move |p| core_model::switch_branch(p, &branch, remote, current.as_deref(), &head), cx);
        cx.spawn(async move |_, cx| {
            let r = t.await;
            app.update(cx, |a, cx| a.refresh_worktrees(repo, cx)).ok()?.await;
            r
        })
    }

    /// ⌘Z: undoes the newest action. The commit box gets its message back unless something was typed since; a refused
    /// undo shows why.
    pub fn undo(&mut self, cx: &mut Context<Self>) -> Task<Option<()>> {
        let t = self.perform("Undoing", core_model::undo_last, cx);
        cx.spawn(async move |this, cx| {
            let undone = t.await.flatten()?;
            this.update(cx, |m, cx| if let Some((msg, amend)) = undone.message && m.state.commit_message.is_empty() {
                m.state.commit_message = msg;
                m.state.amend = amend;
                cx.notify();
            }).ok()
        })
    }

    /// The "Undo <label>" menu title, or None when there's nothing to undo.
    pub fn undo_title(&self) -> Option<String> {
        let s = UndoStack::for_path(&self.state.path);
        (!s.is_empty()).then(|| s.title())
    }

    pub fn operation(&self) -> Option<GitOperation> { self.state.operation }
}
