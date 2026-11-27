//! Window, sidebar, tabs, toolbar, quick switcher, toasts (App.swift, WorktreeView in ChangesView.swift).
pub mod branches;
pub mod keys;
pub mod model;
pub mod sidebar;
pub mod switcher;
pub mod worktrees;

use std::time::Duration;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, ExternalPaths, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
    component::{
        TitleBar,
        input::{InputEvent, InputState},
    },
};

use crate::state::{AppEvent, AppModel, WorktreeModel};
use crate::{theme, ui, views};
use keys::*;
use model::{Status, Worktree, branch_label, last_component, row_title, tilde};

pub use crate::state::{DiffControls, Tab, ViewMode};

struct Toast { id: u64, text: SharedString, undoable: bool }

pub struct Shell {
    /// The app's model; named `workspace` since the shell's first version.
    pub workspace: Entity<AppModel>,
    /// The Changes / History / Pull Requests views (views::changes, views::history, views::pulls).
    pub views: [AnyView; 3],
    sidebar_open: bool,
    focus: FocusHandle,
    filter: Entity<InputState>,
    switcher: Option<Entity<switcher::Switcher>>,
    branches: Option<Entity<branches::BranchPicker>>,
    toast: Option<Toast>,
    toast_timer: Option<Task<()>>,
    next_toast: u64,
    /// The selected worktree's model, observed so the toolbar and alert follow its busy / error / ahead / behind.
    watched: Option<(Entity<WorktreeModel>, Subscription)>,
    modal: Option<sidebar::Modal>,
    swipe: Option<sidebar::Swipe>,
    slide: Option<sidebar::Slide>,
    slide_timer: Option<Task<()>>,
    next_slide: u64,
    page_width: std::rc::Rc<std::cell::Cell<f32>>,
    /// The shown space's index, to slide when a dot click (or a jump) changes it; `settling`: a swipe did.
    last_space: usize,
    settling: bool,
    _subs: Vec<Subscription>,
}

impl Shell {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter worktrees"));
        let subs = vec![
            cx.observe(&app, |s, _, cx| { s.watch_selected(cx); s.follow_space(cx); s.update_undo_title(cx); cx.notify() }),
            cx.subscribe_in(&app, window, |s, _, e: &AppEvent, w, cx| match e {
                AppEvent::Toast { text, undoable } => s.show_toast(text.clone(), *undoable, cx),
                AppEvent::NewWorktree { repo, base } => worktrees::open_add_worktree(&s.workspace, repo.clone(), base.clone(), w, cx),
                _ => {}
            }),
            cx.subscribe(&filter, |_, _, e: &InputEvent, cx| if matches!(e, InputEvent::Change) { cx.notify() }),
        ];
        let views = [
            cx.new(|cx| views::changes::ChangesView::new(app.clone(), window, cx)).into(),
            cx.new(|cx| views::history::HistoryView::new(app.clone(), window, cx)).into(),
            cx.new(|cx| views::pulls::PullsView::new(app.clone(), window, cx)).into(),
        ];
        let mut subs = subs;
        subs.extend(views::install(&app, window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut s = Self { workspace: app, views, sidebar_open: true, focus, filter, switcher: None, branches: None,
                           toast: None, toast_timer: None, next_toast: 0, watched: None, modal: None, swipe: None, slide: None,
                           slide_timer: None, next_slide: 0, page_width: Default::default(), last_space: 0, settling: false, _subs: subs };
        s.last_space = s.space_index(cx);
        s.watch_selected(cx);
        s
    }

    fn watch_selected(&mut self, cx: &mut Context<Self>) {
        let m = self.workspace.read(cx).selected_model();
        if m.as_ref().map(|m| m.entity_id()) == self.watched.as_ref().map(|w| w.0.entity_id()) { return }
        self.watched = m.map(|m| { let sub = cx.observe(&m, |s, _, cx| { s.update_undo_title(cx); cx.notify() }); (m, sub) });
    }

    fn space_index(&self, cx: &App) -> usize {
        let ws = self.workspace.read(cx);
        ws.spaces.iter().position(|s| s.id == ws.current_space).unwrap_or(0)
    }

    /// A dot click, ⌘K or a PR checkout changed the space: slide the pages from the old one (a swipe already did).
    fn follow_space(&mut self, cx: &mut Context<Self>) {
        let ix = self.space_index(cx);
        if ix == self.last_space { return }
        let old = std::mem::replace(&mut self.last_space, ix);
        if std::mem::take(&mut self.settling) { return }
        let step = ix as isize - old as isize;
        self.start_slide(old, step, step as f32 * self.page_width.get(), cx);
    }

    /// Edit › "Undo <label>" follows the selected worktree's stack.
    fn update_undo_title(&mut self, cx: &mut Context<Self>) {
        let t = self.selected().and_then(|m| m.read(cx).undo_title()).unwrap_or_else(|| "Undo".into());
        keys::set_undo_title(t, cx);
    }

    /// ⌘Z (UndoCommands): a text field undoes its own typing (its binding wins); with nothing to undo, a beep;
    /// a refused undo explains why and offers a revert when the commit is already pushed.
    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.selected().filter(|m| m.read(cx).undo_title().is_some()) else { return beep() };
        let t = m.update(cx, |m, cx| m.undo_or_refusal(cx));
        cx.spawn(async move |this, cx| {
            let Some(r) = t.await else { return };
            this.update(cx, |s, cx| { s.modal = Some(sidebar::Modal::CantUndo { message: r.message, revert: r.revert }); cx.notify() }).ok();
        }).detach();
    }

