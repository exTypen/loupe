//! The status list (FileListView): Conflicts / Staged / Changes sections, each a folder tree, multi-selection, the
//! context menu, Space and Delete. Virtualized: one row per visible line, whatever the change count.
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, Modifiers,
    ParentElement as _, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, uniform_list,
    component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    component::tooltip::Tooltip,
};
use loupe_core::git::{FileChange, Kind};
use loupe_core::{lfs, submodules};

use super::{ChangesView, Confirm, counted, tree};
use crate::state::{AppEvent, WorktreeModel};
use crate::{theme, ui};

const ROW: f32 = 26.;

#[derive(Clone)]
enum Item {
    Header { title: &'static str, count: usize, action: Option<(&'static str, bool)> }, // (label, stage?)
    Dir { key: String, row: loupe_core::file_tree::TreeRow, open: bool },
    File { f: FileChange, depth: usize, lock: Option<String> },
    Clean(Option<String>),
}

/// CH-15: which file the diff shows after the selection became `ids`: the first row just added (list order), else
/// the shown one if still selected, else the first selected.
pub(crate) fn shown_for(listed: &[String], old: &HashSet<String>, ids: &HashSet<String>, current: Option<&str>) -> Option<String> {
    listed.iter().find(|i| ids.contains(*i) && !old.contains(*i)).cloned()
        .or_else(|| current.filter(|c| ids.contains(*c)).map(str::to_string))
        .or_else(|| listed.iter().find(|i| ids.contains(*i)).cloned())
}

impl ChangesView {
    fn items(&self, m: &WorktreeModel) -> Vec<Item> {
        let s = &m.state;
        let mut out = vec![];
        let sections: [(&str, &str, &[FileChange], Option<(&str, bool)>); 3] = [
            ("conflicts", "Conflicts", s.conflicted(), None),
            ("staged", "Staged", s.staged(), Some(("Unstage All", false))),
            ("changes", "Changes", s.unstaged(), (!s.unstaged().is_empty()).then_some(("Stage All", true))),
        ];
        for (key, title, files, action) in sections {
            // CH-11: Conflicts and Staged only when non-empty; Changes always.
            if files.is_empty() && key != "changes" { continue }
            out.push(Item::Header { title, count: files.len(), action });
            let closed: HashSet<String> = self.collapsed.iter().filter_map(|c| c.strip_prefix(&format!("{key}/")).map(str::to_string)).collect();
            let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
            for r in tree::rows(&paths, &closed, "") {
                match (&r.dir_path, &r.file_path) {
                    (Some(d), _) => out.push(Item::Dir { key: format!("{key}/{d}"), open: !closed.contains(d), row: r.clone() }),
                    (_, Some(p)) => if let Some(f) = files.iter().find(|f| &f.path == p) {
                        out.push(Item::File { f: f.clone(), depth: r.depth, lock: s.lfs.locks.get(p).cloned() })
                    },
                    _ => {}
                }
            }
        }
        if s.files().is_empty() { out.push(Item::Clean(s.branch.clone())) }
        out
    }

