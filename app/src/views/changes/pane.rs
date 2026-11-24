//! The diff pane (DiffPane, DiffHeader): one file's diff under Changes' own header, or what shows instead of it
//! (the submodule pane, LFS objects with their images, the Markdown preview, the empty and loading states), and the
//! confirmation sheet.
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, px,
    component::tooltip::Tooltip,
};
use loupe_core::git::{FileChange, Kind};
use loupe_core::submodules::{self, SubmoduleDiff};
use loupe_core::lfs;
use loupe_forge::settings;

use super::inspector::FilePanel;
use super::{ChangesView, Confirm};
use crate::diff::{self, DiffSide, DiffView, HunkAction, Layout};
use crate::state::{AppEvent, WorktreeModel, adapt};
use crate::{theme, ui};

#[derive(Default)]
pub(crate) struct State {
    submodule: Option<(String, Option<SubmoduleDiff>)>,
    _sub_task: Option<Task<()>>,
    images: (String, Option<PathBuf>, Option<PathBuf>),
    _img_task: Option<Task<()>>,
    markdown: Option<(String, Option<String>)>,
    _md_task: Option<Task<()>>,
}

pub(crate) fn is_markdown(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
}

pub(crate) fn preview_on() -> bool { settings::shared().bool("markdownPreview") }

/// FI-06: Blame and History need a file with a past (§14: a deleted worktree file has nothing to blame).
pub(crate) fn has_target(f: &FileChange) -> bool {
    !matches!(f.kind, Kind::Untracked | Kind::Added) && f.submodule.is_none() && f.kind != Kind::Conflicted && !(f.kind == Kind::Deleted && !f.staged)
}

impl ChangesView {
    /// CH-29: the hunk buttons by entry. None in Before / After (§14: there "Hunk" meant the whole file).
    fn hunk_actions(&self, f: &FileChange, side: DiffSide) -> Vec<HunkAction> {
        if side != DiffSide::Diff || f.kind == Kind::Untracked { vec![] }
        else if f.staged { vec![HunkAction::Unstage] } else { vec![HunkAction::Discard, HunkAction::Stage] }
    }

    pub(crate) fn update_diff(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let (shown, f) = { let m = m.read(cx); (m.diff.clone(), m.state.selected_file().cloned()) };
        let side = self.app.read(cx).diff_side;
        let (Some(file), Some(f)) = (shown, f) else { if m.read(cx).state.selected_file().is_none() { self.diff = None } return };
        let actions = self.hunk_actions(&f, side);
        let path = file.path().to_string();
        match &self.diff {
            Some(d) => d.update(cx, |d, cx| { d.set_file(file.clone(), cx); d.set_hunk_actions(actions, cx); d.set_side(side, cx) }),
            None => {
                let me = cx.entity().downgrade();
                let app = self.app.clone();
                self.diff = Some(cx.new(|cx| {
                    let mut d = DiffView::new(file, cx);
                    d.set_show_header(false, cx);
                    d.set_highlighter(Some(adapt::highlighter()), cx);
                    d.set_hunk_actions(actions, cx);
                    // DV-20: one column, as the Swift app (no split in Changes).
                    d.set_layout(Layout::Unified, cx);
                    d.set_side(side, cx);
                    d.on_hunk_action(move |a, lines, _, cx| { me.update(cx, |v, cx| v.hunk_action(a, lines, cx)).ok(); });
                    d.on_side_change(move |s, _, _, cx| app.update(cx, |a, cx| a.set_diff_side(s, Layout::Unified, cx)));
                    d
                }));
            }
        }
        // A jump from the Overview lands on its line once that file's diff is here.
        if let Some((p, line)) = self.pending_reveal.clone() && p == path && let Some(d) = &self.diff {
            self.pending_reveal = None;
            d.update(cx, |d, cx| d.reveal(line, cx));
        }
    }