    fn selected(&self) -> Option<Entity<WorktreeModel>> { self.watched.as_ref().map(|w| w.0.clone()) }

    /// Runs `f` on the selected worktree's model (toolbar and menu actions).
    fn on_selected(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut WorktreeModel, &mut Context<WorktreeModel>)) {
        if let Some(m) = self.selected() { m.update(cx, f) }
    }

    fn open_branches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { return };
        if self.branches.take().is_some() { cx.notify(); return }
        let p = cx.new(|cx| branches::BranchPicker::new(m, window, cx));
        self._subs.push(cx.subscribe_in(&p, window, |this, _, _: &gpui_kit::DismissEvent, window, cx| {
            this.branches = None;
            this.focus.focus(window, cx);
            cx.notify();
        }));
        self.branches = Some(p);
        cx.notify();
    }

    /// "Discard of a.txt · Undo" after a destructive action (`undoable`), "Undone: …" after an undo; gone after
    /// six seconds. One timer, no animation: nothing runs while it shows.
    pub fn show_toast(&mut self, text: impl Into<SharedString>, undoable: bool, cx: &mut Context<Self>) {
        self.next_toast += 1;
        let id = self.next_toast;
        self.toast = Some(Toast { id, text: text.into(), undoable });
        self.toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            this.update(cx, |s, cx| if s.toast.as_ref().is_some_and(|t| t.id == id) { s.toast = None; cx.notify() }).ok();
        }));
        cx.notify();
    }

    pub fn hide_toast(&mut self, cx: &mut Context<Self>) { self.toast = None; self.toast_timer = None; cx.notify() }

    fn set_tab(&mut self, t: Tab, cx: &mut Context<Self>) { self.workspace.update(cx, |a, cx| a.set_tab(t, cx)) }

    fn filter_text(&self, cx: &App) -> String { self.filter.read(cx).value().to_string() }

    /// The worktrees the shown space lists, in order: what ⌥⌘1…9 pick from.
    fn visible(&self, cx: &App) -> Vec<(String, Worktree)> {
        let ws = self.workspace.read(cx);
        visible_in(ws, ws.repo_paths(), &self.filter_text(cx))
    }

    fn select_nth(&mut self, n: usize, cx: &mut Context<Self>) {
        if let Some((_, w)) = self.visible(cx).into_iter().nth(n) { self.workspace.update(cx, |ws, cx| ws.select(Some(w.path), cx)) }
    }

    fn open_repository(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Open".into()) });
        let ws = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                ws.update(cx, |ws, cx| for p in paths { ws.add_repo(p.to_string_lossy().into_owned(), cx) });
            }
        }).detach();
    }

    fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_some() { return }
        let s = cx.new(|cx| switcher::Switcher::new(self.workspace.clone(), window, cx));
        self._subs.push(cx.subscribe_in(&s, window, |this, _, _: &gpui_kit::DismissEvent, window, cx| {
            this.switcher = None;
            this.focus.focus(window, cx);
            cx.notify();
        }));
        self.switcher = Some(s);
        cx.notify();
    }

    fn title_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let wt = ws.selected_worktree().cloned();
        let tab_ix = ws.tab.index();
        let (controls, full_mode) = (ws.controls().clone(), ws.full_mode);
        // The model's own numbers: fresher than the sidebar's copy, and busy shows which operation runs.
        let (behind, ahead, busy, branch) = self.selected().map(|m| { let s = &m.read(cx).state; (s.behind, s.ahead, s.busy.clone(), (s.branch.clone(), s.head_oid.clone())) })
            .unwrap_or_default();
        let me = cx.entity();
        let remote_button = |id: &'static str, icon: &str, label: &str, action: Box<dyn gpui_kit::Action>| {
            let running = busy.as_deref() == Some(label);
            title_button(id, icon)
                .when(running, |d| d.bg(theme::selected()))
                .when(busy.is_some(), |d| d.opacity(if running { 1. } else { 0.4 }))
                .when(busy.is_none(), |d| d.on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)))
        };
        TitleBar::new().h(px(42.)).bg(theme::sidebar()).border_b_0()
            .child(div().flex().items_center().size_full().pr_2()
                // Left: the sidebar toggle, then fetch / pull / push beside the branch they act on.
                .child(div().flex().flex_1().items_center().gap_1().min_w_0()
                    .child(title_button("toggle-sidebar", "panel-left").on_click(cx.listener(|s, _, _, cx| { s.sidebar_open = !s.sidebar_open; cx.notify() })))
                    // Nothing selected: the window's own title, as Swift shows it (SH-04).
                    .when(wt.is_none(), |d| d.child(div().pl_2().font_weight(FontWeight::SEMIBOLD).child("Loupe")))
                    .when_some(wt.clone(), |d, wt| d
                        .child(div().w(px(10.)))
                        .child(branch_button(&wt, branch))
                        .child(remote_button("fetch", "refresh-cw", "Fetching", Box::new(Fetch)))
                        .child(remote_button("pull", "arrow-down", "Pulling", Box::new(Pull))
                            .when(behind > 0, |d| d.child(ui::kbd(behind.to_string()).mr_1())))
                        .child(remote_button("push", "arrow-up", "Pushing", Box::new(Push))
                            .when(ahead > 0, |d| d.child(ui::kbd(ahead.to_string()).mr_1())))
                        // What's running, in words: the only progress a git operation gives.
                        .when_some(busy.clone(), |d, b| d.child(div().pl_1().text_size(px(12.)).text_color(theme::text_muted()).child(format!("{b}…"))))))
                // Center: the view switch.
                .when(wt.is_some(), |d| d.child(ui::segmented("tabs", &["Changes", "History", "Pull Requests"], tab_ix,
                    move |i, _, cx| me.update(cx, |s, cx| s.set_tab(Tab::ALL[i], cx)))))
                // Right: Compact/Full and the view mode while a diff shows, then search and settings.
                .child(div().flex().flex_1().items_center().justify_end().gap_1()
                    .when(controls.full, |d| {
                        let app = self.workspace.clone();
                        d.child(ui::segmented("mode", &["Compact", "Full"], full_mode as usize,
                            move |i, _, cx| app.update(cx, |a, cx| a.set_full_mode(i == 1, cx))))
                    })
                    .when_some(controls.view.clone(), |d, (modes, cur, set)| {
                        let names: Vec<&str> = modes.iter().map(|m| match m { ViewMode::Overview => "Overview", ViewMode::Files => "Files", ViewMode::Conversation => "Conversation" }).collect();
                        let sel = modes.iter().position(|m| *m == cur).unwrap_or(0);
                        d.child(ui::segmented("view", &names, sel, move |i, w, cx| set(modes[i], w, cx)))
                    })
                    .child(title_button("go-to", "search").on_click(|_, w, cx| w.dispatch_action(Box::new(GoToWorktree), cx)))
                    .child(title_button("settings", "settings").on_click(|_, w, cx| w.dispatch_action(Box::new(OpenSettings), cx)))))
            .into_any_element()
    }

    fn content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let panel = div().size_full().bg(theme::canvas()).border_l_1().border_t_1().border_color(theme::border())
            .rounded_tl(theme::RADIUS_CARD).overflow_hidden();
        if ws.selected_worktree().is_none() {
            return panel.child(ui::empty_state("git-branch", "No Worktree Selected", "Open a repository to review its changes and pull requests.")
                .child(div().pt_2().child(ui::pill("open-repo-empty", "Open Repository…").on_click(|_, w, cx| w.dispatch_action(Box::new(OpenRepository), cx))))).into_any_element();
        };
        panel.child(self.views[ws.tab.index()].clone()).into_any_element()
    }
}

