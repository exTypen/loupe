//! The title bar's branch picker (BranchPicker in ChangesView.swift): filter, pick, `git switch`. Remote branches
//! with a local counterpart are hidden; a branch checked out in another worktree selects that worktree.
use std::collections::HashMap;

use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    AppContext as _, Context, DismissEvent, Entity, EventEmitter, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
    component::input::{Input, InputEvent, InputState},
};
use loupe_core::git;

use crate::state::WorktreeModel;
use crate::{theme, ui};

pub struct BranchPicker {
    model: Entity<WorktreeModel>,
    input: Entity<InputState>,
    locals: Vec<String>,
    remotes: Vec<String>,
    checked_out: HashMap<String, String>, // local branch → path of the worktree that has it
    working: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for BranchPicker {}

impl BranchPicker {
    pub fn new(model: Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Switch to branch…"));
        input.update(cx, |i, cx| i.focus(window, cx));
        let subs = vec![cx.subscribe(&input, |this, _, e: &InputEvent, cx| match e {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => {
                // §14: Return with nothing typed switched to the newest other branch. It only picks a match.
                if this.input.read(cx).value().trim().is_empty() { return }
                let current = this.model.read(cx).state.branch.clone();
                let (l, r) = this.matching(cx);
                if let Some((b, remote)) = l.into_iter().map(|b| (b, false)).chain(r.into_iter().map(|b| (b, true))).find(|(b, _)| Some(b) != current.as_ref()) {
                    this.switch_to(b, remote, cx)
                }
            }
            _ => {}
        })];
        let dir = model.read(cx).state.path.clone();
        cx.spawn(async move |this, cx| {
            let b = cx.background_executor().spawn(async move { git::branches(&dir) }).await;
            this.update(cx, |p, cx| { p.locals = b.local; p.remotes = b.remote; p.checked_out = b.checked_out; cx.notify() }).ok();
        }).detach();
        Self { model, input, locals: vec![], remotes: vec![], checked_out: HashMap::new(), working: false, error: None, _subs: subs }
    }

    fn matching(&self, cx: &gpui_kit::App) -> (Vec<String>, Vec<String>) {
        let q = self.input.read(cx).value().to_lowercase();
        let f = |v: &[String]| v.iter().filter(|b| q.is_empty() || b.to_lowercase().contains(&q)).cloned().collect::<Vec<_>>();
        (f(&self.locals), f(&self.remotes))
    }

    fn switch_to(&mut self, b: String, remote: bool, cx: &mut Context<Self>) {
        self.working = true;
        self.error = None;
        let elsewhere = self.checked_out.get(&b).cloned().filter(|p| *p != self.model.read(cx).path_str());
        let t = self.model.update(cx, |m, cx| m.switch_branch(b, remote, elsewhere, cx));
        cx.spawn(async move |this, cx| {
            let ok = t.await.is_some();
            this.update(cx, |p, cx| {
                p.working = false;
                // A refusal stays here, where it was asked for, instead of the window's alert.
                let err = p.model.update(cx, |m, cx| { let e = m.state.error.take(); cx.notify(); e });
                if ok { cx.emit(DismissEvent) } else { p.error = err; cx.notify() }
            }).ok();
        }).detach();
    }

    fn row(&self, b: &str, remote: bool, current: Option<&str>, cx: &mut Context<Self>) -> impl IntoElement {
        let is_current = current == Some(b);
        let elsewhere = self.checked_out.get(b).filter(|_| !is_current).map(|p| super::model::last_component(p).to_string());
        let name = b.to_string();
        ui::list_row(SharedString::from(format!("br-{}-{b}", remote as u8)), false).h(px(28.))
            .child(ui::icon(if remote { "cloud" } else { "git-branch" }).size(px(13.)).text_color(theme::text_muted()))
            .child(div().flex_1().min_w_0().truncate().child(b.to_string()))
            .when(is_current, |d| d.child(ui::icon("check").size(px(12.)).text_color(theme::blue())))
            .when_some(elsewhere, |d, w| d.child(div().text_size(px(11.)).text_color(theme::text_faint()).child(w)))
            .when(!is_current && !self.working, |d| d.on_click(cx.listener(move |p, _, _, cx| p.switch_to(name.clone(), remote, cx))))
            .test_support()
    }
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (l, r) = self.matching(cx);
        let current = self.model.read(cx).state.branch.clone();
        let mut list = div().id("branch-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().p_1().gap_px();
        if !l.is_empty() { list = list.child(ui::section_header("Local", None).px_2().pt_1()) }
        for b in &l { list = list.child(self.row(b, false, current.as_deref(), cx)) }
        if !r.is_empty() { list = list.child(ui::section_header("Remote", None).px_2().pt_2()) }
        for b in &r { list = list.child(self.row(b, true, current.as_deref(), cx)) }
        div().absolute().inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .capture_key_down(cx.listener(|_, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" { cx.emit(DismissEvent); cx.stop_propagation() }))
            .child(div().absolute().top(px(44.)).left(px(90.)).w(px(340.)).h(px(420.)).flex().flex_col().overflow_hidden()
                .bg(theme::raised()).border_1().border_color(theme::border_strong()).rounded(theme::RADIUS_CARD).shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().p_2().child(Input::new(&self.input)))
                .when_some(self.error.clone(), |d, e| d.child(div().px_2().pb_1p5().text_size(px(12.)).text_color(theme::red()).child(e)))
                .child(ui::divider())
                .child(list))
    }
}
