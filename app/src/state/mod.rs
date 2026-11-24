//! The app's gpui entities over loupe-core's plain state (Model.swift's AppModel and WorktreeModel).
//!
//! - [`AppModel`] (one per app): spaces, repositories, their worktrees, the selection, accounts and tokens, the
//!   shown tab and the diff mode, plus the event bus views use to hand work to each other ([`AppEvent`]).
//! - [`WorktreeModel`] (one per listed worktree): status, the shown diff, every git action, the file watcher.
//!
//! Everything that runs git blocks, so it runs on the background executor and is applied here on the main thread.
//! Nothing polls: refreshes come from file events (250 ms debounce), actions and ⌘R. See docs/gpui/VIEWS.md.
pub mod adapt;
pub mod scenario;
pub mod worktree;

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, EventEmitter, Task, Window};
use loupe_core::git::{self, Worktree};
use loupe_core::model::{AppState, Saved, Space};
use loupe_forge::{Account, Client, RemoteRepo, account, settings};

pub use worktree::WorktreeModel;

use crate::diff::{DiffSide, Layout};

// MARK: - Shared view vocabulary

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab { Changes, History, Pulls }

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Changes, Tab::History, Tab::Pulls];
    /// The "tab" default's value.
    pub fn key(self) -> &'static str { match self { Tab::Changes => "changes", Tab::History => "history", Tab::Pulls => "pulls" } }
    pub fn from_key(k: &str) -> Self { match k { "history" => Tab::History, "pulls" => Tab::Pulls, _ => Tab::Changes } }
    pub fn title(self) -> &'static str { match self { Tab::Changes => "Changes", Tab::History => "History", Tab::Pulls => "Pull Requests" } }
    pub fn index(self) -> usize { self as usize }
}

/// Overview / Files (/ Conversation) for whatever change is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode { Overview, Files, Conversation }

/// Which of the toolbar's diff controls apply to what a tab shows. A view sets its tab's with
/// [`AppModel::set_controls`] when they change (not on every render).
#[derive(Clone, Default)]
pub struct DiffControls {
    pub full: bool, // a diff is showing: Compact/Full applies
    pub view: Option<(Vec<ViewMode>, ViewMode, Rc<dyn Fn(ViewMode, &mut Window, &mut App)>)>,
}

impl DiffControls {
    fn same(&self, o: &Self) -> bool {
        self.full == o.full && match (&self.view, &o.view) {
            (None, None) => true,
            (Some(a), Some(b)) => a.0 == b.0 && a.1 == b.1,
            _ => false,
        }
    }
}

/// What a worktree's sidebar row shows besides its name.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    pub changes: usize,
    pub ahead: usize,
    pub behind: usize,
    pub merged: bool,
}

/// Cross-view requests. Emitted on the [`AppModel`] entity (`app.update(cx, |a, cx| a.emit(..))`); the shell
/// switches tabs and hosts toasts, and each view subscribes for the ones it serves.
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    /// History: select `sha` in `worktree`'s log (the shell switches to History first).
    ShowCommit { worktree: String, sha: String },
    /// History: show `worktree`'s log of `branch` (e.g. "origin/main") with its newest commit selected (SH-39).
    ShowBranch { worktree: String, branch: String },
    /// Pull Requests: open PR `number` (the shell switches to Pull Requests first).
    OpenPull { worktree: String, number: u64 },
    /// Merge editor: open the conflicted `path` of `worktree` (an overlay the merge view presents).
    OpenMergeEditor { worktree: String, path: String },
    /// CI: the checks overlay for commit `sha` (and the PR it belongs to, when there is one).
    ShowChecks { worktree: String, sha: String, pull: Option<u64> },
    /// PR create: the New Pull Request sheet for `worktree`.
    CreatePull { worktree: String },
    /// History: `worktree`'s log on the Stashes scope (SS-04, the Changes "N stashes" link).
    ShowStashes { worktree: String },
    /// The shell's New Worktree sheet for `repo`, based on `base` (a commit; "" for HEAD): History's
    /// "New Worktree from Here…" (HI-20).
    NewWorktree { repo: String, base: String },
    /// A bottom toast; `undoable` adds an "Undo" link that runs ⌘Z on the selected worktree.
    Toast { text: String, undoable: bool },
    /// ⌘R: reload what's shown (status is already refreshing; History, PR list, an open PR reload).
    Refresh,
}