    pub(crate) fn render_list(&mut self, m: &Entity<WorktreeModel>, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let items = Rc::new(self.items(m.read(cx)));
        let n = items.len();
        let list = uniform_list("changes-files", n, cx.processor(move |v, range: std::ops::Range<usize>, _, cx| {
            range.map(|i| v.render_item(i, &items[i], cx)).collect::<Vec<_>>()
        }))
        .track_scroll(&self.list_scroll)
        .size_full();
        div().id("changes-list").key_context("ChangesList").track_focus(&self.focus).flex_1().min_h_0().px_1().pt_1()
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| v.list_key(e, cx)))
            .child(list)
            .into_any_element()
    }

    fn render_item(&self, i: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let sel = self.selected().map(|m| m.read(cx).state.selected_file_ids.clone()).unwrap_or_default();
        match item {
            Item::Header { title, count, action } => ui::section_header(*title, None).h(px(ROW)).child(div().flex_none().text_color(theme::text_faint()).child(count.to_string()))
                .child(div().flex_1())
                .when_some(*action, |d, (label, stage)| d.child(ui::link_button(SharedString::from(format!("hdr-{label}")), label).text_size(px(11.))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        let Some(m) = v.selected() else { return };
                        let fs = { let s = &m.read(cx).state; if stage { s.unstaged().to_vec() } else { s.staged().to_vec() } };
                        if stage { v.stage(fs, cx) } else { v.unstage(fs, cx) }
                    }))))
                .into_any_element(),
            Item::Dir { key, row, open } => {
                let k = key.clone();
                div().id(("dir", i)).w_full().h(px(ROW)).px_1().rounded(theme::RADIUS_ROW).cursor_pointer().hover(|s| s.bg(theme::hover()))
                    .child(tree::dir_row(row, *open))
                    .on_click(cx.listener(move |v, _, _, cx| { if !v.collapsed.remove(&k) { v.collapsed.insert(k.clone()); } cx.notify() }))
                    .into_any_element()
            }
            Item::File { f, depth, lock } => self.file_row(i, f, *depth, lock.clone(), sel.contains(&f.id()), cx),
            Item::Clean(branch) => div().h(px(220.)).child(ui::empty_state("circle-check", "Working Tree Clean",
                branch.as_ref().map(|b| format!("Nothing to commit on {b}.")).unwrap_or_default())).into_any_element(),
        }
    }

    fn file_row(&self, i: usize, f: &FileChange, depth: usize, lock: Option<String>, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let (icon, color, tip) = tree::status_icon(f.kind);
        let id = f.id();
        let path: SharedString = f.path.clone().into();
        let me = cx.entity().downgrade();
        let menu_id = id.clone();
        ui::list_row(("file", i), selected).w_full().h(px(ROW)).px_1().gap_0()
            .child(tree::file_row_frame(depth)
                .child(if f.submodule.is_some() { ui::icon("package").into_any_element() } else { ui::file_icon(&f.path).into_any_element() })
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(tree::name(&f.path)))
                .when_some(lock, |d, o| d.child(div().id(("lock", i)).child(ui::icon("lock").size(px(11.)))
                    .tooltip(move |w, cx| Tooltip::new(format!("Locked by {o} (Git LFS)")).build(w, cx))))
                .child(div().id(("kind", i)).pr_1().child(ui::icon(icon).size(px(13.)).text_color(color))
                    .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))))
            .tooltip(move |w, cx| Tooltip::new(path.clone()).build(w, cx))
            .on_click(cx.listener(move |v, e: &gpui_kit::ClickEvent, w, cx| {
                w.focus(&v.focus, cx);
                v.click(&id, e.modifiers(), cx);
                // A conflicted file opens in the merge editor (MG-05), as Compact Changes did in Swift.
                if e.click_count() == 2 && let Some(m) = v.selected() {
                    let (wt, f) = { let m = m.read(cx); (m.path_str(), m.state.selected_file().cloned()) };
                    if let Some(f) = f.filter(|f| f.kind == Kind::Conflicted) { v.app.update(cx, |a, cx| a.emit(AppEvent::OpenMergeEditor { worktree: wt, path: f.path }, cx)) }
                }
            }))
            .context_menu(move |menu, window, cx| match me.upgrade() {
                Some(v) => build_menu(&v, menu, &menu_id, window, cx),
                None => menu,
            })
            .into_any_element()
    }

    /// Plain click selects one; ⌘ toggles; ⇧ extends from the last plain click (list order).
    fn click(&mut self, id: &str, mods: Modifiers, cx: &mut Context<Self>) {
        // Every list click ends a peek, the already-selected row too (its selection doesn't change).
        if let Some(b) = self.browser() { b.update(cx, |b, cx| b.end_peek(cx)) }
        let Some(m) = self.selected() else { return };
        let (listed, old, current) = {
            let s = &m.read(cx).state;
            (s.listed().iter().map(FileChange::id).collect::<Vec<_>>(), s.selected_file_ids.clone(), s.selected_file_id.clone())
        };
        let ids: HashSet<String> = if mods.platform {
            let mut ids = old.clone();
            if !ids.remove(id) { ids.insert(id.to_string()); }
            ids
        } else if mods.shift && let Some(a) = self.anchor.as_ref().and_then(|a| listed.iter().position(|x| x == a)) && let Some(b) = listed.iter().position(|x| x == id) {
            listed[a.min(b)..=a.max(b)].iter().cloned().collect()
        } else {
            self.anchor = Some(id.to_string());
            HashSet::from([id.to_string()])
        };
        let shown = shown_for(&listed, &old, &ids, current.as_deref());
        m.update(cx, |m, cx| m.select_files(shown, ids, cx));
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let (ids, cur) = { let s = &m.read(cx).state; (s.listed().iter().map(FileChange::id).collect::<HashSet<_>>(), s.selected_file_id.clone()) };
        let shown = cur.or_else(|| m.read(cx).state.listed().first().map(FileChange::id));
        m.update(cx, |m, cx| m.select_files(shown, ids, cx));
    }

    pub(crate) fn scroll_to_selected(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let Some(id) = m.read(cx).state.selected_file_id.clone() else { return };
        if let Some(i) = self.items(m.read(cx)).iter().position(|it| matches!(it, Item::File { f, .. } if f.id() == id)) {
            self.list_scroll.scroll_to_item(i, ScrollStrategy::Center);
        }
    }

    fn list_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let Some(m) = self.selected() else { return };
        let fs = m.read(cx).state.selected_files();
        match k.key.as_str() {
            // CH-23: anything unstaged in the selection gets staged; all staged → unstaged.
            "space" if !k.modifiers.modified() => {
                if fs.is_empty() { return }
                let unstaged: Vec<FileChange> = fs.iter().filter(|f| !f.staged).cloned().collect();
                if unstaged.is_empty() { self.unstage(fs, cx) } else { self.stage(unstaged, cx) }
            }
            // CH-33
            "backspace" | "delete" if !k.modifiers.modified() || k.modifiers.platform => self.ask_discard(fs, cx),
            "up" if !k.modifiers.modified() => self.move_file(-1, cx),
            "down" if !k.modifiers.modified() => self.move_file(1, cx),
            "a" if k.modifiers.platform => self.select_all(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

}

/// CH-18: for the selection, or for the clicked row when it's outside it.
fn build_menu(view: &Entity<ChangesView>, mut menu: PopupMenu, clicked: &str, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
        let Some(m) = view.read(cx).selected() else { return menu };
        let (fs, dir, lfs_state) = {
            let s = &m.read(cx).state;
            let fs = if s.selected_file_ids.contains(clicked) { s.selected_files() } else { s.files().iter().filter(|f| f.id() == clicked).cloned().collect() };
            (fs, s.path.clone(), s.lfs.clone())
        };
        if fs.is_empty() { return menu }
        let me = view.downgrade();
        let act = |label: String, f: Box<dyn Fn(&mut ChangesView, &mut Context<ChangesView>)>| {
            let me = me.clone();
            PopupMenuItem::new(label).on_click(move |_, _, cx| { me.update(cx, |v, cx| f(v, cx)).ok(); })
        };
        let unstaged: Vec<FileChange> = fs.iter().filter(|f| !f.staged && f.kind != Kind::Conflicted).cloned().collect();
        let staged: Vec<FileChange> = fs.iter().filter(|f| f.staged).cloned().collect();
        let discardable: Vec<FileChange> = unstaged.iter().filter(|f| f.submodule.is_none()).cloned().collect();
        if !unstaged.is_empty() { let u = unstaged.clone(); menu = menu.item(act(counted("Stage", unstaged.len()), Box::new(move |v, cx| v.stage(u.clone(), cx)))) }
        if !staged.is_empty() { let s = staged.clone(); menu = menu.item(act(counted("Unstage", staged.len()), Box::new(move |v, cx| v.unstage(s.clone(), cx)))) }
        if !discardable.is_empty() {
            let d = discardable.clone();
            menu = menu.item(act(format!("Discard Changes{}…", if d.len() == 1 { String::new() } else { format!(" {} Files", d.len()) }), Box::new(move |v, cx| v.ask_discard(d.clone(), cx))));
        }
        let mut paths: Vec<String> = fs.iter().map(|f| f.path.clone()).collect::<HashSet<_>>().into_iter().collect();
        paths.sort();
        let untracked = fs.iter().any(|f| f.kind == Kind::Untracked);
        let p2 = paths.clone();
        menu = menu.item(act(counted("Stash", paths.len()), Box::new(move |v, cx| v.stash(String::new(), untracked, p2.clone(), cx))));
        // SM-05
        let mut subs: Vec<String> = fs.iter().filter(|f| f.submodule.is_some()).map(|f| f.path.clone()).collect::<HashSet<_>>().into_iter().collect();
        subs.sort();
        if !subs.is_empty() {
            menu = menu.separator();
            if subs.len() == 1 {
                let s = subs[0].clone();
                let out = !submodules::is_checked_out(&dir, &s);
                menu = menu.item(act("Open Submodule".into(), Box::new(move |v, cx| v.open_submodule(&s, cx))).disabled(out));
            }
            let s = subs.clone();
            menu = menu.item(act(format!("Update to Recorded Commit{}", if subs.len() == 1 { "" } else { "s" }), Box::new(move |v, cx| v.update_submodules(s.clone(), cx))));
        }
        // LF-07 / LF-08
        let files: Vec<String> = fs.iter().filter(|f| f.submodule.is_none()).map(|f| f.path.clone()).collect::<HashSet<_>>().into_iter().collect();
        if lfs::installed() && !files.is_empty() {
            let me2 = me.clone();
            menu = menu.separator().submenu("Git LFS", window, cx, move |mut sm, _, _| {
                let me = me2.clone();
                let act = |label: String, f: Box<dyn Fn(&mut ChangesView, &mut Context<ChangesView>)>| {
                    let me = me.clone();
                    PopupMenuItem::new(label).on_click(move |_, _, cx| { me.update(cx, |v, cx| f(v, cx)).ok(); })
                };
                let mut files = files.clone();
                files.sort();
                if files.len() == 1 {
                    let p = files[0].clone();
                    if let Some(ext) = lfs::extension_pattern(&p) { let e = ext.clone(); sm = sm.item(act(format!("Track {ext} with LFS"), Box::new(move |v, cx| v.lfs_track(e.clone(), false, cx)))) }
                    let p2 = p.clone();
                    sm = sm.item(act("Track This File with LFS".into(), Box::new(move |v, cx| v.lfs_track(p2.clone(), true, cx))));
                }
                if lfs_state.locking_supported {
                    if files.len() == 1 { sm = sm.separator() }
                    let (locked, free): (Vec<String>, Vec<String>) = files.iter().cloned().partition(|p| lfs_state.locks.contains_key(p));
                    if !free.is_empty() { let f = free.clone(); sm = sm.item(act(counted("Lock", free.len()), Box::new(move |v, cx| v.lfs_lock(f.clone(), false, cx)))) }
                    if !locked.is_empty() { let l = locked.clone(); sm = sm.item(act(counted("Unlock", locked.len()), Box::new(move |v, cx| v.lfs_lock(l.clone(), true, cx)))) }
                }
                sm
            });
        }
        // FI-01/02: one tracked file (§14: not a deleted one either, blame of a missing file fails).
        if fs.len() == 1 && !matches!(fs[0].kind, Kind::Untracked | Kind::Added) && fs[0].submodule.is_none() && !(fs[0].kind == Kind::Deleted && !fs[0].staged) {
            let (a, b) = (fs[0].id(), fs[0].id());
            menu = menu.separator()
                .item(act("Blame".into(), Box::new(move |v, cx| v.open_panel(&a, super::inspector::FilePanel::Blame, cx))))
                .item(act("File History".into(), Box::new(move |v, cx| v.open_panel(&b, super::inspector::FilePanel::History, cx))));
        }
        let abs: Vec<std::path::PathBuf> = fs.iter().map(|f| dir.join(&f.path)).collect();
        let (abs1, abs2) = (abs.clone(), abs);
        let rel = fs.iter().map(|f| f.path.clone()).collect::<Vec<_>>().join("\n");
        menu.separator()
            .item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, _| { let _ = std::process::Command::new("/usr/bin/open").arg("-R").args(&abs1).spawn(); }))
            .item(PopupMenuItem::new("Open").on_click(move |_, _, cx| for p in &abs2 { cx.open_with_system(p) }))
            .item(PopupMenuItem::new(if fs.len() == 1 { "Copy Path" } else { "Copy Paths" }).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(rel.clone()))))
}

