//! The History tab (History.swift, InteractiveRebase.swift, Undo.swift's history side): the paged log with graph
//! lanes and ref badges, scopes, search, the commit detail (message, files, diff against the first parent), the
//! commit actions, interactive rebase by drag and drop or the context menu, and the undo refusal's Revert offer.
//!
//! Rules kept from the Swift app's last commits:
//! - the list reloads only when the worktree's HEAD or remote refs moved (`state.head_oid`, `state.refs_version`),
//!   on ⌘R, after an action that changes the stash list, and on a scope or search change; coming back with
//!   nothing moved costs no git process;
//! - nothing runs while the tab is hidden: leaving it cancels the list, detail and whole-file loads;
//! - only the shown worktree keeps its commit's files; another worktree's come back when it's shown again.
mod graph;
mod panel;
mod rebase;
mod sheets;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, FontWeight, Image, ImageFormat,
    InteractiveElement as _, IntoElement, ListAlignment, ListState, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, list, prelude::FluentBuilder as _, px,
    component::{
        Sizable as _, Selectable as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
        text::TextView,
        tooltip::Tooltip,
    },
};
use loupe_core::diff::FileDiff as CoreDiff;
use loupe_core::file_tree;
use loupe_core::git::{self, Cancel};
use loupe_core::history::{self as core_history, Commit, GraphRow, RefKind, Scope};
use loupe_core::lfs;
use loupe_core::rebase::Zone;

use crate::diff::{self, DiffView};
use crate::shell::keys::{NextFile, PreviousFile, Undo};
use crate::state::{AppEvent, AppModel, DiffControls, Tab, ViewMode, WorktreeModel, adapt};
use crate::views::code::{ChangeOverview, overview as code_overview};
use loupe_code::lens::ChangedPath;
use crate::views::changes::stash::{self, StashOp};
use crate::{theme, ui};
use sheets::{Dialog, Role};

type Act = Rc<dyn Fn(&mut HistoryView, &mut Window, &mut Context<HistoryView>)>;

/// One worktree's History (History.swift's HistoryModel). Kept per worktree while the app runs, so the scope and
/// the selection survive switching worktrees and tabs (not a relaunch, as in Swift).
pub(crate) struct Log {
    commits: Vec<Commit>,
    /// "<HEAD> <refs version>" the list was last loaded for.
    loaded_key: String,
    graph: Vec<GraphRow>,
    graph_width: usize,
    scope: Scope,
    branches: (Vec<String>, Vec<String>),
    not_in_head: HashSet<String>,
    /// The search the list shows: applied with Return, not as it's typed (a fix of §14 History #2).
    search: String,
    loading: bool,
    exhausted: bool,
    selected: Option<String>,
    message: String,
    files: Vec<Shown>,
    selected_path: Option<String>,
    collapsed: HashSet<String>,
    detail_loading: bool,
    /// HEAD's first-parent commits back to the first merge: what a rebase can rewrite (RB-01). `root`: where that
    /// chain ended at the root commit (the commit above it can still meld into it, with --root).
    movable: HashSet<String>,
    root: Option<String>,
    rebasing: bool,
    list: ListState,
    load: Option<(Cancel, Task<()>)>,
    detail: Option<(Cancel, Task<()>)>,
    generation: u64,
}

/// A commit's file: core's diff (for whole-file loads, LFS) and the renderer's copy, made once off the main thread.
#[derive(Clone)]
struct Shown { core: Arc<CoreDiff>, view: Arc<diff::FileDiff> }

impl Shown {
    fn path(&self) -> &str { self.core.path() }
    fn badge(&self) -> &'static str {
        match (&self.core.old_path, &self.core.new_path) {
            (None, _) => "A",
            (_, None) => "D",
            (Some(a), Some(b)) if a != b => "R",
            _ => "M",
        }
    }
}

impl Log {
    fn new() -> Self {
        Log {
            commits: vec![], loaded_key: String::new(), graph: vec![], graph_width: 0, scope: Scope::Current,
            branches: (vec![], vec![]), not_in_head: HashSet::new(), search: String::new(), loading: false, exhausted: false,
            selected: None, message: String::new(), files: vec![], selected_path: None, collapsed: HashSet::new(),
            detail_loading: false, movable: HashSet::new(), root: None, rebasing: false,
            list: ListState::new(0, ListAlignment::Top, px(400.)), load: None, detail: None, generation: 0,
        }
    }
    fn selected_commit(&self) -> Option<&Commit> { self.selected.as_ref().and_then(|s| self.commits.iter().find(|c| &c.sha == s)) }
    fn selected_file(&self) -> Option<&Shown> { self.selected_path.as_ref().and_then(|p| self.files.iter().find(|f| f.path() == p)) }
    fn index_of(&self, sha: &str) -> Option<usize> { self.commits.iter().position(|c| c.sha == sha) }
    fn cancel_load(&mut self) { if let Some((c, _)) = self.load.take() { c.cancel() } self.loading = false }
    fn cancel_detail(&mut self) { if let Some((c, _)) = self.detail.take() { c.cancel() } self.detail_loading = false }
    /// The rebase section and drag and drop exist only on the current branch's unfiltered list.
    fn rewritable_view(&self) -> bool { self.scope == Scope::Current && self.search.is_empty() }
}

/// What a page load brings back.
struct Page {
    commits: Vec<Commit>,
    exhausted: bool,
    graph: (Vec<GraphRow>, usize),
    not_in_head: Option<HashSet<String>>,
    branches: Option<(Vec<String>, Vec<String>)>,
    movable: Option<(HashSet<String>, Option<String>)>,
}

/// The commit a row drag carries.
#[derive(Clone)]
pub(crate) struct DragCommit { sha: String, subject: String }

struct DragPreview(String);
impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().max_w(px(300.)).truncate().rounded(theme::RADIUS_ROW).bg(theme::raised()).border_1()
            .border_color(theme::border_strong()).text_size(px(12.)).text_color(theme::text()).child(self.0.clone())
    }
}