// MARK: - AppModel

pub struct AppModel {
    state: AppState,
    /// Every space's accounts (the "accounts" setting) and their tokens (Keychain, read once off the main thread).
    pub accounts: Vec<Account>,
    pub tokens: HashMap<String, String>,
    pub tokens_loaded: bool,
    models: HashMap<String, Entity<WorktreeModel>>,
    status: HashMap<String, Status>,
    /// The shown tab, Compact/Full ("prFullMode", shared by every diff) and the diff side ("diffSide").
    pub tab: Tab,
    pub full_mode: bool,
    pub diff_side: DiffSide,
    pub layout: Layout,
    controls: [DiffControls; 3],
    /// App-level failure (a folder that isn't a repository): the shell's "Git" alert. Per-worktree errors live on
    /// `WorktreeModel::state.error`.
    pub error: Option<String>,
    /// A full-window overlay or sheet a view presented (checks, merge editor, PR create). One at a time.
    pub overlay: Option<AnyView>,
    /// LOUPE_STRESS run: in-memory account, separate settings domain, no Keychain.
    pub scenario: Option<scenario::Scenario>,
    _subs: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<AppEvent> for AppModel {}

impl Deref for AppModel {
    type Target = AppState;
    fn deref(&self) -> &AppState { &self.state }
}
/// Direct field edits (`collapsed`, a space's name) are saved on the next `cx.notify()`.
impl DerefMut for AppModel {
    fn deref_mut(&mut self) -> &mut AppState { &mut self.state }
}

impl AppModel {
    /// Reads the settings (Swift keys), lists every repo's worktrees and loads tokens in the background.
    pub fn new(scenario: Option<scenario::Scenario>, cx: &mut Context<Self>) -> Self {
        let p = settings::shared();
        let spaces = p.data("spaces").and_then(|d| serde_json::from_slice::<Vec<Space>>(d).ok());
        let saved = Saved {
            spaces,
            repos: p.strings("repos"),
            selection: p.string("selection"),
            current_space: p.string("currentSpace"),
            recent: p.strings("recent"),
            collapsed: p.strings("collapsed"),
        };
        let tab = Tab::from_key(&p.string("tab").unwrap_or_default());
        let full_mode = p.bool("prFullMode");
        let diff_side = match p.string("diffSide").as_deref() { Some("old") => DiffSide::Old, Some("new") => DiffSide::New, _ => DiffSide::Diff };
        let layout = if p.string("diffLayout").as_deref() == Some("split") { Layout::Split } else { Layout::Unified };
        let accounts = if scenario.is_some() { vec![] } else { p.accounts() };
        drop(p);
        let mut m = AppModel {
            state: AppState::new(saved), accounts, tokens: HashMap::new(), tokens_loaded: false, models: HashMap::new(),
            status: HashMap::new(), tab, full_mode, diff_side, layout, controls: Default::default(), error: None,
            overlay: None, scenario, _subs: vec![],
        };
        if let Some(s) = &m.scenario {
            // In memory only: never written to the Keychain or the accounts setting.
            let a = Account::new(&s.forge, &s.user, false, account::DEFAULT_SPACE);
            m.tokens.insert(a.id(), s.token.clone());
            m.accounts.push(a);
            m.tokens_loaded = true;
        } else {
            m.load_tokens(cx);
        }
        // Every notify persists what changed (Prefs::set skips equal values, and saving is debounced off-thread).
        m._subs.push(cx.observe_self(|m, _| m.save()));
        m.refresh_all_worktrees(cx);
        m
    }

    fn save(&self) {
        let mut p = settings::shared();
        p.set("spaces", Some(plist::Value::Data(self.state.spaces_json().into_bytes())));
        p.set_string("currentSpace", &self.state.current_space);
        p.set_strings("recent", &self.state.recent);
        let mut c: Vec<String> = self.state.collapsed.iter().cloned().collect();
        c.sort();
        p.set_strings("collapsed", &c);
        p.set_string("tab", self.tab.key());
        p.set_bool("prFullMode", self.full_mode);
        p.set_string("diffSide", match self.diff_side { DiffSide::Diff => "diff", DiffSide::Old => "old", DiffSide::New => "new" });
        p.set_string("diffLayout", if self.layout == Layout::Split { "split" } else { "unified" });
        if self.scenario.is_none() { p.set_accounts(&self.accounts) }
    }

