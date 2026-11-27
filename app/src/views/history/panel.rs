//! The diff header's Blame and File History buttons and the Markdown Preview toggle, for the commit's file
//! (FileInspector.swift's WithFilePanel, ChangesView.swift's DiffHeader, Conversation.swift's MarkdownPreview).
//! At most one panel is on; which one, and the preview, persist under Swift's keys "filePanel" / "markdownPreview".
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
    component::{checkbox::Checkbox, text::TextView, tooltip::Tooltip},
};
use loupe_core::git::{self, Cancel};
use loupe_core::history::{self as core_history, Commit, LOG_FORMAT};

use super::{HistoryView, Shown};
use crate::diff::{self, Blame};
use crate::state::adapt;
use crate::{theme, ui};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel { None, Blame, History }

impl Panel {
    fn key(self) -> &'static str { match self { Panel::None => "", Panel::Blame => "blame", Panel::History => "history" } }
}

/// One file of one commit: "sha path".
type Key = String;

pub(super) struct FilePanel {
    pub panel: Panel,
    pub preview: bool,
    blame: Option<(Key, Arc<Blame>)>,
    entries: Option<(Key, Vec<(Commit, String)>)>,
    pub picked: Option<(Commit, String)>,
    picked_diff: Option<(String, Arc<diff::FileDiff>)>,
    markdown: Option<(Key, String)>,
    load: Option<(Key, Cancel, Task<()>)>,
}

impl FilePanel {
    pub fn new() -> Self {
        let s = loupe_forge::settings::shared();
        let panel = match s.string("filePanel").as_deref() { Some("blame") => Panel::Blame, Some("history") => Panel::History, _ => Panel::None };
        FilePanel { panel, preview: s.bool("markdownPreview"), blame: None, entries: None, picked: None, picked_diff: None, markdown: None, load: None }
    }
    pub fn cancel(&mut self) { if let Some((_, c, _)) = self.load.take() { c.cancel() } }
}

pub fn is_markdown(path: &str) -> bool {
    std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| ["md", "markdown", "mdx"].contains(&e.to_lowercase().as_str()))
}

/// A file's commits from `rev` back, following renames. Swift logged from HEAD whatever commit showed (§14 History #11).
fn file_log_at(path: &str, rev: &str, dir: &std::path::Path) -> loupe_core::Result<Vec<(Commit, String)>> {
    let format = format!("--format=%x1e{}", LOG_FORMAT.replace("%x1e", ""));
    let out = git::string(dir, ["log", "--follow", "-M", "-n", "1000", &format, "--decorate=full", "--name-status", rev, "--", path])?;
    Ok(out.split('\u{1e}').filter_map(|rec| {
        let lines: Vec<&str> = rec.split('\n').filter(|l| !l.is_empty()).collect();
        let c = core_history::parse_log(lines.first()?.as_bytes()).into_iter().next()?;
        let name = lines.iter().skip(1).last().map(|l| l.rsplit('\t').next().unwrap_or("").to_string()).unwrap_or_else(|| path.into());
        Some((c, if name.is_empty() { path.into() } else { name }))
    }).collect())
}

