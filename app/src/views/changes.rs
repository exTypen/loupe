//! The Changes tab (ChangesView.swift). Owner: the Changes view agent (docs/gpui/VIEWS.md).
//!
//! What's here is the integrator's minimal but real version: the status list (conflicted, staged, unstaged), the
//! selected file's diff with hunk/line Stage and Unstage, file Stage/Unstage, and a commit box (⌘↩). The full view
//! (tree list, multi-select, discard with confirmation, stash, blame/file history, inspector, LFS/submodule rows,
//! overview, banners, commit AI) replaces it.
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
    component::input::{InputEvent, Textarea, TextareaState},
};
use loupe_core::git::{FileChange, Kind};
use loupe_core::model::HunkAction as CoreHunk;

use crate::diff::{DiffView, HunkAction};
use crate::shell::keys::Commit;
use crate::state::{AppModel, DiffControls, Tab, WorktreeModel, adapt};
use crate::{theme, ui};

pub struct ChangesView {
    app: Entity<AppModel>,
    model: Option<(Entity<WorktreeModel>, Subscription)>,
    diff: Option<Entity<DiffView>>,
    message: Entity<TextareaState>,
    _subs: Vec<Subscription>,
    // TEMP code-agent wiring (not committed)
    browser: Option<(Entity<crate::views::code::CodeBrowser>, Subscription)>,
    overview: Option<Entity<crate::views::code::ChangeOverview>>,
    overview_on: bool,
    pending_reveal: Option<(String, usize)>,
    was_full: bool,
    last_sel: Option<String>,
}