impl ChangesView {

    /// SS-04: set-aside changes live in History, on its Stashes scope.
    pub(crate) fn stashes_link(&self, n: usize, cx: &mut Context<Self>) -> AnyElement {
        div().id("stashes-link").flex().items_center().gap_2().px_3().h(px(32.)).flex_none().cursor_pointer().hover(|s| s.bg(theme::hover()))
            .tooltip(|w, cx| Tooltip::new("Show stashes in History").build(w, cx))
            .child(ui::icon("inbox"))
            .child(div().flex_1().child(if n == 1 { "1 stash".to_string() } else { format!("{n} stashes") }))
            .child(ui::icon("chevron-right").size(px(12.)).text_color(theme::text_faint()))
            .on_click(cx.listener(|v, _, _, cx| {
                let Some(worktree) = v.selected().map(|m| m.read(cx).path_str()) else { return };
                v.app.update(cx, |a, cx| a.emit(AppEvent::ShowStashes { worktree }, cx))
            }))
            .into_any_element()
    }

    pub(crate) fn open_panel(&mut self, id: &str, p: super::inspector::FilePanel, cx: &mut Context<Self>) {
        if let Some(m) = self.selected() { let id = id.to_string(); m.update(cx, |m, cx| m.select_file(Some(id), cx)) }
        self.panel.set(p);
        self.update(cx);
    }