    /// Keyed loads for what replaces the table: the submodule pane (by entry and status version), LFS images (by
    /// both oids), the Markdown file (by diff uid).
    pub(crate) fn update_pane(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) {
        let (f, dir, version, core) = { let m = m.read(cx); (m.state.selected_file().cloned(), m.state.path.clone(), m.state.status_version, m.state.diff.clone()) };
        let Some(f) = f else { return };
        if f.submodule.is_some() {
            let key = format!("{} {version}", f.id());
            if self.pane.submodule.as_ref().map(|s| &s.0) != Some(&key) {
                let keep = self.pane.submodule.take().and_then(|s| s.1);
                self.pane.submodule = Some((key.clone(), keep));
                let (sub, staged) = (f.path.clone(), f.staged);
                self.pane._sub_task = Some(cx.spawn(async move |this, cx| {
                    let d = cx.background_executor().spawn(async move { submodules::diff(&dir, &sub, staged) }).await;
                    this.update(cx, |v, cx| if let Some(s) = &mut v.pane.submodule && s.0 == key { s.1 = Some(d); cx.notify() }).ok();
                }));
            }
            return;
        }
        // LF-04: thumbnails of either side, from the object store or (unstaged) the file on disk.
        if let Some(l) = core.as_ref().and_then(lfs::LfsDiff::from_diff).filter(|_| diff::is_image(&f.path)) {
            let key = format!("{} {} {}", f.path, l.old.as_ref().map_or("", |p| &p.oid), l.new.as_ref().map_or("", |p| &p.oid));
            if self.pane.images.0 != key {
                self.pane.images = (key.clone(), None, None);
                let disk = (!f.staged).then(|| dir.join(&f.path));
                let dir = dir.clone();
                self.pane._img_task = Some(cx.spawn(async move |this, cx| {
                    let (a, b) = cx.background_executor().spawn(async move {
                        let common = loupe_core::git::common_dir(&dir);
                        let find = |p: &Option<lfs::LfsPointer>, disk: Option<&std::path::Path>| p.as_ref().and_then(|p| lfs::object_file(p, common.as_deref(), disk));
                        (find(&l.old, None), find(&l.new, disk.as_deref()))
                    }).await;
                    this.update(cx, |v, cx| if v.pane.images.0 == key {
                        v.pane.images = (key, a, b);
                        v.push_images(cx);
                        cx.notify()
                    }).ok();
                }));
            }
        } else if !self.pane.images.0.is_empty() {
            self.pane.images = Default::default();
            if let Some(d) = &self.diff { d.update(cx, |d, cx| d.set_images(None, None, cx)) }
        }
        // DV-44: Changes renders the file on disk.
        if is_markdown(&f.path) && preview_on() && f.kind != Kind::Deleted {
            let key = format!("{} {}", f.path, m.read(cx).diff.as_ref().map_or(0, |d| d.uid));
            if self.pane.markdown.as_ref().map(|x| &x.0) != Some(&key) {
                let keep = self.pane.markdown.take().and_then(|x| x.1);
                self.pane.markdown = Some((key.clone(), keep));
                let p = dir.join(&f.path);
                self.pane._md_task = Some(cx.spawn(async move |this, cx| {
                    let t = cx.background_executor().spawn(async move { std::fs::read_to_string(p).ok() }).await;
                    this.update(cx, |v, cx| if let Some(x) = &mut v.pane.markdown && x.0 == key { x.1 = t; cx.notify() }).ok();
                }));
            }
        }
    }

    /// LF-04: the thumbnails go to the Compact diff and to the Full-mode tab when it shows the same file.
    pub(crate) fn push_images(&self, cx: &mut Context<Self>) {
        let (key, a, b) = self.pane.images.clone();
        if let Some(d) = &self.diff { let (a, b) = (a.clone(), b.clone()); d.update(cx, |d, cx| d.set_images(a, b, cx)) }
        if let (Some(v), Some((p, _))) = (self.full.view(), self.full.blame_target()) && !key.is_empty() && key.starts_with(&format!("{p} ")) {
            v.update(cx, |d, cx| d.set_images(a, b, cx))
        }
    }

    /// CH-39 / DV-02: what the right column shows in Compact mode.
    pub(crate) fn render_pane(&mut self, m: &Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (f, d, empty) = { let m = m.read(cx); (m.state.selected_file().cloned(), m.diff.clone(), m.state.files().is_empty()) };
        let Some(f) = f else {
            return if empty { ui::empty_state("badge-check", "No Changes", "Edits in this worktree show up here as you make them.") }
                   else { ui::empty_state("file-search", "No File Selected", "") }.into_any_element();
        };
        if f.submodule.is_some() { return self.render_submodule(&f, cx) }
        if f.kind == Kind::Conflicted { return self.render_conflict(m, &f, cx) }
        // The previous diff stays while the next loads (no spinner flash); none yet: the spinner.
        let Some(d) = d.filter(|_| self.diff.is_some()) else { return spinner() };
        let preview = is_markdown(&f.path) && preview_on() && f.kind != Kind::Deleted;
        let header = self.header(&d, Some(&f), true, preview, has_target(&f), cx);
        let body = if preview {
            match self.pane.markdown.as_ref().and_then(|x| x.1.clone()) {
                Some(t) => div().id("md").size_full().overflow_y_scroll().p_5().child(div().max_w(px(860.))
                    .child(gpui_kit::component::text::TextView::markdown("md-text", t))).into_any_element(),
                None => spinner(),
            }
        } else { self.diff.clone().map(|d| d.into_any_element()).unwrap_or_else(spinner) };
        let target = has_target(&f);
        ui::card().size_full().child(header).child(self.with_panel(target, body, window, cx)).into_any_element()
    }