pub struct HistoryView {
    pub app: Entity<AppModel>,
    model: Option<(Entity<WorktreeModel>, Subscription)>,
    logs: HashMap<String, Log>,
    search: Entity<InputState>,
    diff: Option<Entity<DiffView>>,
    /// Full mode / Before / After: whole files of the commit showing, by "sha path".
    whole: HashMap<String, Arc<diff::FileDiff>>,
    whole_load: Option<(Cancel, Task<()>)>,
    whole_key: Option<String>,
    shown: bool,
    last_busy: Option<String>,
    /// Drag and drop: the row under the pointer, the zone and whether ⌥ makes it a fixup.
    drop_target: Option<(String, Zone, bool)>,
    avatars: Avatars,
    /// ShowCommit for a commit not loaded yet: selected when a load brings it.
    pending_select: Option<String>,
    file_panel: panel::FilePanel,
    /// HI-14: the commit's Overview instead of its files ("commitOverview", as Swift). Stashes have none.
    overview: bool,
    ov: Option<Entity<ChangeOverview>>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl HistoryView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let subs = vec![
            cx.observe_in(&app, window, |v, _, w, cx| v.sync(w, cx)),
            cx.subscribe_in(&app, window, |v, _, e: &AppEvent, w, cx| v.on_event(e, w, cx)),
            cx.subscribe_in(&search, window, |v, s, e: &InputEvent, w, cx| match e {
                InputEvent::PressEnter { .. } => { let t = s.read(cx).value().to_string(); v.apply_search(t, w, cx) }
                // Clearing the field shows everything again without Return (HI-11).
                InputEvent::Change if s.read(cx).value().is_empty() => v.apply_search(String::new(), w, cx),
                _ => {}
            }),
        ];
        let mut v = HistoryView {
            app, model: None, logs: HashMap::new(), search, diff: None, whole: HashMap::new(), whole_load: None, whole_key: None, shown: false,
            last_busy: None, drop_target: None, avatars: Avatars::load(), pending_select: None, file_panel: panel::FilePanel::new(),
            overview: loupe_forge::settings::shared().bool("commitOverview"), ov: None, focus: cx.focus_handle(), _subs: subs,
        };
        v.sync(window, cx);
        v
    }

    fn selected_model(&self) -> Option<Entity<WorktreeModel>> { self.model.as_ref().map(|m| m.0.clone()) }
    fn path(&self, cx: &App) -> Option<String> { self.selected_model().map(|m| m.read(cx).path_str()) }
    fn log(&self, cx: &App) -> Option<&Log> { self.path(cx).and_then(|p| self.logs.get(&p)) }
    fn log_mut(&mut self, cx: &App) -> Option<&mut Log> { let p = self.path(cx)?; self.logs.get_mut(&p) }

    // MARK: Following the app

    /// The app or the shown worktree changed: follow the selection, and (re)load when shown and HEAD or the refs moved.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (shown, m) = { let a = self.app.read(cx); (a.tab == Tab::History, a.selected_model()) };
        if m.as_ref().map(|m| m.entity_id()) != self.model.as_ref().map(|x| x.0.entity_id()) {
            // Only the shown worktree keeps its commit's files (Swift's releaseHeavyState).
            if let Some(l) = self.log_mut(cx) {
                if l.load.is_some() { l.loaded_key.clear() }
                l.cancel_load(); l.cancel_detail(); l.files.clear()
            }
            self.cancel_whole();
            self.last_busy = m.as_ref().and_then(|m| m.read(cx).state.busy.clone());
            self.model = m.map(|m| (m.clone(), cx.observe_in(&m, window, |v, _, w, cx| v.model_changed(w, cx))));
            self.drop_target = None;
            let (q, stashes) = self.log(cx).map(|l| (l.search.clone(), l.scope == Scope::Stashes)).unwrap_or_default();
            self.search.update(cx, |i, cx| {
                if i.value().as_ref() != q { i.set_value(q, window, cx) }
                i.set_placeholder(if stashes { "Search stashes" } else { "Search" }, window, cx);
            });
            self.shown = false; // a new worktree counts as coming back to the tab
        }
        let appeared = shown && !self.shown;
        self.shown = shown;
        if !shown {
            // Leaving History: what it was loading isn't wanted any more.
            if let Some(l) = self.log_mut(cx) {
                if l.load.is_some() { l.loaded_key.clear() }
                l.cancel_load();
                if l.detail.is_some() { l.files.clear() }
                l.cancel_detail();
            }
            self.cancel_whole();
            return;
        }
        self.reload_if_moved(appeared, window, cx);
        self.sync_diff(window, cx);
        cx.notify();
    }

    fn model_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.selected_model().and_then(|m| m.read(cx).state.busy.clone());
        let ended = if busy.is_none() { self.last_busy.take() } else { None };
        self.last_busy = busy;
        if !self.shown { return }
        // An undo or a stash from Changes changes the stash list without moving HEAD (Model.swift:488, Undo.swift).
        if matches!(ended.as_deref(), Some("Undoing" | "Stashing")) { self.reload(window, cx); return }
        self.reload_if_moved(false, window, cx);
        cx.notify();
    }

    fn key(&self, cx: &App) -> Option<String> {
        let m = self.selected_model()?;
        let s = &m.read(cx).state;
        // Until the first status refresh HEAD isn't known; loading now would only load again in a moment.
        (s.status_version > 0).then(|| format!("{} {}", s.head_oid, s.refs_version))
    }

    fn reload_if_moved(&mut self, appeared: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(path), Some(key)) = (self.path(cx), self.key(cx)) else { return };
        let l = self.logs.entry(path).or_insert_with(Log::new);
        let empty_again = appeared && l.commits.is_empty() && !l.loading;
        if key != l.loaded_key || empty_again {
            if l.loading && l.loaded_key == key { return }
            self.reload(window, cx);
        } else if appeared && l.files.is_empty() && !l.detail_loading && l.selected.is_some() {
            // Its files were let go while another worktree showed: load them again (fixes §14 History #1).
            let s = l.selected.clone();
            self.select(s, window, cx);
        }
    }

    fn on_event(&mut self, e: &AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mine = |w: &str, v: &Self, cx: &App| v.path(cx).as_deref() == Some(w);
        match e {
            AppEvent::Refresh if self.shown => self.reload(window, cx),
            AppEvent::ShowCommit { worktree, sha } if mine(worktree, self, cx) => {
                let found = self.log(cx).and_then(|l| l.index_of(sha));
                match found {
                    Some(i) => { self.select(Some(sha.clone()), window, cx); if let Some(l) = self.log(cx) { l.list.scroll_to_reveal_item(i) } }
                    None => self.pending_select = Some(sha.clone()),
                }
            }
            AppEvent::ShowBranch { worktree, branch } if mine(worktree, self, cx) => self.show_merged(branch.clone(), window, cx),
            AppEvent::ShowStashes { worktree } if mine(worktree, self, cx) => self.set_scope(Scope::Stashes, window, cx),
            _ => {}
        }
    }

    /// SH-39 / HI-21 / PD-30: `branch`'s log with its newest commit selected. After a PR merge the Pulls view has
    /// already fetched the base it went into (AppModel::show_merged), so this only shows it. A branch that doesn't
    /// exist falls back to Current (apply_page).
    fn show_merged(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.path(cx) else { return };
        let l = self.logs.entry(p).or_insert_with(Log::new);
        l.selected = None;
        l.files.clear();
        l.message.clear();
        l.scope = Scope::Branch(branch);
        l.commits.clear();
        l.list.reset(0);
        l.graph.clear();
        self.pending_select = None;
        self.reload(window, cx);
    }

    // MARK: Loading

    /// The first page (and the scope menu's branches, the commits HEAD lacks, the rewritable set) for the current
    /// scope and the applied search. Not keyed: ⌘R and actions use it directly.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(m), Some(path)) = (self.selected_model(), self.path(cx)) else { return };
        let key = self.key(cx).unwrap_or_default();
        let dir = m.read(cx).path().to_path_buf();
        let l = self.logs.entry(path.clone()).or_insert_with(Log::new);
        l.cancel_load();
        l.loading = true;
        l.exhausted = false;
        l.loaded_key = key.clone();
        l.generation += 1;
        let (scope, search, generation) = (l.scope.clone(), l.search.clone(), l.generation);
        let cancel = Cancel::new();
        let c = cancel.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { load_page(&dir, &scope, &search, 0, true, Some(&c)) }).await;
            this.update_in(cx, |v, w, cx| v.apply_page(&path, generation, r, true, w, cx)).ok();
        });
        l.load = Some((cancel, task));
        cx.notify();
    }

    /// The next page, when the last row shows (HI-06).
    fn load_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(m), Some(path)) = (self.selected_model(), self.path(cx)) else { return };
        let dir = m.read(cx).path().to_path_buf();
        let Some(l) = self.logs.get_mut(&path) else { return };
        if l.loading || l.exhausted || l.commits.is_empty() { return }
        l.loading = true;
        let (scope, search, generation, skip) = (l.scope.clone(), l.search.clone(), l.generation, l.commits.len());
        let loaded = l.commits.clone();
        let cancel = Cancel::new();
        let c = cancel.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                let mut p = load_page(&dir, &scope, &search, skip, false, Some(&c))?;
                // The graph is laid out again over everything loaded.
                let mut all = loaded;
                all.extend(p.commits.iter().cloned());
                p.graph = graph_for(&dir, &all, &scope, &search);
                Ok(p)
            }).await;
            this.update_in(cx, |v, w, cx| v.apply_page(&path, generation, r, false, w, cx)).ok();
        });
        l.load = Some((cancel, task));
        cx.notify();
    }

    fn apply_page(&mut self, path: &str, generation: u64, r: loupe_core::Result<Page>, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = self.logs.get_mut(path) else { return };
        if l.generation != generation { return } // a newer load replaced the list meanwhile
        l.load = None;
        l.loading = false;
        let p = match r {
            Ok(p) => p,
            Err(e) => {
                if replace { l.loaded_key.clear() }
                if let Some(m) = self.selected_model() { m.update(cx, |m, cx| m.set_error(e, cx)) }
                cx.notify();
                return;
            }
        };
        l.exhausted = p.exhausted;
        if replace {
            l.commits = p.commits;
            l.list.reset(l.commits.len());
        } else {
            let n = l.commits.len();
            l.list.splice(n..n, p.commits.len());
            l.commits.extend(p.commits);
        }
        (l.graph, l.graph_width) = p.graph;
        if let Some(n) = p.not_in_head { l.not_in_head = n }
        if let Some((set, root)) = p.movable { l.movable = set; l.root = root }
        let mut scope_gone = false;
        if let Some(b) = p.branches {
            if let Scope::Branch(s) = &l.scope && !b.0.contains(s) && !b.1.contains(s) { scope_gone = true }
            l.branches = b;
        }
        let emails: Vec<(String, String)> = l.commits.iter().map(|c| (c.email.clone(), c.sha.clone())).collect();
        if scope_gone {
            // The shown branch was deleted or pruned: back to the current branch.
            l.scope = Scope::Current;
            return self.reload(window, cx);
        }
        if replace {
            let pending = self.pending_select.take();
            let l = self.logs.get_mut(path).unwrap();
            let want = pending.filter(|s| l.index_of(s).is_some()).or(l.selected.clone()).filter(|s| l.index_of(s).is_some());
            match want {
                Some(s) => {
                    let i = l.index_of(&s).unwrap_or(0);
                    l.list.scroll_to_reveal_item(i);
                    if Some(&s) != l.selected.as_ref() || (l.files.is_empty() && !l.detail_loading) { self.select(Some(s), window, cx) }
                }
                None => { let first = l.commits.first().map(|c| c.sha.clone()); self.select(first, window, cx) }
            }
        }
        self.request_avatars(emails, cx);
        cx.notify();
    }

    fn apply_search(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path(cx) else { return };
        let l = self.logs.entry(path).or_insert_with(Log::new);
        if l.search == text { return }
        l.search = text;
        self.reload(window, cx);
    }

    fn set_scope(&mut self, s: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path(cx) else { return };
        let l = self.logs.entry(path).or_insert_with(Log::new);
        if l.scope == s { return }
        l.scope = s;
        l.commits.clear();
        l.graph.clear();
        l.graph_width = 0;
        l.list.reset(0);
        l.selected = None;
        l.files.clear();
        l.message.clear();
        let ph = if l.scope == Scope::Stashes { "Search stashes" } else { "Search" };
        self.search.update(cx, |i, cx| i.set_placeholder(ph, window, cx));
        self.reload(window, cx);
    }

    // MARK: Selection and the detail

    fn select(&mut self, sha: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(m), Some(path)) = (self.selected_model(), self.path(cx)) else { return };
        let dir = m.read(cx).path().to_path_buf();
        let Some(l) = self.logs.get_mut(&path) else { return };
        let changed = l.selected != sha;
        l.selected = sha.clone();
        l.cancel_detail();
        let Some(c) = sha.and_then(|s| l.commits.iter().find(|c| c.sha == s).cloned()) else {
            l.files.clear();
            l.message.clear();
            self.sync_diff(window, cx);
            cx.notify();
            return;
        };
        if changed { l.files.clear(); l.message.clear() }
        l.detail_loading = true;
        let cancel = Cancel::new();
        let x = cancel.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                let (msg, files) = core_history::detail(&c, &dir, Some(&x))?;
                Ok::<_, loupe_core::Error>((msg, files.into_iter().map(|f| Shown { view: Arc::new(adapt::file_diff(&f)), core: Arc::new(f) }).collect::<Vec<_>>()))
            }).await;
            this.update_in(cx, |v, w, cx| {
                let Some(l) = v.logs.get_mut(&path) else { return };
                l.detail = None;
                l.detail_loading = false;
                match r {
                    Ok((msg, files)) => {
                        l.message = msg;
                        l.files = files;
                        // The file shown stays when the next commit has it too.
                        if !l.files.iter().any(|f| Some(f.path()) == l.selected_path.as_deref()) { l.selected_path = l.files.first().map(|f| f.path().to_string()) }
                    }
                    Err(e) => if let Some(m) = v.selected_model() { m.update(cx, |m, cx| m.set_error(e, cx)) },
                }
                v.sync_diff(w, cx);
                cx.notify();
            }).ok();
        });
        l.detail = Some((cancel, task));
        self.sync_diff(window, cx);
        cx.notify();
    }

    /// ↑/↓ and ⌥↑/⌥↓: the neighbouring commit, clamped; nothing selected → the first (HI-07).
    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = self.log(cx) else { return };
        if l.commits.is_empty() { return }
        let i = match l.selected.as_ref().and_then(|s| l.index_of(s)) {
            Some(i) => (i as isize + delta).clamp(0, l.commits.len() as isize - 1) as usize,
            None => 0,
        };
        let sha = l.commits[i].sha.clone();
        l.list.scroll_to_reveal_item(i);
        self.select(Some(sha), window, cx);
    }

    fn select_file(&mut self, p: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.log_mut(cx) { l.selected_path = Some(p) }
        self.sync_diff(window, cx);
        cx.notify();
    }

    fn cancel_whole(&mut self) { if let Some((c, _)) = self.whole_load.take() { c.cancel() } self.file_panel.cancel() }

    /// The diff pane follows the selected file, loading the whole file for Full mode and Before / After; the toolbar
    /// offers Compact/Full while a file shows.
    fn sync_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (whole_mode, full, side, layout) = { let a = self.app.read(cx); (a.whole_files(), a.full_mode, a.diff_side, a.layout) };
        let sel = self.log(cx).and_then(|l| Some((l.selected_commit()?.clone(), l.selected_file()?.clone())));
        let overview = self.update_overview(cx);
        self.set_controls(sel.is_some() && !overview, cx);
        let Some((c, f)) = sel else { self.cancel_whole(); return };
        let key = format!("{} {}", c.sha, f.path());
        // Only the commit showing keeps whole files.
        self.whole.retain(|k, _| k.starts_with(&format!("{} ", c.sha)));
        let file = match self.whole.get(&key) {
            Some(w) if whole_mode => w.clone(),
            _ => {
                // Added, deleted and binary files are whole already.
                if whole_mode && f.core.old_path.is_some() && f.core.new_path.is_some() && !f.core.is_binary { self.load_whole(c.clone(), f.clone(), key, window, cx) }
                f.view.clone()
            }
        };
        self.sync_panel(&c, &f, window, cx);
        let file = self.panel_picked_diff().unwrap_or(file);
        let blame = self.panel_blame(&c, &f);
        let images = self.lfs_images(&f, cx);
        match &self.diff {
            Some(d) => d.update(cx, |d, cx| {
                d.set_file(file, cx);
                d.set_blame(blame, cx);
                d.set_side(side, cx);
                d.set_layout(layout, cx);
                d.set_change_ruler(full, cx);
                d.set_images(images.0, images.1, cx)
            }),
            None => {
                let app = self.app.clone();
                let me = cx.entity().downgrade();
                self.diff = Some(cx.new(|cx| {
                    let mut d = DiffView::new(file, cx);
                    d.set_blame(blame, cx);
                    let me = me.clone();
                    d.header_actions(move |_, cx| HistoryView::header_buttons(&me, cx));
                    d.set_highlighter(Some(adapt::highlighter()), cx);
                    d.set_side(side, cx);
                    d.set_layout(layout, cx);
                    d.set_change_ruler(full, cx);
                    d.set_images(images.0, images.1, cx);
                    d.on_side_change(move |side, layout, _, cx| app.update(cx, |a, cx| a.set_diff_side(side, layout, cx)));
                    d
                }));
            }
        }
    }

    /// The toolbar: Compact / Full while a file shows, Overview / Files for a commit (not a stash).
    fn set_controls(&self, file: bool, cx: &mut Context<Self>) {
        let commit = self.log(cx).and_then(|l| l.selected_commit()).is_some_and(|c| c.selector.is_none());
        let me = cx.entity().downgrade();
        let set: Rc<dyn Fn(ViewMode, &mut Window, &mut App)> = Rc::new(move |m, w, cx| { me.update(cx, |v, cx| v.set_overview(m == ViewMode::Overview, w, cx)).ok(); });
        let view = commit.then(|| (vec![ViewMode::Overview, ViewMode::Files], if self.overview { ViewMode::Overview } else { ViewMode::Files }, set));
        self.app.update(cx, |a, cx| a.set_controls(Tab::History, DiffControls { full: file, view }, cx));
    }

    fn set_overview(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.overview == on { return }
        self.overview = on;
        loupe_forge::settings::shared().set_bool("commitOverview", on);
        self.sync_diff(window, cx);
        cx.notify();
    }

    /// HI-14: the commit's ChangeOverview (first parent → commit), kept while it shows. Returns whether it shows.
    fn update_overview(&mut self, cx: &mut Context<Self>) -> bool {
        let input = self.selected_model().zip(self.log(cx)).and_then(|(m, l)| {
            let c = l.selected_commit().filter(|c| c.selector.is_none() && self.overview)?;
            let changes = l.files.iter().map(|f| ChangedPath { old: f.core.old_path.clone(), new: f.core.new_path.clone() }).collect();
            Some(code_overview::commit_input(&m.read(cx).path_str(), &c.sha, c.parents.first().map(String::as_str), changes))
        });
        let Some(input) = input else { self.ov = None; return false };
        match &self.ov {
            Some(o) => o.update(cx, |o, cx| o.set_input(input, cx)),
            None => {
                let me = cx.entity().downgrade();
                let on_open: code_overview::OnOpen = Rc::new(move |p, line, w, cx| {
                    me.update(cx, |v, cx| {
                        v.overview = false;
                        loupe_forge::settings::shared().set_bool("commitOverview", false);
                        v.select_file(p, w, cx);
                        if let (Some(l), Some(d)) = (line, &v.diff) { d.update(cx, |d, cx| d.reveal(l as u32, cx)) }
                    }).ok();
                });
                self.ov = Some(cx.new(|cx| ChangeOverview::new(input, on_open, cx)));
            }
        }
        true
    }

    fn load_whole(&mut self, c: Commit, f: Shown, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.whole_load.is_some() && self.whole_key.as_deref() == Some(key.as_str()) { return }
        let Some(m) = self.selected_model() else { return };
        self.cancel_whole();
        let dir = m.read(cx).path().to_path_buf();
        let cancel = Cancel::new();
        let x = cancel.clone();
        self.whole_key = Some(key.clone());
        let task = cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { core_history::whole_file(&f.core, &c, &dir, Some(&x)).map(|w| Arc::new(adapt::file_diff(&w))) }).await;
            this.update_in(cx, |v, w, cx| {
                v.whole_load = None;
                v.whole_key = None;
                let Some(r) = r else { return };
                v.whole.insert(key, r);
                v.sync_diff(w, cx);
            }).ok();
        });
        self.whole_load = Some((cancel, task));
    }

    /// An LFS pointer's two sides, when the objects are in the store (images preview there).
    fn lfs_images(&self, f: &Shown, cx: &App) -> (Option<PathBuf>, Option<PathBuf>) {
        let Some(d) = lfs::LfsDiff::from_diff(&f.core) else { return (None, None) };
        let common = self.selected_model().and_then(|m| m.read(cx).state.lfs.common_dir.clone());
        let obj = |p: &Option<lfs::LfsPointer>| p.as_ref().and_then(|p| lfs::object_file(p, common.as_deref(), None));
        (obj(&d.old), obj(&d.new))
    }

    // MARK: Actions (HI-15 to HI-20)

    fn branch_name(&self, cx: &App) -> String { self.selected_model().and_then(|m| m.read(cx).state.branch.clone()).unwrap_or_else(|| "HEAD".into()) }

    /// Runs a commit action through the worktree model (busy, error, undo toast, refresh). A stop on conflicts sends
    /// you to Changes, where the banner offers Continue / Abort.
    fn act(&mut self, label: &str, reload: bool, op: impl FnOnce(&std::path::Path) -> loupe_core::Result<bool> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let t = m.update(cx, |m, cx| m.perform(label, op, cx));
        let app = self.app.clone();
        cx.spawn_in(window, async move |this, cx| {
            let stopped = t.await;
            if stopped == Some(true) { app.update(cx, |a, cx| a.set_tab(Tab::Changes, cx)); }
            // HEAD moving reloads by itself; stash changes don't move it.
            if reload { this.update_in(cx, |v, w, cx| v.reload(w, cx)).ok(); }
        }).detach();
    }

    fn cherry_pick(&mut self, c: Commit, w: &mut Window, cx: &mut Context<Self>) { self.act("Cherry-picking", false, move |p| core_history::cherry_pick(p, &c), w, cx) }
    fn revert(&mut self, c: Commit, w: &mut Window, cx: &mut Context<Self>) { self.act("Reverting", false, move |p| core_history::revert(p, &c), w, cx) }
    fn apply_stash(&mut self, c: Commit, pop: bool, w: &mut Window, cx: &mut Context<Self>) {
        self.stash_op(c, if pop { StashOp::Pop } else { StashOp::Apply }, w, cx)
    }

    /// SS-06..10 through Changes' `stash_op`, which records the undo. A stop on conflicts sends you to Changes.
    fn stash_op(&mut self, c: Commit, op: StashOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let sel = c.selector.clone().unwrap_or_else(|| "stash@{0}".into());
        let t = m.update(cx, |m, cx| m.stash_op(op, sel, c.sha.clone(), c.subject.clone(), cx));
        let app = self.app.clone();
        cx.spawn_in(window, async move |this, cx| {
            if t.await == Some(true) { app.update(cx, |a, cx| a.set_tab(Tab::Changes, cx)); }
            // The stash list changed without HEAD moving.
            this.update_in(cx, |v, w, cx| v.reload(w, cx)).ok();
        }).detach();
    }

    fn confirm_drop_stash(&mut self, c: Commit, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let (q, detail) = stash::drop_question(c.selector.as_deref().unwrap_or("stash"));
        Dialog::confirm(&self.app, q, detail, "Drop Stash",
            move |w, cx| { let c = c.clone(); me.update(cx, |v, cx| v.stash_op(c, StashOp::Drop, w, cx)).ok(); }, window, cx);
    }

    fn new_worktree(&mut self, c: &Commit, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let (repo, base) = (m.read(cx).repo.clone(), c.sha.clone());
        self.app.update(cx, |a, cx| a.emit(AppEvent::NewWorktree { repo, base }, cx));
    }

    fn open_in_browser(&self, c: &Commit, cx: &mut App) {
        let Some(m) = self.selected_model() else { return };
        let (remote, wt) = { let m = m.read(cx); (m.remote.clone(), m.path_str()) };
        let Some(r) = remote else { return };
        let url = self.app.read(cx).account_for(&r, &wt).map(|a| format!("{}/{}/{}/commit/{}", a.base_url.trim_end_matches('/'), r.owner, r.name, c.sha));
        if let Some(u) = url { cx.open_url(&u) }
    }

    // MARK: Undo (UN-01, UN-04)

    /// ⌘Z with History focused: the shell's undo, plus Swift's "Can't Undo" alert, whose "Revert Commit" makes a new
    /// commit taking the pushed one back. That revert is recorded for undo like any revert (fixes §14 History #6).
    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        if m.read(cx).is_busy() { return }
        let t = m.update(cx, |m, cx| m.undo_offering_revert(cx));
        let (app, me) = (self.app.clone(), cx.entity().downgrade());
        cx.spawn_in(window, async move |_, cx| {
            let Some(refusal) = t.await else { return };
            cx.update(|w, cx| {
                let buttons: Vec<(String, Role, Option<Rc<dyn Fn(&mut Window, &mut App)>>)> = match refusal.revert.clone() {
                    Some(sha) => vec![("Cancel".into(), Role::Cancel, None), ("Revert Commit".into(), Role::Default, Some(Rc::new(move |w: &mut Window, cx: &mut App| {
                        let sha = sha.clone();
                        me.update(cx, |v, cx| v.act("Reverting", false, move |p| revert_recorded(p, &sha), w, cx)).ok();
                    })))],
                    None => vec![("OK".into(), Role::Default, None)],
                };
                Dialog::present(&app, "Can't Undo", refusal.message, buttons, w, cx);
            }).ok();
        }).detach();
    }

    // MARK: Avatars (HI-03)

    fn request_avatars(&mut self, emails: Vec<(String, String)>, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let (remote, wt) = { let m = m.read(cx); (m.remote.clone(), m.path_str()) };
        let Some(remote) = remote else { return };
        let Some(client) = self.app.read(cx).client_for(&remote, &wt) else { return };
        for (email, sha) in emails {
            let key = email.to_lowercase();
            if self.avatars.urls.contains_key(&key) || !self.avatars.pending.insert(key.clone()) { continue }
            self.avatars.queue.push_back((key, sha, client.clone(), remote.clone()));
        }
        self.drain_avatars(cx);
    }

    /// One request at a time: a page can show dozens of authors at once. The URLs are saved once per batch.
    fn drain_avatars(&mut self, cx: &mut Context<Self>) {
        if self.avatars.running { return }
        let Some((email, sha, client, remote)) = self.avatars.queue.pop_front() else {
            if self.avatars.dirty { self.avatars.dirty = false; self.avatars.save() }
            return;
        };
        self.avatars.running = true;
        cx.spawn(async move |this, cx| {
            let (url, image) = cx.background_executor().spawn(async move {
                let url = client.commit_author(&remote, &sha).ok().flatten().and_then(|u| u.avatar_url).unwrap_or_default();
                let image = (!url.is_empty()).then(|| loupe_forge::avatar::load(&url)).flatten().map(|b| image_of(&b));
                (url, image)
            }).await;
            this.update(cx, |v, cx| {
                v.avatars.running = false;
                v.avatars.pending.remove(&email);
                if let Some(i) = image { v.avatars.images.insert(url.clone(), i); }
                v.avatars.urls.insert(email, url);
                v.avatars.dirty = true;
                v.drain_avatars(cx);
                cx.notify();
            }).ok();
        }).detach();
    }

    /// Avatar images of URLs resolved in an earlier launch: fetched once, in the background.
    fn ensure_images(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.log(cx) else { return };
        let want: Vec<String> = l.commits.iter().filter_map(|c| self.avatars.url(&c.email).map(String::from))
            .filter(|u| !self.avatars.images.contains_key(u) && !self.avatars.fetching.contains(u)).collect::<HashSet<_>>().into_iter().collect();
        for url in want {
            self.avatars.fetching.insert(url.clone());
            cx.spawn(async move |this, cx| {
                let u = url.clone();
                let image = cx.background_executor().spawn(async move { loupe_forge::avatar::load(&u).map(|b| image_of(&b)) }).await;
                this.update(cx, |v, cx| if let Some(i) = image { v.avatars.images.insert(url, i); cx.notify() }).ok();
            }).detach();
        }
    }
}

