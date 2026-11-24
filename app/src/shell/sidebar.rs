//! The sidebar (Sidebar in App.swift): one page per space with its repos and worktrees, paged by a two-finger swipe
//! or the space dots; repo headers drag to reorder or onto a dot; the context menus and their dialogs.
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _, TouchPhase,
    Window, canvas, div, prelude::FluentBuilder as _, px,
    component::{
        Sizable as _,
        input::{Input, InputState},
        menu::{ContextMenuExt as _, PopupMenuItem},
    },
};

use super::keys::*;
use super::model::{Status, Worktree, last_component};
use super::{Shell, open_in, worktree_row_body, worktrees};
use crate::state::{AppEvent, AppModel};
use crate::{theme, ui};

/// A repo header being dragged: onto another header (reorder) or a space dot (move there).
#[derive(Clone)]
pub struct DraggedRepo { repo: String, name: SharedString }

impl Render for DraggedRepo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded(px(6.)).bg(theme::raised()).border_1().border_color(theme::border_strong())
            .text_color(theme::text()).text_size(theme::UI_SIZE).child(self.name.clone())
    }
}

/// The dialogs the sidebar and ⌘Z open over the window.
pub enum Modal {
    RemoveWorktree { repo: String, path: String, name: String },
    RenameSpace { id: String, input: Entity<InputState> },
    DeleteSpace { id: String, name: String },
    /// A refused undo: why, and the commit to revert instead when there is one.
    CantUndo { message: String, revert: Option<String> },
}

/// A two-finger swipe in progress. The first 4 pt decide its axis; vertical ones are left to the list.
pub struct Swipe { moved: (f32, f32), horizontal: Option<bool>, offset: f32, last: (Instant, f32), velocity: f32 }

/// A slide between pages after a swipe or a dot click: the pages sit side by side and spring from `from` to 0.
pub struct Slide { pub id: u64, pub from: f32, pub old: usize, pub step: isize }

const RUBBER: f32 = 0.35;
const FLICK: f32 = 0.6; // pages per second

