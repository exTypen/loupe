//! CH-09: the Overview of uncommitted work, the code view's `ChangeOverview` of HEAD against the worktree. It
//! recomputes per "<head> <status version>" and keeps the last report on screen meanwhile (§14 lenses 5). A click
//! opens the file at its line through the code browser: a changed file in the list and diff, any other in place
//! (§14 lenses 4: Swift dropped the line).
use std::rc::Rc;

use gpui_kit::{AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Styled as _, div};

use super::ChangesView;
use crate::state::WorktreeModel;
use crate::views::code::{ChangeOverview, overview::{OnOpen, worktree_input}};

impl ChangesView {
    pub(crate) fn update_overview(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let input = worktree_input(m.read(cx));
        if let Some(o) = &self.ov { o.update(cx, |o, cx| o.set_input(input, cx)); return }
        let me = cx.entity().downgrade();
        let on_open: OnOpen = Rc::new(move |p, line, _, cx| {
            me.update(cx, |v, cx| {
                v.overview = false;
                v.ov = None;
                if let Some(b) = v.browser() { b.update(cx, |b, cx| b.open_from_overview(&p, line, cx)) }
                v.update(cx);
            }).ok();
        });
        self.ov = Some(cx.new(|cx| ChangeOverview::new(input, on_open, cx)));
    }

    pub(crate) fn render_overview(&self) -> AnyElement {
        div().size_full().children(self.ov.clone()).into_any_element()
    }
}
