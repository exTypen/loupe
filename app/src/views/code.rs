//! The code browser (CodeBrowser.swift, CodeIndex.swift): Full mode's file tree and file pane with tabs, go to
//! definition / find references with a results panel, back / forward through jumps and the outline. The change
//! overview and its lenses are in `code/overview.rs`. Owner: the Code view agent (docs/gpui/VIEWS.md).
//!
//! Hosts:
//! - Changes: `code::browser_for(&app, &model, window, cx)` (one per worktree, so tabs survive switching). In Full
//!   mode put `CodeBrowser::tree(&browser, cx)` in the left pane and the browser itself on the right. In Compact,
//!   route the DiffView's `on_navigate` to `navigate(word, action, from, cx)`, call `follow(path)` when the list
//!   selection moves, show `results_panel(&browser, cx)` under the diff, reveal the line on
//!   `CodeBrowserEvent::Reveal`, and show the browser instead of the diff while `peeking()` (a jump to a file the
//!   change doesn't touch; Swift flipped the global Full mode there, §14 #8). `end_peek` when a list file is picked.
//!   `open_from_overview(path, line)` is the Overview's `on_open`: it opens the line too (§14 #4).
//! - Pull requests (Full mode): `CodeBrowser::new_rev(app, model, merge_base, head, changed, window, cx)` after the
//!   head is fetched; `set_rev` when the head moves. `header_actions` adds Viewed etc. to the file header.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    div, prelude::FluentBuilder as _, px, uniform_list,
    component::menu::{ContextMenuExt as _, PopupMenuItem},
    component::tooltip::Tooltip,
};
use loupe_core::{FileChange, Kind, file_tree};

use crate::diff::{CodeAction, DiffView, FileDiff};
use crate::shell::keys::{CloseWindow, NavigateBack, NavigateForward};
use crate::state::{AppModel, Tab, WorktreeModel, adapt};
use crate::ui::tabs::{self, NavHit, NavResults, Outline, OutlineSymbol, TabEvent};
pub use crate::ui::tabs::{CodeLocation, CodeNav};
use crate::{theme, ui};

pub mod overview;
pub use overview::{ChangeOverview, OverviewInput};

