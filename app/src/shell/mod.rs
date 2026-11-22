//! Window, sidebar, tabs, toolbar, quick switcher, toasts (App.swift, WorktreeView in ChangesView.swift).
pub mod keys;
pub mod model;
pub mod settings;
pub mod switcher;

use std::time::Duration;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
    component::{
        Sizable as _, TitleBar,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt as _, PopupMenuItem},
    },
};

use crate::{theme, ui};
use keys::*;
use model::{Status, Worktree, Workspace, last_component, row_title, tilde};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab { Changes, History, Pulls }

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Changes, Tab::History, Tab::Pulls];
    /// The "tab" default's value.
    pub fn key(self) -> &'static str { match self { Tab::Changes => "changes", Tab::History => "history", Tab::Pulls => "pulls" } }
    pub fn from_key(k: &str) -> Self { match k { "history" => Tab::History, "pulls" => Tab::Pulls, _ => Tab::Changes } }
    pub fn title(self) -> &'static str { match self { Tab::Changes => "Changes", Tab::History => "History", Tab::Pulls => "Pull Requests" } }
}

/// Overview / Files (/ Conversation) for whatever change is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode { Overview, Files, Conversation }

/// Which of the toolbar's diff controls apply to what's on screen. Views showing a diff set `Shell::controls`.
#[derive(Clone, Default)]
pub struct DiffControls {
    pub full: bool, // a diff is showing: Compact/Full applies
    pub view: Option<(Vec<ViewMode>, ViewMode, std::rc::Rc<dyn Fn(ViewMode, &mut Window, &mut App)>)>,
}

struct Toast { id: u64, text: SharedString, undoable: bool }

pub struct Shell {
    pub workspace: Entity<Workspace>,
    pub tab: Tab,
    pub full_mode: bool, // the "prFullMode" default
    pub controls: DiffControls,
    /// The real Changes / History / Pull Requests views, once the integrator has them; placeholders until then.
    pub views: [Option<AnyView>; 3],
    sidebar_open: bool,
    focus: FocusHandle,
    filter: Entity<InputState>,
    switcher: Option<Entity<switcher::Switcher>>,
    toast: Option<Toast>,
    toast_timer: Option<Task<()>>,
    next_toast: u64,
    _subs: Vec<Subscription>,
}