/// `git revert` of a commit the undo refusal named, recorded for undo like History's Revert.
fn revert_recorded(dir: &std::path::Path, sha: &str) -> loupe_core::Result<bool> {
    let parents = git::trimmed(dir, ["rev-list", "--parents", "-n", "1", sha])?;
    let mut ids = parents.split(' ');
    let sha = ids.next().unwrap_or(sha).to_string();
    let c = Commit { sha, parents: ids.map(String::from).collect(), author: String::new(), email: String::new(), date: 0, refs: vec![], subject: String::new(), selector: None };
    core_history::revert(dir, &c)
}

/// A page off the main thread: the commits, the graph, and on the first page the branches, the commits HEAD lacks
/// and (current scope) what a rebase can reach.
fn load_page(dir: &std::path::Path, scope: &Scope, search: &str, skip: usize, first: bool, cancel: Option<&Cancel>) -> loupe_core::Result<Page> {
    let (commits, exhausted) = core_history::fetch_page(dir, scope, search, skip, cancel)?;
    let graph = if first { graph_for(dir, &commits, scope, search) } else { (vec![], 0) };
    if !first { return Ok(Page { commits, exhausted, graph, not_in_head: None, branches: None, movable: None }) }
    // ponytail: the commits HEAD lacks are read once per reload (Swift reads them again every page; the set is the same).
    let not_in_head = Some(core_history::not_in_head(dir, scope)?);
    let branches = Some(core_history::branch_names(dir));
    let movable = (*scope == Scope::Current).then(|| movable_commits(dir));
    Ok(Page { commits, exhausted, graph, not_in_head, branches, movable })
}