impl HistoryView {
    /// What the shown file needs from the panel: blame, the file's commits, the picked commit's diff or the Markdown.
    /// Called from `sync_diff`; loads only what's missing, one at a time, and drops a load for another file.
    pub(super) fn sync_panel(&mut self, c: &Commit, f: &Shown, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_model() else { return };
        let dir = m.read(cx).path().to_path_buf();
        let key = format!("{} {}", c.sha, f.path());
        let stash = c.selector.is_some();
        let deleted = f.core.new_path.is_none();
        let p = &mut self.file_panel;
        if p.entries.as_ref().is_some_and(|e| e.0 != key) { p.entries = None; p.picked = None; p.picked_diff = None }
        let want: Option<(String, u8)> = if !stash && !deleted && p.panel == Panel::Blame && p.blame.as_ref().is_none_or(|b| b.0 != key) {
            Some((format!("blame {key}"), 0))
        } else if !stash && !deleted && p.panel == Panel::History && p.entries.is_none() {
            Some((format!("history {key}"), 1))
        } else if p.panel == Panel::History && let Some((pc, pp)) = p.picked.clone() && p.picked_diff.as_ref().is_none_or(|d| d.0 != pc.sha) {
            Some((format!("picked {} {pp}", pc.sha), 2))
        } else if p.preview && is_markdown(f.path()) && !deleted && p.markdown.as_ref().is_none_or(|m| m.0 != key) {
            Some((format!("md {key}"), 3))
        } else { None };
        let Some((load_key, what)) = want else { return };
        if p.load.as_ref().is_some_and(|l| l.0 == load_key) { return }
        p.cancel();
        let cancel = Cancel::new();
        let x = cancel.clone();
        let (sha, path, key2) = (c.sha.clone(), f.path().to_string(), key.clone());
        let picked = p.picked.clone();
        let lk = load_key.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            enum Got { Blame(Arc<Blame>), Entries(Vec<(Commit, String)>), Picked(String, Arc<diff::FileDiff>), Md(String), Nothing }
            let got = cx.background_executor().spawn(async move {
                match what {
                    0 => core_history::blame(&path, Some(&sha), &dir, Some(&x)).map(|(lines, commits)| {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                        Got::Blame(Arc::new(Blame {
                            shas: lines.into_iter().map(|l| l.sha).collect(),
                            commits: commits.into_iter().map(|(k, c)| (k, diff::BlameCommit { author: c.author, age: super::sheets::relative(c.date, now) })).collect(),
                        }))
                    }).unwrap_or(Got::Nothing),
                    1 => Got::Entries(file_log_at(&path, &sha, &dir).unwrap_or_default()),
                    2 => {
                        let Some((pc, pp)) = picked else { return Got::Nothing };
                        let out = git::git(&dir, ["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent", &pc.sha, "--", &pp]).cancel(&x).string();
                        match out.ok().and_then(|t| loupe_core::diff::parse(&t).into_iter().next()) {
                            Some(d) => Got::Picked(pc.sha, Arc::new(adapt::file_diff(&d))),
                            None => Got::Nothing,
                        }
                    }
                    _ => git::git(&dir, ["show", &format!("{sha}:{path}")]).cancel(&x).string().map(Got::Md).unwrap_or(Got::Nothing),
                }
            }).await;
            this.update_in(cx, |v, w, cx| {
                let p = &mut v.file_panel;
                if p.load.as_ref().is_some_and(|l| l.0 == lk) { p.load = None } else { return }
                match got {
                    Got::Blame(b) => p.blame = Some((key2, b)),
                    Got::Entries(e) => p.entries = Some((key2, e)),
                    Got::Picked(s, d) => p.picked_diff = Some((s, d)),
                    Got::Md(t) => p.markdown = Some((key2, t)),
                    // A failed load shows nothing rather than load again on every sync.
                    Got::Nothing => match what {
                        0 => p.blame = Some((key2, Arc::new(Blame { shas: vec![], commits: Default::default() }))),
                        3 => p.markdown = Some((key2, String::new())),
                        _ => {}
                    },
                }
                v.sync_diff(w, cx);
                cx.notify();
            }).ok();
        });
        self.file_panel.load = Some((load_key, cancel, task));
    }

    /// The blame for the diff, when Blame is on and it's this file's.
    pub(super) fn panel_blame(&self, c: &Commit, f: &Shown) -> Option<Arc<Blame>> {
        let p = &self.file_panel;
        (p.panel == Panel::Blame).then_some(())?;
        p.blame.as_ref().filter(|b| b.0 == format!("{} {}", c.sha, f.path())).map(|b| b.1.clone())
    }

    /// File History's picked commit replaces the diff.
    pub(super) fn panel_picked_diff(&self) -> Option<Arc<diff::FileDiff>> {
        let p = &self.file_panel;
        if p.panel != Panel::History { return None }
        let (c, _) = p.picked.as_ref()?;
        p.picked_diff.as_ref().filter(|d| d.0 == c.sha).map(|d| d.1.clone())
    }

    pub(super) fn set_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        let p = &mut self.file_panel;
        p.panel = if p.panel == panel { Panel::None } else { panel }; // a second click closes it
        p.picked = None;
        p.picked_diff = None;
        loupe_forge::settings::shared().set_string("filePanel", p.panel.key());
        self.sync_diff(window, cx);
        cx.notify();
    }

    fn set_preview(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.file_panel.preview = on;
        loupe_forge::settings::shared().set_bool("markdownPreview", on);
        self.sync_diff(window, cx);
        cx.notify();
    }

    /// The buttons in the diff header: Blame and File History (not for deleted files or stashes), and Preview for
    /// Markdown.
    pub(super) fn header_buttons(this: &WeakEntity<Self>, cx: &App) -> AnyElement {
        let Some(v) = this.upgrade() else { return div().into_any_element() };
        v.read(cx).buttons_for(v.clone(), cx)
    }

    fn buttons_for(&self, v: gpui_kit::Entity<Self>, cx: &App) -> AnyElement {
        let view = self;
        let Some((c, f)) = view.log(cx).and_then(|l| Some((l.selected_commit()?.clone(), l.selected_file()?.clone()))) else { return div().into_any_element() };
        let panels = c.selector.is_none() && f.core.new_path.is_some();
        let md = is_markdown(f.path());
        let (panel, preview) = (view.file_panel.panel, view.file_panel.preview);
        let button = |id: &'static str, icon: &'static str, tip: &'static str, p: Panel| {
            let v = v.clone();
            div().id(id).p(px(4.)).rounded(px(5.)).cursor_pointer()
                .when(panel == p, |d| d.bg(theme::selected()))
                .hover(|s| s.bg(theme::hover()))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(move |_, w, cx| v.update(cx, |v, cx| v.set_panel(p, w, cx)))
                .child(ui::icon(icon).size(px(14.)).text_color(if panel == p { theme::text() } else { theme::text_muted() }))
        };
        let v2 = v.clone();
        div().flex().items_center().gap_1()
            .when(panels, |d| d.child(button("panel-blame", "square-user", "Blame: who last changed each line", Panel::Blame))
                .child(button("panel-history", "rotate-ccw-clock", "File History", Panel::History)))
            .when(md, |d| d.child(div().id("md-preview").tooltip(|w, cx| Tooltip::new("Show the file rendered").build(w, cx))
                .child(Checkbox::new("md-preview-box").label("Preview").checked(preview)
                    .on_click(move |on, w, cx| { let on = *on; v2.update(cx, |v, cx| v.set_preview(on, w, cx)) }))))
            .into_any_element()
    }

    /// The Markdown preview replaces the diff for a .md file that exists after the commit.
    pub(super) fn render_markdown(&self, c: &Commit, f: &Shown, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = &self.file_panel;
        if !p.preview || !is_markdown(f.path()) || f.core.new_path.is_none() || self.panel_picked_diff().is_some() { return None }
        let key = format!("{} {}", c.sha, f.path());
        let body = match p.markdown.as_ref().filter(|m| m.0 == key) {
            Some((_, t)) => div().max_w(px(860.)).w_full().child(TextView::markdown(SharedString::from(format!("md-{key}")), t.clone())).into_any_element(),
            None => gpui_kit::component::spinner::Spinner::new().into_any_element(),
        };
        // The diff's header stays above the preview, so Preview can be turned off again.
        let (dir, name) = match f.path().rsplit_once('/') { Some((d, n)) => (format!("{d}/"), n.to_string()), None => (String::new(), f.path().to_string()) };
        let header = div().h(px(38.)).flex_none().flex().items_center().gap(px(8.)).px(px(12.)).bg(theme::card_header()).border_b_1().border_color(theme::border())
            .child(div().flex().min_w_0().overflow_hidden().whitespace_nowrap()
                .child(div().text_color(theme::text_muted()).child(dir))
                .child(div().font_weight(FontWeight::MEDIUM).child(name)))
            .child(div().flex_1())
            .child(self.buttons_for(cx.entity(), cx));
        Some(ui::pane().size_full().child(header)
            .child(div().id("md-preview-body").flex_1().min_h_0().overflow_y_scroll().px(px(28.)).py(px(22.)).flex().justify_center().child(body))
            .into_any_element())
    }

    /// File History's list on the left of the diff, and the bar over a picked commit's change.
    pub(super) fn render_file_panel(&self, diff: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.file_panel;
        let Some(f) = self.log(cx).and_then(|l| l.selected_file().cloned()) else { return diff };
        if p.panel != Panel::History || f.core.new_path.is_none() || self.log(cx).and_then(|l| l.selected_commit()).is_some_and(|c| c.selector.is_some()) { return diff }
        let me = cx.entity();
        let name = std::path::Path::new(f.path()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let picked_sha = p.picked.as_ref().map(|x| x.0.sha.clone());
        let list = match &p.entries {
            None => div().flex_1().flex().items_center().justify_center().child(gpui_kit::component::spinner::Spinner::new()).into_any_element(),
            Some((_, e)) if e.is_empty() => div().flex_1().child(ui::empty_state("clock", "No History", "")).into_any_element(),
            Some((_, e)) => div().id("file-history").flex_1().min_h_0().overflow_y_scroll().p_1().children(e.iter().map(|(c, path)| {
                let me = me.clone();
                let sel = picked_sha.as_deref() == Some(&c.sha);
                let (c2, p2) = (c.clone(), path.clone());
                let renamed = (path != f.path()).then(|| path.clone());
                ui::list_row(SharedString::from(format!("fh-{}", c.sha)), sel).flex_col().items_start().gap(px(2.)).py_1()
                    .on_click(move |_, w, cx| { let x = (c2.clone(), p2.clone()); me.update(cx, |v, cx| { v.file_panel.picked = Some(x); v.sync_diff(w, cx); cx.notify() }) })
                    .child(div().w_full().truncate().child(c.subject.clone()))
                    .child(div().flex().gap(px(6.)).text_size(px(11.)).text_color(theme::text_muted())
                        .child(div().font_family(theme::CODE_FONT).child(c.short().to_string()))
                        .child(div().flex_1().truncate().child(c.author.clone()))
                        .child(div().text_color(theme::text_faint()).child(super::sheets::relative(c.date, now))))
                    .when_some(renamed, |d, r| d.child(div().flex().gap_1().text_size(px(10.5)).text_color(theme::text_muted())
                        .child(ui::icon("corner-down-right").size(px(10.))).child(div().truncate().child(r))))
            })).into_any_element(),
        };
        let m2 = me.clone();
        let left = div().w(px(340.)).flex_none().flex().flex_col().border_r_1().border_color(theme::border())
            .child(div().flex().items_center().gap(px(6.)).px_3().py(px(7.)).bg(theme::card_header()).border_b_1().border_color(theme::border())
                .child(div().font_weight(FontWeight::SEMIBOLD).child("History"))
                .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(theme::text_muted()).child(name))
                .child(div().id("fh-close").cursor_pointer().tooltip(|w, cx| Tooltip::new("Close").build(w, cx))
                    .on_click(move |_, w, cx| m2.update(cx, |v, cx| v.set_panel(Panel::History, w, cx)))
                    .child(ui::icon("x").size(px(13.)).text_color(theme::text_muted()))))
            .child(list);
        let right = match p.picked.as_ref() {
            Some((c, _)) => {
                let m3 = me.clone();
                div().flex_1().min_w_0().flex().flex_col()
                    .child(div().flex().items_center().gap_2().px_3().py(px(6.)).bg(theme::accent().opacity(0.08)).text_size(px(12.))
                        .child(ui::icon("rotate-ccw-clock").size(px(13.)).text_color(theme::text_muted()))
                        .child(div().font_family(theme::CODE_FONT).text_color(theme::text_muted()).child(c.short().to_string()))
                        .child(div().flex_1().min_w_0().truncate().child(c.subject.clone()))
                        .child(div().text_size(px(11.)).text_color(theme::text_muted()).child(format!("{} · {}", c.author, super::sheets::relative(c.date, now))))
                        .child(ui::pill("fh-back", "Back to the Change").on_click(move |_, w, cx| m3.update(cx, |v, cx| { v.file_panel.picked = None; v.file_panel.picked_diff = None; v.sync_diff(w, cx); cx.notify() }))))
                    .child(if self.panel_picked_diff().is_some() { div().flex_1().min_h_0().child(diff).into_any_element() }
                           else { div().flex_1().flex().items_center().justify_center().child(gpui_kit::component::spinner::Spinner::new()).into_any_element() })
                    .into_any_element()
            }
            None => div().flex_1().min_w_0().child(diff).into_any_element(),
        };
        div().size_full().flex().child(left).child(right).into_any_element()
    }
}