impl Shell {
    pub fn new(workspace: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = cx.new(|_| workspace);
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter worktrees"));
        let subs = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |_, _, e: &InputEvent, cx| if matches!(e, InputEvent::Change) { cx.notify() }),
        ];
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { workspace, tab: Tab::Changes, full_mode: false, controls: DiffControls::default(), views: [None, None, None],
               sidebar_open: true, focus, filter, switcher: None, toast: None, toast_timer: None, next_toast: 0, _subs: subs }
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

    fn set_tab(&mut self, t: Tab, cx: &mut Context<Self>) { self.tab = t; cx.notify() }

    fn filter_text(&self, cx: &App) -> String { self.filter.read(cx).value().to_string() }

    /// The worktrees the sidebar shows, in order: what ⌥⌘1…9 pick from.
    fn visible(&self, cx: &App) -> Vec<(String, Worktree)> {
        let ws = self.workspace.read(cx);
        let q = self.filter_text(cx).to_lowercase();
        let tokens: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
        ws.repo_paths().iter().flat_map(|r| {
            // Filtering looks inside collapsed repos too: a match shouldn't hide behind a chevron.
            let open = !tokens.is_empty() || !ws.collapsed.contains(r);
            ws.worktrees_of(r).iter().filter(move |_| open).filter(|w| {
                let hay = format!("{} {} {}", last_component(r), w.branch.as_deref().unwrap_or(""), w.name()).to_lowercase();
                tokens.iter().all(|t| hay.contains(t))
            }).map(|w| (r.clone(), w.clone())).collect::<Vec<_>>()
        }).collect()
    }

    fn select_nth(&mut self, n: usize, cx: &mut Context<Self>) {
        if let Some((_, w)) = self.visible(cx).into_iter().nth(n) { self.workspace.update(cx, |ws, cx| { ws.select(Some(w.path)); cx.notify() }) }
    }

    fn open_repository(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Open".into()) });
        let ws = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                ws.update(cx, |ws, cx| { for p in paths { ws.add_repo(&p.to_string_lossy()) } cx.notify() });
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
        let status = wt.as_ref().map(|w| ws.status(&w.path)).unwrap_or_default();
        let tab_ix = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let me = cx.entity();
        TitleBar::new().h(px(42.)).bg(theme::sidebar()).border_b_0()
            .child(div().flex().items_center().size_full().pr_2()
                // Left: the sidebar toggle, then fetch / pull / push beside the branch they act on.
                .child(div().flex().flex_1().items_center().gap_1().min_w_0()
                    .child(title_button("toggle-sidebar", "panel-left").on_click(cx.listener(|s, _, _, cx| { s.sidebar_open = !s.sidebar_open; cx.notify() })))
                    .when_some(wt.clone(), |d, wt| d
                        .child(div().w(px(10.)))
                        .child(branch_button(&wt))
                        .child(title_button("fetch", "refresh-cw").on_click(|_, w, cx| w.dispatch_action(Box::new(Fetch), cx)))
                        .child(title_button("pull", "arrow-down").on_click(|_, w, cx| w.dispatch_action(Box::new(Pull), cx))
                            .when(status.behind > 0, |d| d.child(ui::kbd(status.behind.to_string()).mr_1())))
                        .child(title_button("push", "arrow-up").on_click(|_, w, cx| w.dispatch_action(Box::new(Push), cx))
                            .when(status.ahead > 0, |d| d.child(ui::kbd(status.ahead.to_string()).mr_1())))))
                // Center: the view switch.
                .when(wt.is_some(), |d| d.child(ui::segmented("tabs", &["Changes", "History", "Pull Requests"], tab_ix,
                    move |i, _, cx| me.update(cx, |s, cx| s.set_tab(Tab::ALL[i], cx)))))
                // Right: Compact/Full and the view mode while a diff shows, then search and settings.
                .child(div().flex().flex_1().items_center().justify_end().gap_1()
                    .when(self.controls.full, |d| {
                        let me = cx.entity();
                        d.child(ui::segmented("mode", &["Compact", "Full"], self.full_mode as usize,
                            move |i, _, cx| me.update(cx, |s, cx| { s.full_mode = i == 1; cx.notify() })))
                    })
                    .when_some(self.controls.view.clone(), |d, (modes, cur, set)| {
                        let names: Vec<&str> = modes.iter().map(|m| match m { ViewMode::Overview => "Overview", ViewMode::Files => "Files", ViewMode::Conversation => "Conversation" }).collect();
                        let sel = modes.iter().position(|m| *m == cur).unwrap_or(0);
                        d.child(ui::segmented("view", &names, sel, move |i, w, cx| set(modes[i], w, cx)))
                    })
                    .child(title_button("go-to", "search").on_click(|_, w, cx| w.dispatch_action(Box::new(GoToWorktree), cx)))
                    .child(title_button("settings", "settings").on_click(|_, w, cx| w.dispatch_action(Box::new(OpenSettings), cx)))))
            .into_any_element()
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx).clone();
        let ws = &ws;
        let filtering = !self.filter_text(cx).trim().is_empty();
        let visible = self.visible(cx);
        let sel = ws.selection().map(str::to_string);
        let space = ws.space().clone();
        let multi = ws.spaces.len() > 1;
        let mut n = 0;
        let mut groups = vec![];
        for repo in &space.repos {
            let rows: Vec<&Worktree> = visible.iter().filter(|(r, _)| r == repo).map(|(_, w)| w).collect();
            if filtering && rows.is_empty() { continue }
            let expanded = filtering || !ws.collapsed.contains(repo);
            let changes = ws.change_count(repo);
            let mut g = div().flex().flex_col().gap_px().pb_2().child(self.repo_header(repo, expanded, changes, multi, cx));
            for w in rows {
                let hint = (n < 9).then(|| format!("⌥⌘{}", n + 1));
                n += 1;
                g = g.child(self.worktree_row(repo, w, ws.status(&w.path), sel.as_deref() == Some(&w.path), hint, cx));
            }
            groups.push(g);
        }
        let empty = space.repos.is_empty();
        div().flex().flex_col().size_full().bg(theme::sidebar())
            .when(multi, |d| d.child(div().px_4().pt_1().pb_1().text_size(px(15.)).font_weight(FontWeight::BOLD).child(space.name.clone())))
            .child(div().id("sidebar-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().px_2().pt_1()
                .children(groups)
                .when(empty, |d| d.child(div().px_3().py_2().text_color(theme::text_faint()).child("No repositories")))
                .when(filtering && n == 0 && !empty, |d| d.child(div().px_3().py_2().text_color(theme::text_faint()).child("No matches"))))
            .child(self.sidebar_footer(cx))
            .into_any_element()
    }

    fn repo_header(&self, repo: &str, expanded: bool, changes: usize, multi: bool, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.clone();
        let r = repo.to_string();
        let name = last_component(repo).to_string();
        let others: Vec<(String, String)> = self.workspace.read(cx).spaces.iter()
            .filter(|s| !s.repos.contains(&r)).map(|s| (s.id.clone(), s.name.clone())).collect();
        let (r1, r2, ws1, ws2) = (r.clone(), r.clone(), ws.clone(), ws.clone());
        div().id(SharedString::from(format!("repo-{repo}"))).group("repo").flex().items_center().gap_1().h(px(28.)).px_2()
            .rounded(theme::RADIUS_ROW).cursor_pointer().text_color(theme::text())
            .on_click(move |_, _, cx| ws1.update(cx, |w, cx| { if !w.collapsed.remove(&r1) { w.collapsed.insert(r1.clone()); } cx.notify() }))
            .child(ui::icon(if expanded { "chevron-down" } else { "chevron-right" }).size(px(12.)).text_color(theme::text_faint()))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
            // Collapsed: still show whether anything in this repo has uncommitted work.
            .when(!expanded && changes > 0, |d| d.child(ui::badge(changes.to_string())))
            .child(div().id("add-wt").flex().items_center().justify_center().size(px(20.)).rounded(px(5.))
                .opacity(0.).group_hover("repo", |s| s.opacity(1.)).hover(|s| s.bg(theme::hover()))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(format!("New Worktree in {name}…")).build(w, cx))
                .on_click(|_, w, cx| { cx.stop_propagation(); w.dispatch_action(Box::new(NewWorktree), cx) })
                .child(ui::icon("plus").size(px(12.))))
            .context_menu(move |mut menu, _, _| {
                let (ra, rb, wa) = (r2.clone(), r2.clone(), ws2.clone());
                menu = menu.item(PopupMenuItem::new("New Worktree…").on_click(|_, w, cx| w.dispatch_action(Box::new(NewWorktree), cx)))
                    .item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&ra))));
                if multi {
                    menu = menu.separator();
                    for (id, name) in &others {
                        let (id, r, w) = (id.clone(), r2.clone(), ws2.clone());
                        menu = menu.item(PopupMenuItem::new(format!("Move to {name}")).on_click(move |_, _, cx| w.update(cx, |w, cx| { w.move_repos(&[r.clone()], &id); cx.notify() })));
                    }
                }
                menu.separator()
                    .item(PopupMenuItem::new("Remove from Loupe").on_click(move |_, _, cx| wa.update(cx, |w, cx| { w.remove_repo(&rb); cx.notify() })))
            })
            .into_any_element()
    }

    fn worktree_row(&self, repo: &str, wt: &Worktree, st: Status, selected: bool, hint: Option<String>, _cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.clone();
        let path = wt.path.clone();
        let (p1, p2, p3, p4, p5, p6) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
        let tip: SharedString = match &wt.branch { Some(b) => format!("{}\n{b}", wt.path), None => wt.path.clone() }.into();
        worktree_row_body(SharedString::from(format!("wt-{path}")), repo, wt, st, selected, None)
            .when_some(hint, |d, h| d.child(ui::kbd(h)))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
            .on_click(move |_, _, cx| ws.update(cx, |w, cx| { w.select(Some(p1.clone())); cx.notify() }))
            .context_menu(move |menu, _, _| {
                let (a, b, c, d, e) = (p2.clone(), p3.clone(), p4.clone(), p5.clone(), p6.clone());
                menu.item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&a))))
                    .item(PopupMenuItem::new("Open in Terminal").on_click(move |_, _, _| open_in("com.apple.Terminal", &b)))
                    .item(PopupMenuItem::new("Open in Zed").on_click(move |_, _, _| open_in("dev.zed.Zed", &c)))
                    .item(PopupMenuItem::new("Open in VS Code").on_click(move |_, _, _| open_in("com.microsoft.VSCode", &d)))
                    .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(e.clone()))))
            })
            .into_any_element()
    }

    fn sidebar_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let current = ws.current_space.clone();
        let spaces: Vec<(String, String)> = ws.spaces.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        let w = self.workspace.clone();
        div().flex().flex_col().gap_2().p_2().flex_none()
            .child(div().flex().items_center().gap_2().h(px(30.)).px_2p5().rounded(theme::RADIUS_ROW).bg(theme::raised())
                .child(ui::icon("search").size(px(13.)).text_color(theme::text_faint()))
                .child(div().flex_1().child(Input::new(&self.filter).appearance(false).small().cleanable(true))))
            .child(div().flex().items_center().gap_1()
                .child(ui::icon_button("open-repo", "folder-plus").on_click(|_, w, cx| w.dispatch_action(Box::new(OpenRepository), cx)))
                .child(div().flex_1().flex().justify_center().gap_1().children(spaces.into_iter().map(|(id, name)| {
                    let on = id == current;
                    let w = w.clone();
                    div().id(SharedString::from(format!("space-{id}"))).p_1().cursor_pointer()
                        .tooltip(move |win, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(win, cx))
                        .on_click(move |_, _, cx| w.update(cx, |w, cx| { w.switch_space_to(&id); cx.notify() }))
                        .child(div().size(px(7.)).rounded_full().border_1()
                            .map(|d| if on { d.bg(theme::text()).border_color(theme::text()) } else { d.border_color(theme::text_faint()) }))
                })))
                .child(ui::icon_button("new-space", "plus").on_click(move |_, _, cx| w.update(cx, |w, cx| { w.add_space(); cx.notify() }))))
            .into_any_element()
    }

    fn content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx).clone();
        let panel = div().size_full().bg(theme::canvas()).border_l_1().border_t_1().border_color(theme::border())
            .rounded_tl(theme::RADIUS_CARD).overflow_hidden();
        let Some(wt) = ws.selected_worktree().cloned() else {
            return panel.child(ui::empty_state("git-branch", "No Worktree Selected", "Open a repository to review its changes and pull requests.")
                .child(div().pt_2().child(ui::pill("open-repo-empty", "Open Repository…").on_click(|_, w, cx| w.dispatch_action(Box::new(OpenRepository), cx))))).into_any_element();
        };
        let ix = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        if let Some(v) = self.views[ix].clone() { return panel.child(v).into_any_element() }
        panel.child(placeholder(self.tab, &wt, ws.status(&wt.path))).into_any_element()
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
            .child(div().truncate().text_size(px(11.)).text_color(theme::text_faint()).child(wt.branch_label())))
        .child(div().flex().flex_none().items_center().gap_1p5().text_size(px(11.))
            .when(merged, |d| d.child(ui::badge_tinted("Merged", theme::purple())))
            .when(st.ahead > 0, |d| d.child(div().text_color(theme::text_muted()).child(format!("↑{}", st.ahead))))
            .when(st.behind > 0, |d| d.child(div().text_color(theme::text_muted()).child(format!("↓{}", st.behind))))
            .when(st.changes > 0, |d| d.child(ui::badge(st.changes.to_string())))
            .when(wt.locked, |d| d.child(ui::icon("lock").size(px(11.)))))
}