fn graph_for(dir: &std::path::Path, commits: &[Commit], scope: &Scope, search: &str) -> (Vec<GraphRow>, usize) {
    // The main line is HEAD's, or the shown branch's own when HEAD isn't in it.
    let head = if *scope == Scope::Current || *scope == Scope::Stashes { None } else { git::trimmed(dir, ["rev-parse", "HEAD"]).ok() };
    core_history::graph_for(commits, scope, head.as_deref(), !search.is_empty())
}

/// rebase::movable_commits, plus the root commit when the first-parent chain reaches it without a merge: the commit
/// above it can still meld into it (`--root`).
fn movable_commits(dir: &std::path::Path) -> (HashSet<String>, Option<String>) {
    let out = git::string(dir, ["rev-list", "--first-parent", "--parents", "-n", &loupe_core::rebase::LIMIT.to_string(), "HEAD", "--"]).unwrap_or_default();
    let mut set = HashSet::new();
    for line in out.lines() {
        let ids: Vec<&str> = line.split(' ').collect();
        if ids.len() == 1 { return (set, Some(ids[0].to_string())) }
        if ids.len() != 2 { break }
        set.insert(ids[0].to_string());
    }
    (set, None)
}

fn image_of(bytes: &[u8]) -> Arc<Image> {
    let format = if bytes.starts_with(&[0xff, 0xd8]) { ImageFormat::Jpeg } else if bytes.starts_with(b"GIF") { ImageFormat::Gif } else if bytes.starts_with(b"RIFF") { ImageFormat::Webp } else { ImageFormat::Png };
    Arc::new(Image::from_bytes(format, bytes.to_vec()))
}