impl Shell {
    pub(super) fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (index, count) = { let ws = self.workspace.read(cx); (ws.spaces.iter().position(|s| s.id == ws.current_space).unwrap_or(0), ws.spaces.len()) };
        let w = self.page_width.get().max(1.);
        let width = self.page_width.clone();
        // At rest only the shown space is laid out; its neighbour joins for a swipe or a slide.
        let pages: AnyElement = if let Some(off) = self.swipe.as_ref().filter(|s| s.horizontal == Some(true) && s.offset != 0.).map(|s| s.offset) {
            let n = if off < 0. { index as isize + 1 } else { index as isize - 1 };
            let mut d = div().relative().size_full().child(self.page(index, cx).left(px(off)));
            if n >= 0 && (n as usize) < count { d = d.child(self.page(n as usize, cx).left(px(off + (n - index as isize) as f32 * w))) }
            d.into_any_element()
        } else if let Some(sl) = &self.slide {
            let (id, from, old, step) = (sl.id, sl.from, sl.old, sl.step);
            let mut inner = div().relative().size_full().child(self.page(index, cx));
            if old < count && old != index { inner = inner.child(self.page(old, cx).left(px(-(step as f32) * w))) }
            use gpui_kit::AnimationExt as _;
            div().relative().size_full().child(inner.with_spring(("pager", id),
                gpui_kit::SpringAnimation::new(gpui_kit::SpringConfig::new(500., 44.7, 1.)).to(0f32).from(from),
                |d, x| d.left(px(x)))).into_any_element()
        } else {
            div().relative().size_full().child(self.page(index, cx)).into_any_element()
        };
        div().flex().flex_col().size_full().bg(theme::sidebar())
            .child(div().id("pager").relative().flex_1().min_h_0().overflow_hidden()
                .on_scroll(cx.listener(|s, e: &ScrollEvent, _, cx| s.on_swipe(e, cx)))
                .child(canvas(move |b, _, _| width.set(b.size.width.as_f32()), |_, _, _, _| {}).absolute().size_full())
                .child(pages))
            .child(self.sidebar_footer(cx))
            .into_any_element()
    }

    /// One space's page: its name (with more than one space) over its repos.
    fn page(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui_kit::Div {
        let filter = self.filter_text(cx);
        let filtering = !filter.trim().is_empty();
        let ws = self.workspace.read(cx);
        let Some(space) = ws.spaces.get(ix).cloned() else { return div() };
        let current = space.id == ws.current_space;
        let visible = super::visible_in(ws, &space.repos, &filter);
        let sel = ws.selection().map(str::to_string);
        let multi = ws.spaces.len() > 1;
        let collapsed = ws.collapsed.clone();
        let changes: Vec<usize> = space.repos.iter().map(|r| ws.change_count(r)).collect();
        let status: HashMap<String, Status> = visible.iter().map(|(_, w)| (w.path.clone(), ws.status(&w.path))).collect();
        let mut n = 0;
        let mut groups = vec![];
        for (repo, changes) in space.repos.iter().zip(changes) {
            let rows: Vec<&Worktree> = visible.iter().filter(|(r, _)| r == repo).map(|(_, w)| w).collect();
            if filtering && rows.is_empty() { continue }
            let expanded = filtering || !collapsed.contains(repo);
            let mut g = div().flex().flex_col().gap_px().pb_2().child(self.repo_header(repo, &space.id, &space.repos, expanded, changes, cx));
            for w in rows {
                // ⌥⌘1…9 pick from the shown space only.
                let hint = (current && n < 9).then(|| format!("⌥⌘{}", n + 1));
                n += 1;
                g = g.child(self.worktree_row(repo, w, status[&w.path], sel.as_deref() == Some(&w.path), hint, cx));
            }
            groups.push(g);
        }
        let empty = space.repos.is_empty();
        let me = cx.entity();
        let (sid, sname) = (space.id.clone(), space.name.clone());
        div().absolute().top_0().bottom_0().w_full().flex().flex_col()
            .when(multi, |d| d.child(div().id(SharedString::from(format!("space-title-{}", space.id))).px_4().pt_1().pb_1()
                // The weight goes on the text only: the context menu inherits the element's text style.
                .child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).truncate().child(space.name.clone()))
                .context_menu(move |m, _, _| space_menu(m, &me, &sid, &sname, multi))))
            .child(div().id(SharedString::from(format!("sidebar-list-{}", space.id))).flex().flex_col().flex_1().min_h_0()
                .overflow_y_scroll().restrict_scroll_to_axis().px_2().pt_1()
                .children(groups)
                .when(empty, |d| d.child(div().px_3().py_2().text_color(theme::text_faint()).child("No repositories")))
                .when(filtering && n == 0 && !empty, |d| d.child(div().px_3().py_2().text_color(theme::text_faint()).child("No matches"))))
    }

    fn repo_header(&self, repo: &str, space_id: &str, space_repos: &[String], expanded: bool, changes: usize, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.clone();
        let r = repo.to_string();
        let name = last_component(repo).to_string();
        let others: Vec<(String, String)> = self.workspace.read(cx).spaces.iter()
            .filter(|s| s.id != space_id).map(|s| (s.id.clone(), s.name.clone())).collect();
        let (r1, r2, r3, ws1, ws2) = (r.clone(), r.clone(), r.clone(), ws.clone(), ws.clone());
        let mine: Vec<String> = space_repos.to_vec();
        let (tip, drag) = (name.clone(), DraggedRepo { repo: r.clone(), name: name.clone().into() });
        div().id(SharedString::from(format!("repo-{repo}"))).group("repo").flex().items_center().gap_1().h(px(28.)).px_2()
            .rounded(theme::RADIUS_ROW).cursor_pointer().text_color(theme::text())
            .on_click(move |_, _, cx| ws1.update(cx, |w, cx| { if !w.collapsed.remove(&r1) { w.collapsed.insert(r1.clone()); } cx.notify() }))
            // Drag onto another header to put this repo before it, or onto a space dot to move it there.
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<DraggedRepo>(|s, _, _, _| s.border_t_2().border_color(theme::blue()))
            .can_drop(move |v, _, _| v.downcast_ref::<DraggedRepo>().is_some_and(|d| mine.contains(&d.repo)))
            .on_drop({ let (ws, target) = (ws.clone(), r.clone()); move |d: &DraggedRepo, _, cx| ws.update(cx, |w, cx| w.move_repo(&d.repo, &target, cx)) })
            .child(ui::icon(if expanded { "chevron-down" } else { "chevron-right" }).size(px(12.)).text_color(theme::text_faint()))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
            // Collapsed: still show whether anything in this repo has uncommitted work.
            .when(!expanded && changes > 0, |d| d.child(ui::badge(changes.to_string())))
            .child(div().id("add-wt").flex().items_center().justify_center().size(px(20.)).rounded(px(5.))
                .opacity(0.).group_hover("repo", |s| s.opacity(1.)).hover(|s| s.bg(theme::hover()))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(format!("New Worktree in {tip}…")).build(w, cx))
                .on_click({ let (ws, r) = (ws.clone(), r3.clone()); move |_, w, cx| { cx.stop_propagation(); worktrees::open_add_worktree(&ws, r.clone(), String::new(), w, cx) } })
                .child(ui::icon("plus").size(px(12.))))
            .context_menu(move |mut menu, window, cx| {
                let (ra, rb, rc, rd) = (r2.clone(), r2.clone(), r2.clone(), r2.clone());
                let (wa, wb, wc) = (ws2.clone(), ws2.clone(), ws2.clone());
                menu = menu.item(PopupMenuItem::new("New Worktree…").on_click(move |_, w, cx| worktrees::open_add_worktree(&wc, rd.clone(), String::new(), w, cx)))
                    .item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(Path::new(&ra))))
                    .item(PopupMenuItem::new("Prune Stale Worktrees").on_click(move |_, _, cx| wb.update(cx, |w, cx| w.prune_worktrees(rc.clone(), cx))));
                if !others.is_empty() {
                    let (others, r, ws) = (others.clone(), r2.clone(), ws2.clone());
                    menu = menu.submenu("Move to Space", window, cx, move |mut m, _, _| {
                        for (id, name) in &others {
                            let (id, r, w) = (id.clone(), r.clone(), ws.clone());
                            m = m.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| w.update(cx, |w, cx| w.move_repos(&[r.clone()], &id, cx))));
                        }
                        m
                    });
                }
                menu.separator()
                    .item(PopupMenuItem::new("Remove from Loupe").on_click(move |_, _, cx| wa.update(cx, |w, cx| w.remove_repo(&rb, cx))))
            })
            .into_any_element()
    }

    fn worktree_row(&self, repo: &str, wt: &Worktree, st: Status, selected: bool, hint: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.clone();
        let path = wt.path.clone();
        let main = wt.path == repo;
        // git lists a worktree whose folder was deleted as prunable; selecting it would run git in a missing folder.
        let prunable = wt.is_prunable;
        let tip: SharedString = if prunable { format!("{}\nThe folder is gone. Prune Stale Worktrees removes this row.", wt.path) }
            else { match &wt.branch { Some(b) => format!("{}\n{b}", wt.path), None => wt.path.clone() } }.into();
        let me = cx.entity();
        let app = self.workspace.clone();
        let (r1, r2, p1) = (repo.to_string(), repo.to_string(), path.clone());
        let name = wt.name().to_string();
        worktree_row_body(SharedString::from(format!("wt-{path}")), repo, wt, st, selected, None)
            .when(prunable, |d| d.opacity(0.5).child(ui::badge("missing")))
            .when_some(hint, |d, h| d.child(ui::kbd(h)))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
            .on_click(move |_, _, cx| ws.update(cx, |w, cx| {
                // Deleted with rm -rf since the last listing: list again (it comes back prunable) instead of selecting.
                if prunable || !Path::new(&p1).exists() { w.refresh_worktrees(r1.clone(), cx).detach() } else { w.select(Some(p1.clone()), cx) }
            }))
            .context_menu(move |mut menu, _, cx| {
                // Asked when the menu opens, not per row render: the check may ask the forge (PC-06).
                let can_pr = app.read(cx).model(&path).is_some_and(|m| crate::views::pr_create::show_menu_item(&m, cx));
                let (a, b, c, d, e) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
                menu = menu.item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(Path::new(&a))))
                    .item(PopupMenuItem::new("Open in Terminal").on_click(move |_, _, _| open_in("com.apple.Terminal", &b)))
                    .item(PopupMenuItem::new("Open in Zed").on_click(move |_, _, _| open_in("dev.zed.Zed", &c)))
                    .item(PopupMenuItem::new("Open in VS Code").on_click(move |_, _, _| open_in("com.microsoft.VSCode", &d)))
                    .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(e.clone()))));
                if can_pr {
                    let (ws, p) = (app.clone(), path.clone());
                    menu = menu.separator().item(PopupMenuItem::new("Create Pull Request…")
                        .on_click(move |_, _, cx| ws.update(cx, |a, cx| a.emit(AppEvent::CreatePull { worktree: p.clone() }, cx))));
                }
                if prunable {
                    let (ws, r) = (app.clone(), r2.clone());
                    menu = menu.separator().item(PopupMenuItem::new("Prune Stale Worktrees").on_click(move |_, _, cx| ws.update(cx, |w, cx| w.prune_worktrees(r.clone(), cx))));
                } else if !main {
                    let (me, repo, p, name) = (me.clone(), r2.clone(), path.clone(), name.clone());
                    menu = menu.separator().item(PopupMenuItem::new("Remove Worktree…").on_click(move |_, _, cx| me.update(cx, |s, cx| {
                        s.modal = Some(Modal::RemoveWorktree { repo: repo.clone(), path: p.clone(), name: name.clone() });
                        cx.notify()
                    })));
                }
                menu
            })
            .into_any_element()
    }

    fn sidebar_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let current = ws.current_space.clone();
        let multi = ws.spaces.len() > 1;
        let spaces: Vec<(String, String, Vec<String>)> = ws.spaces.iter().map(|s| (s.id.clone(), s.name.clone(), s.repos.clone())).collect();
        let cur_repos: Vec<String> = ws.space().repos.clone();
        let w = self.workspace.clone();
        let me = cx.entity();
        div().flex().flex_col().gap_2().p_2().flex_none().border_t_1().border_color(theme::border())
            .child(div().flex().items_center().gap_2().h(px(30.)).px_2p5().rounded(theme::RADIUS_ROW).bg(theme::raised())
                .child(ui::icon("search").size(px(13.)).text_color(theme::text_faint()))
                .child(div().flex_1().child(Input::new(&self.filter).appearance(false).small().cleanable(true))))
            .child(div().flex().items_center().gap_1()
                .child(ui::icon_button("open-repo", "folder-plus").tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Open Repository…").build(w, cx))
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(OpenRepository), cx)))
                .child(div().flex_1().flex().justify_center().gap_1().children(spaces.into_iter().map(|(id, name, _)| {
                    let on = id == current;
                    let (w, w2, me, cur_repos) = (w.clone(), w.clone(), me.clone(), cur_repos.clone());
                    let (id2, id3, name2) = (id.clone(), id.clone(), name.clone());
                    div().id(SharedString::from(format!("space-{id}"))).p_1().rounded(px(4.)).cursor_pointer()
                        .tooltip(move |win, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(win, cx))
                        .on_click(move |_, _, cx| w.update(cx, |w, cx| w.switch_space_to(&id, cx)))
                        // Drop a repo header here to move the repo to that space (onto its own dot: to the end).
                        .drag_over::<DraggedRepo>(|s, _, _, _| s.bg(theme::selected()))
                        .can_drop({ let cur = cur_repos.clone(); move |v, _, _| v.downcast_ref::<DraggedRepo>().is_some_and(|d| cur.contains(&d.repo)) })
                        .on_drop(move |d: &DraggedRepo, _, cx| if cur_repos.contains(&d.repo) { w2.update(cx, |w, cx| w.move_repos(&[d.repo.clone()], &id2, cx)) })
                        .child(div().size(px(7.)).rounded_full().border_1()
                            .map(|d| if on { d.bg(theme::text()).border_color(theme::text()) } else { d.border_color(theme::text_faint()) }))
                        .context_menu(move |m, _, _| space_menu(m, &me, &id3, &name2, multi))
                })))
                .child(ui::icon_button("new-space", "plus").tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("New Space").build(w, cx))
                    .on_click(move |_, _, cx| w.update(cx, |w, cx| { w.add_space(cx); }))))
            .into_any_element()
    }

    // MARK: Swipe

    /// Trackpad only (mouse wheels have no precise deltas and always scroll the list). Pages follow the fingers,
    /// rubber-banded past the ends; on release they go to the nearest space, or the next one for a flick.
    fn on_swipe(&mut self, e: &ScrollEvent, cx: &mut Context<Self>) {
        if !e.delta.precise() { return }
        let d = e.delta.pixel_delta(px(16.));
        let (dx, dy) = (d.x.as_f32(), d.y.as_f32());
        match e.touch_phase {
            TouchPhase::Started => self.swipe = Some(Swipe { moved: (0., 0.), horizontal: None, offset: 0., last: (Instant::now(), 0.), velocity: 0. }),
            TouchPhase::Moved => {
                let (index, count) = { let ws = self.workspace.read(cx); (ws.spaces.iter().position(|s| s.id == ws.current_space).unwrap_or(0), ws.spaces.len()) };
                // Momentum after the fingers lift comes with no gesture: it belongs to the list.
                let Some(s) = self.swipe.as_mut() else { return };
                if s.horizontal.is_none() {
                    s.moved.0 += dx; s.moved.1 += dy;
                    if s.moved.0.abs().max(s.moved.1.abs()) < 4. { return }
                    s.horizontal = Some(s.moved.0.abs() > s.moved.1.abs() && count > 1);
                    if s.horizontal != Some(true) { return }
                    s.offset = s.moved.0;
                } else if s.horizontal == Some(true) {
                    let past_end = (s.offset > 0. && index == 0) || (s.offset < 0. && index + 1 >= count);
                    s.offset += if past_end { dx * RUBBER } else { dx };
                } else { return }
                let now = Instant::now();
                let dt = now.duration_since(s.last.0).as_secs_f32();
                if dt > 0.005 { s.velocity = (s.offset - s.last.1) / dt; s.last = (now, s.offset) }
                self.slide = None;
                cx.notify();
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(s) = self.swipe.take() else { return };
                if s.horizontal != Some(true) { return }
                let w = self.page_width.get().max(1.);
                let step = swipe_step(s.offset, s.velocity, w);
                let (index, count) = { let ws = self.workspace.read(cx); (ws.spaces.iter().position(|s| s.id == ws.current_space).unwrap_or(0) as isize, ws.spaces.len() as isize) };
                let step = (index + step).clamp(0, count - 1) - index;
                // The new page springs from where it sits now.
                self.start_slide(index as usize, step, s.offset + step as f32 * w, cx);
                if step != 0 {
                    self.settling = true;
                    self.workspace.update(cx, |a, cx| a.switch_space_by(step, cx));
                }
                cx.notify();
            }
        }
    }

    /// The pages spring to rest; the old one is dropped once they settle (~0.3 s), so nothing stays laid out.
    pub(super) fn start_slide(&mut self, old: usize, step: isize, from: f32, cx: &mut Context<Self>) {
        if from.abs() < 0.5 { self.slide = None; return }
        self.next_slide += 1;
        let id = self.next_slide;
        self.slide = Some(Slide { id, from, old, step });
        self.slide_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(450)).await;
            this.update(cx, |s, cx| if s.slide.as_ref().is_some_and(|x| x.id == id) { s.slide = None; cx.notify() }).ok();
        }));
    }

    // MARK: Dialogs

    pub(super) fn modal_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let modal = self.modal.as_ref()?;
        let me = cx.entity();
        let close = { let me = me.clone(); move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| me.update(cx, |s, cx| { s.modal = None; cx.notify() }) };
        let app = self.workspace.clone();
        let (title, message, body, buttons): (String, Option<String>, Option<AnyElement>, Vec<AnyElement>) = match modal {
            Modal::RemoveWorktree { repo, path, name } => {
                let remove = |force: bool, id: &'static str, label: &'static str| {
                    let (me, app, repo, path) = (me.clone(), app.clone(), repo.clone(), path.clone());
                    ui::pill_primary(id, label, theme::red()).on_click(move |_, _, cx| {
                        app.update(cx, |a, cx| a.remove_worktree_confirmed(repo.clone(), path.clone(), force, cx));
                        me.update(cx, |s, cx| { s.modal = None; cx.notify() });
                    }).into_any_element()
                };
                (format!("Remove worktree “{name}”?"), Some("The directory is deleted. The branch is kept.".into()), None,
                 vec![ui::pill("modal-cancel", "Cancel").on_click(close.clone()).into_any_element(),
                      remove(true, "modal-force", "Force Remove (discard changes)"), remove(false, "modal-remove", "Remove")])
            }
            Modal::RenameSpace { id, input } => {
                let (me, app, id, inp) = (me.clone(), app.clone(), id.clone(), input.clone());
                ("Rename Space".into(), None, Some(div().child(Input::new(input)).into_any_element()),
                 vec![ui::pill("modal-cancel", "Cancel").on_click(close.clone()).into_any_element(),
                      ui::pill_primary("modal-rename", "Rename", theme::blue()).on_click(move |_, _, cx| {
                          let name = inp.read(cx).value().trim().to_string();
                          rename_space(&app, &id, name, cx);
                          me.update(cx, |s, cx| { s.modal = None; cx.notify() });
                      }).into_any_element()])
            }
            Modal::DeleteSpace { id, name } => {
                let (me, app, id) = (me.clone(), app.clone(), id.clone());
                (format!("Delete space “{name}”?"),
                 Some("Its repositories move to the neighbouring space. Its accounts are signed out and their tokens removed from the Keychain.".into()), None,
                 vec![ui::pill("modal-cancel", "Cancel").on_click(close.clone()).into_any_element(),
                      ui::pill_primary("modal-delete", "Delete", theme::red()).on_click(move |_, _, cx| {
                          app.update(cx, |a, cx| a.delete_space(&id, cx));
                          me.update(cx, |s, cx| { s.modal = None; cx.notify() });
                      }).into_any_element()])
            }
            Modal::CantUndo { message, revert } => {
                let mut b = vec![];
                if let Some(sha) = revert.clone() {
                    let me = me.clone();
                    b.push(ui::pill("modal-cancel", "Cancel").on_click(close.clone()).into_any_element());
                    b.push(ui::pill_primary("modal-revert", "Revert Commit", theme::blue()).on_click(move |_, _, cx| me.update(cx, |s, cx| {
                        s.modal = None;
                        s.on_selected(cx, |m, cx| m.revert_refused(sha.clone(), cx).detach());
                        cx.notify()
                    })).into_any_element());
                } else {
                    b.push(ui::pill_primary("modal-ok", "OK", theme::blue()).on_click(close.clone()).into_any_element());
                }
                ("Can't Undo".into(), Some(message.clone()), None, b)
            }
        };
        let is_rename = matches!(modal, Modal::RenameSpace { .. });
        Some(div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .capture_key_down(cx.listener(move |s, e: &gpui_kit::KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                "escape" => { s.modal = None; cx.notify(); cx.stop_propagation() }
                "enter" if is_rename => {
                    if let Some(Modal::RenameSpace { id, input }) = s.modal.take() {
                        let name = input.read(cx).value().trim().to_string();
                        rename_space(&s.workspace, &id, name, cx);
                    }
                    cx.notify(); cx.stop_propagation()
                }
                _ => {}
            }))
            .child(ui::card().w(px(420.)).p_4().gap_3().bg(theme::raised())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .when_some(message, |d, m| d.child(div().text_size(px(12.)).text_color(theme::text_muted()).child(m)))
                .children(body)
                .child(div().flex().flex_wrap().justify_end().gap_2().children(buttons)))
            .into_any_element())
    }

    pub(super) fn ask_rename_space(&mut self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Name").default_value(name));
        input.update(cx, |i, cx| i.focus(window, cx));
        self.modal = Some(Modal::RenameSpace { id, input });
        cx.notify();
    }
}