    /// The Swift app froze its launch for 4–9 s reading the Keychain on the main thread: read it once, here, off it.
    fn load_tokens(&mut self, cx: &mut Context<Self>) {
        let accts = self.accounts.clone();
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { account::load_tokens(&accts) }).await;
            this.update(cx, |m, cx| {
                m.tokens.extend(found);
                m.tokens_loaded = true;
                for wm in m.models.values() { wm.update(cx, |w, cx| w.detect_remote(cx)) }
                cx.notify();
            }).ok();
        }).detach();
    }

    /// Adds or replaces (same id) an account and stores its token (Keychain, except in a scenario run).
    pub fn set_account(&mut self, a: Account, token: Option<String>, cx: &mut Context<Self>) {
        if self.scenario.is_none() {
            let (acct, t) = (a.clone(), token.clone());
            cx.background_spawn(async move { acct.save_token(t.as_deref()) }).detach();
        }
        self.accounts.retain(|x| x.id() != a.id());
        match token { Some(t) => { self.tokens.insert(a.id(), t); } None => { self.tokens.remove(&a.id()); } }
        self.accounts.push(a);
        for wm in self.models.values() { wm.update(cx, |w, cx| w.detect_remote(cx)) }
        cx.notify();
    }

    pub fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(a) = self.accounts.iter().find(|a| a.id() == id).cloned() {
            if self.scenario.is_none() { cx.background_spawn(async move { a.save_token(None) }).detach() }
        }
        self.accounts.retain(|a| a.id() != id);
        self.tokens.remove(id);
        cx.notify();
    }

    // MARK: Reading

    pub fn space(&self) -> &Space { self.state.space() }
    pub fn worktrees_of(&self, repo: &str) -> &[Worktree] { self.state.worktrees.get(repo).map_or(&[], |v| v.as_slice()) }
    pub fn selection(&self) -> Option<&str> { self.state.selection.as_deref() }
    pub fn selected_worktree(&self) -> Option<&Worktree> {
        let s = self.state.selection.as_deref()?;
        self.state.worktrees.values().flatten().find(|w| w.path == s)
    }
    /// The selected worktree's model: what every view shows.
    pub fn selected_model(&self) -> Option<Entity<WorktreeModel>> { self.selection().and_then(|s| self.models.get(s).cloned()) }
    pub fn model(&self, path: &str) -> Option<Entity<WorktreeModel>> { self.models.get(path).cloned() }
    pub fn status(&self, path: &str) -> Status { self.status.get(path).copied().unwrap_or_default() }
    /// Uncommitted changes summed over a repo's worktrees: a collapsed header still shows it.
    pub fn change_count(&self, repo: &str) -> usize { self.worktrees_of(repo).iter().map(|w| self.status(&w.path).changes).sum() }
    pub fn controls(&self) -> &DiffControls { &self.controls[self.tab.index()] }

    /// The space holding `worktree`'s repo (else the current one).
    pub fn space_of(&self, worktree: &str) -> &str {
        let repo = self.state.repo_containing(worktree).unwrap_or(worktree);
        self.state.spaces.iter().find(|s| s.repos.iter().any(|r| r == repo)).map_or(&self.state.current_space, |s| &s.id)
    }

    /// The account for `remote` in the space that holds `worktree`'s repo.
    pub fn account_for(&self, remote: &RemoteRepo, worktree: &str) -> Option<&Account> {
        account::find_account(&self.accounts, self.space_of(worktree), remote)
    }

    /// A forge client for `remote` (None: no account, or its token isn't loaded).
    pub fn client_for(&self, remote: &RemoteRepo, worktree: &str) -> Option<Client> {
        let a = self.account_for(remote, worktree)?;
        Some(a.client(self.tokens.get(&a.id())?))
    }

    /// The credential env for git to talk to `remote` with the space's token (core::git::auth_env); empty when
    /// there's no account: SSH remotes and the user's own helpers keep working.
    pub fn git_auth(&self, remote: Option<&RemoteRepo>, worktree: &str) -> Vec<(String, String)> {
        let Some(a) = remote.and_then(|r| self.account_for(r, worktree)) else { return vec![] };
        let Some(t) = self.tokens.get(&a.id()) else { return vec![] };
        git::auth_env(&a.base_url, &a.username, t)
    }

    // MARK: Navigation and the event bus

    pub fn set_tab(&mut self, t: Tab, cx: &mut Context<Self>) {
        if self.tab != t { self.tab = t; cx.notify() }
    }

    pub fn set_full_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.full_mode == on { return }
        self.full_mode = on;
        if let Some(m) = self.selected_model() { m.update(cx, |m, cx| m.load_diff(cx)) }
        cx.notify();
    }

    /// Before / After show the file itself, so changing the side reloads the diff (whole file).
    pub fn set_diff_side(&mut self, side: DiffSide, layout: Layout, cx: &mut Context<Self>) {
        if self.diff_side == side && self.layout == layout { return }
        let reload = (self.diff_side == DiffSide::Diff) != (side == DiffSide::Diff);
        self.diff_side = side;
        self.layout = layout;
        if reload && let Some(m) = self.selected_model() { m.update(cx, |m, cx| m.load_diff(cx)) }
        cx.notify();
    }

    /// Whether a worktree diff loads whole: Full mode, or one side alone.
    pub fn whole_files(&self) -> bool { self.full_mode || self.diff_side != DiffSide::Diff }

    pub fn set_controls(&mut self, tab: Tab, c: DiffControls, cx: &mut Context<Self>) {
        if self.controls[tab.index()].same(&c) { return }
        self.controls[tab.index()] = c;
        cx.notify();
    }

    /// Emits `e`, first switching to the tab that serves it.
    pub fn emit(&mut self, e: AppEvent, cx: &mut Context<Self>) {
        match &e {
            AppEvent::ShowCommit { worktree, .. } | AppEvent::ShowBranch { worktree, .. } | AppEvent::ShowStashes { worktree } => { self.go_to(worktree, cx); self.set_tab(Tab::History, cx) }
            AppEvent::OpenPull { worktree, .. } => { self.go_to(worktree, cx); self.set_tab(Tab::Pulls, cx) }
            _ => {}
        }
        cx.emit(e);
    }

    pub fn toast(&mut self, text: impl Into<String>, undoable: bool, cx: &mut Context<Self>) {
        cx.emit(AppEvent::Toast { text: text.into(), undoable });
    }

    fn go_to(&mut self, worktree: &str, cx: &mut Context<Self>) {
        if self.selection() != Some(worktree) { self.select(Some(worktree.into()), cx) }
    }

    /// Shows `view` over the whole window (sheets, the checks overlay, the merge editor). The view dismisses itself
    /// with [`AppModel::dismiss`] (Esc, Close).
    pub fn present(&mut self, view: AnyView, cx: &mut Context<Self>) { self.overlay = Some(view); cx.notify() }
    pub fn dismiss(&mut self, cx: &mut Context<Self>) { if self.overlay.take().is_some() { cx.notify() } }

    // MARK: Selection and spaces

    /// Only the shown worktree keeps its big, quick-to-rebuild state (the shown diff).
    pub fn select(&mut self, sel: Option<String>, cx: &mut Context<Self>) {
        let old = self.state.select(sel.clone());
        if old != sel {
            if let Some(m) = old.and_then(|o| self.models.get(&o)) { m.update(cx, |m, cx| m.release_heavy_state(cx)) }
            if let Some(m) = sel.and_then(|s| self.models.get(&s)) { m.update(cx, |m, cx| m.restore_heavy_state(cx)) }
        }
        cx.notify();
    }

    pub fn switch_space_to(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(sel) = self.state.spaces.iter().find(|s| s.id == id && id != self.state.current_space).map(|s| s.selection.clone()) else { return };
        self.state.current_space = id.into();
        self.select(sel, cx);
    }

    /// Bounds-checked: the space list can change under a swipe.
    pub fn switch_space_by(&mut self, offset: isize, cx: &mut Context<Self>) {
        let i = self.state.spaces.iter().position(|s| s.id == self.state.current_space).unwrap_or(0) as isize + offset;
        if let Some(id) = usize::try_from(i).ok().and_then(|i| self.state.spaces.get(i)).map(|s| s.id.clone()) { self.switch_space_to(&id, cx) }
    }

    pub fn add_space(&mut self, cx: &mut Context<Self>) -> String {
        let old = self.state.selection.clone();
        let id = self.state.add_space();
        self.after_selection_change(old, cx);
        id
    }

    /// Repos go to the neighbouring space; the space's accounts and their tokens are deleted.
    pub fn delete_space(&mut self, id: &str, cx: &mut Context<Self>) {
        let old = self.state.selection.clone();
        if !self.state.delete_space(id) { return }
        for a in self.accounts.iter().filter(|a| a.space_id() == id).map(|a| a.id()).collect::<Vec<_>>() { self.remove_account(&a, cx) }
        self.after_selection_change(old, cx);
    }

    pub fn move_repos(&mut self, repos: &[String], to: &str, cx: &mut Context<Self>) {
        let old = self.state.selection.clone();
        self.state.move_repos(repos, to);
        self.after_selection_change(old, cx);
    }

    pub fn move_repo(&mut self, repo: &str, before: &str, cx: &mut Context<Self>) { self.state.move_repo(repo, before); cx.notify() }

    fn after_selection_change(&mut self, old: Option<String>, cx: &mut Context<Self>) {
        let new = self.state.selection.clone();
        if old != new {
            if let Some(m) = old.and_then(|o| self.models.get(&o)) { m.update(cx, |m, cx| m.release_heavy_state(cx)) }
            if let Some(m) = new.and_then(|s| self.models.get(&s)) { m.update(cx, |m, cx| m.restore_heavy_state(cx)) }
        }
        cx.notify();
    }

    // MARK: Repositories and worktrees

    /// Opened from the File menu or dropped on the window: normalised to the repository's main worktree (adding a
    /// linked worktree adds its repo), moved into the current space, listed, then what was opened is selected.
    pub fn add_repo(&mut self, path: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let main = cx.background_executor().spawn(async move { loupe_core::model::main_worktree(Path::new(&p)) }).await;
            let Ok(main) = main else {
                this.update(cx, |m, cx| { m.error = Some(format!("{path} is not a git repository.")); cx.notify() }).ok();
                return;
            };
            let Ok(t) = this.update(cx, |m, cx| { m.state.add_repo(&main); m.refresh_worktrees(main.clone(), cx) }) else { return };
            t.await;
            this.update(cx, |m, cx| m.select(Some(path), cx)).ok();
        }).detach();
    }

    pub fn remove_repo(&mut self, repo: &str, cx: &mut Context<Self>) {
        let old = self.state.selection.clone();
        for p in self.state.remove_repo(repo) { self.forget(&p) }
        if old.as_deref().is_some_and(|s| !self.state.worktrees.values().flatten().any(|w| w.path == s)) { self.state.select(None); }
        self.after_selection_change(old, cx);
    }

    /// Its model (and file watcher), undo stack and last rewrite.
    fn forget(&mut self, path: &str) {
        self.models.remove(path);
        self.status.remove(path);
        loupe_core::model::forget(Path::new(path));
    }

    pub fn refresh_all_worktrees(&mut self, cx: &mut Context<Self>) {
        for r in self.state.all_repos() { self.refresh_worktrees(r, cx).detach() }
    }

    /// Re-lists `repo`'s worktrees: gone ones lose their model, watcher and undo history (the selection moves to the
    /// repository), new ones get a model, and every one refreshes its status.
    pub fn refresh_worktrees(&mut self, repo: String, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let r = repo.clone();
            let Ok(list) = cx.background_executor().spawn(async move { git::worktrees(Path::new(&r)) }).await else { return };
            this.update(cx, |m, cx| {
                let old = m.state.selection.clone();
                let gone = m.state.apply_worktrees(&repo, list);
                for g in &gone { m.models.remove(g); m.status.remove(g); }
                let paths: Vec<String> = m.worktrees_of(&repo).iter().map(|w| w.path.clone()).collect();
                let app = cx.entity().downgrade();
                for p in &paths {
                    match m.models.get(p) {
                        Some(wm) => wm.update(cx, |w, cx| { w.set_worktrees(paths.clone()); w.refresh(cx) }),
                        None => {
                            let selected = m.state.selection.as_deref() == Some(p.as_str());
                            let wm = cx.new(|cx| WorktreeModel::new(p.clone(), repo.clone(), app.clone(), paths.clone(), selected, cx));
                            m.models.insert(p.clone(), wm);
                        }
                    }
                }
                m.after_selection_change(old, cx);
            }).ok();
        })
    }

    /// A worktree model finished a refresh: update its sidebar row (notifies only when it changed).
    pub(crate) fn set_status(&mut self, path: &str, s: Status, cx: &mut Context<Self>) {
        if self.status.get(path) != Some(&s) { self.status.insert(path.into(), s); cx.notify() }
    }

    /// Adds a worktree (New Worktree sheet), lists it and selects it.
    pub fn add_worktree(&mut self, repo: String, path: String, branch: String, base: String, new_branch: bool, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        cx.spawn(async move |this, cx| {
            let (r, p) = (repo.clone(), path.clone());
            cx.background_executor().spawn(async move {
                let names = git::all_branch_names(Path::new(&r));
                loupe_core::model::add_worktree_for_branch(Path::new(&r), &p, &branch, &base, new_branch, &names)
            }).await.map_err(|e| e.user_message().unwrap_or_default())?;
            let t = this.update(cx, |m, cx| m.refresh_worktrees(repo, cx)).map_err(|e| e.to_string())?;
            t.await;
            this.update(cx, |m, cx| m.select(Some(path), cx)).ok();
            Ok(())
        })
    }

    /// Removes a worktree (git worktree remove [--force]); the selection moves to the repository.
    pub fn remove_worktree(&mut self, repo: String, path: String, force: bool, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        cx.spawn(async move |this, cx| {
            let (r, p) = (repo.clone(), path.clone());
            cx.background_executor().spawn(async move { loupe_core::model::remove_worktree(Path::new(&r), &p, force) }).await
                .map_err(|e| e.user_message().unwrap_or_default())?;
            let t = this.update(cx, |m, cx| {
                m.forget(&path);
                if m.selection() == Some(path.as_str()) { m.select(Some(repo.clone()), cx) }
                m.refresh_worktrees(repo, cx)
            }).map_err(|e| e.to_string())?;
            t.await;
            Ok(())
        })
    }

    /// Checks out PR `number` in a sibling worktree "<repo>-pr-N" (or selects the worktree already on pr/N).
    pub fn checkout_pull(&mut self, worktree: &str, number: u64, cx: &mut Context<Self>) {
        if let Some(w) = self.state.worktree_with_branch(&format!("pr/{number}")).map(|w| w.path.clone()) { return self.select(Some(w), cx) }
        let Some(wm) = self.models.get(worktree).cloned() else { return };
        let repo = self.state.repo_containing(worktree).unwrap_or(worktree).to_string();
        let (remote, env) = { let w = wm.read(cx); (w.state.remote_name.clone(), self.git_auth(w.remote.as_ref(), worktree)) };
        wm.update(cx, |w, cx| {
            let r = repo.clone();
            let t = w.perform(&format!("Checking out PR #{number}"), move |_| loupe_core::model::checkout_pr_worktree(Path::new(&r), &remote, number, &env), cx);
            cx.spawn(async move |w, cx| {
                let Some(dest) = t.await else { return };
                let Some(app) = w.update(cx, |w, _| w.app.clone()).ok() else { return };
                let Ok(t) = app.update(cx, |a, cx| a.refresh_worktrees(repo, cx)) else { return };
                t.await;
                app.update(cx, |a, cx| a.select(Some(dest), cx)).ok();
            }).detach();
        });
    }
}

/// The shell's "+" in a repo header and the AddWorktree sheet's default path.
pub fn suggest_worktree_path(repo: &str, branch: &str) -> String { loupe_core::model::suggest_worktree_path(repo, branch) }

/// Makes the app's AppModel, from the scenario when LOUPE_STRESS is set.
pub fn init(cx: &mut App) -> Entity<AppModel> {
    let scenario = scenario::from_env(); // first: it picks the settings domain
    crate::shell::keys::load_shortcuts(cx);
    cx.new(|cx| AppModel::new(scenario, cx))
}
