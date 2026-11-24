//! The Changes tab (ChangesView.swift, FileInspector.swift, CommitAI.swift's UI, LFS.swift and Submodules.swift's
//! views, FileTree.swift). Owner: the Changes view agent (docs/gpui/VIEWS.md).
//!
//! Layout (CH-08): banners, then a split: the status list (Compact) or the worktree tree (Full) with the stashes link
//! and the commit box under it; on the right the Overview, the Full-mode file pane, the conflict pane or the diff pane.
//! The parts live in `changes/*.rs` as more `impl ChangesView` blocks.
//!
//! Performance rules kept from the Swift commits: everything this view loads is keyed (blame by diff uid, the
//! submodule pane and the Overview by status version, the Full tree by status version and change count) and runs
//! once per key; a load is a dropped `Task` when its key moves or the view stops showing it; nothing polls.
mod banners;
mod commit;
mod full;
mod inspector;
mod list;
mod overview;
mod pane;
pub mod stash;
pub mod tree;

use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Render, Styled as _, Subscription, Task, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, px,
    component::input::{InputEvent, InputState, TextareaState},
};
use loupe_core::git::{FileChange, Kind};
use loupe_forge::settings;

use crate::diff::{DiffView, HunkAction};
use crate::shell::keys::{CloseWindow, Commit, NextFile, PreviousFile};
use crate::state::{AppModel, DiffControls, Tab, ViewMode, WorktreeModel};
use crate::{theme, ui};

