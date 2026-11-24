//! CH-09: the Overview of uncommitted work (ChangeOverview of HEAD against the worktree). The lens analysis is
//! loupe_code's; the rich cards (CO-*) are the code view's. This draws each report's facts plainly and hands a click
//! back to the list. Keyed by "<head> <status version>", but the last report stays on screen while the next one is
//! read (§14 lenses 5: Swift flashed "Reading the change…" on every refresh).
use std::collections::HashMap;

use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Task, div, prelude::FluentBuilder as _, px,
};
use loupe_code::lens::{self, ChangedPath, LensReport, Snapshot};
use loupe_core::git::{FileChange, Kind};

use super::ChangesView;
use super::pane::spinner;
use crate::state::WorktreeModel;
use crate::{theme, ui};

#[derive(Default)]
pub(crate) struct State {
    key: String,
    reports: Option<Vec<LensReport>>,
    _task: Option<Task<()>>,
}

/// One entry per path: deleted files have no new side, new ones no old side, a rename its old path.
pub(crate) fn changed_paths(files: &[FileChange]) -> Vec<ChangedPath> {
    let mut by: HashMap<&str, Vec<&FileChange>> = HashMap::new();
    for f in files { by.entry(&f.path).or_default().push(f) }
    let mut out: Vec<ChangedPath> = by.into_values().map(|fs| {
        let f = fs.iter().find(|f| !f.staged).unwrap_or(&fs[0]);
        let gone = fs.iter().any(|f| f.kind == Kind::Deleted);
        let new = fs.iter().any(|f| matches!(f.kind, Kind::Added | Kind::Untracked));
        ChangedPath { old: (!new).then(|| f.orig_path.clone().unwrap_or_else(|| f.path.clone())), new: (!gone).then(|| f.path.clone()) }
    }).collect();
    out.sort_by(|a, b| a.path().cmp(b.path()));
    out
}

impl ChangesView {
    pub(crate) fn update_overview(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let (key, changes, head, dir) = {
            let s = &m.read(cx).state;
            (format!("{} {}", s.head_oid, s.status_version), changed_paths(s.files()), s.head_oid.clone(), m.read(cx).path_str())
        };
        if key == self.ov.key { return }
        self.ov.key = key.clone();
        let a = if head.is_empty() || head.starts_with('(') { Snapshot::Empty } else { Snapshot::Rev("HEAD".into()) };
        self.ov._task = Some(cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { lens::analyze(&changes, &a, &Snapshot::Worktree, None, &dir) }).await;
            this.update(cx, |v, cx| if v.ov.key == key { v.ov.reports = Some(r); cx.notify() }).ok();
        }));
    }

    /// A click leaves the Overview for that path's unstaged entry (else its staged one) and its line (§14 lenses 4:
    /// Swift dropped the line).
    fn open_fact(&mut self, path: &str, line: Option<usize>, cx: &mut Context<Self>) {
        self.overview = false;
        let Some(m) = self.selected() else { return };
        let id = { let s = &m.read(cx).state; s.files().iter().find(|f| f.path == path && !f.staged).or(s.files().iter().find(|f| f.path == path)).map(FileChange::id) };
        if let Some(id) = id { m.update(cx, |m, cx| m.select_file(Some(id), cx)); self.scroll_to_selected(cx) }
        if let Some(l) = line { self.pending_reveal = Some((path.to_string(), l as u32)) }
        self.update(cx);
    }

    pub(crate) fn render_overview(&mut self, _: &Entity<WorktreeModel>, cx: &mut Context<Self>) -> AnyElement {
        let Some(reports) = self.ov.reports.clone() else { return spinner() };
        let mut col = div().id("overview").size_full().overflow_y_scroll().p_4().flex().flex_col().gap_3();
        let mut k = 0usize;
        for r in &reports {
            let mut card = ui::card().child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child(r.title.clone()), div().child(r.summary.clone())));
            let mut body = div().flex().flex_col().p_2().gap_0p5();
            for s in &r.sections {
                body = body.child(div().pt_1().px_1().text_size(px(11.)).text_color(theme::text_faint()).child(s.title.clone()));
                for f in &s.facts {
                    k += 1;
                    let (path, line) = (f.path.clone(), f.line);
                    body = body.child(div().id(("fact", k)).flex().items_center().gap_2().px_1().h(px(24.)).rounded(theme::RADIUS_ROW)
                        .when(path.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(theme::hover())))
                        .when(f.kind != lens::FactKind::Info, |d| d.child(ui::badge(f.kind.label())))
                        .child(div().flex_none().text_color(theme::text()).child(f.title.clone()))
                        .when_some(f.detail.clone(), |d, t| d.child(div().min_w_0().truncate().text_color(theme::text_muted()).child(t)))
                        .child(div().flex_1())
                        .when_some(path.clone(), |d, p| d.child(div().text_size(px(11.)).text_color(theme::text_faint()).child(p)))
                        .when_some(path, |d, p| d.on_click(cx.listener(move |v, _, _, cx| v.open_fact(&p, line, cx)))));
                }
            }
            if r.sections.is_empty() {
                for (kind, files) in &r.kinds {
                    body = body.child(div().pt_1().px_1().text_size(px(11.)).text_color(theme::text_faint()).child(format!("{kind} {}", files.len())));
                    for c in files {
                        k += 1;
                        let p = c.path().to_string();
                        body = body.child(div().id(("ovf", k)).flex().items_center().gap_2().px_1().h(px(24.)).rounded(theme::RADIUS_ROW).cursor_pointer().hover(|s| s.bg(theme::hover()))
                            .child(ui::file_icon(&p)).child(div().truncate().child(p.clone()))
                            .on_click(cx.listener(move |v, _, _, cx| v.open_fact(&p, None, cx))));
                    }
                }
            }
            card = card.child(body);
            col = col.child(card);
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_entry_per_path() {
        let mut r = FileChange::new("b.rs", Kind::Renamed, true);
        r.orig_path = Some("a.rs".into());
        let fs = vec![FileChange::new("x", Kind::Modified, true), FileChange::new("x", Kind::Modified, false), FileChange::new("new", Kind::Untracked, false),
                      FileChange::new("gone", Kind::Deleted, false), r];
        let c = changed_paths(&fs);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0], ChangedPath { old: Some("a.rs".into()), new: Some("b.rs".into()) });
        assert_eq!(c[1], ChangedPath { old: Some("gone".into()), new: None });
        assert_eq!(c[2], ChangedPath { old: None, new: Some("new".into()) });
    }
}