    // MARK: LFS and submodule actions

    fn lfs_track(&mut self, pattern: String, literal: bool, cx: &mut Context<Self>) {
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.perform("Tracking with LFS", move |p| lfs::track(p, &pattern, literal), cx).detach()) }
    }

    /// LF-08: then the locks again at once, past the once-a-minute cap.
    fn lfs_lock(&mut self, files: Vec<String>, unlock: bool, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let env = { let w = m.read(cx); self.app.read(cx).git_auth(w.remote.as_ref(), &w.path_str()) };
        let e2 = env.clone();
        let t = m.update(cx, |m, cx| m.perform(if unlock { "Unlocking" } else { "Locking" }, move |p| lfs::lock(p, &files, unlock, &e2), cx));
        let weak = m.downgrade();
        cx.spawn(async move |_, cx| {
            t.await;
            let Ok((path, key)) = weak.read_with(cx, |m, _| (m.state.path.clone(), m.state.lfs.key(&m.state.path))) else { return };
            let ans = cx.background_executor().spawn(async move { lfs::load_locks(&path, &key, &env) }).await;
            weak.update(cx, |m, cx| { m.state.lfs.apply_locks(ans); cx.notify() }).ok();
        }).detach();
    }

    pub(crate) fn open_submodule(&mut self, sub: &str, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let p = m.read(cx).state.path.join(sub).to_string_lossy().into_owned();
        self.app.update(cx, |a, cx| a.add_repo(p, cx));
    }

    /// SM-03: asks first when a submodule has commits on no branch.
    pub(crate) fn update_submodules(&mut self, subs: Vec<String>, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let dir = m.read(cx).state.path.clone();
        cx.spawn(async move |this, cx| {
            let s2 = subs.clone();
            let stranded = cx.background_executor().spawn(async move { submodules::stranded(&dir, &s2) }).await;
            this.update(cx, |v, cx| if stranded.is_empty() { v.update_submodules_now(subs, cx) } else { v.confirm = Some(Confirm::Submodules(subs, stranded)); cx.notify() }).ok();
        }).detach();
    }

    pub(crate) fn update_submodules_now(&mut self, subs: Vec<String>, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let env = { let w = m.read(cx); self.app.read(cx).git_auth(w.remote.as_ref(), &w.path_str()) };
        m.update(cx, |m, cx| m.perform("Updating submodules", move |p| submodules::update(p, &subs, &env), cx).detach());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diff_follows_the_row_just_added() {
        let listed: Vec<String> = ["C:x", "S:a", "W:a", "W:b"].iter().map(|s| s.to_string()).collect();
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        assert_eq!(shown_for(&listed, &set(&["S:a"]), &set(&["S:a", "W:b"]), Some("S:a")).as_deref(), Some("W:b"));
        assert_eq!(shown_for(&listed, &set(&["S:a", "W:b"]), &set(&["S:a"]), Some("W:b")).as_deref(), Some("S:a"));
        assert_eq!(shown_for(&listed, &set(&["S:a", "W:b"]), &set(&["W:b"]), Some("W:b")).as_deref(), Some("W:b"));
        assert_eq!(shown_for(&listed, &set(&[]), &set(&[]), None), None);
    }
}