/// What the browser shows: the worktree on disk, or a pull request's head (changed files diffed from the merge base).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Worktree,
    Rev { base: String, head: String, changed: Vec<RevFile> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RevFile { pub path: String, pub old_path: Option<String>, pub kind: Kind }

#[derive(Clone, Debug, PartialEq)]
pub enum CodeBrowserEvent {
    /// Compact mode jumped to a changed file the change list now selects: reveal `line` in the host's diff.
    Reveal { path: String, line: usize },
}

pub struct CodeBrowser {
    app: Entity<AppModel>,
    model: Entity<WorktreeModel>,
    source: Source,
    pub nav: CodeNav,
    results: Option<NavResults>,
    busy: Option<String>,
    lookup: Option<(Arc<AtomicBool>, Task<()>)>,
    tree: Option<Vec<String>>, // None: loading
    tree_key: String,
    tree_task: Option<Task<()>>,
    collapsed: HashSet<String>,
    collapse_applied: bool,
    files: HashMap<String, Arc<FileDiff>>,
    files_version: u64,
    loading: Option<(String, Task<()>)>,
    diff: Option<Entity<DiffView>>,
    reveal: Option<usize>,
    outline: (String, Outline), // for (path, uid)
    outline_task: Option<(Arc<AtomicBool>, Task<()>)>,
    peek: bool,
    header_actions: Option<Rc<dyn Fn(&str, &mut Window, &mut App) -> AnyElement>>,
    /// LF-04: (path, old, new) thumbnails of an LFS image the host resolved, shown while that file is open.
    images: (String, Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    _subs: Vec<Subscription>,
}

impl EventEmitter<CodeBrowserEvent> for CodeBrowser {}

thread_local! {
    static BROWSERS: std::cell::RefCell<HashMap<gpui_kit::EntityId, gpui_kit::WeakEntity<CodeBrowser>>> = Default::default();
}

/// The worktree's browser, made once: its tabs, history and folding stay while you're elsewhere (heavy state is
/// released when the worktree is deselected). The host holds the strong handle.
pub fn browser_for<V: 'static>(app: &Entity<AppModel>, model: &Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<V>) -> Entity<CodeBrowser> {
    let id = model.entity_id();
    if let Some(b) = BROWSERS.with(|m| m.borrow().get(&id).and_then(|w| w.upgrade())) { return b }
    let b = cx.new(|cx| CodeBrowser::new(app.clone(), model.clone(), window, cx));
    BROWSERS.with(|m| m.borrow_mut().insert(id, b.downgrade()));
    b
}

impl CodeBrowser {
    pub fn new(app: Entity<AppModel>, model: Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(app, model, Source::Worktree, window, cx)
    }

    pub fn new_rev(app: Entity<AppModel>, model: Entity<WorktreeModel>, base: String, head: String, changed: Vec<RevFile>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(app, model, Source::Rev { base, head, changed }, window, cx)
    }

    fn with_source(app: Entity<AppModel>, model: Entity<WorktreeModel>, source: Source, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        // ⌘W closes the open tab rather than the window while the browser shows; with no tab open it does nothing
        // (CommandWInterceptor). Anywhere else it passes through.
        // Back / Forward (⌃- / ⌃⇧-, or as rebound in Settings) work window-wide too, as the Swift buttons'
        // shortcuts did, not only while focus is inside the file.
        let cmd_w = cx.intercept_keystrokes(move |e, window, cx| {
            let k = &e.keystroke;
            let is = |a: &dyn gpui_kit::Action| window.bindings_for_action(a).iter().any(|b| b.match_keystrokes(std::slice::from_ref(k)) == Some(false));
            let close = k.key == "w" && k.modifiers.platform && !k.modifiers.shift && !k.modifiers.alt && !k.modifiers.control;
            let (back, forward) = if close { (false, false) } else { (is(&NavigateBack), is(&NavigateForward)) };
            if !close && !back && !forward { return }
            let handled = weak.update(cx, |v, cx| {
                // Back / Forward also work in Compact, where ⌘-click works too (Swift had them only in Full, §14 #7).
                if close {
                    if !v.showing_full(cx) { return false }
                    if let Some(p) = v.nav.selected.clone() { v.handle(TabEvent::Close(p), cx) }
                    return true
                }
                if !v.on_host(cx) || (back && v.nav.back.is_empty()) || (forward && v.nav.forward.is_empty()) { return false }
                if back { v.go_back(cx) } else { v.go_forward(cx) }
                true
            });
            if handled.unwrap_or(false) { cx.stop_propagation() }
        });
        let subs = vec![cx.observe(&model, |v, _, cx| v.sync(cx)), cx.observe(&app, |v, _, cx| v.sync(cx)), cmd_w];
        let mut v = Self {
            app, model, source, nav: CodeNav::default(), results: None, busy: None, lookup: None, tree: None,
            tree_key: String::new(), tree_task: None, collapsed: HashSet::new(), collapse_applied: false,
            files: HashMap::new(), files_version: 0, loading: None, diff: None, reveal: None,
            outline: (String::new(), Outline::None), outline_task: None, peek: false, header_actions: None, images: Default::default(), _subs: subs,
        };
        v.sync(cx);
        v
    }

    /// The PR's head moved: tree and files reload; tabs stay.
    pub fn set_rev(&mut self, base: String, head: String, changed: Vec<RevFile>, cx: &mut Context<Self>) {
        let s = Source::Rev { base, head, changed };
        if s == self.source { return }
        self.source = s;
        self.files.clear();
        self.tree_key.clear();
        self.sync(cx);
        if let Some(p) = self.nav.selected.clone() { self.load(p, false, cx) }
    }

    /// Extra controls for the file header (the PR's Viewed checkbox); gets the shown path.
    pub fn header_actions(&mut self, f: impl Fn(&str, &mut Window, &mut App) -> AnyElement + 'static) { self.header_actions = Some(Rc::new(f)) }

    pub fn peeking(&self) -> bool { self.peek }
    pub fn end_peek(&mut self, cx: &mut Context<Self>) { if self.peek { self.peek = false; cx.notify() } }
    pub fn results(&self) -> Option<&NavResults> { self.results.as_ref() }
    pub fn clear_results(&mut self, cx: &mut Context<Self>) { if self.results.take().is_some() { cx.notify() } }
    pub fn diff_view(&self) -> Option<&Entity<DiffView>> { self.diff.as_ref() }
    pub fn is_busy(&self) -> bool { self.busy.is_some() }

    fn dir(&self, cx: &App) -> String { self.model.read(cx).path_str() }
    fn host_tab(&self) -> Tab { if matches!(self.source, Source::Worktree) { Tab::Changes } else { Tab::Pulls } }
    fn full(&self, cx: &App) -> bool { self.app.read(cx).full_mode }
    fn shows_pane(&self, cx: &App) -> bool { self.full(cx) || self.peek }
    fn on_host(&self, cx: &App) -> bool { self.app.read(cx).tab == self.host_tab() && self.model.read(cx).is_selected() }
    fn showing_full(&self, cx: &App) -> bool {
        let a = self.app.read(cx);
        a.tab == self.host_tab() && self.model.read(cx).is_selected() && (a.full_mode || self.peek)
    }
    fn rev(&self) -> Option<&str> { match &self.source { Source::Rev { head, .. } => Some(head), Source::Worktree => None } }

    /// How the change touches `path`, if it does: worktree status (unstaged preferred) or the PR's file.
    pub fn change(&self, path: &str, cx: &App) -> Option<FileChange> {
        match &self.source {
            Source::Worktree => {
                let fs = self.model.read(cx).state.files();
                fs.iter().find(|f| f.path == path && !f.staged).or_else(|| fs.iter().find(|f| f.path == path)).cloned()
            }
            Source::Rev { changed, .. } => changed.iter().find(|f| f.path == path)
                .map(|f| FileChange { path: f.path.clone(), orig_path: f.old_path.clone(), kind: f.kind, staged: false, submodule: None }),
        }
    }

    // MARK: Following the model

    fn sync(&mut self, cx: &mut Context<Self>) {
        let (selected, version, changes) = { let m = self.model.read(cx); (m.is_selected(), m.state.status_version, m.state.change_count()) };
        if !selected { self.release(cx); return }
        // Nothing loads while the pane isn't shown: Compact never lists the tree.
        if !self.shows_pane(cx) { return }
        let key = match &self.source { Source::Worktree => format!("{version} {changes}"), Source::Rev { head, .. } => head.clone() };
        if key != self.tree_key && self.tree_task.is_none() { self.load_tree(key, cx) }
        if matches!(self.source, Source::Worktree) && self.files_version != version {
            // A status refresh: open files may have changed. Keep showing the shown one while it reloads, so nothing
            // flashes a spinner (Swift dropped them and showed one each refresh, §14 #5).
            self.files_version = version;
            let shown = self.nav.selected.clone();
            self.files.retain(|p, _| Some(p) == shown.as_ref());
            if let Some(p) = shown { self.load(p, true, cx) }
        }
        if self.nav.tabs.is_empty() && self.full(cx) && let Some(f) = self.model.read(cx).state.selected_file().map(|f| f.path.clone()) {
            // Entering Full mode with nothing open: the change the list had selected.
            self.show(&f, cx);
        }
    }

    /// Leaving the worktree: whole files and the tree reload when it shows again; tabs and folding stay.
    fn release(&mut self, cx: &mut Context<Self>) {
        if self.tree.is_none() && self.files.is_empty() && self.results.is_none() && self.diff.is_none() && self.tree_task.is_none() { return }
        self.files.clear();
        self.tree = None;
        self.tree_key.clear();
        self.tree_task = None;
        self.loading = None;
        self.results = None;
        if let Some((c, _)) = self.lookup.take() { c.store(true, Ordering::Relaxed) }
        self.busy = None;
        self.diff = None;
        self.cancel_outline();
        self.outline = (String::new(), Outline::None);
        cx.notify();
    }

    fn load_tree(&mut self, key: String, cx: &mut Context<Self>) {
        let dir = self.dir(cx);
        let rev = self.rev().map(str::to_string);
        let changed: Vec<String> = match &self.source {
            Source::Worktree => self.model.read(cx).state.files().iter().map(|f| f.path.clone()).collect(),
            Source::Rev { changed, .. } => changed.iter().map(|f| f.path.clone()).collect(),
        };
        let status_known = self.model.read(cx).state.status_version > 0 || rev.is_some();
        self.tree_task = Some(cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                let args: Vec<&str> = match &rev {
                    Some(h) => vec!["ls-tree", "-r", "-z", "--name-only", h],
                    None => vec!["ls-files", "-co", "--exclude-standard", "-z"],
                };
                let out = loupe_core::git::run(&dir, &args).ok()?;
                let mut set: HashSet<String> = out.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
                if rev.is_none() { set.extend(changed.iter().cloned()) }   // deleted files stay reachable
                let mut paths: Vec<String> = set.into_iter().collect();
                file_tree::sort_paths(&mut paths);
                let dirs: HashSet<String> = file_tree::all_dirs(&paths).into_iter().collect();
                let open: HashSet<String> = file_tree::all_dirs(&changed).into_iter().collect();
                Some((paths, dirs, open))
            }).await;
            this.update(cx, |v, cx| {
                v.tree_task = None;
                v.tree_key = key;
                // A failed listing is an empty tree, not a spinner forever (§14 #3).
                let Some((paths, dirs, open)) = r else { v.tree = Some(vec![]); cx.notify(); return };
                // Fold everything but where something changed, once the changes are known; a clean worktree keeps
                // what the user expanded across refreshes (§14 #2).
                if !v.collapse_applied && status_known {
                    v.collapsed = dirs.difference(&open).cloned().collect();
                    v.collapse_applied = true;
                }
                v.tree = Some(paths);
                cx.notify();
                v.sync(cx);   // the key may have moved while listing
            }).ok();
        }));
    }

    // MARK: Files

    /// Loads `path` whole: a changed file with its changes inline, any other as plain lines. `again`: reload a
    /// shown file in place (a status refresh); it's replaced only when its text changed.
    fn load(&mut self, path: String, again: bool, cx: &mut Context<Self>) {
        if !again && self.files.contains_key(&path) { self.apply_shown(cx); return }
        if self.loading.as_ref().is_some_and(|(p, _)| *p == path) && !again { return }
        let dir = self.dir(cx);
        let change = self.change(&path, cx);
        let source = self.source.clone();
        let (p, key) = (path.clone(), path.clone());
        // A new load replaces (and so cancels) the previous one: only the shown file is ever loading.
        let task = cx.spawn(async move |this, cx| {
            let f = cx.background_executor().spawn(async move { load_file(&dir, &p, change.as_ref(), &source).map(|d| adapt::file_diff(&d)) }).await;
            this.update(cx, |v, cx| {
                if v.loading.as_ref().is_some_and(|(lp, _)| *lp == path) { v.loading = None }
                let Some(f) = f else { cx.notify(); return };
                if v.files.get(&path).is_some_and(|old| old.text == f.text && old.lines.len() == f.lines.len() && old.additions == f.additions) { return }
                v.files.insert(path, Arc::new(f));
                v.apply_shown(cx);
            }).ok();
        });
        self.loading = Some((key, task));
        cx.notify();
    }

    /// The selected file into the DiffView, its pending line revealed, its outline asked for.
    fn apply_shown(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.nav.selected.clone() else { cx.notify(); return };
        let Some(f) = self.files.get(&p).cloned() else { cx.notify(); return };
        match &self.diff {
            Some(d) => d.update(cx, |d, cx| d.set_file(f.clone(), cx)),
            None => self.diff = Some(self.make_diff(f.clone(), cx)),
        }
        if let (Some(line), Some(d)) = (self.reveal.take(), &self.diff) { d.update(cx, |d, cx| d.reveal(line as u32, cx)) }
        self.apply_images(cx);
        self.ask_outline(&p, &f, cx);
        cx.notify();
    }

    /// LF-04: Changes resolves an LFS pointer image's objects (as for its Compact diff) and hands them here.
    pub fn set_images(&mut self, path: String, old: Option<std::path::PathBuf>, new: Option<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.images = (path, old, new);
        self.apply_images(cx);
    }

    fn apply_images(&self, cx: &mut Context<Self>) {
        let Some(d) = &self.diff else { return };
        let (a, b) = if self.nav.selected.as_deref() == Some(self.images.0.as_str()) { (self.images.1.clone(), self.images.2.clone()) } else { (None, None) };
        d.update(cx, |d, cx| d.set_images(a, b, cx));
    }

    fn make_diff(&self, f: Arc<FileDiff>, cx: &mut Context<Self>) -> Entity<DiffView> {
        let me = cx.weak_entity();
        let me2 = me.clone();
        let app = self.app.clone();
        let (side, layout) = { let a = self.app.read(cx); (a.diff_side, a.layout) };
        cx.new(|cx| {
            let mut d = DiffView::new(f, cx);
            d.set_highlighter(Some(adapt::highlighter()), cx);
            d.set_side(side, cx);
            d.set_layout(layout, cx);
            d.set_change_ruler(true, cx);
            d.on_navigate(move |w, a, _, cx| { me.update(cx, |v, cx| { let from = v.nav.selected.clone().unwrap_or_default(); v.navigate(w, a, from, cx) }).ok(); });
            d.on_side_change(move |s, l, _, cx| app.update(cx, |a, cx| a.set_diff_side(s, l, cx)));
            d.header_actions(move |w, cx| me2.upgrade().map(|b| CodeBrowser::header(&b, w, cx)).unwrap_or_else(|| div().into_any_element()));
            d
        })
    }

    // MARK: Outline

    fn cancel_outline(&mut self) {
        if let Some((c, _)) = self.outline_task.take() { c.store(true, Ordering::Relaxed) }
    }

    /// Off the main thread, cached by content in loupe_code; leaving the file cancels the parse.
    fn ask_outline(&mut self, path: &str, f: &Arc<FileDiff>, cx: &mut Context<Self>) {
        let key = format!("{path}\0{}", f.uid);
        if self.outline.0 == key { return }
        self.cancel_outline();
        self.outline = (key.clone(), Outline::None);
        if f.is_binary || loupe_code::index::grammar(path).is_none() { return }
        let text = new_side(f);
        let cancel = Arc::new(AtomicBool::new(false));
        let (c, p) = (cancel.clone(), path.to_string());
        let task = cx.spawn(async move |this, cx| {
            let o = cx.background_executor().spawn(async move { let o = loupe_code::index::outline_of(&text, &p, &c); (o, c.load(Ordering::Relaxed)) }).await;
            this.update(cx, |v, cx| {
                if v.outline.0 != key || o.1 { return }
                v.outline.1 = match o.0 {
                    loupe_code::index::Outline::TooLarge => Outline::TooLarge,
                    loupe_code::index::Outline::Symbols(s) => Outline::Symbols(s.iter().map(|s| OutlineSymbol { name: s.name.clone(), kind: kind_name(s.kind).into(), line: s.line }).collect()),
                };
                v.outline_task = None;
                cx.notify();
            }).ok();
        });
        self.outline_task = Some((cancel, task));
    }

    // MARK: Navigation

    /// Shows `path` without recording a jump: a tree or tab click. Its line is forgotten, so Back doesn't store
    /// another file's line (§14 #6).
    pub fn show(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.nav.tabs.iter().any(|t| t == path) {
            let at = self.nav.selected.as_ref().and_then(|s| self.nav.tabs.iter().position(|t| t == s)).map_or(self.nav.tabs.len(), |i| i + 1);
            self.nav.tabs.insert(at, path.into());
        }
        if self.nav.selected.as_deref() != Some(path) { self.nav.line = None }
        self.nav.selected = Some(path.into());
        self.reveal = None;
        self.load(path.into(), false, cx);
    }

    /// A jump: recorded for Back, revealed at `line`. In Compact a changed file shows in the change list; any other
    /// opens here in place (`peeking`) without switching every view to Full mode.
    pub fn open(&mut self, path: &str, line: usize, cx: &mut Context<Self>) {
        self.nav.open(path, line);
        self.go(path, Some(line), cx);
    }

    /// `line` None opens the file without landing on a line (an Overview row that has none, CO-20).
    fn go(&mut self, path: &str, line: Option<usize>, cx: &mut Context<Self>) {
        if !self.full(cx) && matches!(self.source, Source::Worktree) {
            if let Some(c) = self.change(path, cx) {
                self.peek = false;
                let id = c.id();
                self.model.update(cx, |m, cx| if m.state.selected_file().map(|f| f.path.as_str()) != Some(path) { m.select_file(Some(id), cx) });
                if let Some(line) = line { cx.emit(CodeBrowserEvent::Reveal { path: path.into(), line }) }
                cx.notify();
                return;
            }
            self.peek = true;
        }
        self.reveal = line;
        if self.files.contains_key(path) { self.apply_shown(cx) } else { self.load(path.into(), false, cx) }
    }

    /// The Overview's onOpen in Changes: the file, at its line (Swift dropped the line, §14 #4). A file the change
    /// doesn't touch opens too, instead of only closing the Overview.
    pub fn open_from_overview(&mut self, path: &str, line: Option<usize>, cx: &mut Context<Self>) {
        self.nav.open(path, line.unwrap_or(1));
        self.go(path, line, cx)
    }

    /// Compact: keep up with the list's file without opening tabs.
    pub fn follow(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.nav.selected.as_deref() != Some(path) { self.nav.follow(path); self.nav.line = None }
        self.end_peek(cx);
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.nav.back.is_empty() { return }
        self.nav.go_back();
        if let (Some(p), Some(l)) = (self.nav.selected.clone(), self.nav.line) { self.go(&p, Some(l), cx) }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if self.nav.forward.is_empty() { return }
        self.nav.go_forward();
        if let (Some(p), Some(l)) = (self.nav.selected.clone(), self.nav.line) { self.go(&p, Some(l), cx) }
    }

    /// ⌘-click / Go to Definition / Find References on `word`. A new lookup cancels the running one.
    pub fn navigate(&mut self, word: String, action: CodeAction, from: String, cx: &mut Context<Self>) {
        if let Some((c, _)) = self.lookup.take() { c.store(true, Ordering::Relaxed) }
        let cancel = Arc::new(AtomicBool::new(false));
        let (dir, rev, c) = (self.dir(cx), self.rev().map(str::to_string), cancel.clone());
        self.busy = Some(word.clone());
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let w = word.clone();
            let found = cx.background_executor().spawn(async move {
                if action == CodeAction::Definition {
                    let defs = loupe_code::index::definitions(&w, rev.as_deref(), &dir, &from, &c);
                    if !defs.is_empty() { return Err(defs) }
                }
                Ok(loupe_code::index::references(&w, rev.as_deref(), &dir, loupe_code::index::REFERENCE_LIMIT + 1))
            }).await;
            this.update(cx, |v, cx| {
                v.busy = None;
                v.lookup = None;
                match found {
                    Err(defs) if defs.len() == 1 => { v.results = None; let d = &defs[0]; let (p, l) = (d.path.clone(), d.line); v.open(&p, l, cx) }
                    Err(defs) => v.results = Some(NavResults::Definitions(word, defs.iter().map(|d| NavHit { path: d.path.clone(), line: d.line, text: format!("{} {}", kind_name(d.kind), d.name) }).collect())),
                    Ok(refs) => v.results = Some(NavResults::References(word, refs.into_iter().map(|r| NavHit { path: r.path, line: r.line, text: r.text }).collect(), action == CodeAction::Definition)),
                }
                cx.notify();
            }).ok();
        });
        self.lookup = Some((cancel, task));
    }

    fn handle(&mut self, e: TabEvent, cx: &mut Context<Self>) {
        match e {
            TabEvent::Select(p) => self.show(&p, cx),
            TabEvent::Close(p) => { self.nav.close_tab(&p); self.after_close(cx) }
            TabEvent::CloseOthers(p) => { self.nav.close_other_tabs(&p); self.after_close(cx) }
            TabEvent::CloseAll => { self.nav.close_all(); self.after_close(cx) }
            TabEvent::Open(l) => self.open(&l.path, l.line, cx),
            TabEvent::Back => self.go_back(cx),
            TabEvent::Forward => self.go_forward(cx),
            TabEvent::DismissResults => { self.results = None; cx.notify() }
        }
    }

    fn after_close(&mut self, cx: &mut Context<Self>) {
        self.nav.line = None;
        match self.nav.selected.clone() { Some(p) => self.load(p, false, cx), None => { self.peek = false; cx.notify() } }
    }

    fn on_event(this: &Entity<Self>) -> tabs::OnTabEvent {
        let this = this.downgrade();
        Rc::new(move |e, _, cx| { this.update(cx, |v, cx| v.handle(e, cx)).ok(); })
    }

    // MARK: Elements for the host

    /// The file header's controls: busy word, back / forward, outline, the host's extras, then Stage / Discard /
    /// Unstage for a changed worktree file.
    fn header(this: &Entity<Self>, window: &mut Window, cx: &mut App) -> AnyElement {
        let v = this.read(cx);
        let Some(path) = v.nav.selected.clone() else { return div().into_any_element() };
        let nav = tabs::nav_controls(&v.nav, &path, &v.outline.1, v.busy.as_deref(), Self::on_event(this));
        let extra = v.header_actions.clone().map(|f| f(&path, window, cx));
        let v = this.read(cx);
        let change = if matches!(v.source, Source::Worktree) { v.change(&path, cx) } else { None };
        let m = v.model.clone();
        div().flex().items_center().gap_1()
            .child(nav)
            .children(extra)
            .when_some(change, |d, c| {
                let (m1, c1) = (m.clone(), c.clone());
                if c.staged {
                    d.child(ui::pill("bf-unstage", "Unstage File").on_click(move |_, _, cx| m1.update(cx, |m, cx| m.unstage(vec![c1.clone()], cx).detach())))
                } else {
                    let (m2, c2) = (m.clone(), c.clone());
                    d.child(ui::pill("bf-discard", "Discard…").on_click(move |_, _, cx| m2.update(cx, |m, cx| m.discard(vec![c2.clone()], cx).detach())))
                        .child(ui::pill_primary("bf-stage", "Stage File", theme::blue()).on_click(move |_, _, cx| m1.update(cx, |m, cx| m.stage(vec![c1.clone()], cx).detach())))
                }
            })
            .into_any_element()
    }

    /// The definitions / references panel (220 high), for a Compact host to put under its diff.
    pub fn results_panel(this: &Entity<Self>, cx: &App) -> Option<AnyElement> {
        this.read(cx).results.as_ref().map(|r| Self::panel(r, this))
    }

    fn panel(r: &NavResults, this: &Entity<Self>) -> AnyElement {
        div().h(px(220.)).flex_none().border_t_1().border_color(theme::border()).child(tabs::results_panel(r, Self::on_event(this))).into_any_element()
    }

    /// Full mode's left pane: every file, changed ones with their status.
    pub fn tree(this: &Entity<Self>, cx: &App) -> AnyElement {
        let v = this.read(cx);
        let Some(tree) = v.tree.as_ref() else {
            return div().size_full().flex().items_center().justify_center().child(ui::icon("loader").size(px(16.))).into_any_element()
        };
        if tree.is_empty() { return ui::empty_state("folder", "No Files", "Nothing is tracked in this worktree yet.").into_any_element() }
        let rows = Rc::new(file_tree::rows(tree, &v.collapsed));
        let kinds: Rc<HashMap<String, Kind>> = Rc::new(match &v.source {
            Source::Worktree => v.model.read(cx).state.files().iter().map(|f| f.path.clone())
                .filter_map(|p| v.change(&p, cx).map(|c| (p, c.kind))).collect(),
            Source::Rev { changed, .. } => changed.iter().map(|f| (f.path.clone(), f.kind)).collect(),
        });
        let locks: Rc<HashMap<String, String>> = Rc::new(v.model.read(cx).state.lfs.locks.clone());
        let selected = v.nav.selected.clone();
        let me = this.downgrade();
        uniform_list("code-tree", rows.len(), move |range, _, _| {
            range.map(|i| {
                let r = rows[i].clone();
                let me = me.clone();
                let row = div().id(i).w_full().flex().items_center().gap(px(6.)).h(px(24.)).pr_2().pl(px(6. + 10. * r.depth as f32))
                    .rounded(theme::RADIUS_ROW).text_size(px(12.5)).cursor_pointer();
                if let Some(dir) = r.dir_path.clone() {
                    let open = rows.get(i + 1).is_some_and(|n| n.depth > r.depth);
                    let (d1, d2, me2) = (dir.clone(), dir.clone(), me.clone());
                    row.hover(|s| s.bg(theme::hover())).text_color(theme::text_muted())
                        .on_click(move |_, _, cx| { me.update(cx, |v, cx| v.toggle_dir(&d1, cx)).ok(); })
                        .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(10.)))
                        .child(ui::icon(if open { "folder-open" } else { "folder" }).size(px(13.)))
                        .child(div().flex_1().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().child(r.name.clone()))
                        .context_menu(move |menu, _, _| {
                            let (a, b, c, d) = (me2.clone(), me2.clone(), me2.clone(), d2.clone());
                            menu.item(PopupMenuItem::new(if open { "Collapse" } else { "Expand" }).on_click(move |_, _, cx| { a.update(cx, |v, cx| v.toggle_dir(&d, cx)).ok(); }))
                                .separator()
                                .item(PopupMenuItem::new("Expand All").on_click(move |_, _, cx| { b.update(cx, |v, cx| { v.collapsed.clear(); cx.notify() }).ok(); }))
                                .item(PopupMenuItem::new("Collapse All").on_click(move |_, _, cx| { c.update(cx, |v, cx| { v.collapsed = v.tree.as_deref().map(|t| file_tree::all_dirs(t).into_iter().collect()).unwrap_or_default(); cx.notify() }).ok(); }))
                        })
                        .into_any_element()
                } else {
                    let p = r.file_path.clone().unwrap_or_default();
                    let kind = kinds.get(&p).copied();
                    let sel = selected.as_deref() == Some(p.as_str());
                    let tip: SharedString = p.clone().into();
                    let lock = locks.get(&p).cloned();
                    let p2 = p.clone();
                    row.map(|d| if sel { d.bg(theme::selected()) } else { d.hover(|s| s.bg(theme::hover())) })
                        .text_color(if kind.is_some() || sel { theme::text() } else { theme::text_muted() })
                        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                        .on_click(move |_, _, cx| { me.update(cx, |v, cx| v.show(&p2, cx)).ok(); })
                        .child(div().w(px(10.)).flex_none())
                        .child(ui::file_icon(&p))
                        .child(div().flex_1().min_w_0().truncate().child(r.name.clone()))
                        .when_some(lock, |d, o| d.child(ui::icon("lock").size(px(11.)).text_color(theme::accent())).child(ui::kbd(o)))
                        .when_some(kind, |d, k| d.child(status_icon(k)))
                        .into_any_element()
                }
            }).collect::<Vec<_>>()
        }).size_full().py_1().px_1().into_any_element()
    }

    fn toggle_dir(&mut self, d: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(d) { self.collapsed.insert(d.to_string()); }
        cx.notify();
    }
}