/// Empty or whitespace-only names are ignored.
fn rename_space(app: &Entity<AppModel>, id: &str, name: String, cx: &mut gpui_kit::App) {
    if name.is_empty() { return }
    app.update(cx, |a, cx| if let Some(s) = a.spaces.iter_mut().find(|s| s.id == id) { s.name = name; cx.notify() });
}

fn space_menu(menu: gpui_kit::component::menu::PopupMenu, me: &Entity<Shell>, id: &str, name: &str, multi: bool) -> gpui_kit::component::menu::PopupMenu {
    let (m1, m2, i1, i2, n1, n2) = (me.clone(), me.clone(), id.to_string(), id.to_string(), name.to_string(), name.to_string());
    menu.item(PopupMenuItem::new("Rename…").on_click(move |_, w, cx| m1.update(cx, |s, cx| s.ask_rename_space(i1.clone(), n1.clone(), w, cx))))
        .item(PopupMenuItem::new("Delete…").disabled(!multi).on_click(move |_, _, cx| m2.update(cx, |s, cx| {
            s.modal = Some(Modal::DeleteSpace { id: i2.clone(), name: n2.clone() });
            cx.notify()
        })))
}


/// Where released pages go: the next space for a flick faster than 0.6 pages/s, else the nearest. Fingers moving
/// left (negative offset) go to the next space.
fn swipe_step(offset: f32, velocity: f32, width: f32) -> isize {
    let pages_per_s = -velocity / width;
    if pages_per_s.abs() > FLICK { pages_per_s.signum() as isize } else { (-offset / width).round() as isize }
}

#[cfg(test)]
mod tests {
    use super::swipe_step;

    #[test]
    fn released_pages_go_to_the_nearest_space_or_follow_a_flick() {
        assert_eq!(swipe_step(-40., 0., 250.), 0);    // a short drag springs back
        assert_eq!(swipe_step(-140., 0., 250.), 1);   // past half a page: next space
        assert_eq!(swipe_step(140., 0., 250.), -1);
        assert_eq!(swipe_step(-30., -400., 250.), 1); // 1.6 pages/s flick left
        assert_eq!(swipe_step(30., 400., 250.), -1);
        assert_eq!(swipe_step(-30., -100., 250.), 0); // 0.4 pages/s is no flick
    }
}
