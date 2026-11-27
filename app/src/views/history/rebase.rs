//! Interactive rebase from History (InteractiveRebase.swift's Rebaser, RebaseMenu, RebaseSheets, RewordSheet and
//! RebasePlannerSheet): the context menu's rewrites, drag and drop in the list, the confirmations, the Reword sheet
//! and the planner. The plan, the todo and running it are loupe-core's (`rebase`).
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
    component::{
        Selectable as _, Sizable as _,
        button::Button,
        input::{InputEvent, Textarea, TextareaState},
        menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
        tooltip::Tooltip,
    },
};
use loupe_core::history::Commit;
use loupe_core::rebase::{self, RebaseAction, RebaseItem, RebaseRange, Zone};

use super::HistoryView;
use super::sheets::Dialog;
use crate::state::{AppModel, Tab};
use crate::{theme, ui};

impl HistoryView {
    /// RB-02. Gated by what a rebase can reach (HEAD's first parents back to the first merge), as drag and drop is:
    /// Swift left these enabled below a merge and failed later with the merge error (fixes §14 History #5).
    pub(super) fn rebase_menu(&mut self, menu: PopupMenu, c: &Commit, cx: &mut Context<Self>) -> PopupMenu {
        let Some(l) = self.log(cx) else { return menu };
        let head = self.selected_model().map(|m| m.read(cx).state.head_oid.clone()).unwrap_or_default();
        let local = l.movable.contains(&c.sha);
        let linear = c.parents.len() < 2 && local;
        let parent_ok = local && c.parents.len() == 1 && (l.movable.contains(&c.parents[0]) || l.root.as_ref() == Some(&c.parents[0]));
        let child_ok = l.index_of(&c.sha).and_then(|i| i.checked_sub(1)).is_some_and(|i| l.movable.contains(&l.commits[i].sha));
        let running = l.rebasing;
        let me = cx.entity();
        let item = |label: &str, enabled: bool, f: Rc<dyn Fn(&mut HistoryView, &mut Window, &mut Context<HistoryView>)>| {
            let me = me.clone();
            PopupMenuItem::new(label.to_string()).disabled(!enabled || running)
                .on_click(move |_, w, cx| { let f = f.clone(); me.update(cx, |v, cx| f(v, w, cx)) })
        };
        let (a, b, s, f, u, d, x, p) = (c.clone(), c.clone(), c.clone(), c.clone(), c.clone(), c.clone(), c.clone(), c.clone());
        menu.item(item("Reword…", linear, Rc::new(move |v, w, cx| v.begin_reword(a.clone(), w, cx))))
            .item(item("Edit (Stop Here)", linear, Rc::new(move |v, w, cx| v.set_action(RebaseAction::Edit, &b, w, cx))))
            .item(item("Squash into Parent", parent_ok, Rc::new(move |v, w, cx| v.meld_into_parent(&s, false, w, cx))))
            .item(item("Fixup into Parent", parent_ok, Rc::new(move |v, w, cx| v.meld_into_parent(&f, true, w, cx))))
            .item(item("Move Up", c.sha != head && linear && child_ok, Rc::new(move |v, w, cx| v.shift(&u, true, w, cx))))
            .item(item("Move Down", parent_ok, Rc::new(move |v, w, cx| v.shift(&d, false, w, cx))))
            .item(item("Drop…", linear, Rc::new(move |v, w, cx| v.set_action(RebaseAction::Drop, &x, w, cx))))
            .separator()
            .item(item("Interactive Rebase from Here…", local, Rc::new(move |v, w, cx| v.open_planner(&p, w, cx))))
    }

    pub(super) fn can_drop(&self, dragged: &str, target: &str, cx: &App) -> bool {
        self.log(cx).is_some_and(|l| dragged != target && l.movable.contains(dragged) && !l.rebasing)
    }

    /// RB-03: dropped above / below / onto another row. The older of the two starts the range.
    pub(super) fn drop_commit(&mut self, sha: String, zone: Zone, target: String, fixup: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_drop(&sha, &target, cx) { return }
        let Some(oldest) = self.log(cx).and_then(|l| l.commits.iter().rev().find(|c| c.sha == sha || c.sha == target)).map(|c| c.sha.clone()) else { return };
        let label = if zone == Zone::Onto { if fixup { "Fixup" } else { "Squash" } } else { "Reorder Commits" };
        self.rewrite(label, oldest, move |items| rebase::move_item(items, &sha, zone, &target, fixup), window, cx);
    }