/// `repos`' listed worktrees in sidebar order: inside expanded repos, or every repo while the filter matches.
pub(crate) fn visible_in(ws: &AppModel, repos: &[String], filter: &str) -> Vec<(String, Worktree)> {
    let q = filter.to_lowercase();
    let tokens: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
    repos.iter().flat_map(|r| {
        // Filtering looks inside collapsed repos too: a match shouldn't hide behind a chevron.
        let open = !tokens.is_empty() || !ws.collapsed.contains(r);
        ws.worktrees_of(r).iter().filter(move |_| open).filter(|w| {
            let hay = format!("{} {} {}", last_component(r), w.branch.as_deref().unwrap_or(""), w.name()).to_lowercase();
            tokens.iter().all(|t| hay.contains(t))
        }).map(|w| (r.clone(), w.clone())).collect::<Vec<_>>()
    }).collect()
}

/// Focus is in a text input (gpui-component's "Input" key context).
fn in_text(w: &Window) -> bool { w.context_stack().iter().any(|c| c.contains("Input")) }

/// NSBeep: ⌘Z with nothing to undo.
fn beep() {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "AppKit", kind = "framework")]
        unsafe extern "C" { fn NSBeep(); }
        unsafe { NSBeep() }
    }
}

/// A row's look, shared by the sidebar and the quick switcher: icon, title over branch, merged/ahead/behind,
/// change count, lock.
/// `lead`: the quick switcher's repo name, first.
pub(crate) fn worktree_row_body(id: SharedString, repo: &str, wt: &Worktree, st: Status, selected: bool, lead: Option<String>) -> gpui_kit::Stateful<gpui_kit::Div> {
    let main = wt.path == repo;
    // Not on the main worktree: the repo's own folder often sits on a long-lived branch main contains.
    let merged = st.merged && !main;
    let (icon, color) = if main { ("folder", theme::text_muted()) } else if merged { ("git-merge", theme::purple()) } else { ("git-branch", theme::blue()) };
    ui::list_row(id, selected).h(px(40.)).pl_2p5()
        .when_some(lead, |d, l| d.child(div().w(px(130.)).flex_none().truncate().text_color(theme::text_muted()).child(l)))
        .child(ui::icon(icon).text_color(color))
        .child(div().flex().flex_col().flex_1().min_w_0()
            .child(div().truncate().child(row_title(wt, repo)))
            .child(div().truncate().text_size(px(11.)).text_color(theme::text_faint()).child(branch_label(wt))))
        .child(div().flex().flex_none().items_center().gap_1p5().text_size(px(11.))
            .when(merged, |d| d.child(ui::badge_tinted("Merged", theme::purple())))
            .when(st.ahead > 0, |d| d.child(div().text_color(theme::text_muted()).child(format!("↑{}", st.ahead))))
            .when(st.behind > 0, |d| d.child(div().text_color(theme::text_muted()).child(format!("↓{}", st.behind))))
            .when(st.changes > 0, |d| d.child(ui::badge(st.changes.to_string())))
            .when(wt.is_locked, |d| d.child(ui::icon("lock").size(px(11.)))))
}

