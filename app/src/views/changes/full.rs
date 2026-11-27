//! Full mode in Changes (CH-41/42) and code navigation in Compact (DV-29, CB-*): both are the worktree's
//! `CodeBrowser` (views/code.rs). Full mode puts its tree on the left and the browser (tabs, the whole file with its
//! changes inline, back / forward, outline) on the right, inside the Blame / File History panel. In Compact, ⌘-click
//! in the diff asks the browser, its results show under the diff, and a jump to a file the change doesn't touch
//! shows the browser in place ("peek") instead of switching every view to Full (§14 #8).
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
    component::tooltip::Tooltip,
};
use loupe_core::git::FileChange;

use super::ChangesView;
use super::inspector::FilePanel;
use super::pane::has_target;
use crate::diff::DiffView;
use crate::state::WorktreeModel;
use crate::views::code::{self, CodeBrowser, CodeBrowserEvent};
use crate::{theme, ui};

/// The entry a path shows in Full mode: its unstaged one, else its staged one.
pub(crate) fn change<'a>(files: &'a [FileChange], path: &str) -> Option<&'a FileChange> {
    files.iter().find(|f| f.path == path && !f.staged).or_else(|| files.iter().find(|f| f.path == path))
}

/// Blame / File History toggles for the browser's file header (FI-01..06 in Full mode, as Swift's DiffHeader had).
fn panel_buttons(me: WeakEntity<ChangesView>, path: &str, cx: &App) -> AnyElement {
    let Some(v) = me.upgrade() else { return div().into_any_element() };
    let v = v.read(cx);
    let target = v.selected().is_none_or(|m| change(m.read(cx).state.files(), path).is_none_or(has_target));
    if !target { return div().into_any_element() }
    let on = v.panel.panel;
    let button = |id: &'static str, icon: &'static str, tip: &'static str, p: FilePanel| {
        let me = me.clone();
        ui::icon_button(id, icon).size(px(24.)).when(on == p, |d| d.bg(theme::blue().opacity(0.15)).text_color(theme::blue()))
            .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
            .on_click(move |_, _, cx| { me.update(cx, |v, cx| {
                v.panel.toggle(p);
                v.update(cx);
                // The header is the browser's diff view's: draw it again for the button's state.
                if let Some(d) = v.full_view(cx) { d.update(cx, |_, cx| cx.notify()) }
            }).ok(); })
    };
    div().flex().items_center().gap_1()
        .child(button("full-blame", "square-user", "Blame: who last changed each line", FilePanel::Blame))
        .child(button("full-history", "clock", "File History", FilePanel::History))
        .child(div().w(px(1.)).h(px(14.)).bg(theme::border()))
        .into_any_element()
}

/// The Compact diff's ⌘-click (DV-29) and Go to Definition / Find References go to the browser.
pub(crate) fn wire_navigation(me: WeakEntity<ChangesView>, d: &mut DiffView) {
    d.on_navigate(move |word, action, _, cx| {
        let Some(v) = me.upgrade() else { return };
        let (b, from) = { let v = v.read(cx); (v.browser(), v.selected().and_then(|m| m.read(cx).state.selected_file().map(|f| f.path.clone())).unwrap_or_default()) };
        if let Some(b) = b { b.update(cx, |b, cx| b.navigate(word, action, from, cx)) }
    });
}

impl ChangesView {
    pub(crate) fn browser(&self) -> Option<Entity<CodeBrowser>> { self.browser.as_ref().map(|b| b.0.clone()) }

    /// One browser per worktree (its tabs and history survive switching). A jump to a changed file in Compact comes
    /// back as `Reveal`: the list selects it and the diff lands on the line.
    pub(crate) fn attach_browser(&mut self, m: &Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) {
        let b = code::browser_for(&self.app, m, window, cx);
        let me = cx.entity().downgrade();
        b.update(cx, |b, _| b.header_actions(move |path, _, cx| panel_buttons(me.clone(), path, cx)));
        let subs = vec![
            cx.subscribe(&b, |v, _, e: &CodeBrowserEvent, cx| {
                let CodeBrowserEvent::Reveal { path, line } = e;
                v.pending_reveal = Some((path.clone(), *line as u32));
                v.scroll_to_selected(cx);
                v.update(cx);
            }),
            // Peeking, results and the open tab decide what this view shows and what Blame follows.
            cx.observe(&b, |v, _, cx| v.update(cx)),
        ];
        self.browser = Some((b, subs));
    }

    /// Mode changes and list moves, told to the browser.
    pub(crate) fn update_browser(&mut self, m: &Entity<WorktreeModel>, sel_changed: bool, cx: &mut Context<Self>) {
        let Some(b) = self.browser() else { return };
        let full = self.app.read(cx).full_mode;
        let sel = m.read(cx).state.selected_file().map(|f| f.path.clone());
        if full != self.was_full {
            self.was_full = full;
            if full {
                // CH-10: going Full opens the list's file.
                if let Some(p) = &sel { b.update(cx, |b, cx| b.show(p, cx)) }
            } else {
                // Back to Compact: the open tab's file is selected in the list when it's a change.
                let id = b.read(cx).nav.selected.clone().and_then(|p| b.read(cx).change(&p, cx)).map(|c| c.id());
                if let Some(id) = id { m.update(cx, |m, cx| m.select_file(Some(id), cx)) }
            }
        } else if sel_changed && !full && let Some(p) = &sel {
            b.update(cx, |b, cx| b.follow(p, cx));
        }
    }

    /// The file Full mode shows and its diff uid: what Blame follows there.
    pub(crate) fn full_target(&self, cx: &App) -> Option<(String, u64)> {
        let b = self.browser()?;
        let b = b.read(cx);
        let p = b.nav.selected.clone()?;
        let d = b.diff_view()?.read(cx);
        (d.file().path() == p).then(|| (p, d.file().uid))
    }

    pub(crate) fn full_view(&self, cx: &App) -> Option<Entity<DiffView>> { self.browser().and_then(|b| b.read(cx).diff_view().cloned()) }

    /// CH-41: every file of the worktree.
    pub(crate) fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(b) = self.browser() else { return div().flex_1().into_any_element() };
        // The view's focus lives here in Full mode, as on the list in Compact: a tab switch focuses it (DV-32), and an
        // undrawn handle would swallow every key, ⌘1–⌘3 included, until a click.
        div().id("changes-tree").track_focus(&self.focus).flex_1().min_h_0().child(CodeBrowser::tree(&b, cx)).into_any_element()
    }

    /// CH-42: the browser, inside the Blame / File History panel.
    pub(crate) fn render_full_pane(&mut self, m: &Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(b) = self.browser() else { return div().into_any_element() };
        let path = b.read(cx).nav.selected.clone();
        let target = path.is_some_and(|p| change(m.read(cx).state.files(), &p).is_none_or(has_target));
        let body = div().id("full-browser").size_full().child(b).into_any_element();
        ui::card().size_full().child(self.with_panel(target, body, window, cx)).into_any_element()
    }
}