    fn set_action(&mut self, action: RebaseAction, c: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        let sha = c.sha.clone();
        self.rewrite(action.title(), c.sha.clone(), move |items| rebase::with_action(items, &sha, action, None), window, cx);
    }

    /// RB-06: the parent is the oldest commit rewritten.
    fn meld_into_parent(&mut self, c: &Commit, fixup: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = c.parents.first().cloned() else { return };
        let sha = c.sha.clone();
        let action = if fixup { RebaseAction::Fixup } else { RebaseAction::Squash };
        self.rewrite(if fixup { "Fixup into Parent" } else { "Squash into Parent" }, p, move |items| rebase::with_action(items, &sha, action, None), window, cx);
    }

    /// RB-07: swaps the commit with the row above it (up) or its parent (down).
    fn shift(&mut self, c: &Commit, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sha = c.sha.clone();
        if up {
            let Some(child) = self.log(cx).and_then(|l| l.index_of(&sha).and_then(|i| i.checked_sub(1)).map(|i| l.commits[i].sha.clone())) else { return };
            self.rewrite("Move Commit Up", sha.clone(), move |items| rebase::move_item(items, &sha, Zone::Above, &child, false), window, cx);
        } else {
            let Some(p) = c.parents.first().cloned() else { return };
            let p2 = p.clone();
            self.rewrite("Move Commit Down", p, move |items| rebase::move_item(items, &sha, Zone::Below, &p2, false), window, cx);
        }
    }

    fn begin_reword(&mut self, c: Commit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let dir = m.read(cx).path().to_path_buf();
        let (app, me) = (self.app.clone(), cx.entity().downgrade());
        cx.spawn_in(window, async move |_, cx| {
            let sha = c.sha.clone();
            let msg = cx.background_executor().spawn(async move { rebase::commit_message(&dir, &sha) }).await.unwrap_or(c.subject.clone());
            cx.update(|w, cx| RewordSheet::present(&app, me, c.sha.clone(), msg, w, cx)).ok();
        }).detach();
    }

    pub(super) fn open_planner(&mut self, c: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let (dir, sha) = (m.read(cx).path().to_path_buf(), c.sha.clone());
        let (app, me) = (self.app.clone(), cx.entity().downgrade());
        cx.spawn_in(window, async move |_, cx| {
            let r = cx.background_executor().spawn(async move { rebase::rebase_range(&sha, &dir) }).await;
            cx.update(|w, cx| match r {
                Ok(range) => Planner::present(&app, me, range, w, cx),
                Err(e) => { m.update(cx, |m, cx| m.set_error(e, cx)); }
            }).ok();
        }).detach();
    }