impl ChangesView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message"));
        let subs = vec![
            cx.observe_in(&app, window, |v, _, w, cx| v.follow_selection(w, cx)),
            cx.subscribe(&message, |v, input, e: &InputEvent, cx| match e {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    if let Some(m) = v.selected() { m.update(cx, |m, cx| m.set_commit_message(text, cx)) }
                }
                InputEvent::PressEnter { secondary: true, .. } => v.commit(cx),
                _ => {}
            }),
        ];
        let mut v = Self { app, model: None, diff: None, message, _subs: subs, browser: None, overview: None, overview_on: false, pending_reveal: None, was_full: false, last_sel: None };
        v.follow_selection(window, cx);
        v
    }

    fn selected(&self) -> Option<Entity<WorktreeModel>> { self.model.as_ref().map(|m| m.0.clone()) }

    fn follow_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.app.read(cx).selected_model();
        if m.as_ref().map(|m| m.entity_id()) == self.model.as_ref().map(|x| x.0.entity_id()) { return }
        self.model = m.map(|m| (m.clone(), cx.observe_in(&m, window, |v, _, w, cx| v.sync(w, cx))));
        self.overview = None; self.overview_on = false;
        self.browser = self.model.as_ref().map(|(m, _)| {
            let b = crate::views::code::browser_for(&self.app, m, window, cx);
            let sub = cx.subscribe(&b, |v, _, e: &crate::views::code::CodeBrowserEvent, cx| {
                let crate::views::code::CodeBrowserEvent::Reveal { path, line } = e;
                v.pending_reveal = Some((path.clone(), *line)); v.apply_reveal(cx);
            });
            (b, sub)
        });
        self.sync(window, cx);
    }

    /// Model → view: the shown diff, the commit box text (cleared by a commit, put back by an undo), the toolbar.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected() else { self.diff = None; cx.notify(); return };
        let (shown, staged, msg, has_file) = {
            let m = m.read(cx);
            (m.diff.clone(), m.state.selected_file().map(|f| f.staged), m.state.commit_message.clone(), m.state.selected_file().is_some())
        };
        match shown {
            Some(file) => {
                let actions = if staged == Some(true) { vec![HunkAction::Unstage] } else { vec![HunkAction::Stage] };
                match &self.diff {
                    Some(d) if Arc::ptr_eq(d.read(cx).file(), &file) => d.update(cx, |d, cx| d.set_hunk_actions(actions, cx)),
                    Some(d) => d.update(cx, |d, cx| { d.set_file(file, cx); d.set_hunk_actions(actions, cx) }),
                    None => self.diff = Some(self.make_diff_view(file, actions, cx)),
                }
            }
            None => self.diff = None,
        }
        if self.message.read(cx).value().as_ref() != msg { self.message.update(cx, |i, cx| i.set_value(msg, window, cx)) }
        self.apply_reveal(cx);
        let full = self.app.read(cx).full_mode;
        let sel = m.read(cx).state.selected_file().map(|f| f.path.clone());
        if let Some(b) = self.browser.as_ref().map(|b| b.0.clone()) {
            if sel != self.last_sel { if self.last_sel.is_some() || sel.is_some() { if !full && let Some(p) = &sel { b.update(cx, |b, cx| b.follow(p, cx)) } } }
            if full != self.was_full {
                if full { if let Some(p) = &sel { b.update(cx, |b, cx| b.show(p, cx)) } }
                else if let Some(p) = b.read(cx).nav.selected.clone() && let Some(c) = b.read(cx).change(&p, cx) { m.update(cx, |m, cx| m.select_file(Some(c.id()), cx)) }
            }
        }
        if sel != self.last_sel && self.last_sel.is_some() { self.overview_on = false; self.overview = None }
        self.last_sel = sel; self.was_full = full;
        let no_files = m.read(cx).state.files().is_empty();
        if self.overview_on && !no_files {
            let input = crate::views::code::overview::worktree_input(m.read(cx));
            match &self.overview {
                Some(o) => o.update(cx, |o, cx| o.set_input(input, cx)),
                None => {
                    let b = self.browser.as_ref().map(|b| b.0.clone());
                    let me = cx.entity().downgrade();
                    let on_open: crate::views::code::overview::OnOpen = std::rc::Rc::new(move |p, l, _, cx| {
                        me.update(cx, |v, cx| v.set_overview(false, cx)).ok();
                        if let Some(b) = &b { b.update(cx, |b, cx| b.open_from_overview(&p, l, cx)) }
                    });
                    self.overview = Some(cx.new(|cx| crate::views::code::ChangeOverview::new(input, on_open, cx)));
                }
            }
        }
        let me = cx.entity().downgrade();
        let view = (!no_files).then(|| (vec![crate::state::ViewMode::Overview, crate::state::ViewMode::Files],
            if self.overview_on { crate::state::ViewMode::Overview } else { crate::state::ViewMode::Files },
            std::rc::Rc::new(move |mode, _: &mut Window, cx: &mut gpui_kit::App| { me.update(cx, |v, cx| v.set_overview(mode == crate::state::ViewMode::Overview, cx)).ok(); }) as std::rc::Rc<dyn Fn(crate::state::ViewMode, &mut Window, &mut gpui_kit::App)>));
        self.app.update(cx, |a, cx| a.set_controls(Tab::Changes, DiffControls { full: !no_files && !self.overview_on && (full || has_file), view }, cx));
        cx.notify();
    }

    fn apply_reveal(&mut self, cx: &mut Context<Self>) {
        let Some((p, l)) = self.pending_reveal.clone() else { return };
        if let Some(d) = &self.diff && d.read(cx).file().path() == p { d.update(cx, |d, cx| d.reveal(l as u32, cx)); self.pending_reveal = None }
    }

    fn set_overview(&mut self, on: bool, cx: &mut Context<Self>) {
        self.overview_on = on;
        if !on { self.overview = None }
        cx.notify();
        let m = self.selected();
        if let Some(m) = m { m.update(cx, |_, cx| cx.notify()) }
    }

    fn make_diff_view(&self, file: Arc<crate::diff::FileDiff>, actions: Vec<HunkAction>, cx: &mut Context<Self>) -> Entity<DiffView> {
        let me = cx.entity().downgrade();
        let me2 = me.clone();
        let (side, layout) = { let a = self.app.read(cx); (a.diff_side, a.layout) };
        let app = self.app.clone();
        cx.new(|cx| {
            let mut d = DiffView::new(file, cx);
            d.set_highlighter(Some(adapt::highlighter()), cx);
            d.set_hunk_actions(actions, cx);
            d.set_side(side, cx);
            d.set_layout(layout, cx);
            d.on_hunk_action(move |a, lines, _, cx| {
                let action = match a { HunkAction::Stage => CoreHunk::Stage, HunkAction::Unstage => CoreHunk::Unstage, HunkAction::Discard => CoreHunk::Discard };
                if let Some(v) = me.upgrade() && let Some(m) = v.read(cx).selected() { m.update(cx, |m, cx| m.apply_lines(action, lines, cx).detach()) }
            });
            d.on_side_change(move |side, layout, _, cx| app.update(cx, |a, cx| a.set_diff_side(side, layout, cx)));
            d.on_navigate(move |w, a, _, cx| {
                if let Some(v) = me2.upgrade() && let Some(b) = v.read(cx).browser.as_ref().map(|b| b.0.clone()) {
                    let from = v.read(cx).diff.as_ref().map(|d| d.read(cx).file().path().to_string()).unwrap_or_default();
                    b.update(cx, |b, cx| b.navigate(w, a, from, cx))
                }
            });
            d
        })
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.commit(cx).detach()) }
    }

    fn row(&self, f: &FileChange, selected: bool, _cx: &mut Context<Self>) -> impl IntoElement {
        let (name, dir) = match f.path.rsplit_once('/') { Some((d, n)) => (n.to_string(), d.to_string()), None => (f.path.clone(), String::new()) };
        let color = match f.kind {
            Kind::Added | Kind::Untracked => theme::green(),
            Kind::Deleted => theme::red(),
            Kind::Conflicted => theme::accent(),
            Kind::Renamed | Kind::Copied => theme::purple(),
            _ => theme::blue(),
        };
        let (id, file, staged) = (f.id(), f.clone(), f.staged);
        let m = self.selected();
        let m2 = m.clone();
        ui::list_row(SharedString::from(format!("f-{}", f.id())), selected).group("file").h(px(26.))
            .child(ui::file_icon(&f.path))
            .child(div().flex().flex_1().min_w_0().gap_1p5().items_baseline()
                .child(div().flex_none().max_w_full().truncate().child(name))
                .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(theme::text_faint()).child(dir)))
            .when(f.kind != Kind::Conflicted, |d| d.child(ui::icon_button(SharedString::from(format!("st-{}", f.id())), if staged { "minus" } else { "plus" })
                .opacity(0.).group_hover("file", |s| s.opacity(1.))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    if let Some(m) = &m2 { let fs = vec![file.clone()]; m.update(cx, |m, cx| if staged { m.unstage(fs, cx).detach() } else { m.stage(fs, cx).detach() }) }
                })))
            .child(div().w(px(12.)).flex_none().text_size(px(11.)).font_family(theme::CODE_FONT).text_color(color).child(f.kind.letter().to_string()))
            .on_click(move |_, _, cx| if let Some(m) = &m { m.update(cx, |m, cx| m.select_file(Some(id.clone()), cx)) })
    }
}

