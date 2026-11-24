//! Blame and File History for the file showing (FileInspector.swift's WithFilePanel). One of them at a time,
//! persisted under "filePanel"; the History list is "filePanelWidth" wide.
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, prelude::FluentBuilder as _, px,
    component::tooltip::Tooltip,
};
use loupe_core::history::{self, Commit};
use loupe_forge::settings;

pub(crate) use loupe_code::inspector::FilePanel;

use super::pane::{has_target, spinner};
use super::{ChangesView, Drag, pref_f32};
use crate::diff::{self, DiffView};
use crate::state::{WorktreeModel, adapt};
use crate::{theme, ui};

pub(crate) struct Panel {
    pub panel: FilePanel,
    pub width: f32,
    blame_key: String,
    _blame_task: Option<Task<()>>,
    history: Option<(String, Option<Vec<(Commit, String)>>)>, // the file, its commits (None while loading)
    _history_task: Option<Task<()>>,
    pub picked: Option<(Commit, String)>,
    picked_view: Option<(String, Option<Entity<DiffView>>, bool)>, // key, the diff (None while loading), binary
    _picked_task: Option<Task<()>>,
}

impl Panel {
    pub fn load() -> Self {
        let panel = FilePanel::from_stored(&settings::shared().string(FilePanel::KEY).unwrap_or_default());
        Panel { panel, width: pref_f32(FilePanel::WIDTH_KEY, 340.).clamp(260., 700.), blame_key: String::new(), _blame_task: None,
                history: None, _history_task: None, picked: None, picked_view: None, _picked_task: None }
    }
    /// The header buttons: a second click closes, the other button switches.
    pub fn toggle(&mut self, p: FilePanel) { self.set(if self.panel == p { FilePanel::None } else { p }) }
    /// The context menu's Blame / File History set it (CH-20).
    pub fn set(&mut self, p: FilePanel) {
        self.panel = p;
        settings::shared().set_string(FilePanel::KEY, p.stored());
    }
}

/// "3 d", "2 mo": the blame gutter's age.
pub(crate) fn age(secs: i64, now: i64) -> String {
    let d = (now - secs).max(0);
    match d {
        d if d < 3600 => format!("{} min", d / 60),
        d if d < 86400 => format!("{} h", d / 3600),
        d if d < 86400 * 30 => format!("{} d", d / 86400),
        d if d < 86400 * 365 => format!("{} mo", d / (86400 * 30)),
        d => format!("{} y", d / (86400 * 365)),
    }
}