    /// Loads `oldest^..HEAD`, applies `change` and submits it.
    fn rewrite(&mut self, label: &str, oldest: String, change: impl FnOnce(&[RebaseItem]) -> Vec<RebaseItem> + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let dir = m.read(cx).path().to_path_buf();
        let label = label.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { rebase::rebase_range(&oldest, &dir) }).await;
            this.update_in(cx, |v, w, cx| match r {
                Ok(range) => { let plan = change(&range.items); v.submit(&label, range, plan, false, false, w, cx) }
                Err(e) => m.update(cx, |m, cx| m.set_error(e, cx)),
            }).ok();
        }).detach();
    }

    /// RB-08: asks first when the plan rewrites commits a remote has or drops one. The planner passes
    /// `pushed_ok` (its footer already says what's pushed) but its drops are still confirmed (fixes §14 History #7).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn submit(&mut self, label: &str, range: RebaseRange, plan: Vec<RebaseItem>, pushed_ok: bool, drops_ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        let original: Vec<String> = range.items.iter().map(|i| i.sha.clone()).collect();
        if rebase::first_change(&original, &plan).is_none() { return } // nothing changes
        if let Err(e) = rebase::build(&plan, "/") {
            if let Some(m) = self.selected_model() { m.update(cx, |m, cx| m.set_error(e, cx)) }
            return;
        }
        let pushed = if pushed_ok { vec![] } else { rebase::rewritten_pushed(&range, &plan) };
        let dropping: Vec<RebaseItem> = if drops_ok { vec![] } else { plan.iter().filter(|i| i.action == RebaseAction::Drop).cloned().collect() };
        if pushed.is_empty() && dropping.is_empty() { return self.execute(label.to_string(), range, plan, window, cx) }
        let (title, message, button) = if !pushed.is_empty() {
            let list: Vec<String> = pushed.iter().take(8).map(|i| format!("{} {}", i.short(), i.subject())).collect();
            (if pushed.len() == 1 { "Rewrite a pushed commit?".to_string() } else { format!("Rewrite {} pushed commits?", pushed.len()) },
             format!("A remote already has these commits:\n{}{}\n\nAfter the rewrite the branch needs a force push, and anyone who built on them has to rebase.",
                     list.join("\n"), if pushed.len() > 8 { "\n…" } else { "" }),
             "Rewrite Anyway")
        } else if dropping.len() == 1 {
            (format!("Drop “{}”?", dropping[0].subject()), "Its changes leave the branch. Undo can bring it back while Loupe is open; after that, git reflog still has it.".into(), "Drop Commit")
        } else {
            (format!("Drop {} commits?", dropping.len()), "Their changes leave the branch. Undo can bring them back while Loupe is open; after that, git reflog still has them.".into(), "Drop Commits")
        };
        let me = cx.entity().downgrade();
        let label = label.to_string();
        Dialog::confirm(&self.app, title, message, button, move |w, cx| {
            let (label, range, plan) = (label.clone(), range.clone(), plan.clone());
            me.update(cx, |v, cx| v.submit(&label, range, plan, true, true, w, cx)).ok();
        }, &self.focus, window, cx);
    }

    /// RB-11: one rewrite at a time; recorded for undo when it finishes here, sent to Changes when it stops.
    fn execute(&mut self, label: String, range: RebaseRange, plan: Vec<RebaseItem>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(m), Some(path)) = (self.selected_model(), self.path(cx)) else { return };
        let Some(l) = self.logs.get_mut(&path) else { return };
        if l.rebasing { return }
        l.rebasing = true;
        cx.notify();
        let t = m.update(cx, |m, cx| m.perform("Rebasing", move |p| rebase::execute(&label, &range, &plan, p), cx));
        let app = self.app.clone();
        cx.spawn_in(window, async move |this, cx| {
            let stopped = t.await;
            if stopped == Some(true) { app.update(cx, |a, cx| a.set_tab(Tab::Changes, cx)); }
            this.update_in(cx, |v, w, cx| {
                if let Some(l) = v.logs.get_mut(&path) { l.rebasing = false }
                // Always, like Swift's history.reload() after a rewrite: a failed one leaves HEAD where it was, and a
                // status refresh mid-rebase can load the list while HEAD is detached on the final commit, so the key
                // (HEAD oid) doesn't move when the branch is reattached and the rows kept a "HEAD" badge.
                v.reload(w, cx);
                cx.notify();
            }).ok();
        }).detach();
    }
}

// MARK: - Reword

/// RB-04: the whole message in a monospaced editor. ⌘↩ rewords (Swift's Return was swallowed by the editor, §14
/// History #9); Esc cancels.
pub struct RewordSheet {
    app: Entity<AppModel>,
    history: WeakEntity<HistoryView>,
    sha: String,
    text: Entity<TextareaState>,
    focus: FocusHandle,
    _sub: gpui_kit::Subscription,
}

