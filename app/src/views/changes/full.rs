//! Full mode in Changes (CH-41/42): every file of the worktree as a tree, the open ones in tabs, each whole with its
//! changes inline. Code navigation (⌘-click, results, outline) is the code browser's (CB-*); the tabs and tree here
//! are the hooks it plugs into.
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, px, uniform_list,
    component::menu::{ContextMenuExt as _, PopupMenuItem},
    component::tooltip::Tooltip,
};
use loupe_core::git::{FileChange, Kind};

use super::pane::{has_target, spinner};
use super::{ChangesView, tree};
use crate::diff::{self, DiffView, Layout};
use crate::state::{WorktreeModel, adapt};
use crate::ui::tabs::{CodeNav, TabEvent, tab_bar};
use crate::{theme, ui};

#[derive(Default)]
pub(crate) struct State {
    tree: Option<Vec<String>>,
    tree_key: String,
    _tree_task: Option<Task<()>>,
    collapsed: Option<HashSet<String>>, // None until the first tree: then everything without a change folds
    nav: CodeNav,
    file_key: String,
    file: Option<(String, Arc<diff::FileDiff>)>,
    view: Option<Entity<DiffView>>,
    _file_task: Option<Task<()>>,
    was_full: bool,
    scroll: UniformListScrollHandle,
}

impl State {
    /// Compact mode keeps up with the list's file, so Full opens on it.
    pub fn follow(&mut self, path: &str) { self.nav.follow(path) }
    pub fn view(&self) -> Option<Entity<DiffView>> { self.view.clone() }
    /// The open file and its diff uid, for blame.
    pub fn blame_target(&self) -> Option<(String, u64)> { self.file.as_ref().map(|(p, d)| (p.clone(), d.uid)) }
}

/// The entry a path shows in Full mode: its unstaged one, else its staged one.
pub(crate) fn change<'a>(files: &'a [FileChange], path: &str) -> Option<&'a FileChange> {
    files.iter().find(|f| f.path == path && !f.staged).or_else(|| files.iter().find(|f| f.path == path))
}

impl ChangesView {
    pub(crate) fn update_full(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let (dir, version, count, files, sel) = {
            let s = &m.read(cx).state;
            (s.path.clone(), s.status_version, s.change_count(), s.files().to_vec(), s.selected_file().map(|f| f.path.clone()))
        };
        // CH-10: going Full opens the list's file (CH-42: with no tab open, the selected change opens).
        if !self.full.was_full {
            self.full.was_full = true;
            if self.full.nav.tabs.is_empty() && let Some(p) = sel.or(self.full.nav.selected.clone()) { self.full.nav.open(&p, 1) }
        }
        let key = format!("{version} {count}");
        if key != self.full.tree_key {
            self.full.tree_key = key.clone();
            let d = dir.clone();
            self.full._tree_task = Some(cx.spawn(async move |this, cx| {
                let paths = cx.background_executor().spawn(async move {
                    let out = loupe_core::git::run(&d, ["ls-files", "-co", "--exclude-standard", "-z"]).unwrap_or_default();
                    out.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>()
                }).await;
                this.update(cx, |v, cx| if v.full.tree_key == key {
                    let changed: HashSet<String> = v.selected().map(|m| m.read(cx).state.files().iter().map(|f| f.path.clone()).collect()).unwrap_or_default();
                    // §14 lenses 2: folded once, then left as the user leaves it (Swift re-folded a clean tree on every refresh).
                    if v.full.collapsed.is_none() { v.full.collapsed = Some(tree::fold_unchanged(&paths, &changed)) }
                    v.full.tree = Some(paths);
                    cx.notify()
                }).ok();
            }));
        }
        let Some(path) = self.full.nav.selected.clone() else { self.full.file = None; self.full.view = None; self.full.file_key.clear(); return };
        let c = change(&files, &path).cloned();
        let key = format!("{path} {version}");
        if key == self.full.file_key { return }
        self.full.file_key = key.clone();
        // The open file stays on screen while it reloads (§14 lenses 5: Swift flashed a spinner every refresh).
        self.full._file_task = Some(cx.spawn(async move |this, cx| {
            let p = path.clone();
            let d = cx.background_executor().spawn(async move {
                let d = match &c {
                    // The whole file with its changes against HEAD (staged and not), as CodeBrowser loaded it.
                    Some(f) if f.kind != Kind::Untracked => {
                        let mut a = vec!["diff", "--no-color", "--no-ext-diff", "-M", "-U1000000", "HEAD", "--", f.path.as_str()];
                        a.extend(f.orig_path.as_deref());
                        loupe_core::git::run(&dir, a).ok().and_then(|d| loupe_core::diff::whole_file(loupe_core::diff::parse_bytes(&d).into_iter().next()))
                            .or_else(|| loupe_core::model::load_diff(&dir, f, true, None).ok().flatten())
                    }
                    Some(f) => loupe_core::model::load_diff(&dir, f, true, None).ok().flatten(),
                    None => std::fs::read(dir.join(&p)).ok().map(|b| loupe_core::diff::plain_file(&p, &b)),
                };
                d.map(|d| Arc::new(adapt::file_diff(&d)))
            }).await;
            this.update(cx, |v, cx| {
                if v.full.file_key != key { return }
                let Some(d) = d else { v.full.file = None; v.full.view = None; cx.notify(); return };
                match &v.full.view {
                    Some(view) => view.update(cx, |x, cx| x.set_file(d.clone(), cx)),
                    None => v.full.view = Some(cx.new(|cx| {
                        let mut x = DiffView::new(d.clone(), cx);
                        x.set_show_header(false, cx);
                        x.set_highlighter(Some(adapt::highlighter()), cx);
                        x.set_layout(Layout::Unified, cx);
                        x.set_change_ruler(true, cx);
                        x
                    })),
                }
                v.full.file = Some((path, d));
                v.push_images(cx);
                v.update(cx);
            }).ok();
        }));
    }