/// FT-02's StatusIcon.
pub fn status_icon(k: Kind) -> AnyElement {
    let (name, color) = match k {
        Kind::Added | Kind::Untracked => ("square-plus", theme::green()),
        Kind::Deleted => ("square-minus", theme::red()),
        Kind::Renamed | Kind::Copied => ("square-arrow-right", theme::blue()),
        Kind::Conflicted => ("triangle-alert", theme::accent()),
        _ => ("square-dot", theme::accent()),
    };
    ui::icon(name).size(px(13.)).text_color(color).into_any_element()
}

fn kind_name(k: loupe_code::index::SymbolKind) -> &'static str {
    use loupe_code::index::SymbolKind as S;
    match k {
        S::Function => "function", S::Method => "method", S::Class => "class", S::Interface => "interface", S::Module => "module",
        S::Type => "type", S::Constant => "constant", S::Macro => "macro", S::Field => "field", S::Other => "other",
    }
}

/// The text a whole-file diff ends with: its new side (the outline's input).
fn new_side(f: &FileDiff) -> Arc<str> {
    use crate::diff::LineKind as L;
    f.lines.iter().enumerate().filter(|(_, l)| matches!(l.kind, L::Context | L::Added)).map(|(i, _)| f.line_text(i)).collect::<Vec<_>>().join("\n").into()
}