impl RewordSheet {
    fn present(app: &Entity<AppModel>, history: WeakEntity<HistoryView>, sha: String, message: String, window: &mut Window, cx: &mut App) {
        let a = app.clone();
        let v = cx.new(|cx| {
            // The caret starts after the message, as in Swift's TextEditor, so typing adds to it.
            let end = gpui_kit::component::input::Position::new(message.matches('\n').count() as u32, u32::MAX);
            let text = cx.new(|cx| {
                let mut t = TextareaState::new(window, cx);
                t.set_value(message, window, cx);
                t.set_cursor_position(end, window, cx);
                t
            });
            let sub = cx.subscribe_in(&text, window, |v: &mut Self, _, e: &InputEvent, w, cx| match e {
                InputEvent::PressEnter { secondary: true, .. } => v.finish(w, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            });
            RewordSheet { app: a, history, sha, text, focus: cx.focus_handle(), _sub: sub }
        });
        v.update(cx, |v, cx| v.text.update(cx, |t, cx| t.focus(window, cx)));
        app.update(cx, |a, cx| a.present(v.into(), cx));
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text.read(cx).value().trim().to_string();
        if text.is_empty() { return }
        self.app.update(cx, |a, cx| a.dismiss(cx));
        let sha = self.sha.clone();
        self.history.update(cx, |v, cx| {
            let s = sha.clone();
            v.rewrite("Reword", sha, move |items| rebase::with_action(items, &s, RebaseAction::Reword, Some(text)), window, cx)
        }).ok();
    }
}

impl Render for RewordSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ok = !self.text.read(cx).value().trim().is_empty();
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" { v.app.update(cx, |a, cx| a.dismiss(cx)) }))
            .child(ui::card().w(px(560.)).p_4().gap_3().bg(theme::raised())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Reword {}", &self.sha[..self.sha.len().min(8)])))
                .child(div().h(px(180.)).p_1().rounded(px(6.)).bg(theme::canvas()).border_1().border_color(theme::border_strong())
                    .font_family(theme::CODE_FONT).child(Textarea::new(&self.text).appearance(false).h_full()))
                .child(div().flex().items_center().justify_end().gap_2()
                    .child(ui::kbd("⌘↩"))
                    .child(ui::pill("reword-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.app.update(cx, |a, cx| a.dismiss(cx)))))
                    .child(ui::pill_primary("reword-ok", "Reword", theme::blue()).when(!ok, |d| d.opacity(0.4))
                        .when(ok, |d| d.on_click(cx.listener(|v, _, w, cx| v.finish(w, cx)))))))
    }
}

// MARK: - Planner

/// RB-10: `oldest^..HEAD` with an action per commit, newest on top like History; drag rows to reorder.
pub struct Planner {
    app: Entity<AppModel>,
    history: WeakEntity<HistoryView>,
    range: RebaseRange,
    items: Vec<RebaseItem>, // newest first, as shown
    editors: HashMap<String, (Entity<TextareaState>, gpui_kit::Subscription)>,
    focus: FocusHandle,
}

#[derive(Clone)]
struct PlanDrag(usize, String);

impl Planner {
    fn present(app: &Entity<AppModel>, history: WeakEntity<HistoryView>, range: RebaseRange, window: &mut Window, cx: &mut App) {
        let a = app.clone();
        let items: Vec<RebaseItem> = range.items.iter().rev().cloned().collect();
        let v = cx.new(|cx| Planner { app: a, history, range, items, editors: HashMap::new(), focus: cx.focus_handle() });
        let f = v.read(cx).focus.clone();
        f.focus(window, cx);
        app.update(cx, |a, cx| a.present(v.into(), cx));
    }

    fn plan(&self) -> Vec<RebaseItem> { self.items.iter().rev().cloned().collect() } // oldest first, as git applies it