/// Author email → forge avatar (History.swift's AuthorAvatars): resolved once per email through one of its commits and
/// remembered across launches in "authorAvatars" ("" = no account: initials).
struct Avatars {
    urls: HashMap<String, String>,
    images: HashMap<String, Arc<Image>>,
    pending: HashSet<String>,
    fetching: HashSet<String>,
    queue: VecDeque<(String, String, loupe_forge::Client, loupe_forge::RemoteRepo)>,
    running: bool,
    dirty: bool,
}

impl Avatars {
    fn load() -> Self {
        Avatars { urls: loupe_forge::settings::shared().string_map("authorAvatars"), images: HashMap::new(), pending: HashSet::new(),
                  fetching: HashSet::new(), queue: VecDeque::new(), running: false, dirty: false }
    }
    fn save(&self) { loupe_forge::settings::shared().set_string_map("authorAvatars", &self.urls) }
    fn url(&self, email: &str) -> Option<&str> { self.urls.get(&email.to_lowercase()).map(String::as_str).filter(|u| !u.is_empty()) }
}

// MARK: - The undo refusal, from the history side

impl WorktreeModel {
    /// `undo`, but a refusal comes back to the caller (for the "Can't Undo" alert) instead of the "Git" alert.
    pub fn undo_offering_revert(&mut self, cx: &mut Context<Self>) -> Task<Option<loupe_core::undo::UndoRefusal>> {
        let t = self.perform("Undoing", |p| match loupe_core::model::undo_last(p) {
            Err(loupe_core::Error::Refused(r)) => Ok::<_, loupe_core::Error>(Err(r)),
            other => other.map(Ok),
        }, cx);
        cx.spawn(async move |this, cx| {
            match t.await? {
                Err(r) => Some(r),
                Ok(undone) => {
                    let undone = undone?;
                    this.update(cx, |m, cx| if let Some((msg, amend)) = undone.message && m.state.commit_message.is_empty() {
                        m.state.commit_message = msg;
                        m.state.amend = amend;
                        cx.notify();
                    }).ok();
                    None
                }
            }
        })
    }
}

// MARK: - Rendering