fn title_button(id: &'static str, icon: &str) -> gpui_kit::Stateful<gpui_kit::Div> {
    // Keep the press: without this a click with a little drift would start a window drag.
    ui::icon_button(id, icon).on_mouse_down(MouseButton::Left, |_, w, _| w.prevent_default())
}

/// The branch and the worktree's folder, two lines; a click opens the branch picker.
fn branch_button(wt: &Worktree) -> impl IntoElement {
    let title = if wt.head.is_empty() { String::new() } else { wt.branch.clone().unwrap_or_else(|| "detached HEAD".into()) };
    div().id("branch").flex().flex_col().justify_center().h(px(34.)).px_2().max_w(px(300.)).rounded(px(7.)).cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .on_mouse_down(MouseButton::Left, |_, w, _| w.prevent_default())
        .on_click(|_, w, cx| w.dispatch_action(Box::new(ShowBranches), cx))
        .child(div().flex().items_center().gap_1()
            .child(div().truncate().text_size(px(13.)).line_height(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(ui::icon("chevron-down").size(px(11.)).text_color(theme::text_faint())))
        .child(div().truncate().text_size(px(11.)).line_height(px(14.)).text_color(theme::text_faint()).child(tilde(&wt.path)))
}

fn open_in(bundle: &str, path: &str) {
    let _ = std::process::Command::new("/usr/bin/open").args(["-b", bundle, path]).spawn();
}

/// What each tab shows until its view is ported: the kit, with the selected worktree's name in it.
fn placeholder(tab: Tab, wt: &Worktree, st: Status) -> impl IntoElement {
    use ui::LineKind::*;
    let body = div().flex().flex_col().gap_4().w_full().max_w(px(980.)).px_6().py_6();
    let body = match tab {
        Tab::Changes => body
            .child(ui::section_header("Changes", Some(st.changes)))
            .child(ui::card()
                .child(ui::card_header(div().flex().items_center().gap_2().child(ui::icon("file")).child(div().font_family(theme::CODE_FONT).child("app/src/shell/mod.rs")),
                    div().flex().gap_1p5().child(div().text_color(theme::green()).child("+3")).child(div().text_color(theme::red()).child("-1")).child("1.8s")))
                .child(div().py_1().font_family(theme::CODE_FONT).text_size(theme::CODE_SIZE).line_height(theme::CODE_LINE_HEIGHT)
                    .child(ui::code_line(None, "@@ -39,6 +39,8 @@ impl Shell", Hunk))
                    .child(ui::code_line(Some(39), "    pub fn new(workspace: Workspace) -> Self {", Context))
                    .child(ui::code_line(Some(40), "        let tab = Tab::Changes;", Removed))
                    .child(ui::code_line(Some(40), "        let tab = Tab::from_key(&saved);", Added))
                    .child(ui::code_line(Some(41), "        let full_mode = false;", Added))
                    .child(ui::code_line(Some(42), "        Self { workspace, tab, full_mode }", Context)))
                .child(ui::card_footer(div().flex().gap_1p5().child(ui::icon("check").size(px(12.)).text_color(theme::green()))
                    .child(div().text_color(theme::green()).child("Done")).child("15.1s"))))
            .child(div().flex().gap_2().child(ui::pill("commit", "Commit")).child(ui::pill("stash", "Stash")))
            .into_any_element(),
        Tab::History => body
            .child(ui::section_header(format!("History of {}", wt.branch_label()), None))
            .child(ui::card().p_1().children([
                ("fde8251", "Quitting Loupe no longer hangs", "2h"), ("585173a", "Merging a pull request takes you to what you merged", "5h"),
                ("5f97dac", "Loupe no longer works in the background", "1d"), ("1d10383", "Switching views no longer redoes work", "2d"),
            ].into_iter().enumerate().map(|(i, (sha, msg, t))| ui::list_row(("commit", i), i == 0)
                .child(ui::code_chip(sha)).child(div().flex_1().min_w_0().truncate().child(msg)).child(ui::kbd(t)))))
            .into_any_element(),
        Tab::Pulls => body
            .child(ui::section_header("Pull Requests", Some(2)))
            .child(ui::card().p_1()
                .child(ui::list_row("pr-1", false).child(ui::icon("git-pull-request").text_color(theme::green()))
                    .child(div().flex_1().truncate().child("GPUI port: shell and widget kit")).child(ui::badge_tinted("gpui-shell", theme::blue())).child(ui::kbd("#42")))
                .child(ui::list_row("pr-2", false).child(ui::icon("git-merge").text_color(theme::purple()))
                    .child(div().flex_1().truncate().child("Idle CPU fixes")).child(ui::badge_tinted("Merged", theme::purple())).child(ui::kbd("#41"))))
            .into_any_element(),
    };
    div().id("placeholder").size_full().overflow_y_scroll().flex().justify_center().child(body)
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
        let _ = window;
        div().id("shell").track_focus(&self.focus).key_context("Loupe").relative().flex().flex_col().size_full()
            .bg(theme::sidebar()).text_color(theme::text()).font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .on_action(cx.listener(|s, _: &OpenRepository, _, cx| s.open_repository(cx)))
            .on_action(cx.listener(|s, _: &GoToWorktree, w, cx| s.open_switcher(w, cx)))
            .on_action(cx.listener(|s, _: &ToggleSidebar, _, cx| { s.sidebar_open = !s.sidebar_open; cx.notify() }))
            .on_action(cx.listener(|s, _: &ChangesTab, _, cx| s.set_tab(Tab::Changes, cx)))
            .on_action(cx.listener(|s, _: &HistoryTab, _, cx| s.set_tab(Tab::History, cx)))
            .on_action(cx.listener(|s, _: &PullsTab, _, cx| s.set_tab(Tab::Pulls, cx)))
            .on_action(cx.listener(|s, _: &FullMode, _, cx| { s.full_mode = !s.full_mode; cx.notify() }))
            .on_action(cx.listener(|s, _: &SelectWorktree1, _, cx| s.select_nth(0, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree2, _, cx| s.select_nth(1, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree3, _, cx| s.select_nth(2, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree4, _, cx| s.select_nth(3, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree5, _, cx| s.select_nth(4, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree6, _, cx| s.select_nth(5, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree7, _, cx| s.select_nth(6, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree8, _, cx| s.select_nth(7, cx)))
            .on_action(cx.listener(|s, _: &SelectWorktree9, _, cx| s.select_nth(8, cx)))
            .on_action(cx.listener(|s, _: &OpenSettings, _, cx| settings::open(s.workspace.clone(), cx)))
            .on_action(|_: &CloseWindow, w, _| w.remove_window())
            .on_action(|_: &Minimize, w, _| w.minimize_window())
            .on_action(|_: &Zoom, w, _| w.zoom_window())
            // A folder dropped anywhere on the window opens as a repository.
            .on_drop(cx.listener(|s, paths: &ExternalPaths, _, cx| s.workspace.update(cx, |w, cx| {
                for p in paths.paths() { w.add_repo(&p.to_string_lossy()) }
                cx.notify()
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
            .children(self.switcher.clone())
    }
}

/// The main window, 1440×900 by default, at least 860×560, with Loupe's own title bar.
pub fn open_main_window(workspace: Workspace, cx: &mut App) {
    use gpui_kit::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, size};
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Loupe".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(15.))) }),
        app_owns_titlebar_drag: true,
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(900.)), cx))),
        window_min_size: Some(size(px(860.), px(560.))),
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(workspace, window, cx))).expect("main window");
}