/// "2 days ago": the History rows and the pick bar.
pub(crate) fn relative(secs: i64, now: i64) -> String {
    let d = (now - secs).max(0);
    let (n, unit) = match d {
        d if d < 60 => return "just now".into(),
        d if d < 3600 => (d / 60, "minute"),
        d if d < 86400 => (d / 3600, "hour"),
        d if d < 86400 * 7 => (d / 86400, "day"),
        d if d < 86400 * 30 => (d / (86400 * 7), "week"),
        d if d < 86400 * 365 => (d / (86400 * 30), "month"),
        d => (d / (86400 * 365), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub(crate) fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) }

impl ChangesView {
    /// Blame follows the diff (keyed by its uid, so an edit re-blames); History loads once per file.
    pub(crate) fn update_panel(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let (f, dir, uid) = { let m = m.read(cx); (m.state.selected_file().cloned(), m.state.path.clone(), m.diff.as_ref().map_or(0, |d| d.uid)) };
        let target = f.filter(has_target).filter(|_| !self.app.read(cx).full_mode).map(|f| f.path);
        // Full mode blames the open tab's file instead (full.rs).
        let files = m.read(cx).state.files().to_vec();
        let full_target = self.app.read(cx).full_mode.then(|| self.full_target(cx)).flatten()
            .filter(|(p, _)| super::full::change(&files, p).is_none_or(has_target));
        let target = target.or(full_target.as_ref().map(|t| t.0.clone()));
        let uid = full_target.map_or(uid, |t| t.1);
        let blame_key = match (&target, self.panel.panel) { (Some(p), FilePanel::Blame) => format!("{p} {uid}"), _ => String::new() };
        if blame_key != self.panel.blame_key {
            self.panel.blame_key = blame_key.clone();
            if blame_key.is_empty() { self.set_blame(None, cx) } else {
                let (p, d) = (target.clone().unwrap(), dir.clone());
                self.panel._blame_task = Some(cx.spawn(async move |this, cx| {
                    let r = cx.background_executor().spawn(async move { history::blame(&p, None, &d, None).ok() }).await;
                    this.update(cx, |v, cx| if v.panel.blame_key == blame_key {
                        // FI-01: a failure is silent, no gutter.
                        let b = r.map(|(lines, commits)| {
                            let n = now();
                            Arc::new(diff::Blame {
                                shas: lines.into_iter().map(|l| l.sha).collect(),
                                commits: commits.into_iter().map(|(k, c)| (k, diff::BlameCommit { author: c.author, age: age(c.date, n) })).collect::<HashMap<_, _>>(),
                            })
                        });
                        v.set_blame(b, cx)
                    }).ok();
                }));
            }
        }
        match (&target, self.panel.panel) {
            (Some(p), FilePanel::History) if self.panel.history.as_ref().map(|h| &h.0) != Some(p) => {
                self.panel.history = Some((p.clone(), None));
                self.panel.picked = None;
                let (p, d, key) = (p.clone(), dir.clone(), p.clone());
                self.panel._history_task = Some(cx.spawn(async move |this, cx| {
                    let r = cx.background_executor().spawn(async move { history::file_log(&p, &d).unwrap_or_default() }).await;
                    this.update(cx, |v, cx| if let Some(h) = &mut v.panel.history && h.0 == key { h.1 = Some(r); cx.notify() }).ok();
                }));
            }
            (None, _) => { self.panel.history = None; self.panel.picked = None }
            _ => {}
        }
        self.update_picked(&dir, cx);
    }

    fn set_blame(&mut self, b: Option<Arc<diff::Blame>>, cx: &mut Context<Self>) {
        for d in [self.diff.clone(), self.full_view(cx)].into_iter().flatten() { d.update(cx, |d, cx| d.set_blame(b.clone(), cx)) }
    }

    /// FI-04: the picked commit's change to the file; whole in Full mode or Before / After. A stale answer is
    /// dropped by its key.
    fn update_picked(&mut self, dir: &std::path::Path, cx: &mut Context<Self>) {
        let whole = self.app.read(cx).whole_files();
        let Some((c, path)) = self.panel.picked.clone().filter(|_| self.panel.panel == FilePanel::History) else { self.panel.picked_view = None; return };
        let key = format!("{} {path} {whole}", c.sha);
        if self.panel.picked_view.as_ref().map(|p| &p.0) == Some(&key) { return }
        self.panel.picked_view = Some((key.clone(), None, false));
        let dir = dir.to_path_buf();
        let full_mode = self.app.read(cx).full_mode;
        self.panel._picked_task = Some(cx.spawn(async move |this, cx| {
            let d = cx.background_executor().spawn(async move {
                let data = loupe_core::git::run(&dir, loupe_code::inspector::commit_diff_args(&c.sha, &path, whole)).ok()?;
                let d = loupe_core::diff::parse_bytes(&data).into_iter().next();
                let d = if whole { loupe_core::diff::whole_file(d) } else { d }?;
                Some(Arc::new(adapt::file_diff(&d)))
            }).await;
            this.update(cx, |v, cx| {
                if v.panel.picked_view.as_ref().map(|p| &p.0) != Some(&key) { return }
                let view = d.map(|d| cx.new(|cx| {
                    let mut dv = DiffView::new(d, cx);
                    dv.set_show_header(false, cx);
                    dv.set_highlighter(Some(adapt::highlighter()), cx);
                    dv.set_change_ruler(full_mode, cx);
                    dv
                }));
                v.panel.picked_view = Some((key, view, false));
                cx.notify();
            }).ok();
        }));
    }

    /// The panel around a diff body: History's list on the left, the pick bar over the picked commit's diff.
    pub(crate) fn with_panel(&mut self, target: bool, body: AnyElement, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !target || self.panel.panel != FilePanel::History { return div().flex_1().min_h_0().child(body).into_any_element() }
        let file = self.panel.history.as_ref().map(|h| h.0.clone()).unwrap_or_default();
        let entries = self.panel.history.as_ref().and_then(|h| h.1.clone());
        let picked = self.panel.picked.as_ref().map(|p| p.0.sha.clone());
        let n = now();
        let mut list = div().id("file-history").flex_1().min_h_0().overflow_y_scroll().p_1();
        match &entries {
            None => list = list.child(spinner()),
            Some(e) if e.is_empty() => list = list.child(ui::empty_state("clock", "No History", "")),
            Some(e) => for (i, (c, p)) in e.iter().enumerate() {
                let sel = picked.as_deref() == Some(c.sha.as_str());
                let pick = (c.clone(), p.clone());
                list = list.child(ui::list_row(("fh", i), sel).flex_col().items_start().gap_0p5().py_1()
                    .child(div().w_full().truncate().text_color(theme::text()).child(c.subject.clone()))
                    .child(div().flex().gap_2().text_size(px(11.)).text_color(theme::text_faint())
                        .child(div().font_family(theme::CODE_FONT).child(c.short().to_string()))
                        .child(div().truncate().child(format!("{} · {}", c.author, relative(c.date, n)))))
                    .when(*p != file, |d| d.child(div().flex().items_center().gap_1().text_size(px(11.)).text_color(theme::text_muted())
                        .child(ui::icon("corner-down-right").size(px(11.))).child(div().truncate().child(p.clone()))))
                    .on_click(cx.listener(move |v, _, _, cx| { v.panel.picked = Some(pick.clone()); v.update(cx) })));
            },
        }
        let side = div().w(px(self.panel.width)).flex_none().h_full().flex().flex_col().border_r_1().border_color(theme::border())
            .child(div().h(px(32.)).flex_none().flex().items_center().gap_1p5().px_3().border_b_1().border_color(theme::border())
                .child(div().font_weight(FontWeight::SEMIBOLD).child("History"))
                .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(theme::text_muted()).child(super::tree::name(&file)))
                .child(ui::icon_button("fh-close", "x").size(px(22.)).tooltip(|w, cx| Tooltip::new("Close").build(w, cx))
                    .on_click(cx.listener(|v, _, _, cx| { v.panel.set(FilePanel::None); v.update(cx) }))))
            .child(list);
        let main = match self.panel.picked.clone() {
            None => body,
            Some((c, _)) => {
                let content = match self.panel.picked_view.as_ref().map(|p| p.1.clone()) {
                    Some(Some(d)) => d.into_any_element(),
                    _ => spinner(),
                };
                div().size_full().flex().flex_col()
                    .child(div().flex().items_center().gap_2().px_3().h(px(32.)).flex_none().bg(theme::blue().opacity(0.08)).border_b_1().border_color(theme::border())
                        .child(ui::icon("clock"))
                        .child(div().font_family(theme::CODE_FONT).text_size(px(11.5)).text_color(theme::text_muted()).child(c.short().to_string()))
                        .child(div().min_w_0().truncate().child(c.subject.clone()))
                        .child(div().flex_none().text_size(px(11.)).text_color(theme::text_muted()).child(format!("{} · {}", c.author, relative(c.date, n))))
                        .child(div().flex_1())
                        .child(ui::pill("fh-back", "Back to the Change").h(px(24.)).text_size(px(12.)).on_click(cx.listener(|v, _, _, cx| { v.panel.picked = None; v.update(cx) }))))
                    .child(div().flex_1().min_h_0().child(content))
                    .into_any_element()
            }
        };
        div().flex_1().min_h_0().flex().child(side).child(self.handle(Drag::Panel, cx)).child(div().flex_1().min_w_0().h_full().child(main)).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_short() {
        assert_eq!(age(0, 3 * 86400), "3 d");
        assert_eq!(age(0, 120), "2 min");
        assert_eq!(relative(0, 86400), "1 day ago");
        assert_eq!(relative(0, 3 * 3600), "3 hours ago");
        let mut p = Panel { panel: FilePanel::None, ..Panel::load() };
        p.panel = FilePanel::Blame;
        p.panel = if p.panel == FilePanel::Blame { FilePanel::None } else { FilePanel::Blame };
        assert_eq!(p.panel, FilePanel::None);
    }
}