impl HistoryView {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(l) = self.log(cx) else { return div().into_any_element() };
        let title = l.scope.title().to_string();
        let label = if title.chars().count() > 18 { format!("{}…", title.chars().take(17).collect::<String>()) } else { title.clone() };
        let icon = match l.scope { Scope::Stashes => "inbox", Scope::All => "git-fork", _ => "git-branch" };
        let (locals, remotes) = l.branches.clone();
        let me = cx.entity();
        div().flex().items_center().gap_1p5().p_2().flex_none()
            .child(div().id("history-search").flex_1().h(px(28.)).px_2().flex().items_center().gap_1p5().rounded(px(7.)).bg(theme::raised())
                .border_1().border_color(theme::border())
                .tooltip(|w, cx| Tooltip::new("Commit message or author").build(w, cx))
                .child(ui::icon("search").size(px(13.)).text_color(theme::text_faint()))
                .child(div().flex_1().child(Input::new(&self.search).appearance(false).small().cleanable(true))))
            .child(Button::new("history-scope").ghost().small().label(label).child(ui::icon(icon).size(px(13.)))
                .dropdown_caret(true).selected(false).tooltip(format!("Which history to show: {title}"))
                .dropdown_menu(move |menu, window, cx| {
                    let pick = |s: Scope| { let me = me.clone(); move |_: &gpui_kit::ClickEvent, w: &mut Window, cx: &mut App| me.update(cx, |v, cx| v.set_scope(s.clone(), w, cx)) };
                    let mut menu = menu.item(PopupMenuItem::new("Current Branch").on_click(pick(Scope::Current)))
                        .item(PopupMenuItem::new("All Branches").on_click(pick(Scope::All)));
                    for (title, bs) in [("Local Branches", locals.clone()), ("Remote Branches", remotes.clone())] {
                        if bs.is_empty() { continue }
                        let me = me.clone();
                        menu = menu.submenu(title, window, cx, move |mut m, _, _| {
                            for b in &bs {
                                let (me, s) = (me.clone(), Scope::Branch(b.clone()));
                                m = m.item(PopupMenuItem::new(b.clone()).on_click(move |_, w, cx| me.update(cx, |v, cx| v.set_scope(s.clone(), w, cx))));
                            }
                            m.scrollable(true)
                        });
                    }
                    menu.separator().item(PopupMenuItem::new("Stashes").on_click(pick(Scope::Stashes)))
                }))
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(l) = self.log(cx) else { return div().flex_1().into_any_element() };
        if l.commits.is_empty() {
            // The first load shows a spinner, not a blank list (fixes §14 History #12).
            if l.loading { return div().flex_1().flex().items_center().justify_center().child(gpui_kit::component::spinner::Spinner::new().small()).into_any_element() }
            let (icon, title, msg) = if !l.search.is_empty() { ("search", "No Matches", "") }
                else if l.scope == Scope::Stashes { ("inbox", "No Stashes", "Stash changes from the Changes tab to set them aside.") }
                else { ("rotate-ccw-clock", "No Commits", "") };
            return div().flex_1().child(ui::empty_state(icon, title, msg)).into_any_element();
        }
        let this = cx.entity();
        let count = l.commits.len();
        let more = l.loading;
        div().flex_1().min_h_0().flex().flex_col()
            .child(list(l.list.clone(), move |ix, window, cx| {
                // The last row showing asks for the next page (Swift: the last row's onAppear).
                if ix + 1 == count { let t = this.clone(); window.defer(cx, move |w, cx| t.update(cx, |v, cx| v.load_more(w, cx))) }
                this.read(cx).render_row(ix, &this, cx)
            }).flex_1().px_1p5())
            .when(more, |d| d.child(div().h(px(26.)).flex_none().flex().items_center().justify_center().child(gpui_kit::component::spinner::Spinner::new().small())))
            .into_any_element()
    }

    fn render_row(&self, ix: usize, this: &Entity<Self>, cx: &App) -> AnyElement {
        let Some(m) = self.selected_model() else { return div().into_any_element() };
        let Some(l) = self.log(cx) else { return div().into_any_element() };
        let Some(c) = l.commits.get(ix).cloned() else { return div().into_any_element() };
        let is_head = c.sha == m.read(cx).state.head_oid;
        let selected = l.selected.as_deref() == Some(&c.sha);
        let graph = l.graph.get(ix).cloned().filter(|_| l.graph_width > 0);
        let movable = l.rewritable_view() && l.movable.contains(&c.sha) && c.parents.len() < 2;
        let target = self.drop_target.as_ref().filter(|t| t.0 == c.sha && cx.has_active_drag()).cloned();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let tip = format!("{}\n{} <{}>\n{}", c.subject, c.author, c.email, sheets::abbreviated(c.date));
        let (t1, t2, sha) = (this.clone(), this.clone(), c.sha.clone());
        let menu_commit = c.clone();
        let row = div().id(SharedString::from(format!("commit-{}", c.sha))).relative().flex().w_full().gap(px(9.)).pl(px(6.)).pr_2()
            .rounded(theme::RADIUS_ROW).cursor_pointer()
            .when(selected, |d| d.bg(theme::selected()))
            .when(!selected, |d| d.hover(|s| s.bg(theme::hover())))
            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
            .on_click(move |_, w, cx| t1.update(cx, |v, cx| { v.focus.focus(w, cx); v.select(Some(sha.clone()), w, cx) }))
            .when_some(graph, |d, g| d.child(graph::cell(g, l.graph_width)))
            .child(div().pt(px(8.)).flex_none().child(self.avatar(&c.author, &c.email, 26.)))
            .child(div().flex().flex_col().flex_1().min_w_0().py(px(6.)).gap(px(3.))
                .child(div().flex().items_center().gap_1()
                    .when(is_head, |d| d.child(div().id("head-dot").size(px(6.)).flex_none().rounded_full().bg(theme::accent())
                        .tooltip(|w, cx| Tooltip::new("HEAD").build(w, cx))))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::text())
                        .when(is_head, |d| d.font_weight(FontWeight::SEMIBOLD)).child(c.subject.clone())))
                .child(div().flex().items_center().gap(px(6.)).text_size(px(11.))
                    .child(div().flex_none().font_family(theme::CODE_FONT).text_color(theme::text_muted()).child(c.short().to_string()))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::text_muted()).child(c.author.clone()))
                    .child(div().flex_none().text_color(theme::text_faint()).child(sheets::relative(c.date, now))))
                .when(!c.refs.is_empty(), |d| d.child(ref_badges(&c.sha, &c.refs, selected))))
            .when_some(target, |d, (_, zone, fixup)| d.child(drop_indicator(zone, fixup)));
        let row = if !movable { row } else {
            let (t3, t4, sha2) = (this.clone(), this.clone(), c.sha.clone());
            let drag = DragCommit { sha: c.sha.clone(), subject: c.subject.clone() };
            row.on_drag(drag, |d, _, _, cx| cx.new(|_| DragPreview(d.subject.clone())))
                .on_drag_move::<DragCommit>(move |e, _, cx| {
                    if !e.bounds.contains(&e.event.position) { return }
                    let h = f32::from(e.bounds.size.height);
                    let y = f32::from(e.event.position.y - e.bounds.top());
                    let zone = if y < h * 0.3 { Zone::Above } else if y > h * 0.7 { Zone::Below } else { Zone::Onto };
                    let fixup = e.event.modifiers.alt;
                    let dragged = e.drag(cx).sha.clone();
                    let target = sha2.clone();
                    t3.update(cx, |v, cx| {
                        let want = v.can_drop(&dragged, &target, cx).then(|| (target.clone(), zone, fixup));
                        if v.drop_target != want { v.drop_target = want; cx.notify() }
                    });
                })
                .on_drop::<DragCommit>(move |d, w, cx| {
                    let fixup = w.modifiers().alt;
                    t4.update(cx, |v, cx| {
                        let t = v.drop_target.take();
                        cx.notify();
                        if let Some((target, zone, _)) = t { v.drop_commit(d.sha.clone(), zone, target, fixup, w, cx) }
                    });
                })
        };
        row.context_menu(move |menu, w, cx| t2.update(cx, |v, cx| v.commit_menu(menu, &menu_commit, w, cx))).into_any_element()
    }

    fn avatar(&self, login: &str, email: &str, size: f32) -> AnyElement {
        if let Some(i) = self.avatars.url(email).and_then(|u| self.avatars.images.get(u)) {
            return gpui_kit::img(i.clone()).size(px(size)).rounded_full().flex_none().into_any_element();
        }
        // Swift's Avatar: the login's first letter on HSB(hue, 0.45, 0.75), i.e. HSL(hue, 0.40, 0.58).
        let initials: String = login.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let hue = loupe_forge::avatar::initials_hue(login);
        div().size(px(size)).flex_none().rounded_full().flex().items_center().justify_center()
            .bg(gpui_kit::hsla(hue, 0.40, 0.58, 1.)).text_size(px(size * 0.45)).font_weight(FontWeight::SEMIBOLD).text_color(gpui_kit::white())
            .child(if initials.is_empty() { "?".to_string() } else { initials })
            .into_any_element()
    }

    /// The commit context menu (HI-16), with the rebase section on the current branch (RB-02).
    fn commit_menu(&mut self, menu: PopupMenu, c: &Commit, _: &mut Window, cx: &mut Context<Self>) -> PopupMenu {
        let me = cx.entity();
        let item = |label: &str, f: Act| {
            let me = me.clone();
            PopupMenuItem::new(label.to_string()).on_click(move |_, w, cx| { let f = f.clone(); me.update(cx, |v, cx| f(v, w, cx)) })
        };
        let copy = |label: &str, text: String| PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
        let c1 = c.clone();
        if c.selector.is_some() {
            let (a, b, d) = (c.clone(), c.clone(), c.clone());
            return menu.item(item("Pop", Rc::new(move |v, w, cx| v.apply_stash(a.clone(), true, w, cx))))
                .item(item("Apply", Rc::new(move |v, w, cx| v.apply_stash(b.clone(), false, w, cx))))
                .item(item("Drop…", Rc::new(move |v, w, cx| v.confirm_drop_stash(d.clone(), w, cx))))
                .separator()
                .item(copy("Copy Message", c.subject.clone()));
        }
        let (not_in_head, rewritable) = match self.log(cx) { Some(l) => (l.not_in_head.contains(&c.sha), l.rewritable_view()), None => return menu };
        let mut menu = if not_in_head {
            menu.item(item(&format!("Cherry-pick onto {}", self.branch_name(cx)), Rc::new(move |v, w, cx| v.cherry_pick(c1.clone(), w, cx))))
        } else {
            menu.item(item("Revert Commit", Rc::new(move |v, w, cx| v.revert(c1.clone(), w, cx))))
        };
        menu = menu.separator();
        if rewritable { menu = self.rebase_menu(menu, c, cx).separator() }
        let c2 = c.clone();
        menu = menu.item(copy("Copy SHA", c.sha.clone()))
            .item(copy("Copy Subject", c.subject.clone()))
            .item(item("New Worktree from Here…", Rc::new(move |v, _, cx| v.new_worktree(&c2, cx))));
        let has_account = self.selected_model().is_some_and(|m| {
            let m = m.read(cx);
            m.remote.as_ref().is_some_and(|r| self.app.read(cx).account_for(r, &m.path_str()).is_some())
        });
        if has_account {
            let c3 = c.clone();
            menu = menu.item(item("Open in Browser", Rc::new(move |v, _, cx| v.open_in_browser(&c3, cx))));
        }
        menu
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(l) = self.log(cx) else { return div().size_full().into_any_element() };
        let Some(c) = l.selected_commit().cloned() else {
            return div().size_full().child(ui::empty_state("rotate-ccw-clock", "Select a Commit", "")).into_any_element();
        };
        let header = self.render_header(&c, cx);
        if let Some(o) = self.ov.clone().filter(|_| self.overview && c.selector.is_none()) {
            return div().flex().flex_col().size_full().child(header).child(div().flex_1().min_h_0().child(o)).into_any_element();
        }
        let tree = self.render_tree(cx);
        let l = self.log(cx).unwrap();
        let right = match (l.selected_file(), &self.diff) {
            (Some(f), Some(d)) => {
                let body = self.render_markdown(&c, f, cx).unwrap_or_else(|| d.clone().into_any_element());
                div().size_full().p_2().child(self.render_file_panel(body, cx)).into_any_element()
            }
            _ if l.detail_loading => div().size_full().flex().items_center().justify_center().child(gpui_kit::component::spinner::Spinner::new()).into_any_element(),
            _ => div().size_full().into_any_element(),
        };
        div().flex().flex_col().size_full()
            .child(header)
            .child(div().flex_1().min_h_0().child(ui::h_resizable("commit-split")
                .child(ui::resizable_panel().size(px(240.)).size_range(px(180.)..px(300.)).child(tree))
                .child(ui::resizable_panel().child(right))))
            .into_any_element()
    }

    fn render_header(&self, c: &Commit, cx: &mut Context<Self>) -> AnyElement {
        let actions = self.header_actions(c, cx);
        let Some(l) = self.log(cx) else { return div().into_any_element() };
        let body = l.message.split_once('\n').map(|(_, b)| b.trim().to_string()).unwrap_or_default();
        let (adds, dels) = l.files.iter().fold((0, 0), |(a, d), f| (a + f.core.additions, d + f.core.deletions));
        let n = l.files.len();
        let me = cx.entity();
        let sha = c.sha.clone();
        // A stash's parents (its base, index and untracked commits) are never in the list: no links (fixes §14 History #4).
        let parents: Vec<(String, bool)> = if c.selector.is_some() { vec![] } else { c.parents.iter().map(|p| (p.clone(), l.index_of(p).is_some())).collect() };
        let merge = c.parents.len() > 1;
        let loading = l.detail_loading;
        div().flex().flex_col().gap(px(6.)).px(px(14.)).py(px(10.)).flex_none().bg(theme::card_header()).border_b_1().border_color(theme::border())
            .child(div().flex().items_start().gap_3()
                .child(div().flex_1().min_w_0().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).line_clamp(3).child(c.subject.clone()))
                .child(div().flex().flex_none().gap_1p5().children(actions)))
            .when(!body.is_empty(), |d| d.child(div().id("commit-body").max_h(px(120.)).overflow_y_scroll().text_size(px(12.5))
                .child(TextView::markdown(SharedString::from(format!("body-{}", c.sha)), body))))
            .child(div().flex().items_center().gap(px(10.)).text_size(px(12.5))
                .child(self.avatar(&c.author, &c.email, 18.))
                .child(div().font_weight(FontWeight::MEDIUM).child(c.author.clone()))
                .child(div().text_color(theme::text_muted()).child(sheets::abbreviated(c.date)))
                .child(div().id("copy-sha").flex().items_center().gap_1().cursor_pointer().font_family(theme::CODE_FONT).text_color(theme::text_muted())
                    .hover(|s| s.text_color(theme::text()))
                    .tooltip(|w, cx| Tooltip::new("Copy full SHA").build(w, cx))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(sha.clone())))
                    .child(c.short().to_string()).child(ui::icon("copy").size(px(12.))))
                .when(!parents.is_empty(), |d| d.child(div().text_color(theme::text_faint()).child(if merge { "merge of" } else { "parent" })))
                .children(parents.into_iter().map(|(p, loaded)| {
                    let me = me.clone();
                    let tip = if loaded { "Go to parent" } else { "Parent not loaded yet — scroll the list to load more" };
                    let label = p[..p.len().min(8)].to_string();
                    div().id(SharedString::from(format!("parent-{p}"))).font_family(theme::CODE_FONT).text_color(theme::blue()).cursor_pointer()
                        .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                        .on_click(move |_, w, cx| if loaded {
                            let p = p.clone();
                            me.update(cx, |v, cx| {
                                if let Some(l) = v.log(cx) && let Some(i) = l.index_of(&p) { l.list.scroll_to_reveal_item(i) }
                                v.select(Some(p), w, cx)
                            })
                        })
                        .child(label)
                }))
                .child(div().flex_1())
                .child(div().text_color(theme::text_muted()).child(if n == 1 { "1 file".to_string() } else { format!("{n} files") }))
                .child(div().text_color(theme::green()).child(format!("+{adds}")))
                .child(div().text_color(theme::red()).child(format!("\u{2212}{dels}")))
                .when(loading, |d| d.child(gpui_kit::component::spinner::Spinner::new().small())))
            .into_any_element()
    }

    /// HI-15: Drop… / Apply / Pop for a stash; Cherry-pick for a commit HEAD lacks; otherwise Revert.
    fn header_actions(&self, c: &Commit, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let branch = self.branch_name(cx);
        let busy = self.selected_model().is_some_and(|m| m.read(cx).is_busy());
        let button = |id: &str, label: String, tip: Option<String>, primary: Option<gpui_kit::Hsla>, f: Act| {
            let b = match primary { Some(c) => ui::pill_primary(SharedString::from(id.to_string()), label, c), None => ui::pill(SharedString::from(id.to_string()), label) };
            b.h(px(24.)).when_some(tip, |d, t| d.tooltip(move |w, cx| Tooltip::new(t.clone()).build(w, cx)))
                .when(busy, |d| d.opacity(0.4))
                .when(!busy, |d| d.on_click(cx.listener(move |v, _, w, cx| f(v, w, cx))))
                .into_any_element()
        };
        let c1 = c.clone();
        if c.selector.is_some() {
            let (a, b) = (c.clone(), c.clone());
            return vec![
                button("stash-drop", "Drop…".into(), None, Some(theme::red()), Rc::new(move |v, w, cx| v.confirm_drop_stash(c1.clone(), w, cx))),
                button("stash-apply", "Apply".into(), Some("Apply the changes and keep the stash".into()), None, Rc::new(move |v, w, cx| v.apply_stash(a.clone(), false, w, cx))),
                button("stash-pop", "Pop".into(), Some("Apply the changes and drop the stash".into()), Some(theme::blue()), Rc::new(move |v, w, cx| v.apply_stash(b.clone(), true, w, cx))),
            ];
        }
        let not_in_head = self.log(cx).is_some_and(|l| l.not_in_head.contains(&c.sha));
        if not_in_head {
            vec![button("cherry-pick", format!("Cherry-pick onto {branch}"), Some(format!("Apply this commit on top of {branch}")), None, Rc::new(move |v, w, cx| v.cherry_pick(c1.clone(), w, cx)))]
        } else {
            // "Revert Commit" in the header too, as in the menu (§14 History #10).
            vec![button("revert", "Revert Commit".into(), Some(format!("Make a new commit on {branch} that undoes this one")), None, Rc::new(move |v, w, cx| v.revert(c1.clone(), w, cx)))]
        }
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(l) = self.log(cx) else { return div().into_any_element() };
        if l.files.is_empty() && !l.detail_loading { return div().size_full().child(ui::empty_state("file", "No File Changes", "")).into_any_element() }
        let paths: Vec<String> = l.files.iter().map(|f| f.path().to_string()).collect();
        let by_path: HashMap<&str, &Shown> = l.files.iter().map(|f| (f.path(), f)).collect();
        let rows = file_tree::rows(&paths, &l.collapsed);
        let me = cx.entity();
        let mut list = div().id("commit-tree").flex().flex_col().size_full().overflow_y_scroll().p_1p5().gap_px();
        for r in rows {
            let indent = px(8. + r.depth as f32 * 14.);
            let me = me.clone();
            if let Some(dir) = r.dir_path.clone() {
                let open = !l.collapsed.contains(&dir);
                list = list.child(ui::list_row(SharedString::from(r.id.clone()), false).h(px(24.)).pl(indent)
                    .on_click(move |_, _, cx| me.update(cx, |v, cx| {
                        // The folded folders stay folded from commit to commit.
                        if let Some(l) = v.log_mut(cx) && !l.collapsed.remove(&dir) { l.collapsed.insert(dir.clone()); }
                        cx.notify()
                    }))
                    .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(11.)).text_color(theme::text_faint()))
                    .child(ui::icon(if open { "folder-open" } else { "folder" }).size(px(13.)))
                    .child(div().flex_1().min_w_0().truncate().child(r.name.clone())));
                continue;
            }
            let Some(path) = r.file_path.clone() else { continue };
            let Some(f) = by_path.get(path.as_str()) else { continue };
            let selected = l.selected_path.as_deref() == Some(&path);
            let badge = f.badge();
            let color = match badge { "A" => theme::green(), "D" => theme::red(), "R" => theme::purple(), _ => theme::blue() };
            let (adds, dels, bin) = (f.core.additions, f.core.deletions, f.core.is_binary);
            list = list.child(ui::list_row(SharedString::from(format!("tf-{path}")), selected).h(px(24.)).pl(indent + px(15.))
                .child(ui::file_icon(f.path()))
                .child(div().flex_1().min_w_0().truncate().child(r.name.clone()))
                .child(div().flex().flex_none().gap_1().text_size(px(11.)).font_family(theme::CODE_FONT)
                    .when(bin, |d| d.child(div().text_color(theme::text_faint()).child("bin")))
                    .when(adds > 0, |d| d.child(div().text_color(theme::green()).child(format!("+{adds}"))))
                    .when(dels > 0, |d| d.child(div().text_color(theme::red()).child(format!("\u{2212}{dels}"))))
                    .child(div().w(px(10.)).text_color(color).child(badge)))
                .on_click(move |_, w, cx| { let p = path.clone(); me.update(cx, |v, cx| v.select_file(p, w, cx)) }));
        }
        list.into_any_element()
    }
}