impl Render for ChangesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(m) = self.selected() else { return div().size_full().into_any_element() };
        let (conflicted, staged, unstaged, sel, busy, can_commit, amend) = {
            let s = &m.read(cx).state;
            let msg_ok = !s.commit_message.trim().is_empty();
            (s.conflicted().to_vec(), s.staged().to_vec(), s.unstaged().to_vec(), s.selected_file_id.clone(), s.busy.clone(),
             msg_ok && (!s.staged().is_empty() || s.amend), s.amend)
        };
        let total = m.read(cx).state.change_count();
        let mut list = div().id("changes-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().px_2().pb_2().gap_px();
        for (title, files) in [("Conflicts", &conflicted), ("Staged", &staged), ("Changes", &unstaged)] {
            if files.is_empty() { continue }
            list = list.child(ui::section_header(title, Some(files.len())).px_2().pt_2().pb_1());
            for f in files.iter() { list = list.child(self.row(f, sel.as_deref() == Some(f.id().as_str()), cx)) }
        }
        let (m1, m2) = (m.clone(), m.clone());
        let left = div().flex().flex_col().w(px(300.)).flex_none().h_full().border_r_1().border_color(theme::border())
            .child(if total == 0 { div().flex_1().child(ui::empty_state("circle-check", "No Changes", "The working tree is clean.")).into_any_element() } else { list.into_any_element() })
            .child(div().flex().flex_col().gap_2().p_2().border_t_1().border_color(theme::border())
                .child(div().h(px(84.)).px_2().py_1().rounded(theme::RADIUS_ROW).bg(theme::raised()).border_1().border_color(theme::border())
                    .child(Textarea::new(&self.message).appearance(false).h_full()))
                .child(div().flex().items_center().gap_2()
                    .child(div().id("amend").flex().items_center().gap_1().cursor_pointer().text_size(px(12.)).text_color(theme::text_muted())
                        .child(ui::icon(if amend { "square-check" } else { "square" }).size(px(13.)))
                        .child("Amend")
                        .on_click(move |_, _, cx| m1.update(cx, |m, cx| { let on = !m.state.amend; m.set_amend(on, cx) })))
                    .child(div().flex_1())
                    .when_some(busy, |d, b| d.child(div().text_size(px(12.)).text_color(theme::text_muted()).child(format!("{b}…"))))
                    .child(ui::pill_primary("commit", if amend { "Amend" } else { "Commit" }, theme::blue())
                        .when(!can_commit, |d| d.opacity(0.4))
                        .when(can_commit, |d| d.on_click(move |_, _, cx| m2.update(cx, |m, cx| m.commit(cx).detach()))))));
        let full = self.app.read(cx).full_mode;
        let browser = self.browser.as_ref().map(|b| b.0.clone());
        let peek = browser.as_ref().is_some_and(|b| b.read(cx).peeking());
        let right_code = if self.overview_on && let Some(o) = &self.overview { Some(div().flex_1().min_w_0().h_full().child(o.clone()).into_any_element()) }
            else if (full || peek) && let Some(b) = &browser { Some(div().flex_1().min_w_0().h_full().child(b.clone()).into_any_element()) } else { None };
        let left = if full && let Some(b) = &browser {
            div().flex().flex_col().w(px(300.)).flex_none().h_full().border_r_1().border_color(theme::border())
                .child(div().flex_1().min_h_0().child(crate::views::code::CodeBrowser::tree(b, cx))).into_any_element()
        } else { left.into_any_element() };
        let results = if !full && !peek && !self.overview_on { browser.as_ref().and_then(|b| crate::views::code::CodeBrowser::results_panel(b, cx)) } else { None };
        let right = if let Some(r) = right_code { r } else { let r = match &self.diff {
            Some(d) => div().flex_1().min_w_0().h_full().child(d.clone()).into_any_element(),
            None => div().flex_1().child(ui::empty_state("file", if total == 0 { "" } else { "No File Selected" }, "")).into_any_element(),
        }; div().flex_1().min_w_0().h_full().flex().flex_col().child(div().flex_1().min_h_0().flex().child(r)).children(results).into_any_element() };
        div().flex().size_full().font_weight(FontWeight::NORMAL)
            .on_action(cx.listener(|v, _: &Commit, _, cx| v.commit(cx)))
            .child(left)
            .child(right)
            .into_any_element()
    }
}