/// What the confirmation sheet asks about.
pub(crate) enum Confirm {
    /// CH-32: discard these files.
    Files(Vec<FileChange>),
    /// CH-36/37: discard these lines (indices into the shown diff).
    Lines(BTreeSet<usize>),
    /// SM-03: update submodules that have commits on no branch.
    Submodules(Vec<String>, Vec<(String, usize)>),
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Drag { List, Panel }

pub struct ChangesView {
    app: Entity<AppModel>,
    model: Option<(Entity<WorktreeModel>, Subscription)>,
    focus: FocusHandle,
    confirm_focus: FocusHandle,
    // Status list
    collapsed: HashSet<String>, // "<section>/<dir>", view state as in Swift (not persisted)
    anchor: Option<String>,     // ⇧-click's other end
    list_scroll: UniformListScrollHandle,
    shown_id: Option<String>,   // the file the view last showed: a new one leaves the Overview
    confirm: Option<Confirm>,
    // Commit box
    summary: Entity<InputState>,
    description: Entity<TextareaState>,
    stash_msg: Entity<InputState>,
    stash_open: bool,
    stash_untracked: bool,
    ai_paths: Option<std::collections::HashMap<String, String>>,
    generating: Option<(String, Task<()>)>,
    // Diff pane
    diff: Option<Entity<DiffView>>,
    pane: pane::State,
    panel: inspector::Panel,
    list_width: f32,
    drag: Option<(Drag, f32, f32)>, // what, mouse x at the start, width at the start
    // Overview and Full mode
    overview: bool,
    ov: overview::State,
    pending_reveal: Option<(String, u32)>,
    full: full::State,
    _subs: Vec<Subscription>,
}

pub(crate) fn pref_f32(key: &str, default: f32) -> f32 { settings::shared().f64(key).map_or(default, |v| v as f32) }

impl ChangesView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let summary = cx.new(|cx| InputState::new(window, cx).placeholder("Summary"));
        let description = cx.new(|cx| TextareaState::new(window, cx).placeholder("Description"));
        let stash_msg = cx.new(|cx| InputState::new(window, cx).placeholder("Message (optional)"));
        let subs = vec![
            cx.observe_in(&app, window, |v, _, w, cx| v.follow_selection(w, cx)),
            cx.subscribe_in(&summary, window, |v, _, e: &InputEvent, w, cx| match e {
                InputEvent::Change => v.message_edited(w, cx),
                InputEvent::PressEnter { .. } => if v.can_commit(cx) { v.commit(cx) },
                _ => {}
            }),
            cx.subscribe_in(&description, window, |v, _, e: &InputEvent, w, cx| match e {
                InputEvent::Change => v.message_edited(w, cx),
                InputEvent::PressEnter { secondary: true, .. } => v.commit(cx),
                _ => {}
            }),
            cx.subscribe_in(&stash_msg, window, |v, _, e: &InputEvent, _, cx| if let InputEvent::PressEnter { .. } = e { v.stash_all(cx) }),
        ];
        let mut v = Self {
            app, model: None, focus: cx.focus_handle(), confirm_focus: cx.focus_handle(), collapsed: HashSet::new(), anchor: None,
            list_scroll: UniformListScrollHandle::new(), shown_id: None, confirm: None,
            summary, description, stash_msg, stash_open: false, stash_untracked: true, ai_paths: loupe_forge::ai::installed(),
            generating: None, diff: None, pane: Default::default(), panel: inspector::Panel::load(),
            list_width: pref_f32("changesListWidth", 300.).clamp(220., 380.), drag: None,
            overview: false, ov: Default::default(), pending_reveal: None, full: Default::default(), _subs: subs,
        };
        // CM-08: the AI tools are looked up once, off the main thread.
        if v.ai_paths.is_none() {
            cx.spawn(async move |this, cx| {
                let p = cx.background_executor().spawn(async { loupe_forge::ai::scan() }).await;
                this.update(cx, |v, cx| { v.ai_paths = Some(p); cx.notify() }).ok();
            }).detach();
        }
        v.follow_selection(window, cx);
        v
    }

    pub(crate) fn selected(&self) -> Option<Entity<WorktreeModel>> { self.model.as_ref().map(|m| m.0.clone()) }

    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.app.read(cx).selected_model();
        if m.as_ref().map(|m| m.entity_id()) != self.model.as_ref().map(|x| x.0.entity_id()) {
            self.model = m.map(|m| (m.clone(), cx.observe_in(&m, window, |v, _, w, cx| v.sync(w, cx))));
            // Another worktree: its own list, panes and loads (the old ones are dropped with their keys).
            self.anchor = None;
            self.confirm = None;
            self.stash_open = false;
            self.pane = Default::default();
            self.ov = Default::default();
            self.full = Default::default();
            self.overview = false;
        }
        self.sync(window, cx);
    }

    /// Model → view, on every change of the selected worktree or the app (mode, side). Every load below is keyed,
    /// so a notify that changed nothing they depend on starts nothing.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_message(window, cx);
        self.update(cx);
    }

    /// The part that needs no window: the diff view, keyed loads, the toolbar.
    pub(crate) fn update(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else {
            self.diff = None;
            self.app.update(cx, |a, cx| a.set_controls(Tab::Changes, DiffControls::default(), cx));
            cx.notify();
            return;
        };
        let (sel, empty) = { let s = &m.read(cx).state; (s.selected_file_id.clone(), s.files().is_empty()) };
        if sel != self.shown_id {
            // CH-09: picking a file shows its diff; FI-04: another file drops the History pick.
            if sel.is_some() { self.overview = false }
            self.shown_id = sel.clone();
            self.panel.picked = None;
            if !self.app.read(cx).full_mode && let Some(f) = m.read(cx).state.selected_file() { self.full.follow(&f.path) }
        }
        if empty { self.overview = false }
        let full_mode = self.app.read(cx).full_mode;
        self.share_lfs_locks(&m, cx);
        self.update_diff(&m, cx);
        self.update_pane(&m, cx);
        self.update_panel(&m, cx);
        if self.overview { self.update_overview(&m, cx) }
        if full_mode { self.update_full(&m, cx) } else { self.leave_full(cx) }
        self.set_controls(empty, sel.is_some() || full_mode, cx);
        cx.notify();
    }

    /// LF-05/06: locks are asked once a minute per repository, and a worktree that refreshed while a sibling's request
    /// ran only got the placeholder (empty) answer. Take the repository's answer from a sibling that has it, so the
    /// badges show without another request. ponytail: belongs in WorktreeModel::load_lfs_locks (requested).
    fn share_lfs_locks(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let (repo, mine) = { let w = m.read(cx); (w.repo.clone(), w.state.lfs.clone()) };
        if !mine.used || mine.common_dir.is_none() { return }
        let app = self.app.read(cx);
        let best = app.worktrees_of(&repo).iter().filter_map(|w| app.model(&w.path)).filter(|o| o != m)
            .map(|o| o.read(cx).state.lfs.clone())
            .find(|l| l.common_dir == mine.common_dir && l.locking_supported);
        if let Some(l) = best && (l.locks != mine.locks || !mine.locking_supported) {
            m.update(cx, |w, _| { w.state.lfs.locks = l.locks; w.state.lfs.locking_supported = true });
        }
    }

    fn set_controls(&mut self, empty: bool, has_file: bool, cx: &mut Context<Self>) {
        let c = if empty { DiffControls::default() } else {
            let me = cx.entity().downgrade();
            let set: Rc<dyn Fn(ViewMode, &mut Window, &mut gpui_kit::App)> = Rc::new(move |mode, _, cx| {
                me.update(cx, |v, cx| { v.overview = mode == ViewMode::Overview; v.update(cx) }).ok();
            });
            DiffControls {
                full: !self.overview && has_file,
                view: Some((vec![ViewMode::Overview, ViewMode::Files], if self.overview { ViewMode::Overview } else { ViewMode::Files }, set)),
            }
        };
        self.app.update(cx, |a, cx| a.set_controls(Tab::Changes, c, cx));
    }

    // MARK: Actions shared by the list, the header and the menus

    pub(crate) fn stage(&mut self, fs: Vec<FileChange>, cx: &mut Context<Self>) {
        // §14: a conflicted file staged with its markers in would count as resolved; Mark Resolved is the way.
        let fs: Vec<FileChange> = fs.into_iter().filter(|f| f.kind != Kind::Conflicted).collect();
        if fs.is_empty() { return }
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.stage(fs, cx).detach()) }
    }

    pub(crate) fn unstage(&mut self, fs: Vec<FileChange>, cx: &mut Context<Self>) {
        if fs.is_empty() { return }
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.unstage(fs, cx).detach()) }
    }

    /// CH-32/33/34: asks first. Submodules (git restore leaves them) and conflicted files (§14: restore fails on them)
    /// are left out.
    pub(crate) fn ask_discard(&mut self, fs: Vec<FileChange>, cx: &mut Context<Self>) {
        let fs: Vec<FileChange> = fs.into_iter().filter(|f| !f.staged && f.submodule.is_none() && f.kind != Kind::Conflicted).collect();
        if fs.is_empty() { return }
        self.confirm = Some(Confirm::Files(fs));
        cx.notify();
    }

    /// A hunk or line action from the diff: discard asks first (the same sheet as files, §14 "two styles").
    pub(crate) fn hunk_action(&mut self, a: HunkAction, lines: BTreeSet<usize>, cx: &mut Context<Self>) {
        if a == HunkAction::Discard { self.confirm = Some(Confirm::Lines(lines)); cx.notify(); return }
        self.apply_lines(a, lines, cx)
    }

    fn apply_lines(&mut self, a: HunkAction, lines: BTreeSet<usize>, cx: &mut Context<Self>) {
        let action = match a { HunkAction::Stage => loupe_core::model::HunkAction::Stage, HunkAction::Unstage => loupe_core::model::HunkAction::Unstage, HunkAction::Discard => loupe_core::model::HunkAction::Discard };
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.apply_lines(action, lines, cx).detach()) }
    }

    fn confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.confirm.take() else { return };
        match c {
            Confirm::Files(fs) => if let Some(m) = self.selected() { m.update(cx, |m, cx| m.discard(fs, cx).detach()) },
            Confirm::Lines(lines) => self.apply_lines(HunkAction::Discard, lines, cx),
            Confirm::Submodules(subs, _) => self.update_submodules_now(subs, cx),
        }
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if !self.can_commit(cx) { return }
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.commit(cx).detach()) }
    }

    /// CH-16 / DV-23: ±1 through the list, clamped; nothing selected, nothing happens.
    fn move_file(&mut self, d: isize, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        let next = {
            let s = &m.read(cx).state;
            let list = s.listed();
            let Some(i) = list.iter().position(|f| Some(f.id()) == s.selected_file_id) else { return };
            list[(i as isize + d).clamp(0, list.len() as isize - 1) as usize].id()
        };
        self.anchor = Some(next.clone());
        m.update(cx, |m, cx| m.select_file(Some(next), cx));
        self.scroll_to_selected(cx);
    }

    // MARK: Splitter drags (the list and the History panel)

    fn drag_to(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some((what, start, w0)) = self.drag else { return };
        match what {
            Drag::List => self.list_width = (w0 + x - start).clamp(220., 380.),
            Drag::Panel => self.panel.width = (w0 + x - start).clamp(260., 700.),
        }
        cx.notify();
    }

    fn end_drag(&mut self) {
        let Some((what, ..)) = self.drag.take() else { return };
        let mut p = settings::shared();
        match what {
            Drag::List => p.set_f64("changesListWidth", self.list_width as f64),
            Drag::Panel => p.set_f64("filePanelWidth", self.panel.width as f64),
        }
    }

    /// FT-09 / FI-05: a 1 pt line with a 9 pt grip; double-click goes back to the default.
    pub(crate) fn handle(&self, what: Drag, cx: &mut Context<Self>) -> AnyElement {
        div().id(if what == Drag::List { "split-list" } else { "split-panel" }).w(px(9.)).mx(px(-4.)).h_full().flex_none().flex().justify_center()
            .cursor(gpui_kit::CursorStyle::ResizeLeftRight)
            .child(div().w(px(1.)).h_full().bg(theme::border()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |v, e: &gpui_kit::MouseDownEvent, _, cx| {
                if e.click_count == 2 {
                    match what { Drag::List => v.list_width = 300., Drag::Panel => v.panel.width = 340. }
                    v.drag = Some((what, 0., 0.));
                    v.end_drag();
                    cx.notify();
                    return;
                }
                let w = if what == Drag::List { v.list_width } else { v.panel.width };
                v.drag = Some((what, f32::from(e.position.x), w));
                cx.stop_propagation();
            }))
            .into_any_element()
    }
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(m) = self.selected() else { return div().size_full().into_any_element() };
        let full_mode = self.app.read(cx).full_mode;
        let stash_count = m.read(cx).state.stash_count;
        let left = div().flex().flex_col().w(px(self.list_width)).flex_none().h_full().min_h_0()
            .child(if full_mode { self.render_tree(&m, cx) } else { self.render_list(&m, window, cx) })
            .when(stash_count > 0, |d| d.child(self.stashes_link(stash_count, cx)))
            .child(ui::divider())
            .child(self.render_commit_box(&m, window, cx));
        let right = div().flex_1().min_w(px(360.)).h_full().min_h_0().p_2()
            .child(if self.overview { self.render_overview(&m, cx) }
                   else if full_mode { self.render_full_pane(&m, window, cx) }
                   else { self.render_pane(&m, window, cx) });
        div().id("changes").relative().flex().flex_col().size_full().font_weight(FontWeight::NORMAL)
            .on_action(cx.listener(|v, _: &Commit, _, cx| v.commit(cx)))
            .on_action(cx.listener(|v, _: &NextFile, _, cx| v.move_file(1, cx)))
            .on_action(cx.listener(|v, _: &PreviousFile, _, cx| v.move_file(-1, cx)))
            // CH-42: ⌘W closes the open tab in Full mode (nothing when none is open); elsewhere it closes the window.
            .on_action(cx.listener(|v, a: &CloseWindow, w, cx| {
                if v.app.read(cx).full_mode { v.close_tab(cx) } else { let _ = a; w.remove_window() }
            }))
            .on_mouse_move(cx.listener(|v, e: &gpui_kit::MouseMoveEvent, _, cx| {
                if v.drag.is_some() { if e.pressed_button == Some(MouseButton::Left) { v.drag_to(f32::from(e.position.x), cx) } else { v.end_drag() } }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, _, _, _| v.end_drag()))
            // The confirmation sheet answers Return and Esc wherever focus is in this view.
            .capture_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, w, cx| {
                // DV-32: ⌘F finds in the shown diff even while the list has focus (Swift's window-wide key monitor).
                if e.keystroke.key == "f" && e.keystroke.modifiers.platform && !e.keystroke.modifiers.shift && v.confirm.is_none() {
                    let d = if v.app.read(cx).full_mode { v.full.view() } else { v.diff.clone() };
                    if let Some(d) = d { d.update(cx, |d, cx| d.open_find(w, cx)); cx.stop_propagation(); return }
                }
                // SS-01: Esc closes the stash popover, as a popover does.
                if v.stash_open && e.keystroke.key == "escape" { v.stash_open = false; cx.notify(); cx.stop_propagation(); return }
                if v.confirm.is_none() { return }
                match e.keystroke.key.as_str() {
                    "escape" => { v.confirm = None; cx.notify() }
                    "enter" => v.confirmed(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .children(self.render_banners(&m, cx))
            .child(div().flex().flex_1().min_h_0().child(left).child(self.handle(Drag::List, cx)).child(right))
            .children(self.render_confirm(window, cx))
            .into_any_element()
    }
}

/// "Stage" / "Stage 3 Files": the menus' counted labels.
pub(crate) fn counted(verb: &str, n: usize) -> String { if n == 1 { verb.into() } else { format!("{verb} {n} Files") } }