fn ref_badges(sha: &str, refs: &[String], selected: bool) -> AnyElement {
    // ponytail: up to 3 badges then "+N" (Swift's ViewThatFits measures what fits); measure when columns get wide.
    const FIT: usize = 3;
    let shown = refs.len().min(FIT);
    let badge = |name: String, icon: &str, tint: gpui_kit::Hsla| {
        let (fg, bg) = if selected { (gpui_kit::white(), gpui_kit::white().opacity(0.2)) } else { (tint, tint.opacity(0.12)) };
        let tip = name.clone();
        div().id(SharedString::from(format!("ref-{sha}-{name}"))).flex().items_center().gap_1().px(px(5.)).py(px(1.)).max_w(px(160.)).flex_shrink_0()
            .rounded(theme::RADIUS_PILL).bg(bg).text_color(fg).text_size(px(10.5)).font_weight(FontWeight::MEDIUM)
            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
            .child(ui::icon(icon).size(px(10.)).text_color(fg))
            .child(div().min_w_0().truncate().child(name))
    };
    let parse = |r: &str| {
        let (name, kind) = core_history::parse_ref(r);
        let (icon, tint): (&str, gpui_kit::Hsla) = match kind {
            RefKind::Head => ("git-branch", theme::accent()),
            RefKind::Tag => ("tag", gpui_kit::rgb(0xff9500).into()),
            RefKind::Remote => ("cloud", theme::text_muted()),
            RefKind::Local => ("git-branch", theme::green()),
            RefKind::Other => ("circle-dot", theme::accent()),
        };
        (name, icon, tint)
    };
    let mut row = div().flex().items_center().gap_1().min_w_0().overflow_hidden();
    for r in &refs[..shown] { let (n, i, t) = parse(r); row = row.child(badge(n, i, t)) }
    if refs.len() > shown {
        let rest: Vec<String> = refs[shown..].iter().map(|r| core_history::parse_ref(r).0).collect();
        let tip = rest.join("\n");
        let (fg, bg) = if selected { (gpui_kit::white(), gpui_kit::white().opacity(0.2)) } else { (theme::text_muted(), theme::text_muted().opacity(0.12)) };
        row = row.child(div().id(SharedString::from(format!("refs-more-{sha}"))).flex_none().px(px(5.)).py(px(1.)).rounded(theme::RADIUS_PILL)
            .bg(bg).text_color(fg).text_size(px(10.5)).font_weight(FontWeight::MEDIUM)
            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
            .child(format!("+{}", rest.len())));
    }
    row.into_any_element()
}