    /// Leaving Full: the open tab's file is selected in the list when it's a change.
    pub(crate) fn leave_full(&mut self, cx: &mut Context<Self>) {
        if !self.full.was_full { return }
        self.full.was_full = false;
        let (Some(m), Some(p)) = (self.selected(), self.full.nav.selected.clone()) else { return };
        let id = change(m.read(cx).state.files(), &p).map(FileChange::id);
        if let Some(id) = id { m.update(cx, |m, cx| m.select_file(Some(id), cx)) }
    }

    pub(crate) fn close_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.full.nav.selected.clone() { self.full.nav.close_tab(&p); self.update(cx) }
    }

    fn open_in_full(&mut self, p: &str, cx: &mut Context<Self>) { self.full.nav.open(p, 1); self.update(cx) }

    /// CH-41: every file; unchanged ones dimmed, changed ones with their mark and any lock.
    pub(crate) fn render_tree(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) -> AnyElement {
        let Some(paths) = self.full.tree.clone() else { return div().flex_1().child(spinner()).into_any_element() };
        // §14 lenses 3: an empty worktree says so instead of spinning forever.
        if paths.is_empty() { return div().flex_1().child(ui::empty_state("folder", "No Files", "")).into_any_element() }
        let collapsed = self.full.collapsed.clone().unwrap_or_default();
        let rows = Rc::new(tree::rows(&paths, &collapsed, ""));
        let (files, locks) = { let s = &m.read(cx).state; (s.files().to_vec(), s.lfs.locks.clone()) };
        let selected = self.full.nav.selected.clone();
        let all = Rc::new(paths);
        let n = rows.len();
        let list = uniform_list("full-tree", n, cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range.map(|i| {
                let r = &rows[i];
                let me = cx.entity().downgrade();
                let dir = r.dir_path.clone();
                let all2 = all.clone();
                let el = match (&r.dir_path, &r.file_path) {
                    (Some(d), _) => {
                        let open = !collapsed.contains(d);
                        let d2 = d.clone();
                        div().id(("t", i)).w_full().h(px(24.)).px_1().rounded(theme::RADIUS_ROW).cursor_pointer().hover(|s| s.bg(theme::hover()))
                            .child(tree::dir_row(r, open))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                let c = v.full.collapsed.get_or_insert_default();
                                if !c.remove(&d2) { c.insert(d2.clone()); }
                                cx.notify()
                            }))
                    }
                    (_, Some(p)) => {
                        let c = change(&files, p);
                        let tip: SharedString = p.clone().into();
                        let p2 = p.clone();
                        ui::list_row(("t", i), selected.as_deref() == Some(p.as_str())).w_full().h(px(24.)).px_1().gap_0()
                            .child(tree::file_row_frame(r.depth)
                                .child(ui::file_icon(p))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis()
                                    .text_color(if c.is_none() { theme::text_muted() } else { theme::text() }).child(r.name.clone()))
                                .when_some(locks.get(p).cloned(), |d, o| d.child(div().id(("tl", i)).child(ui::icon("lock").size(px(11.)))
                                    .tooltip(move |w, cx| Tooltip::new(format!("Locked by {o} (Git LFS)")).build(w, cx))))
                                .when_some(c.map(|c| c.kind), |d, k| d.child(div().pr_1().child(tree::icon_el(k)))))
                            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                            .on_click(cx.listener(move |v, _, _, cx| v.open_in_full(&p2, cx)))
                    }
                    _ => div().id(("t", i)),
                };
                // FT-04
                el.context_menu(move |mut menu, _, cx| {
                    let me = me.clone();
                    let act = |label: &str, f: Rc<dyn Fn(&mut HashSet<String>)>| {
                        let me = me.clone();
                        PopupMenuItem::new(label.to_string()).on_click(move |_, _, cx| { me.update(cx, |v, cx| { f(v.full.collapsed.get_or_insert_default()); cx.notify() }).ok(); })
                    };
                    if let Some(d) = dir.clone() {
                        let open = !me.upgrade().is_some_and(|v| v.read(cx).full.collapsed.as_ref().is_some_and(|c| c.contains(&d)));
                        let d2 = d.clone();
                        menu = menu.item(act(if open { "Collapse" } else { "Expand" }, Rc::new(move |c| { if !c.remove(&d2) { c.insert(d2.clone()); } }))).separator();
                    }
                    let all3 = all2.clone();
                    menu.item(act("Expand All", Rc::new(|c| c.clear())))
                        .item(act("Collapse All", Rc::new(move |c| *c = tree::all_dirs(&all3))))
                }).into_any_element()
            }).collect::<Vec<_>>()
        })).track_scroll(&self.full.scroll).size_full();
        div().flex_1().min_h_0().px_1().pt_1().child(list).into_any_element()
    }

    /// CH-42
    pub(crate) fn render_full_pane(&mut self, m: &Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let files = m.read(cx).state.files().to_vec();
        let me = cx.entity().downgrade();
        let on: crate::ui::tabs::OnTabEvent = Rc::new(move |e, _, cx| { me.update(cx, |v, cx| {
            match e {
                TabEvent::Select(p) => v.full.nav.selected = Some(p),
                TabEvent::Close(p) => v.full.nav.close_tab(&p),
                TabEvent::CloseOthers(p) => v.full.nav.close_other_tabs(&p),
                TabEvent::CloseAll => v.full.nav.close_all(),
                _ => {}
            }
            v.update(cx)
        }).ok(); });
        let f2 = files.clone();
        let bar = (!self.full.nav.tabs.is_empty()).then(|| tab_bar(&self.full.nav, move |p| change(&f2, p).map(|c| tree::status_icon(c.kind).1), on));
        let Some(path) = self.full.nav.selected.clone() else {
            return ui::card().size_full().children(bar).child(ui::empty_state("file-search", "No File Open", "Open one from the tree, or ⌘-click a name to go to its definition.")).into_any_element();
        };
        let c = change(&files, &path).cloned();
        let shown = self.full.file.as_ref().filter(|(p, _)| *p == path).map(|(_, d)| d.clone());
        let Some(d) = shown else { return ui::card().size_full().children(bar).child(spinner()).into_any_element() };
        let target = c.as_ref().is_none_or(has_target);
        let header = self.header(&d, c.as_ref(), c.is_some(), false, target, cx);
        let body = if !d.is_binary && d.lines.is_empty() { ui::empty_state("file", "Empty File", "").into_any_element() }
            else { self.full.view.clone().map(|v| v.into_any_element()).unwrap_or_else(spinner) };
        ui::card().size_full().children(bar).child(header).child(self.with_panel(target, body, window, cx)).into_any_element()
    }
}