    fn set_action(&mut self, i: usize, action: RebaseAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get_mut(i) else { return };
        item.action = action;
        if matches!(action, RebaseAction::Reword | RebaseAction::Squash) && !self.editors.contains_key(&item.sha) {
            let (sha, text) = (item.sha.clone(), item.effective_message().to_string());
            let e = cx.new(|cx| { let mut t = TextareaState::new(window, cx); t.set_value(text, window, cx); t });
            let s = sha.clone();
            let sub = cx.subscribe(&e, move |v: &mut Self, e, ev: &InputEvent, cx| if matches!(ev, InputEvent::Change) {
                let t = e.read(cx).value().to_string();
                if let Some(it) = v.items.iter_mut().find(|i| i.sha == s) { it.new_message = Some(t) }
                cx.notify()
            });
            self.editors.insert(sha, (e, sub));
        }
        cx.notify();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (range, plan) = (self.range.clone(), self.plan());
        self.app.update(cx, |a, cx| a.dismiss(cx));
        self.history.update(cx, |v, cx| v.submit("Interactive Rebase", range, plan, true, false, window, cx)).ok();
    }
}

impl Render for Planner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan();
        let problem = rebase::build(&plan, "/").err().map(|e| e.to_string());
        let pushed = rebase::rewritten_pushed(&self.range, &plan);
        let original: Vec<String> = self.range.items.iter().map(|i| i.sha.clone()).collect();
        let can_start = problem.is_none() && rebase::first_change(&original, &plan).is_some();
        let n = self.items.len();
        let base = self.range.base.as_ref().map(|b| b[..b.len().min(8)].to_string()).unwrap_or_else(|| "the root".into());
        let warning = problem.clone().or_else(|| (!pushed.is_empty()).then(|| {
            let names: Vec<&str> = pushed.iter().take(3).map(|i| i.short()).collect();
            format!("Rewrites {} pushed commit{}: {}{}", pushed.len(), if pushed.len() == 1 { "" } else { "s" }, names.join(", "), if pushed.len() > 3 { "…" } else { "" })
        }));
        let me = cx.entity();
        let rows = self.items.iter().enumerate().map(|(i, item)| {
            let is_pushed = self.range.pushed.contains(&item.sha);
            let drop = item.action == RebaseAction::Drop;
            let subject = item.new_message.as_ref().map(|m| m.lines().next().unwrap_or("").to_string()).unwrap_or_else(|| item.subject().to_string());
            let editor = matches!(item.action, RebaseAction::Reword | RebaseAction::Squash).then(|| self.editors.get(&item.sha).map(|e| e.0.clone())).flatten();
            let tip = if item.action == RebaseAction::Squash { "This commit's part of the combined message" } else { "The new message" };
            let (m1, m2) = (me.clone(), me.clone());
            div().id(SharedString::from(format!("plan-{}", item.sha))).flex().flex_col().gap_1().px_3().py_1p5().border_b_1().border_color(theme::border())
                .on_drag(PlanDrag(i, subject.clone()), |d, _, _, cx| cx.new(|_| super::DragPreview(d.1.clone())))
                .drag_over::<PlanDrag>(|s, _, _, _| s.bg(theme::hover()))
                .on_drop::<PlanDrag>(move |d, _, cx| m1.update(cx, |v, cx| {
                    if d.0 < v.items.len() && d.0 != i { let it = v.items.remove(d.0); v.items.insert(i.min(v.items.len()), it); cx.notify() }
                }))
                .child(div().flex().items_center().gap_2()
                    .child(Button::new(SharedString::from(format!("plan-action-{i}"))).label(item.action.title()).small().outline().dropdown_caret(true).selected(false)
                        .dropdown_menu(move |mut menu, _, _| {
                            for a in RebaseAction::ALL {
                                let m = m2.clone();
                                menu = menu.item(PopupMenuItem::new(a.title()).on_click(move |_, w, cx| m.update(cx, |v, cx| v.set_action(i, a, w, cx))));
                            }
                            menu
                        }))
                    .child(div().flex_none().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_muted()).child(item.short().to_string()))
                    .child(div().flex_1().min_w_0().truncate().when(drop, |d| d.line_through().text_color(theme::text_muted())).child(subject))
                    .when(is_pushed, |d| d.child(div().id(SharedString::from(format!("pushed-{i}"))).tooltip(|w, cx| Tooltip::new("A remote already has this commit").build(w, cx))
                        .child(ui::icon("cloud").size(px(13.))))))
                .when_some(editor, |d, e| d.child(div().id(SharedString::from(format!("plan-msg-{i}"))).h(px(64.)).p_1().rounded(px(4.)).bg(theme::canvas()).border_1().border_color(theme::border())
                    .font_family(theme::CODE_FONT).tooltip(move |w, cx| Tooltip::new(tip).build(w, cx)).child(Textarea::new(&e).appearance(false).h_full())))
        }).collect::<Vec<_>>();
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" { v.app.update(cx, |a, cx| a.dismiss(cx)) }))
            .child(ui::card().w(px(700.)).h(px(520.)).bg(theme::raised())
                .child(div().flex().flex_col().gap(px(2.)).p(px(14.)).border_b_1().border_color(theme::border())
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Interactive Rebase"))
                    .child(div().text_size(px(11.5)).text_color(theme::text_muted())
                        .child(format!("{n} commit{} onto {base}. Newest on top; drag to reorder. Squash and fixup meld into the commit below.", if n == 1 { "" } else { "s" }))))
                .child(div().id("plan-list").flex_1().min_h_0().overflow_y_scroll().children(rows))
                .child(div().flex().items_center().gap_2().p_3().border_t_1().border_color(theme::border()).text_size(px(12.5))
                    .when_some(warning, |d, w| d.child(div().flex().items_center().gap_1p5().min_w_0().text_color(theme::accent())
                        .child(ui::icon("triangle-alert").size(px(13.)).text_color(theme::accent())).child(div().line_clamp(2).child(w))))
                    .child(div().flex_1())
                    .child(ui::pill("plan-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.app.update(cx, |a, cx| a.dismiss(cx)))))
                    .child(ui::pill_primary("plan-start", "Start Rebase", theme::blue()).when(!can_start, |d| d.opacity(0.4))
                        .when(can_start, |d| d.on_click(cx.listener(|v, _, w, cx| v.start(w, cx)))))))
    }
}