fn drop_indicator(zone: Zone, fixup: bool) -> AnyElement {
    let line = || div().absolute().left_0().right_0().h(px(2.)).bg(theme::accent());
    match zone {
        Zone::Above => line().top_0().into_any_element(),
        Zone::Below => line().bottom_0().into_any_element(),
        Zone::Onto => div().absolute().inset_0().rounded(px(5.)).border_2().border_color(theme::accent()).bg(theme::accent().opacity(0.1))
            .flex().items_center().justify_end().pr(px(6.))
            .child(div().px(px(6.)).py(px(2.)).rounded(theme::RADIUS_PILL).bg(theme::accent()).text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD).text_color(gpui_kit::white()).child(if fixup { "Fixup" } else { "Squash" }))
            .into_any_element(),
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.selected_model().is_none() { return div().size_full().into_any_element() }
        self.ensure_images(cx);
        let toolbar = self.render_toolbar(cx);
        let list = self.render_list(cx);
        let detail = self.render_detail(cx);
        let left = div().flex().flex_col().size_full().bg(theme::canvas()).child(toolbar).child(ui::divider()).child(list);
        div().id("history").track_focus(&self.focus).key_context("History").size_full().flex()
            .on_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, w, cx| {
                if e.keystroke.modifiers.modified() { return }
                match e.keystroke.key.as_str() {
                    "down" => { v.move_selection(1, w, cx); cx.stop_propagation() }
                    "up" => { v.move_selection(-1, w, cx); cx.stop_propagation() }
                    _ => {}
                }
            }))
            .on_action(cx.listener(|v, _: &NextFile, w, cx| v.move_selection(1, w, cx)))
            .on_action(cx.listener(|v, _: &PreviousFile, w, cx| v.move_selection(-1, w, cx)))
            .on_action(cx.listener(|v, _: &Undo, w, cx| v.undo(w, cx)))
            .child(ui::h_resizable("history-split")
                .child(ui::resizable_panel().size(px(340.)).size_range(px(260.)..px(420.)).child(left))
                .child(ui::resizable_panel().child(detail)))
            .into_any_element()
    }
}