/// The file as it is: a changed file with its changes inline (worktree against HEAD, a PR from its merge base), any
/// other one as plain lines (from disk, or at the PR's head).
pub fn load_file(dir: &str, path: &str, change: Option<&FileChange>, source: &Source) -> Option<loupe_core::diff::FileDiff> {
    use loupe_core::{diff, git};
    match source {
        Source::Worktree => {
            if let Some(c) = change {
                if matches!(c.kind, Kind::Untracked | Kind::Added | Kind::Deleted) {
                    if let Ok(Some(d)) = git::diff_for(c, std::path::Path::new(dir), 1_000_000, None) { return Some(d) }   // the diff is the file
                } else if let Ok(data) = git::run(dir, ["diff", "--no-color", "--no-ext-diff", "-M", "-U1000000", "HEAD", "--", path])
                    && let Some(d) = diff::whole_file(diff::parse_bytes(&data).into_iter().next()) { return Some(d) }
            }
            std::fs::read(std::path::Path::new(dir).join(path)).ok().map(|data| diff::plain_file(path, &data))
        }
        Source::Rev { base, head, .. } => {
            if let Some(c) = change {
                let range = format!("{base}...{head}");
                let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-M", "-U1000000", range.as_str(), "--"];
                args.extend(c.orig_path.as_deref());
                args.push(path);
                if let Ok(data) = git::run(dir, &args) && let Some(d) = diff::parse_bytes(&data).into_iter().next() {
                    return if d.is_binary || c.kind == Kind::Deleted { Some(d) } else { diff::whole_file(Some(d)) };
                }
            }
            git::run(dir, ["show", &format!("{head}:{path}")]).ok().map(|data| diff::plain_file(path, &data))
        }
    }
}