    /// DV-03: icon, "old →", path, +N −N, Diff / Before / After, Blame and History, Preview, the actions.
    pub(crate) fn header(&self, d: &Arc<diff::FileDiff>, f: Option<&FileChange>, counts: bool, preview: bool, target: bool, cx: &mut Context<Self>) -> AnyElement {
        let path = d.path().to_string();
        let old = d.old_path.clone().filter(|o| d.new_path.as_ref().is_some_and(|n| n != o));
        let side = self.app.read(cx).diff_side;
        let both = d.additions > 0 && d.deletions > 0;
        let md = is_markdown(&path);
        let seg = |id: &'static str, label: &'static str, s: DiffSide, cx: &mut Context<Self>| {
            let on = side == s;
            div().id(id).px_2p5().h(px(22.)).flex().items_center().rounded(px(5.)).text_size(px(11.5)).cursor_pointer()
                .when(on, |d| d.bg(theme::selected()).text_color(theme::text())).when(!on, |d| d.text_color(theme::text_muted()))
                .child(label)
                .on_click(cx.listener(move |v, _, _, cx| v.app.update(cx, |a, cx| a.set_diff_side(s, Layout::Unified, cx))))
        };
        let panel_btn = |id: &'static str, icon: &'static str, tip: &'static str, p: FilePanel, cx: &mut Context<Self>| {
            let on = self.panel.panel == p;
            ui::icon_button(id, icon).size(px(24.)).when(on, |d| d.bg(theme::blue().opacity(0.15)).text_color(theme::blue()))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |v, _, _, cx| { v.panel.toggle(p); v.update(cx) }))
        };
        div().h(px(38.)).flex_none().flex().items_center().gap_2().px_3().bg(theme::card_header()).border_b_1().border_color(theme::border())
            .child(ui::file_icon(&path))
            .when_some(old, |d, o| d.child(div().text_color(theme::text_muted()).whitespace_nowrap().child(o)).child(ui::icon("arrow-right").size(px(12.))))
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).child(path.clone()))
            .when(counts, |el| el.child(div().flex().gap_1p5().font_family(theme::CODE_FONT).text_size(px(11.))
                .child(div().text_color(theme::green()).child(format!("+{}", d.additions)))
                .child(div().text_color(theme::red()).child(format!("−{}", d.deletions)))))
            .child(div().flex_1())
            .when(both && !(md && preview), |el| el.child(div().id("side").flex().p(px(2.)).gap_0p5().rounded(px(7.)).border_1().border_color(theme::border())
                .tooltip(|w, cx| Tooltip::new("The changes, the file before them, or the file after them").build(w, cx))
                .child(seg("side-diff", "Diff", DiffSide::Diff, cx)).child(seg("side-old", "Before", DiffSide::Old, cx)).child(seg("side-new", "After", DiffSide::New, cx))))
            .when(target, |el| el
                .child(panel_btn("blame", "square-user", "Blame: who last changed each line", FilePanel::Blame, cx))
                .child(panel_btn("history", "clock", "File History", FilePanel::History, cx))
                .child(div().w(px(1.)).h(px(14.)).bg(theme::border())))
            .when(md, |el| el.child(div().id("preview").flex().items_center().gap_1().cursor_pointer().text_size(px(12.))
                .tooltip(|w, cx| Tooltip::new("Show the file rendered").build(w, cx))
                .child(ui::icon(if preview { "square-check" } else { "square" }).size(px(13.))).child("Preview")
                .on_click(cx.listener(|v, _, _, cx| { let on = !preview_on(); settings::shared().set_bool("markdownPreview", on); v.update(cx) }))))
            .when_some(f.cloned(), |el, f| el.child(self.file_actions(f, cx)))
            .into_any_element()
    }

    /// CH-28 / DV-04
    fn file_actions(&self, f: FileChange, cx: &mut Context<Self>) -> AnyElement {
        let small = |p: gpui_kit::Stateful<gpui_kit::Div>| p.h(px(24.)).px_2p5().text_size(px(12.));
        let row = div().flex().items_center().gap_1p5();
        if f.staged {
            return row.child(small(ui::pill("unstage-file", "Unstage File")).on_click(cx.listener(move |v, _, _, cx| v.unstage(vec![f.clone()], cx)))).into_any_element();
        }
        if f.kind == Kind::Conflicted { return row.into_any_element() }
        let f2 = f.clone();
        row.child(small(ui::pill("discard-file", "Discard…")).on_click(cx.listener(move |v, _, _, cx| v.ask_discard(vec![f.clone()], cx))))
            .child(small(ui::pill_primary("stage-file", "Stage File", theme::blue())).on_click(cx.listener(move |v, _, _, cx| v.stage(vec![f2.clone()], cx))))
            .into_any_element()
    }

    /// SM-01..04
    fn render_submodule(&self, f: &FileChange, cx: &mut Context<Self>) -> AnyElement {
        let d = self.pane.submodule.as_ref().and_then(|s| s.1.clone());
        let checked_out = self.selected().is_some_and(|m| submodules::is_checked_out(&m.read(cx).state.path, &f.path));
        let behind = d.as_ref().is_some_and(|d| !d.commits.is_empty() && !d.commits.iter().any(|c| c.added));
        let changed = f.submodule.is_some_and(|s| s.commit_changed);
        let small = |p: gpui_kit::Stateful<gpui_kit::Div>| p.h(px(24.)).px_2p5().text_size(px(12.));
        let (p1, p2, f1, f2) = (f.path.clone(), f.path.clone(), f.clone(), f.clone());
        let mut actions = div().flex().items_center().gap_1p5()
            .child(small(ui::pill("open-sub", "Open Submodule")).when(!checked_out, |d| d.opacity(0.4))
                .when(checked_out, |d| d.on_click(cx.listener(move |v, _, _, cx| v.open_submodule(&p1, cx)))));
        if f.staged {
            actions = actions.child(small(ui::pill("unstage-sub", "Unstage")).on_click(cx.listener(move |v, _, _, cx| v.unstage(vec![f1.clone()], cx))));
        } else {
            if changed {
                let b = if behind { ui::pill_primary("update-sub", "Update to Recorded Commit", theme::blue()) } else { ui::pill("update-sub", "Update to Recorded Commit") };
                actions = actions.child(small(b).tooltip(|w, cx| Tooltip::new("Check the submodule out at the commit this repository records (git submodule update)").build(w, cx))
                    .on_click(cx.listener(move |v, _, _, cx| v.update_submodules(vec![p2.clone()], cx))));
            }
            let tip = if behind { "Record the older commit checked out in the submodule (a rewind)" } else { "Record the submodule's checked-out commit in this repository" };
            let b = if !behind { ui::pill_primary("stage-sub", "Stage Pointer", theme::blue()) } else { ui::pill("stage-sub", "Stage Pointer") };
            actions = actions.child(small(b).when(!changed, |d| d.opacity(0.4)).tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .when(changed, |d| d.on_click(cx.listener(move |v, _, _, cx| v.stage(vec![f2.clone()], cx)))));
        }
        let header = div().h(px(38.)).flex_none().flex().items_center().gap_2().px_3().bg(theme::card_header()).border_b_1().border_color(theme::border())
            .child(ui::icon("package")).child(div().flex_1().font_weight(FontWeight::MEDIUM).child(f.path.clone())).child(actions);
        let body = match d {
            None => spinner(),
            Some(d) => {
                let back = d.commits.iter().filter(|c| !c.added).count();
                let fwd = d.commits.len() - back;
                let summary = [(fwd > 0).then(|| format!("{fwd} new commit{}", if fwd == 1 { "" } else { "s" })), (back > 0).then(|| format!("{back} commit{} left behind", if back == 1 { "" } else { "s" }))]
                    .into_iter().flatten().collect::<Vec<_>>().join(", ");
                let mut col = div().id("sub-body").size_full().overflow_y_scroll().p_5().flex().flex_col().gap_3()
                    .child(div().flex().items_center().gap_2().font_weight(FontWeight::SEMIBOLD).child(ui::icon("package")).child("Submodule"));
                if d.old.is_some() || d.new.is_some() {
                    col = col.child(div().flex().items_center().gap_2().font_family(theme::CODE_FONT)
                            .child(d.old.clone().unwrap_or("none".into())).child(ui::icon("arrow-right").size(px(12.))).child(d.new.clone().unwrap_or("none".into()))
                            .when_some(d.note.clone(), |el, n| el.child(div().font_family(theme::UI_FONT).text_size(px(11.)).text_color(theme::text_muted()).child(n))))
                        .child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(if f.staged { "The recorded commit, before and after this change" } else { "The recorded commit, and the one checked out in the submodule" }));
                }
                if !d.commits.is_empty() {
                    col = col.child(div().pt_1().font_weight(FontWeight::MEDIUM).child(summary));
                    for c in d.commits.iter().take(300) {
                        col = col.child(div().flex().items_center().gap_1p5()
                            .child(ui::icon(if c.added { "circle-plus" } else { "undo-2" }).text_color(if c.added { theme::green() } else { theme::accent() }))
                            .child(div().min_w_0().truncate().child(c.subject.clone())));
                    }
                    if d.commits.len() > 300 { col = col.child(div().text_size(px(11.)).text_color(theme::text_muted()).child(format!("and {} more", d.commits.len() - 300))) }
                }
                if d.modified_content || d.untracked_content {
                    let t = if d.modified_content && d.untracked_content { "Contains modified and untracked files" } else if d.modified_content { "Contains modified files" } else { "Contains untracked files" };
                    col = col.child(div().pt_1().flex().items_center().gap_1p5().text_color(theme::text_muted()).child(ui::icon("pencil")).child(t))
                        .child(div().text_size(px(11.)).text_color(theme::text_muted()).child("Open the submodule to commit or discard them."));
                }
                col.into_any_element()
            }
        };
        ui::card().size_full().child(header).child(body).into_any_element()
    }

    /// MG-05's entry: the merge editor is an overlay in this app (views/merge.rs); this pane opens it.
    fn render_conflict(&self, m: &Entity<WorktreeModel>, f: &FileChange, cx: &mut Context<Self>) -> AnyElement {
        let (wt, path) = (m.read(cx).path_str(), f.path.clone());
        ui::empty_state("git-merge", "Conflicted", format!("{} has conflicts. Resolve them in the merge editor.", f.path))
            .child(div().pt_2().child(ui::pill_primary("open-merge", "Open Merge Editor", theme::blue())
                .on_click(cx.listener(move |v, _, _, cx| { let (w, p) = (wt.clone(), path.clone()); v.app.update(cx, |a, cx| a.emit(AppEvent::OpenMergeEditor { worktree: w, path: p }, cx)) }))))
            .into_any_element()
    }

    /// CH-32 / CH-36 / SM-03: one sheet style for every discard (§14: Swift had two).
    pub(crate) fn render_confirm(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.confirm.as_ref()?;
        let (title, message, button): (String, String, String) = match c {
            Confirm::Files(fs) => ("Discard changes?".into(), "Tracked changes are reverted; untracked files are moved to the Trash. Edit › Undo brings them back.".into(), format!("Discard {} file(s)", fs.len())),
            Confirm::Lines(_) => ("Discard these changes?".into(), "The change is removed from the working tree. Edit › Undo brings it back.".into(), "Discard".into()),
            Confirm::Submodules(_, s) => ("Leave commits that are on no branch?".into(),
                format!("{}\n\nThey were made on a detached HEAD. After the update they're only in the submodule's reflog. Open the submodule and create a branch to keep them.",
                    s.iter().map(|(p, n)| format!("{p}: {n} commit{}", if *n == 1 { "" } else { "s" })).collect::<Vec<_>>().join("\n")),
                "Update Anyway".into()),
        };
        let focus = self.confirm_focus.clone();
        if !focus.is_focused(window) { window.focus(&focus, cx) }
        Some(div().id("confirm").absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&focus)
            .child(ui::card().w(px(420.)).p_4().gap_3().bg(theme::raised())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(message))
                .child(div().flex().justify_end().gap_2()
                    .child(ui::pill("confirm-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| { v.confirm = None; cx.notify() })))
                    .child(ui::pill_primary("confirm-ok", SharedString::from(button), theme::red()).on_click(cx.listener(|v, _, _, cx| v.confirmed(cx))))))
            .into_any_element())
    }
}

pub(crate) fn spinner() -> AnyElement {
    div().size_full().flex().items_center().justify_center().child(ui::icon("loader").size(px(18.)).text_color(theme::text_faint())).into_any_element()
}