fn title_button(id: &'static str, icon: &str) -> gpui_kit::Stateful<gpui_kit::Div> {
    // Keep the press: without this a click with a little drift would start a window drag.
    ui::icon_button(id, icon).on_mouse_down(MouseButton::Left, |_, w, _| w.prevent_default())
}

/// The branch and the worktree's folder, two lines; a click opens the branch picker.
/// `live`: the worktree model's branch and HEAD (fresher than the worktree list's).
fn branch_button(wt: &Worktree, live: (Option<String>, String)) -> impl IntoElement {
    let (branch, head) = if live.1.is_empty() { (wt.branch.clone(), wt.head.clone()) } else { live };
    let title = if head.is_empty() { String::new() } else { branch.unwrap_or_else(|| "detached HEAD".into()) };
    div().id("branch").flex().flex_col().justify_center().h(px(34.)).px_2().max_w(px(300.)).rounded(px(7.)).cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .on_mouse_down(MouseButton::Left, |_, w, _| w.prevent_default())
        .on_click(|_, w, cx| w.dispatch_action(Box::new(ShowBranches), cx))
        .child(div().flex().items_center().gap_1()
            .child(div().truncate().text_size(px(13.)).line_height(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(ui::icon("chevron-down").size(px(11.)).text_color(theme::text_faint())))
        .child(div().truncate().text_size(px(11.)).line_height(px(14.)).text_color(theme::text_faint()).child(tilde(&wt.path)))
}

pub(crate) fn open_in(bundle: &str, path: &str) {
    let _ = std::process::Command::new("/usr/bin/open").args(["-b", bundle, path]).spawn();
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = self.title_bar(cx);
        let content = self.content(cx);
        let body = if self.sidebar_open {
            let sidebar = self.sidebar(cx);
            ui::h_resizable("shell-split")
                .child(ui::resizable_panel().size(px(260.)).size_range(px(200.)..px(360.)).child(sidebar))
                .child(ui::resizable_panel().child(content))
                .into_any_element()
        } else {
            content.into_any_element()
        };
        // An overlay that closed took the focus with it: with nothing focused, the toolbar's dispatched actions
        // (settings, fetch, pull, push) reach no listener. Give it back to the window.
        if window.focused(cx).is_none() { self.focus.focus(window, cx) }
        div().id("shell").track_focus(&self.focus).key_context("Loupe").relative().flex().flex_col().size_full()
            .bg(theme::sidebar()).text_color(theme::text()).font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .capture_key_down(cx.listener(|s, e, _, cx| s.alert_keys(e, cx)))
            .on_action(cx.listener(|s, _: &OpenRepository, _, cx| s.open_repository(cx)))
            .on_action(cx.listener(|s, _: &GoToWorktree, w, cx| s.open_switcher(w, cx)))
            .on_action(cx.listener(|s, _: &ToggleSidebar, _, cx| { s.sidebar_open = !s.sidebar_open; cx.notify() }))
            .on_action(cx.listener(|s, _: &ChangesTab, _, cx| s.set_tab(Tab::Changes, cx)))
            .on_action(cx.listener(|s, _: &HistoryTab, _, cx| s.set_tab(Tab::History, cx)))
            .on_action(cx.listener(|s, _: &PullsTab, _, cx| s.set_tab(Tab::Pulls, cx)))
            .on_action(cx.listener(|s, _: &FullMode, _, cx| s.workspace.update(cx, |a, cx| { let on = !a.full_mode; a.set_full_mode(on, cx) })))
            .on_action(cx.listener(|s, _: &Fetch, _, cx| s.on_selected(cx, |m, cx| m.fetch(cx).detach())))
            .on_action(cx.listener(|s, _: &Pull, _, cx| s.on_selected(cx, |m, cx| m.pull(cx).detach())))
            .on_action(cx.listener(|s, _: &Push, _, cx| s.on_selected(cx, |m, cx| m.push(cx).detach())))
            .on_action(cx.listener(|s, _: &ShowBranches, w, cx| s.open_branches(w, cx)))
            // ⌘R: status here; History and the PR views reload on the event. Worktrees aren't re-listed (SH-38).
            .on_action(cx.listener(|s, _: &Refresh, _, cx| {
                s.on_selected(cx, |m, cx| m.refresh(cx));
                s.workspace.update(cx, |a, cx| a.emit(AppEvent::Refresh, cx));
            }))
            // ⌘Z in a text field never gets here (keys::NOT_IN_TEXT); Edit › Undo clicked while one has focus does,
            // and goes to the field, as Swift's undo: went to the first responder.
            .on_action(cx.listener(|s, _: &Undo, w, cx| if in_text(w) { w.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx) } else { s.undo(cx) }))
            // No git redo (as in the Swift app); text fields take ⇧⌘Z themselves.
            .on_action(|_: &Redo, w, cx| if in_text(w) { w.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx) } else { beep() })
            .on_action(cx.listener(|s, _: &NewWorktree, w, cx| {
                let repo = s.workspace.read(cx).selected_model().map(|m| m.read(cx).repo.clone());
                if let Some(r) = repo { worktrees::open_add_worktree(&s.workspace, r, String::new(), w, cx) }
            }))
            .on_action(|_: &ToggleFullScreen, w, _| w.toggle_fullscreen())
            .on_action(cx.listener(|s, _: &SelectWorktree1, _, cx| s.select_nth(0, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree2, _, cx| s.select_nth(1, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree3, _, cx| s.select_nth(2, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree4, _, cx| s.select_nth(3, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree5, _, cx| s.select_nth(4, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree6, _, cx| s.select_nth(5, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree7, _, cx| s.select_nth(6, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree8, _, cx| s.select_nth(7, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree9, _, cx| s.select_nth(8, cx)))
            .on_action(cx.listener(|s, _: &OpenSettings, _, cx| views::settings::open(s.workspace.clone(), cx)))
            .on_action(|_: &CloseWindow, w, _| w.remove_window())
            .on_action(|_: &Minimize, w, _| w.minimize_window())
            .on_action(|_: &Zoom, w, _| w.zoom_window())
            // A folder dropped anywhere on the window opens as a repository.
            .on_drop(cx.listener(|s, paths: &ExternalPaths, _, cx| s.workspace.update(cx, |w, cx| {
                for p in paths.paths() { w.add_repo(p.to_string_lossy().into_owned(), cx) }
            })))
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
            .when_some(self.toast.as_ref(), |d, t| d.child(
                div().absolute().bottom(px(18.)).left_0().right_0().flex().justify_center().child(
                    ui::toast(if t.undoable { "circle-check" } else { "undo-2" }, t.text.clone())
                        .when(t.undoable, |d| d.child(ui::link_button("toast-undo", "Undo")
                            .on_click(|_, w, cx| w.dispatch_action(Box::new(Undo), cx))))
                        .child(div().id("toast-close").cursor_pointer().on_click(cx.listener(|s, _, _, cx| s.hide_toast(cx)))
                            .child(ui::icon("x").size(px(12.)).text_color(theme::text_faint()))))))
            .children(self.workspace.read(cx).overlay.clone())
            .children(self.branches.clone())
            .children(self.switcher.clone())
            .children(self.modal_view(cx))
            .children(self.alert(cx))
    }
}

impl Shell {
    /// Return or Esc on the alert, Esc on a dialog: what NSAlert and confirmationDialog do. Here at the root,
    /// because nothing inside them has focus.
    fn alert_keys(&mut self, e: &gpui_kit::KeyDownEvent, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        if !matches!(key, "enter" | "escape") || e.keystroke.modifiers.modified() { return }
        let model = self.selected().filter(|m| m.read(cx).state.error.is_some());
        if let Some(m) = model { m.update(cx, |m, cx| m.clear_error(cx)) }
        else if self.workspace.read(cx).error.is_some() { self.workspace.update(cx, |a, cx| { a.error = None; cx.notify() }) }
        else if key == "escape" && self.modal.is_some() { self.modal = None; cx.notify() }
        else if matches!(self.modal, Some(sidebar::Modal::CantUndo { revert: None, .. })) { self.modal = None; cx.notify() }
        else { return }
        cx.stop_propagation();
    }

    /// The "Git" alert (SH-06, SH-25): the selected worktree's error, else the app's. OK clears it.
    fn alert(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = self.selected().filter(|m| m.read(cx).state.error.is_some());
        // Swift: app-level failures are titled "Error", a worktree's git failures "Git".
        let title = if model.is_some() { "Git" } else { "Error" };
        let text = match &model { Some(m) => m.read(cx).state.error.clone()?, None => self.workspace.read(cx).error.clone()? };
        let app = self.workspace.clone();
        let ok = move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| match &model {
            Some(m) => m.update(cx, |m, cx| m.clear_error(cx)),
            None => app.update(cx, |a, cx| { a.error = None; cx.notify() }),
        };
        Some(div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .child(ui::card().w(px(420.)).p_4().gap_3().bg(theme::raised())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().id("alert-text").max_h(px(300.)).overflow_y_scroll().text_size(px(12.)).text_color(theme::text_muted()).child(text))
                .child(div().flex().justify_end().child(ui::pill_primary("alert-ok", "OK", theme::blue()).on_click(ok))))
            .into_any_element())
    }
}

/// The main window, 1440×900 by default, at least 860×560, with Loupe's own title bar.
pub fn open_main_window(app: Entity<AppModel>, cx: &mut App) {
    use gpui_kit::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, size};
    // A LOUPE_STRESS run fills the visible frame (Swift's StressDriver: setFrame(visibleFrame)). It has to start
    // there: gpui can't move a window later, and a centered window resized to the screen ran under the Dock.
    let bounds = match app.read(cx).scenario.is_some().then(|| cx.primary_display()).flatten() {
        Some(d) => d.visible_bounds(),
        None => Bounds::centered(None, size(px(1440.), px(900.)), cx),
    };
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Loupe".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(15.))) }),
        app_owns_titlebar_drag: true,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(860.), px(560.))),
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(app, window, cx))).expect("main window");
}

impl WorktreeModel {
    /// ⌘Z: like `undo`, but a refusal comes back to the caller (the "Can't Undo" dialog) instead of the Git alert.
    pub fn undo_or_refusal(&mut self, cx: &mut Context<Self>) -> Task<Option<loupe_core::undo::UndoRefusal>> {
        let t = self.perform("Undoing", |p| Ok::<_, String>(loupe_core::model::undo_last(p)), cx);
        cx.spawn(async move |this, cx| {
            match t.await? {
                Ok(Some(undone)) => {
                    this.update(cx, |m, cx| if let Some((msg, amend)) = undone.message && m.state.commit_message.is_empty() {
                        m.state.commit_message = msg;
                        m.state.amend = amend;
                        cx.notify();
                    }).ok();
                    None
                }
                Ok(None) => None,
                Err(loupe_core::Error::Refused(r)) => Some(r),
                Err(e) => { this.update(cx, |m, cx| m.set_error(e, cx)).ok(); None }
            }
        })
    }

    /// The refused undo's "Revert Commit". Recorded for ⌘Z like a revert from History (the Swift app's wasn't:
    /// FLOWS.md §14 History #6).
    pub fn revert_refused(&mut self, sha: String, cx: &mut Context<Self>) -> Task<Option<()>> {
        use loupe_core::undo::{Recipe, Reset, UndoStack};
        let label = format!("Revert of {}", &sha[..sha.len().min(7)]);
        self.perform("Reverting", move |p| UndoStack::for_path(p).run(
            Some(Recipe::Reset(Reset { offer_revert: true, ..Reset::new(label) })),
            || loupe_core::history::revert_sha(p, &sha)), cx)
    }
}