impl Render for CodeBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let on = Self::on_event(&this);
        let changed = |p: &str| self.change(p, cx).map(|c| match c.kind {
            Kind::Added | Kind::Untracked => theme::green(), Kind::Deleted => theme::red(), Kind::Renamed | Kind::Copied => theme::purple(), _ => theme::blue(),
        });
        let bar = (!self.nav.tabs.is_empty()).then(|| tabs::tab_bar(&self.nav, changed, on.clone()));
        let body = match &self.nav.selected {
            None => ui::empty_state("file-search", "No File Open", "Open one from the tree, or ⌘-click a name to go to its definition.").into_any_element(),
            Some(p) => match (self.files.get(p), &self.diff) {
                (Some(f), _) if !f.is_binary && f.lines.is_empty() => ui::empty_state("file", "Empty File", "").into_any_element(),
                (Some(_), Some(d)) => div().size_full().p_2().child(d.clone()).into_any_element(),
                _ => div().size_full().flex().items_center().justify_center().child(ui::icon("loader").size(px(16.))).into_any_element(),
            },
        };
        div().size_full().flex().flex_col().min_w_0().bg(theme::canvas())
            .on_action(cx.listener(|v, _: &NavigateBack, _, cx| v.go_back(cx)))
            .on_action(cx.listener(|v, _: &NavigateForward, _, cx| v.go_forward(cx)))
            .on_action(cx.listener(|v, _: &CloseWindow, _, cx| if let Some(p) = v.nav.selected.clone() { v.handle(TabEvent::Close(p), cx) }))
            .on_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" && v.results.take().is_some() { cx.stop_propagation(); cx.notify() }))
            .children(bar)
            .child(div().flex_1().min_h_0().child(body))
            .children(self.results.as_ref().map(|r| Self::panel(r, &this)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    fn repo(name: &str) -> Dir {
        let d = Dir(std::env::temp_dir().join(format!("loupe-code-{name}-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&d.0);
        std::fs::create_dir_all(&d.0).unwrap();
        let g = |a: &[&str]| {
            let s = std::process::Command::new("git").args(a).current_dir(&d.0).env("GIT_CONFIG_NOSYSTEM", "1").env("HOME", &d.0).output().unwrap();
            assert!(s.status.success(), "{a:?}: {}", String::from_utf8_lossy(&s.stderr));
        };
        g(&["init", "-q"]);
        std::fs::write(d.0.join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        std::fs::write(d.0.join("b.txt"), "x\n").unwrap();
        g(&["add", "."]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "i"]);
        std::fs::write(d.0.join("a.rs"), "fn one() {}\nfn three() {}\n").unwrap();
        d
    }

    #[test]
    fn loads_changed_files_whole_and_others_plain() {
        let d = repo("load");
        let dir = d.0.to_str().unwrap();
        let c = FileChange::new("a.rs", Kind::Modified, false);
        let f = load_file(dir, "a.rs", Some(&c), &Source::Worktree).unwrap();
        assert_eq!((f.additions, f.deletions), (1, 1));
        assert!(f.lines.iter().all(|l| l.kind != loupe_core::diff::LineKind::Hunk));
        let p = load_file(dir, "b.txt", None, &Source::Worktree).unwrap();
        assert_eq!((p.lines.len(), p.additions), (1, 0));
        let h = load_file(dir, "a.rs", None, &Source::Rev { base: "HEAD".into(), head: "HEAD".into(), changed: vec![] }).unwrap();
        assert_eq!(h.text(&h.lines[1]), "fn two() {}");
    }

    #[test]
    fn whole_file_new_side_feeds_the_outline() {
        let d = repo("outline");
        let c = FileChange::new("a.rs", Kind::Modified, false);
        let f = adapt::file_diff(&load_file(d.0.to_str().unwrap(), "a.rs", Some(&c), &Source::Worktree).unwrap());
        let text = new_side(&f);
        assert_eq!(&*text, "fn one() {}\nfn three() {}");
        let cancel = AtomicBool::new(false);
        let names: Vec<String> = loupe_code::index::outline(&text, "a.rs", &cancel).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["one", "three"]);
    }
}
