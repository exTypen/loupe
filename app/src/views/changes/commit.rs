//! The commit box (CommitBox): Summary and Description over one message, the AI button (CommitAI), Amend, Stash…
//! and Commit.
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
    component::input::{Input, Textarea},
    component::tooltip::Tooltip,
};
use loupe_forge::Cancel;
use loupe_forge::{ai, settings};

use super::ChangesView;
use crate::state::WorktreeModel;
use crate::{theme, ui};

/// CM-02: the Summary is the first line, the Description everything after the blank line.
pub(crate) fn split(msg: &str) -> (String, String) {
    match msg.split_once('\n') {
        Some((t, d)) => (t.to_string(), d.trim_start_matches('\n').to_string()),
        None => (msg.to_string(), String::new()),
    }
}

pub(crate) fn join(t: &str, d: &str) -> String { if d.is_empty() { t.to_string() } else { format!("{t}\n\n{d}") } }

impl ChangesView {
    fn fields(&self, cx: &gpui_kit::App) -> (String, String) {
        (self.summary.read(cx).value().to_string(), self.description.read(cx).value().to_string())
    }

    /// Model → fields, when the message changed under them (a commit cleared it, Amend or the AI filled it, an undo
    /// put it back). What the fields already say is left alone, so a Return at the start of the Description stays
    /// (§14: Swift ate it).
    pub(crate) fn sync_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.selected().map(|m| m.read(cx).state.commit_message.clone()).unwrap_or_default();
        let (t, d) = self.fields(cx);
        if join(&t, &d) == msg { return }
        let (t, d) = split(&msg);
        self.summary.update(cx, |i, cx| i.set_value(t, window, cx));
        self.description.update(cx, |i, cx| i.set_value(d, window, cx));
    }

    /// Fields → model. Newlines typed or pasted into the Summary become spaces.
    pub(crate) fn message_edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (t, d) = self.fields(cx);
        if t.contains('\n') { let flat = t.replace('\n', " "); self.summary.update(cx, |i, cx| i.set_value(flat, window, cx)); return }
        let msg = join(&t, &d);
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.set_commit_message(msg, cx)) }
    }

    /// CM-04, and not while git runs (§14: a double ⌘↩ made a second commit that failed).
    pub(crate) fn can_commit(&self, cx: &gpui_kit::App) -> bool {
        let Some(m) = self.selected() else { return false };
        let s = &m.read(cx).state;
        !self.fields(cx).0.trim().is_empty() && (!s.staged().is_empty() || s.amend) && s.busy.is_none()
    }

    pub(crate) fn stash(&mut self, message: String, untracked: bool, paths: Vec<String>, cx: &mut Context<Self>) {
        if let Some(m) = self.selected() { m.update(cx, |m, cx| m.stash(message, untracked, paths, cx).detach()) }
    }

    /// SS-02
    pub(crate) fn stash_all(&mut self, cx: &mut Context<Self>) {
        if !self.stash_open { return }
        let msg = self.stash_msg.read(cx).value().to_string();
        self.stash_open = false;
        self.stash(msg, self.stash_untracked, vec![], cx);
        // The next frame has a window to clear the field with.
        let input = self.stash_msg.clone();
        cx.defer(move |cx| if let Some(w) = cx.active_window() { w.update(cx, |_, w, cx| input.update(cx, |i, cx| i.set_value("", w, cx))).ok(); });
        cx.notify();
    }

    fn ai_tool(&self) -> Option<ai::Selected> { ai::selected(&settings::shared(), self.ai_paths.as_ref()?) }

    /// CM-09: the reply replaces the message, unless something was typed meanwhile (§14: Swift overwrote it).
    fn generate(&mut self, m: Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let Some(tool) = self.ai_tool() else { return };
        let (dir, amend, staged, before) = { let s = &m.read(cx).state; (m.read(cx).path_str(), s.amend, !s.staged().is_empty(), s.commit_message.clone()) };
        let key = dir.clone();
        let task = cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                let p = settings::shared().clone();
                ai::message(Some(&tool), &p, &dir, amend, staged, Some(&Cancel::new()))
            }).await;
            this.update(cx, |v, cx| { v.generating = None; cx.notify() }).ok();
            m.update(cx, |m, cx| match r {
                Ok(msg) => if m.state.commit_message == before { m.set_commit_message(msg, cx) },
                Err(e) => m.set_error(e, cx),
            });
        });
        self.generating = Some((key, task));
        cx.notify();
    }

    pub(crate) fn render_commit_box(&mut self, m: &Entity<WorktreeModel>, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (staged, amend, has_files, msg_empty, busy) = {
            let s = &m.read(cx).state;
            (s.staged().len(), s.amend, !s.files().is_empty(), s.commit_message.is_empty(), s.busy.is_some())
        };
        let can = self.can_commit(cx);
        let tool = self.ai_tool();
        let generating = self.generating.as_ref().is_some_and(|g| g.0 == m.read(cx).path_str());
        let usable = has_files || amend;
        let m2 = m.clone();
        let label = if amend { "Amend".to_string() } else if staged == 0 { "Commit".into() } else { format!("Commit {staged}") };
        let field = || div().px_2().rounded(theme::RADIUS_ROW).bg(theme::raised()).border_1().border_color(theme::border());
        div().relative().flex().flex_col().gap_2().p_2p5().flex_none().bg(theme::card())
            .child(div().flex().items_center().gap_1p5()
                .child(field().flex_1().h(px(28.)).flex().items_center().child(Input::new(&self.summary).appearance(false)))
                // CM-08
                .when_some(tool, |d, t| d.child(if generating {
                    div().w(px(20.)).flex().justify_center().child(ui::icon("loader").text_color(theme::text_faint())).into_any_element()
                } else {
                    let tip = format!("{} the message with {}, from the staged changes (or all of them when nothing is staged)", if msg_empty { "Write" } else { "Rewrite" }, t.harness.name);
                    div().id("ai").flex().items_center().justify_center().size(px(24.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(theme::hover()))
                        .child(ui::icon("sparkles").size(px(15.)).text_color(if usable { theme::blue() } else { theme::text_faint() }))
                        .when(!usable, |d| d.opacity(0.5))
                        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                        .when(usable, |d| d.on_click(cx.listener(move |v, _, _, cx| v.generate(m2.clone(), cx))))
                        .into_any_element()
                })))
            .child(field().h(px(56.)).py_1().child(Textarea::new(&self.description).appearance(false).h_full()))
            .child(div().flex().items_center().gap_2()
                .child(div().id("amend").flex().items_center().gap_1p5().cursor_pointer().text_size(px(12.5))
                    .child(ui::icon(if amend { "square-check" } else { "square" }).size(px(14.)))
                    .child("Amend")
                    .on_click(cx.listener(|v, _, _, cx| if let Some(m) = v.selected() { m.update(cx, |m, cx| { let on = !m.state.amend; m.set_amend(on, cx) }) })))
                .child(div().flex_1())
                .child(ui::pill("stash", "Stash…").h(px(26.))
                    .when(!has_files, |d| d.opacity(0.4))
                    .tooltip(|w, cx| Tooltip::new("Set all changes aside without committing them").build(w, cx))
                    .when(has_files, |d| d.on_click(cx.listener(|v, _, w, cx| {
                        v.stash_open = !v.stash_open;
                        if v.stash_open { v.stash_msg.update(cx, |i, cx| i.focus(w, cx)) }
                        cx.notify()
                    }))))
                .child(ui::pill_primary("commit", label, theme::blue()).h(px(26.)).min_w(px(70.))
                    .when(!can || busy, |d| d.opacity(0.4))
                    .when(can, |d| d.on_click(cx.listener(|v, _, _, cx| v.commit(cx))))))
            .when(self.stash_open && has_files, |d| d.child(self.stash_popover(cx)))
            .into_any_element()
    }

    /// SS-01: over the box, above the button.
    fn stash_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.stash_untracked;
        div().absolute().bottom(px(44.)).right(px(10.)).w(px(288.)).p_3().flex().flex_col().gap_2p5()
            .rounded(theme::RADIUS_CARD).bg(theme::raised()).border_1().border_color(theme::border_strong()).shadow_lg()
            .on_mouse_down_out(cx.listener(|v, _, _, cx| { v.stash_open = false; cx.notify() }))
            .child(div().h(px(28.)).px_2().flex().items_center().rounded(theme::RADIUS_ROW).bg(theme::canvas()).border_1().border_color(theme::border())
                .child(Input::new(&self.stash_msg).appearance(false)))
            .child(div().id("stash-untracked").flex().items_center().gap_1p5().cursor_pointer().text_size(px(12.5))
                .child(ui::icon(if on { "square-check" } else { "square" }).size(px(14.)))
                .child("Include untracked files")
                .on_click(cx.listener(|v, _, _, cx| { v.stash_untracked = !v.stash_untracked; cx.notify() })))
            .child(div().flex().justify_end().child(ui::pill_primary("stash-go", "Stash", theme::blue()).h(px(26.))
                .on_click(cx.listener(|v, _, _, cx| v.stash_all(cx)))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_and_description_share_one_message() {
        assert_eq!(split("Fix it\n\nBecause."), ("Fix it".into(), "Because.".into()));
        assert_eq!(split("Fix it"), ("Fix it".into(), String::new()));
        assert_eq!(join("Fix it", ""), "Fix it");
        assert_eq!(join("Fix it", "Because."), "Fix it\n\nBecause.");
        // A Return typed at the start of the Description survives a round trip through the model.
        let (t, d) = split(&join("T", "\nbody"));
        assert_eq!((t.as_str(), d.as_str()), ("T", "body"));
    }
}
