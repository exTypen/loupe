//! ChangeOverview.swift: the lenses' report for a change from A to B (summary tiles, the branch row, one section per
//! lens), computed off the main thread when its key changes. The decisions (what a row says, what folds) live in
//! `loupe_code::overview`; this file only draws them.
//!
//! Hosts build an [`OverviewInput`] (`worktree_input`, `commit_input`, `pull_input`) and an `on_open(path, line)`.
//! Fixed against the Swift app (FLOWS.md §14): a recompute for the same A/B (a status refresh in Changes) keeps the
//! shown report until the new one arrives instead of flashing "Reading the change…" (§14 #5); config rows show the
//! detail beside a tag ("new file · 3 keys", §14 #13); branch-row commits open History (§14 #14).
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, Context, ElementId, FontWeight, HighlightStyle, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Task, Window, anchored, canvas, deferred, div, point, prelude::FluentBuilder as _, px,
    component::tooltip::Tooltip,
};
use loupe_code::lens::{self, BranchStatus, ChangedPath, DBGroup, DBTable, FactKind, LensFact, LensReport, ReportKind, Snapshot};
use loupe_code::overview as ov;
use loupe_code::syntax::{self, SyntaxSpan, TokenKind};
use loupe_core::Kind;

use crate::state::WorktreeModel;
use crate::{theme, ui};

/// What to analyze, and when to do it again.
#[derive(Clone, Debug, PartialEq)]
pub struct OverviewInput {
    pub dir: String,
    pub a: Snapshot,
    pub b: Snapshot,
    pub changes: Vec<ChangedPath>,
    /// Recompute when this changes (Changes "headOID statusVersion", History "sha fileCount", PR "base head count ref").
    pub key: String,
    /// The branch the change goes into, when its tip is further on than A: (ref, name), a PR's base branch.
    pub target: Option<(String, String)>,
}

pub type OnOpen = Rc<dyn Fn(String, Option<usize>, &mut Window, &mut App)>;

/// Changes: HEAD (nothing on an unborn branch) against the worktree, one entry per path, the unstaged one preferred.
pub fn worktree_input(m: &WorktreeModel) -> OverviewInput {
    let s = &m.state;
    let changes = changed_paths(s.files());
    let unborn = s.head_oid.is_empty() || s.head_oid.starts_with('(');
    OverviewInput {
        dir: m.path_str(),
        a: if unborn { Snapshot::Empty } else { Snapshot::Rev("HEAD".into()) },
        b: Snapshot::Worktree,
        changes,
        key: format!("{} {}", s.head_oid, s.status_version),
        target: None,
    }
}

/// The status list as the lenses see it: one entry per path (sorted), deleted → no new side, added → no old side.
pub fn changed_paths(files: &[loupe_core::FileChange]) -> Vec<ChangedPath> {
    let mut by: BTreeMap<&str, Vec<&loupe_core::FileChange>> = BTreeMap::new();
    for f in files { by.entry(f.path.as_str()).or_default().push(f) }
    by.values().map(|fs| {
        let f = fs.iter().find(|f| !f.staged).unwrap_or(&fs[0]);
        let gone = fs.iter().any(|f| f.kind == Kind::Deleted);
        let new = fs.iter().any(|f| matches!(f.kind, Kind::Added | Kind::Untracked));
        ChangedPath { old: (!new).then(|| f.orig_path.clone().unwrap_or_else(|| f.path.clone())), new: (!gone).then(|| f.path.clone()) }
    }).collect()
}

/// History: the first parent (nothing for a root commit) against the commit.
pub fn commit_input(dir: &str, sha: &str, parent: Option<&str>, changes: Vec<ChangedPath>) -> OverviewInput {
    OverviewInput {
        dir: dir.into(), a: parent.map_or(Snapshot::Empty, |p| Snapshot::Rev(p.into())), b: Snapshot::Rev(sha.into()),
        key: format!("{sha} {}", changes.len()), changes, target: None,
    }
}

/// A pull request: the merge base against its head, with the base branch's tip (`target_ref`, e.g.
/// "refs/remotes/origin/main") and name. The host fetches the head and finds the merge base first.
pub fn pull_input(dir: &str, merge_base: &str, head: &str, changes: Vec<ChangedPath>, target_ref: &str, target_name: &str) -> OverviewInput {
    OverviewInput {
        dir: dir.into(), a: Snapshot::Rev(merge_base.into()), b: Snapshot::Rev(head.into()),
        key: format!("{merge_base} {head} {} {target_ref}", changes.len()), changes,
        target: Some((target_ref.into(), target_name.into())),
    }
}

struct Computed { key: String, scope: (String, Snapshot, Snapshot), reports: Arc<Vec<LensReport>>, branch: Option<BranchStatus> }

pub struct ChangeOverview {
    input: OverviewInput,
    computed: Option<Computed>,
    _task: Task<()>,
    computing: Option<String>,
    on_open: OnOpen,
    on_commit: Option<Rc<dyn Fn(String, &mut Window, &mut App)>>,
    scroll: ScrollHandle,
    /// Toggled disclosures, "show all"s and collapsed sections, by key; reset when the view is rebuilt.
    toggled: HashSet<String>,
    open_kind: Option<String>,
    hover: Option<String>,
    width: Rc<Cell<f32>>,
}

impl ChangeOverview {
    pub fn new(input: OverviewInput, on_open: OnOpen, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            input: input.clone(), computed: None, _task: Task::ready(()), computing: None, on_open, on_commit: None,
            scroll: ScrollHandle::new(), toggled: HashSet::new(), open_kind: None, hover: None, width: Rc::new(Cell::new(900.)),
        };
        v.compute(cx);
        v
    }

    /// A branch-row commit was clicked (the PR host shows it in History).
    pub fn on_commit(&mut self, f: impl Fn(String, &mut Window, &mut App) + 'static) { self.on_commit = Some(Rc::new(f)) }

    /// New inputs: recomputes when the key moved. The old report stays up while the same A → B recomputes.
    pub fn set_input(&mut self, input: OverviewInput, cx: &mut Context<Self>) {
        if input == self.input { return }
        let scope_changed = (&input.dir, &input.a, &input.b) != (&self.input.dir, &self.input.a, &self.input.b);
        self.input = input;
        if scope_changed { self.toggled.clear(); self.open_kind = None }
        self.compute(cx);
        cx.notify();
    }

    pub fn reports(&self) -> Option<&[LensReport]> { self.computed.as_ref().map(|c| c.reports.as_slice()) }
    pub fn is_reading(&self) -> bool { self.shown().is_none() }

    fn compute(&mut self, cx: &mut Context<Self>) {
        if self.computed.as_ref().is_some_and(|c| c.key == self.input.key) || self.computing.as_ref() == Some(&self.input.key) { return }
        let input = self.input.clone();
        self.computing = Some(input.key.clone());
        // Dropping the task (a new key, or the view going away) drops the result: a cancelled run never stores it.
        self._task = cx.spawn(async move |this, cx| {
            let i = input.clone();
            let (reports, branch) = cx.background_executor().spawn(async move {
                let up = i.target.as_ref().map(|t| t.0.as_str());
                let reports = lens::analyze(&i.changes, &i.a, &i.b, up, &i.dir);
                let branch = match (&i.target, &i.b) {
                    (Some((r, name)), Snapshot::Rev(head)) => {
                        let mut paths: Vec<String> = i.changes.iter().flat_map(|c| c.old.iter().chain(c.new.iter()).cloned()).collect();
                        paths.sort(); paths.dedup();
                        lens::branch_status(head, r, name, &paths, &i.dir)
                    }
                    _ => None,
                };
                (reports, branch)
            }).await;
            this.update(cx, |v, cx| {
                if v.input.key != input.key { return }
                v.computing = None;
                v.computed = Some(Computed { key: input.key.clone(), scope: (input.dir.clone(), input.a.clone(), input.b.clone()), reports: Arc::new(reports), branch });
                cx.notify();
            }).ok();
        });
    }

    /// What to draw: the report for this key, or the last one for the same A → B while it recomputes.
    fn shown(&self) -> Option<&Computed> {
        let c = self.computed.as_ref()?;
        (c.key == self.input.key || c.scope == (self.input.dir.clone(), self.input.a.clone(), self.input.b.clone())).then_some(c)
    }

    fn is(&self, k: &str) -> bool { self.toggled.contains(k) }

    fn toggle(&self, k: impl Into<String>, cx: &mut Context<Self>) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
        let k = k.into();
        cx.listener(move |v, _, _, cx| { if !v.toggled.remove(&k) { v.toggled.insert(k.clone()); } cx.notify() })
    }

    /// Hover highlight, "Open path:line", click to open: anything with a location. Without one, a plain row.
    fn openable(&self, id: impl Into<String>, path: Option<&str>, line: Option<usize>, content: gpui_kit::Div, _cx: &mut Context<Self>) -> AnyElement {
        let id: String = id.into();
        let Some(p) = path.map(str::to_string) else { return content.into_any_element() };
        let tip: SharedString = format!("Open {p}{}", line.map(|l| format!(":{l}")).unwrap_or_default()).into();
        let open = self.on_open.clone();
        content.id(ElementId::Name(id.into())).cursor_pointer().hover(|s| s.bg(theme::hover()))
            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
            .on_click(move |_, w, cx| open(p.clone(), line, w, cx))
            .into_any_element()
    }
}

// MARK: - Shared look

fn kind_color(k: FactKind) -> Hsla {
    match k {
        FactKind::Added => theme::green(),
        FactKind::Removed => theme::red(),
        FactKind::Changed => theme::accent(),
        FactKind::Renamed => theme::purple(),
        FactKind::Info => theme::text_muted(),
        FactKind::Warning => theme::accent(),
    }
}

/// The circled +, −, pencil, → of each fact.
fn status_symbol(k: FactKind, size: f32) -> AnyElement {
    let name = match k {
        FactKind::Added => "circle-plus",
        FactKind::Removed => "circle-minus",
        FactKind::Changed => "circle-dot",
        FactKind::Renamed => "circle-arrow-right",
        FactKind::Info => "circle",
        FactKind::Warning => "triangle-alert",
    };
    let label: SharedString = k.label().into();
    div().id(ElementId::Name(format!("sym-{}", k.label()).into())).flex_none().w(px(size + 2.)).flex().justify_center()
        .tooltip(move |w, cx| Tooltip::new(label.clone()).build(w, cx))
        .child(ui::icon(name).size(px(size)).text_color(kind_color(k)))
        .into_any_element()
}

fn report_tint(k: ReportKind) -> Hsla {
    // ponytail: theme.rs has no brown/indigo/gray tile tints; derived from the nearest tokens (requested).
    match k {
        ReportKind::Files => theme::blue(),
        ReportKind::Database => theme::accent(),
        ReportKind::Dependencies => theme::accent().opacity(0.65),
        ReportKind::Configuration => theme::text_faint(),
        ReportKind::FollowUps => theme::purple(),
        ReportKind::Code => theme::blue().opacity(0.7),
    }
}

fn report_icon(k: ReportKind) -> &'static str {
    match k {
        ReportKind::Files => "files",
        ReportKind::Database => "database",
        ReportKind::Dependencies => "package",
        ReportKind::Configuration => "sliders-horizontal",
        ReportKind::FollowUps => "list-todo",
        ReportKind::Code => "code",
    }
}

fn kind_bar_color(kind: &str) -> Hsla {
    match kind {
        "Code" => theme::blue(),
        "Tests" => theme::green().opacity(0.75),
        "SQL" => theme::accent(),
        "Dependencies" => theme::accent().opacity(0.6),
        "Config" => theme::text_muted(),
        "Build & CI" => theme::purple().opacity(0.75),
        "Docs" => theme::green(),
        "Assets" => theme::red().opacity(0.7),
        _ => theme::text_faint().opacity(0.6),
    }
}

/// A rounded group of rows, as in System Settings.
fn grouped() -> gpui_kit::Div {
    div().flex().flex_col().w_full().rounded(px(8.)).bg(theme::card()).border_1().border_color(theme::border()).overflow_hidden()
}

fn hairline(inset: f32) -> gpui_kit::Div { div().pl(px(inset)).child(ui::divider()) }

fn subheading(t: impl Into<SharedString>) -> gpui_kit::Div {
    div().pl(px(2.)).text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(theme::text_muted()).child(t.into())
}

fn caption(t: impl Into<SharedString>) -> gpui_kit::Div { div().text_size(px(11.)).text_color(theme::text_muted()).child(t.into()) }
fn faint(t: impl Into<SharedString>) -> gpui_kit::Div { div().text_size(px(11.)).text_color(theme::text_faint()).child(t.into()) }
fn mono(t: impl Into<SharedString>, size: f32) -> gpui_kit::Div {
    div().font_family(theme::CODE_FONT).text_size(px(size)).text_color(theme::text_muted()).child(t.into())
}

/// "+2 −6 ~3", zeroes left out.
fn deltas(a: usize, r: usize, c: usize) -> gpui_kit::Div {
    div().flex().gap(px(5.)).text_size(px(11.)).font_family(theme::CODE_FONT)
        .when(a > 0, |d| d.child(div().text_color(theme::green()).child(format!("+{a}"))))
        .when(r > 0, |d| d.child(div().text_color(theme::red()).child(format!("−{r}"))))
        .when(c > 0, |d| d.child(div().text_color(theme::accent()).child(format!("~{c}"))))
}

fn link(id: impl Into<String>, label: impl Into<SharedString>) -> gpui_kit::Stateful<gpui_kit::Div> {
    div().id(ElementId::Name(id.into().into())).text_size(px(11.)).text_color(theme::blue()).cursor_pointer()
        .hover(|s| s.text_color(theme::text())).child(label.into())
}

fn small_button(id: impl Into<String>, label: impl Into<SharedString>) -> gpui_kit::Stateful<gpui_kit::Div> {
    div().id(ElementId::Name(id.into().into())).flex_none().px(px(8.)).h(px(20.)).flex().items_center().rounded(px(5.))
        .bg(theme::raised()).border_1().border_color(theme::border_strong()).text_size(px(11.)).text_color(theme::text())
        .cursor_pointer().hover(|s| s.bg(theme::selected())).child(label.into())
}

fn chevron(open: bool) -> gpui_kit::Svg {
    ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(10.)).text_color(theme::text_muted())
}

// MARK: - Code snippets

/// The diff palette's defaults (diff/view.rs): ponytail, the selected code theme isn't public outside diff/ yet.
fn token_color(k: TokenKind) -> Hsla {
    match k {
        TokenKind::Keyword | TokenKind::Macro => theme::purple(),
        TokenKind::String => theme::green(),
        TokenKind::Comment => theme::text_faint(),
        TokenKind::Number | TokenKind::Attribute => theme::accent(),
        TokenKind::Type => theme::blue(),
        TokenKind::Function => theme::text(),
        _ => theme::text_muted(),
    }
}

fn highlighted(line: String, spans: &[SyntaxSpan]) -> StyledText {
    let n = line.len();
    let runs: Vec<_> = spans.iter().filter(|s| s.range().end <= n && line.is_char_boundary(s.range().start) && line.is_char_boundary(s.range().end))
        .map(|s| (s.range(), HighlightStyle { color: Some(token_color(s.kind)), ..Default::default() })).collect();
    StyledText::new(if line.is_empty() { " ".to_string() } else { line }).with_highlights(runs)
}

/// A source excerpt: dedented, trimmed, highlighted, tinted for what happened to it, cut at `max` lines.
fn code_block(id: String, text: &str, path: &str, kind: Option<FactKind>, max: usize, expanded: bool, toggle: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    let all: Vec<String> = ov::code_block_lines(text).into_iter().map(|l| l.replace('\t', "    ")).collect();
    let shown = ov::code_block_shown(all.len(), max, expanded);
    let spans = syntax::highlight_snippet(&all[..shown], path);
    let (tint, mark) = match kind {
        Some(FactKind::Added) => (Some(theme::green()), Some("+")),
        Some(FactKind::Removed) => (Some(theme::red()), Some("−")),
        _ => (None, None),
    };
    div().flex().flex_col().w_full().rounded(px(6.)).border_1().border_color(theme::border()).bg(theme::canvas())
        .when_some(tint, |d, t| d.bg(t.opacity(0.07)))
        .child(div().id(ElementId::Name(format!("{id}-scroll").into())).overflow_x_scroll().px(px(9.)).py(px(7.))
            .font_family(theme::CODE_FONT).text_size(px(11.)).line_height(px(15.)).text_color(theme::text())
            .children(all[..shown].iter().zip(spans).map(|(l, s)| div().flex().gap(px(8.)).whitespace_nowrap()
                .when_some(mark, |d, m| d.child(div().w(px(8.)).flex_none().text_color(theme::text_muted()).child(m)))
                .child(highlighted(l.clone(), &s)))))
        .when(all.len() > max, |d| d.child(ui::divider()).child(div().px(px(9.)).py(px(4.))
            .child(link(format!("{id}-more"), if expanded { "Show Less".to_string() } else { format!("Show All {} Lines", all.len()) }).on_click(toggle))))
        .into_any_element()
}

/// Two versions as one diff, lines matched ignoring indentation.
fn line_diff(id: String, before: &str, after: &str) -> AnyElement {
    let rows = ov::line_diff(before, after);
    let texts: Vec<String> = rows.iter().map(|r| r.1.replace('\t', "    ")).collect();
    let spans = syntax::highlight_snippet(&texts, "x.sql");
    div().id(ElementId::Name(id.into())).overflow_x_scroll().w_full().py(px(6.)).rounded(px(6.)).border_1().border_color(theme::border())
        .bg(theme::canvas()).font_family(theme::CODE_FONT).text_size(px(11.)).line_height(px(15.)).text_color(theme::text())
        .children(rows.iter().zip(texts).zip(spans).map(|(((m, _), t), s)| div().flex().gap(px(6.)).px(px(9.)).whitespace_nowrap()
            .when(*m == '+', |d| d.bg(theme::added_bg())).when(*m == '-', |d| d.bg(theme::removed_bg()))
            .child(div().w(px(8.)).flex_none().text_color(if *m == '+' { theme::green() } else { theme::red() })
                .child(match m { '+' => "+", '-' => "−", _ => " " }))
            .child(highlighted(t, &s))))
        .into_any_element()
}

// MARK: - Render

impl Render for ChangeOverview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(c) = self.shown() else {
            return div().size_full().flex().flex_col().items_center().justify_center().gap_2()
                .child(ui::icon("loader").size(px(16.)))
                .child(div().text_size(px(12.5)).text_color(theme::text_muted()).child(ov::READING))
                .into_any_element();
        };
        let (reports, branch) = (c.reports.clone(), c.branch.clone());
        let width = self.width.clone();
        let me = cx.entity_id();
        let mut body = vec![];
        body.push(div().flex().flex_col().gap(px(10.)).child(self.summary(&reports, cx))
            .when_some(branch, |d, b| d.child(self.branch_row(&b, cx))).into_any_element());
        let nothing = ov::nothing_structural(&reports);
        if nothing { body.push(div().text_size(px(12.5)).text_color(theme::text_muted()).child(ov::NOTHING_STRUCTURAL).into_any_element()) }
        for r in ov::sections(&reports) { body.push(self.section(r, cx)) }
        div().id("overview").size_full().relative().overflow_y_scroll().track_scroll(&self.scroll)
            .child(canvas(move |b, _, cx| {
                let w = f32::from(b.size.width);
                if (w - width.get()).abs() > 1. { width.set(w); cx.notify(me) }
            }, |_, _, _, _| {}).absolute().size_full())
            .child(div().flex().flex_col().gap(px(28.)).px(px(24.)).py(px(20.)).w_full().children(body))
            .into_any_element()
    }
}

impl ChangeOverview {
    fn summary(&self, reports: &[LensReport], cx: &mut Context<Self>) -> AnyElement {
        let tiles = ov::summary_tiles(reports);
        let nothing = ov::nothing_structural(reports) as usize;
        let section_ix = |r: &LensReport| 1 + nothing + ov::sections(reports).position(|s| s.kind == r.kind).unwrap_or(0);
        div().flex().gap(px(10.)).w_full().children(tiles.into_iter().map(|r| {
            let files = r.kind == ReportKind::Files;
            let id = format!("tile-{:?}", r.kind);
            let hover = self.hover.as_deref() == Some(id.as_str());
            let m = &r.metric;
            let ix = section_ix(r);
            let scroll = self.scroll.clone();
            let summary: SharedString = r.summary.clone().into();
            let number = div().flex().flex_col().gap(px(8.)).flex_none()
                .child(div().flex().items_baseline().gap(px(5.))
                    .child(div().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(m.value.to_string()))
                    .child(div().text_size(px(12.5)).text_color(theme::text_muted()).child(m.unit.clone())))
                .child(div().flex().gap(px(6.)).h(px(13.)).items_center()
                    .child(if ov::tile_restates(r) { caption(r.summary.clone()).truncate().into_any_element() } else { deltas(m.added, m.removed, m.changed).into_any_element() })
                    .when_some(ov::files_to_read(r), |d, n| d.child(caption(format!("{n} to read")))));
            let id2 = id.clone();
            div().id(ElementId::Name(id.clone().into())).flex().flex_col().gap(px(8.)).p(px(12.)).flex_1().min_w_0()
                .rounded(px(8.)).border_1().border_color(theme::border())
                .bg(if hover && !files { theme::raised() } else { theme::card() })
                .on_hover(cx.listener(move |v, h: &bool, _, cx| {
                    let next = if *h { Some(id2.clone()) } else if v.hover.as_deref() == Some(id2.as_str()) { None } else { v.hover.clone() };
                    if next != v.hover { v.hover = next; cx.notify() }
                }))
                .tooltip(move |w, cx| Tooltip::new(summary.clone()).build(w, cx))
                .when(!files, |d| d.cursor_pointer().on_click(move |_, _, _| scroll.scroll_to_top_of_item(ix)))
                .child(div().flex().items_center().gap(px(7.))
                    .child(div().size(px(20.)).flex_none().rounded(px(5.)).bg(report_tint(r.kind)).flex().items_center().justify_center()
                        .child(ui::icon(report_icon(r.kind)).size(px(11.)).text_color(theme::text())))
                    .child(div().flex_1().truncate().text_size(px(12.5)).text_color(theme::text_muted()).child(r.title.clone()))
                    .child(ui::icon("chevron-right").size(px(10.)).text_color(theme::text_faint()).opacity(if hover && !files { 1. } else { 0. })))
                .map(|d| if files {
                    // Beside the number while there's room, under it in a narrow tile: a wrap does both.
                    d.child(div().flex().flex_wrap().items_end().gap(px(16.)).child(number)
                        .child(div().flex_1().min_w(px(240.)).child(self.file_kinds(r, cx))))
                } else { d.child(number) })
                .into_any_element()
        }))
        .into_any_element()
    }

    /// The thin bar split by kind, and the "● 2 SQL" chips, each opening its files.
    fn file_kinds(&self, r: &LensReport, cx: &mut Context<Self>) -> AnyElement {
        let facts = r.sections.first().map(|s| s.facts.clone()).unwrap_or_default();
        let total = facts.iter().map(|f| f.count.unwrap_or(0)).sum::<usize>().max(1) as f32;
        div().flex().flex_col().gap(px(6.)).w_full()
            .child(div().flex().gap(px(1.5)).h(px(4.)).w_full().rounded_full().overflow_hidden()
                .children(facts.iter().map(|f| div().h_full().min_w(px(2.)).flex_grow(f.count.unwrap_or(0) as f32 / total).flex_basis(px(0.))
                    .bg(kind_bar_color(&f.title)))))
            .child(div().flex().flex_wrap().gap_x(px(12.)).gap_y(px(3.)).children(facts.iter().map(|f| {
                let open = self.open_kind.as_deref() == Some(f.title.as_str());
                let t = f.title.clone();
                let files = r.kinds.get(&f.title).cloned().unwrap_or_default();
                div().relative().child(div().id(ElementId::Name(format!("chip-{t}").into())).flex().items_center().gap(px(4.)).cursor_pointer()
                    .text_size(px(11.)).text_color(if open { theme::text() } else { theme::text_muted() }).hover(|s| s.text_color(theme::text()))
                    .on_click(cx.listener(move |v, _, _, cx| { v.open_kind = Some(t.clone()); cx.notify() }))
                    .child(ui::dot(kind_bar_color(&f.title)))
                    .child(ov::kind_chip_label(f)))
                    .when(open, |d| d.child(self.kind_files(&f.title, &files, cx)))
            })))
            .into_any_element()
    }

    fn kind_files(&self, kind: &str, files: &[ChangedPath], cx: &mut Context<Self>) -> AnyElement {
        let rows = files.iter().map(|c| {
            let p = c.path().to_string();
            let (dir, name) = (loupe_code_parent(&p), p.rsplit('/').next().unwrap_or(&p).to_string());
            let open = self.on_open.clone();
            let p2 = p.clone();
            div().id(ElementId::Name(format!("kf-{p}").into())).flex().items_center().gap(px(7.)).px(px(12.)).py(px(4.))
                .text_size(px(12.)).cursor_pointer().hover(|s| s.bg(theme::hover()))
                .tooltip({ let t: SharedString = format!("Open {p}").into(); move |w, cx| Tooltip::new(t.clone()).build(w, cx) })
                .on_click(cx.listener(move |v, _, w, cx| { v.open_kind = None; cx.notify(); open(p2.clone(), None, w, cx) }))
                .child(ui::file_icon(&p))
                .child(div().flex_none().text_color(theme::text()).child(name))
                .child(div().flex_1().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().text_color(theme::text_muted()).child(dir))
                .child(status_symbol(ov::change_kind(c), 10.))
        });
        let scrolls = files.len() > 16;
        deferred(anchored().snap_to_window_with_margin(px(8.)).offset(point(px(0.), px(18.))).child(
            div().id("kind-files").w(px(400.)).flex().flex_col().pb(px(8.)).rounded(px(8.)).bg(theme::raised())
                .border_1().border_color(theme::border_strong()).shadow_lg().occlude()
                .on_mouse_down_out(cx.listener(|v, _, _, cx| { v.open_kind = None; cx.notify() }))
                .child(div().px(px(12.)).pt(px(10.)).pb(px(4.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(kind.to_string()))
                .child(div().id("kind-files-list").flex().flex_col().when(scrolls, |d| d.h(px(460.)).overflow_y_scroll()).children(rows))))
            .with_priority(1).into_any_element()
    }

    fn section(&self, r: &LensReport, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("collapsed-{:?}", r.kind);
        let collapsed = self.is(&key);
        div().flex().flex_col().gap(px(12.)).w_full()
            .child(div().id(ElementId::Name(key.clone().into())).flex().items_center().gap(px(6.)).cursor_pointer().on_click(self.toggle(key, cx))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(r.title.clone()))
                .child(chevron(!collapsed))
                .child(div().pl(px(4.)).flex_1().truncate().text_size(px(12.5)).text_color(theme::text_muted()).child(r.summary.clone())))
            .when(!collapsed, |d| d.child(match r.kind {
                ReportKind::Files => div().into_any_element(),
                ReportKind::Database => self.database(&r.database, cx),
                ReportKind::Dependencies => self.fact_sections(r, 10, "Packages", cx),
                ReportKind::Configuration => self.fact_sections(r, 10, "Keys", cx),
                ReportKind::FollowUps => self.fact_sections(r, 8, "Lines", cx),
                ReportKind::Code => self.fact_sections(r, 6, "", cx),
            }))
            .into_any_element()
    }

    /// The first `limit` rows in a group, and "Show N More …" / "Show Fewer".
    fn capped(&self, key: String, rows: Vec<AnyElement>, limit: usize, noun: &str, inset: f32, cx: &mut Context<Self>) -> gpui_kit::Div {
        let all = self.is(&key);
        let n = rows.len();
        let mut g = grouped();
        for (i, r) in rows.into_iter().enumerate().take(if all { n } else { limit }) {
            if i > 0 { g = g.child(hairline(inset)) }
            g = g.child(r);
        }
        if n > limit {
            g = g.child(hairline(inset)).child(div().px(px(12.)).py(px(7.))
                .child(link(format!("{key}-link"), if all { "Show Fewer".to_string() } else { format!("Show {} More {noun}", n - limit) }).on_click(self.toggle(key, cx))));
        }
        g
    }

    // MARK: Dependencies, configuration, code, follow-ups

    fn fact_sections(&self, r: &LensReport, limit: usize, noun: &str, cx: &mut Context<Self>) -> AnyElement {
        div().flex().flex_col().gap(px(14.)).w_full().children(r.sections.iter().enumerate().map(|(si, s)| {
            let rows = s.facts.iter().enumerate().map(|(i, f)| {
                let id = format!("{:?}-{si}-{i}", r.kind);
                let row = match r.kind {
                    ReportKind::Dependencies => dependency_row(f),
                    ReportKind::Configuration => config_row(f),
                    ReportKind::FollowUps => follow_up_row(f),
                    _ => code_row(f),
                };
                self.openable(id, f.path.as_deref(), f.line, row, cx)
            }).collect();
            // The Code lens's noun was its section title ("Show 2 More Copied"); "Items" reads better (§14 #12).
            let noun = if noun.is_empty() { "Items" } else { noun };
            let inset = if r.kind == ReportKind::FollowUps { 10. } else { 32. };
            div().flex().flex_col().gap(px(6.)).child(subheading(s.title.clone()))
                .child(self.capped(format!("cap-{:?}-{si}", r.kind), rows, limit, noun, inset, cx))
        })).into_any_element()
    }

    // MARK: Database

    fn database(&self, groups: &[DBGroup], cx: &mut Context<Self>) -> AnyElement {
        div().flex().flex_col().gap(px(20.)).w_full().children(groups.iter().map(|g| {
            let mut col = div().flex().flex_col().gap(px(16.)).w_full()
                .child(div().flex().items_center().gap(px(5.)).text_size(px(12.5)).text_color(theme::text_muted())
                    .child(ui::icon("folder").size(px(13.)))
                    .child(div().flex_1().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().child(g.root.clone())));
            if !g.warnings.is_empty() {
                let rows = g.warnings.iter().enumerate().map(|(i, w)| self.warning_row(format!("w-{}-{i}", g.root), w, cx)).collect();
                col = col.child(self.capped(format!("cap-w-{}", g.root), rows, 6, "Warnings", 36., cx));
            }
            if !g.tables.is_empty() {
                col = col.child(div().flex().flex_col().gap(px(8.)).child(subheading("Tables")).child(self.masonry(g, cx)));
            }
            if !g.objects.is_empty() {
                let mut grp = grouped();
                for (i, o) in g.objects.iter().enumerate() {
                    if i > 0 { grp = grp.child(hairline(34.)) }
                    grp = grp.child(self.object_row(format!("o-{}-{i}", g.root), o, cx));
                }
                col = col.child(div().flex().flex_col().gap(px(8.)).child(subheading("Functions, Views and Types")).child(grp));
            }
            if !g.data.is_empty() {
                let mut grp = grouped();
                for (i, d) in g.data.iter().enumerate() {
                    if i > 0 { grp = grp.child(hairline(34.)) }
                    grp = grp.child(self.data_row(format!("d-{}-{i}", g.root), d, cx));
                }
                col = col.child(div().flex().flex_col().gap(px(8.)).child(subheading("Data")).child(grp));
            }
            col
        })).into_any_element()
    }

    /// Cards in as many 300–640 wide columns as fit, each under the shortest column (heights estimated by rows).
    fn masonry(&self, g: &DBGroup, cx: &mut Context<Self>) -> AnyElement {
        let width = (self.width.get() - 48.).max(300.);
        let (n, w) = ov::masonry_columns(width, g.tables.len(), 300., 640., 12.);
        let est = |t: &DBTable| 34. + 20. * t.columns.len().min(12) as f32 + 22. * t.extras.len() as f32 + if t.sql.is_some() { 30. } else { 0. };
        let heights: Vec<f32> = g.tables.iter().map(est).collect();
        let places = ov::masonry_place(n, w, 12., &heights);
        let mut cols: Vec<Vec<AnyElement>> = (0..n).map(|_| vec![]).collect();
        for (i, t) in g.tables.iter().enumerate() {
            let c = ((places[i].0 / (w + 12.)).round() as usize).min(n - 1);
            cols[c].push(self.table_box(format!("t-{}-{i}", g.root), t, cx));
        }
        div().flex().gap(px(12.)).items_start().children(cols.into_iter().map(|c| div().flex().flex_col().gap(px(12.)).flex_1().min_w(px(300.)).max_w(px(640.)).children(c)))
            .into_any_element()
    }

    fn table_box(&self, id: String, t: &DBTable, cx: &mut Context<Self>) -> AnyElement {
        let show_all = self.is(&format!("{id}-all"));
        let rows = ov::table_rows(t, show_all);
        let limit = ov::table_row_limit(t, show_all);
        let header = div().flex().items_center().gap(px(8.)).px(px(12.)).py(px(8.))
            .child(ui::icon("table").size(px(13.)))
            .child(div().flex().flex_col().flex_1().min_w_0()
                .child(div().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(t.name.clone()))
                .when_some(t.old_name.clone(), |d, o| d.child(caption(format!("Renamed from {o}")))))
            .child(div().flex().items_center().gap(px(4.)).child(status_symbol(t.kind, 11.)).child(caption(ov::table_status_label(t))));
        let mut g = grouped().child(self.openable(format!("{id}-h"), t.path.as_deref(), t.line, header, cx));
        if !ov::table_columns_hidden(t, show_all) {
            let mut list = div().flex().flex_col().py(px(5.));
            for (k, row) in rows.iter().take(limit).enumerate() {
                list = list.child(match *row {
                    ov::TableRow::Column(i) => { let c = &t.columns[i]; self.openable(format!("{id}-c{i}"), c.path.as_deref(), c.line, column_row(c, t.kind), cx) }
                    ov::TableRow::Gap(n) => div().id(ElementId::Name(format!("{id}-g{k}").into())).pl(px(40.)).py(px(3.)).cursor_pointer()
                        .on_click(self.toggle(format!("{id}-all"), cx)).child(faint(format!("{n} unchanged columns"))).into_any_element(),
                });
            }
            if rows.len() > limit {
                list = list.child(div().pl(px(40.)).py(px(3.)).child(link(format!("{id}-more"), format!("Show {} More Columns", rows.len() - limit)).on_click(self.toggle(format!("{id}-all"), cx))));
            }
            g = g.child(ui::divider()).child(list);
        }
        if !t.extras.is_empty() {
            g = g.child(hairline(12.)).child(div().flex().flex_col().gap(px(2.)).py(px(5.)).children(t.extras.iter().enumerate().map(|(i, f)| {
                let row = self.extra_row(format!("{id}-x{i}"), f, cx);
                self.openable(format!("{id}-x{i}-o"), f.path.as_deref(), f.line, row, cx)
            })));
        }
        if let Some(sql) = &t.sql {
            let k = format!("{id}-sql");
            let open = self.is(&k);
            g = g.child(hairline(12.)).child(div().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(7.))
                .child(self.disclosure(k.clone(), ov::table_sql_title(t), open, cx))
                .when(open, |d| d.child(code_block(format!("{k}-cb"), sql, "x.sql", None, 30, self.is(&format!("{k}-cb")), self.toggle(format!("{k}-cb"), cx)))));
        }
        g.into_any_element()
    }

    fn disclosure(&self, key: String, title: &str, open: bool, cx: &mut Context<Self>) -> AnyElement {
        div().id(ElementId::Name(format!("{key}-disc").into())).flex().items_center().gap(px(4.)).cursor_pointer()
            .text_size(px(11.)).text_color(theme::text_muted()).on_click(self.toggle(key, cx))
            .child(chevron(open)).child(title.to_string()).into_any_element()
    }

    fn extra_row(&self, id: String, f: &LensFact, cx: &mut Context<Self>) -> gpui_kit::Div {
        let icon = match ov::extra_icon(f) { "list.bullet" => "list", "bolt" => "zap", "key" => "key-round", _ => "shield-check" };
        let body = match ov::extra_body(f) {
            ov::ExtraBody::Diff { before, after } => Some(div().pt(px(3.)).child(line_diff(format!("{id}-ld"), &before, &after)).into_any_element()),
            ov::ExtraBody::Code { text, kind } => Some(div().pt(px(3.)).child(code_block(format!("{id}-cb"), &text, "x.sql", Some(kind), 14, self.is(&format!("{id}-cb")), self.toggle(format!("{id}-cb"), cx))).into_any_element()),
            ov::ExtraBody::Inline(s) => Some(mono(s, 10.5).line_clamp(2).into_any_element()),
            ov::ExtraBody::None => None,
        };
        div().flex().items_start().gap(px(6.)).px(px(12.)).py(px(2.5)).w_full()
            .child(status_symbol(f.kind, 10.))
            .child(ui::icon(icon).size(px(10.)).mt(px(2.)))
            .child(div().flex().flex_col().gap(px(1.)).flex_1().min_w_0()
                .child(div().flex().gap(px(5.)).child(div().truncate().text_size(px(12.)).text_color(theme::text()).child(f.title.clone()))
                    .child(faint(f.tag.clone().unwrap_or_default())))
                .children(body)
                .children(f.notes.iter().map(|n| div().text_size(px(10.5)).text_color(theme::text_muted()).line_clamp(3).child(n.clone()))))
    }

    fn object_row(&self, id: String, o: &lens::DBObject, cx: &mut Context<Self>) -> AnyElement {
        let code = ov::object_is_code(o);
        let k = format!("{id}-def");
        let open = ov::object_open_by_default(o) != self.is(&k);   // the toggle flips the default
        let header = div().flex().items_center().gap(px(8.))
            .child(status_symbol(o.kind, 12.))
            .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme::text()).truncate().child(o.name.clone()))
            .child(faint(o.ty.clone()))
            .child(div().flex_1())
            .child(caption(ov::object_status_label(o)));
        div().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(8.))
            .child(self.openable(format!("{id}-h"), o.path.as_deref(), o.line, header, cx))
            .children(o.notes.iter().map(|n| caption(n.clone()).pl(px(22.)).line_clamp(3)))
            .when_some(ov::object_inline(o), |d, s| d.child(mono(s, 11.).pl(px(22.)).line_clamp(2)))
            .when(code && (o.before.is_some() || o.after.is_some()), |d| d.child(div().flex().flex_col().gap(px(6.)).pl(px(22.))
                .child(self.disclosure(k.clone(), "Definition", open, cx))
                .when(open, |d| d.child(match (o.kind, &o.before, &o.after) {
                    (FactKind::Changed, Some(b), Some(a)) => line_diff(format!("{id}-ld"), b, a),
                    _ => code_block(format!("{id}-cb"), o.after.as_deref().or(o.before.as_deref()).unwrap_or(""), "x.sql", None, 12, self.is(&format!("{id}-cb")), self.toggle(format!("{id}-cb"), cx)),
                }))))
            .into_any_element()
    }

    fn data_row(&self, id: String, d: &lens::DBData, cx: &mut Context<Self>) -> AnyElement {
        let k = format!("{id}-st");
        let open = self.is(&k);
        let stmts = ov::data_statements(d);
        let n = stmts.len();
        let header = div().flex().items_center().gap(px(8.))
            .child(status_symbol(d.kind, 12.))
            .child(div().flex().text_size(px(12.5)).child(div().text_color(theme::text_muted()).child(ov::data_verb(d)))
                .child(div().text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(d.table.clone())))
            .child(div().flex_1())
            .child(caption(ov::data_summary(d)))
            .child(faint(d.path.rsplit('/').next().unwrap_or("").to_string()).truncate());
        let mut blocks = vec![];
        if open {
            for (i, (b, a)) in stmts.iter().enumerate() {
                for (side, text, kind) in [("b", b, FactKind::Removed), ("a", a, FactKind::Added)] {
                    let Some(t) = text else { continue };
                    let kk = format!("{id}-{i}{side}");
                    blocks.push(code_block(kk.clone(), t, "x.sql", Some(kind), 10, self.is(&kk), self.toggle(kk, cx)));
                }
            }
        }
        div().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(8.))
            .child(self.openable(format!("{id}-h"), Some(&d.path), d.line, header, cx))
            .child(div().flex().flex_col().gap(px(6.)).pl(px(22.))
                .child(self.disclosure(k, &if n == 1 { "Statement".to_string() } else { format!("{n} statements") }, open, cx))
                .children(blocks))
            .into_any_element()
    }

    fn warning_row(&self, id: String, f: &LensFact, cx: &mut Context<Self>) -> AnyElement {
        let k = format!("{id}-show");
        let show = self.is(&k);
        let open = self.on_open.clone();
        let (p, line) = (f.path.clone(), f.line);
        div().flex().flex_col().gap(px(8.)).px(px(12.)).py(px(9.))
            .child(div().flex().items_start().gap(px(10.))
                .child(ui::icon("triangle-alert").size(px(14.)).text_color(theme::accent()))
                .child(div().flex().flex_col().gap(px(2.)).flex_1().min_w_0()
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(f.title.clone()))
                    .children(f.notes.iter().map(|n| caption(n.clone()))))
                .when_some(ov::warning_file_label(f), |d, l| d.child(faint(l).max_w(px(260.)).truncate()))
                .when(f.after.is_some(), |d| d.child(small_button(format!("{id}-sb"), if show { "Hide" } else { "Show" }).on_click(self.toggle(k.clone(), cx))))
                .when_some(p, |d, p| d.child(small_button(format!("{id}-ob"), "Open").on_click(move |_, w, cx| open(p.clone(), line, w, cx)))))
            .when(show, |d| d.when_some(f.after.clone(), |d, s| d.child(div().pl(px(24.)).child(code_block(format!("{id}-cb"), &s, "x.sql", None, 14, self.is(&format!("{id}-cb")), self.toggle(format!("{id}-cb"), cx))))))
            .into_any_element()
    }

    // MARK: Branch

    fn branch_row(&self, s: &BranchStatus, cx: &mut Context<Self>) -> AnyElement {
        let conflicts = s.conflicts.clone().unwrap_or_default();
        let mut g = grouped().child(div().flex().items_center().gap(px(10.)).px(px(12.)).py(px(9.))
            .child(ui::icon("git-branch").size(px(13.)))
            .child(div().flex().flex_col().gap(px(1.)).flex_1()
                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(ov::branch_headline(s)))
                .child(caption(ov::branch_touching(s))))
            .when_some(ov::branch_merge_label(s), |d, l| {
                let clean = conflicts.is_empty();
                let c = if clean { theme::green() } else { theme::red() };
                d.child(div().flex().items_center().gap(px(4.)).px(px(8.)).py(px(3.)).rounded_full().bg(c.opacity(0.12))
                    .text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(c)
                    .child(ui::icon(if clean { "circle-check" } else { "octagon-x" }).size(px(11.)).text_color(c)).child(l))
            }));
        if !conflicts.is_empty() {
            let open = self.is("br-conf");
            g = g.child(ui::divider()).child(self.disclosure_row("br-conf", "Conflicting files".into(), conflicts.len(), open, cx));
            if open {
                let rows = conflicts.iter().map(|f| {
                    let row = div().flex().items_center().gap(px(7.)).pl(px(36.)).pr(px(12.)).py(px(4.)).text_size(px(12.))
                        .child(ui::file_icon(f))
                        .child(div().flex_none().text_color(theme::text()).child(f.rsplit('/').next().unwrap_or(f).to_string()))
                        .child(div().flex_1().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().text_color(theme::text_muted()).child(loupe_code_parent(f)))
                        .when_some(ov::conflict_authors(s, f), |d, a| d.child(faint(a).truncate()));
                    self.openable(format!("brc-{f}"), Some(f), None, row, cx)
                }).collect();
                g = g.child(self.capped("cap-brc".into(), rows, 8, "Files", 36., cx).border_0().rounded_none());
            }
        }
        if !s.touching.is_empty() {
            let open = self.is("br-com");
            g = g.child(ui::divider()).child(self.disclosure_row("br-com", format!("Commits on {} that change these files", s.target), s.touching.len(), open, cx));
            if open {
                let rows = s.touching.iter().map(|c| {
                    let row = div().flex().items_center().gap(px(8.)).pl(px(36.)).pr(px(12.)).py(px(4.)).text_size(px(12.)).w_full()
                        .child(mono(c.id.clone(), 11.))
                        .child(div().truncate().text_color(theme::text()).child(c.subject.clone()))
                        .child(div().flex_none().text_color(theme::text_muted()).child(format!("{} · {}", c.author, c.date)))
                        .child(div().flex_1())
                        .child(faint(c.files.iter().map(|f| f.rsplit('/').next().unwrap_or(f)).collect::<Vec<_>>().join(", ")).truncate());
                    // Every other row opens what it names; a commit opens in History (§14 #14).
                    match self.on_commit.clone() {
                        Some(f) => { let sha = c.id.clone(); row.id(ElementId::Name(format!("brm-{}", c.id).into())).cursor_pointer().hover(|s| s.bg(theme::hover()))
                            .on_click(move |_, w, cx| f(sha.clone(), w, cx)).into_any_element() }
                        None => row.into_any_element(),
                    }
                }).collect();
                g = g.child(self.capped("cap-brm".into(), rows, 8, "Commits", 36., cx).border_0().rounded_none());
            }
        }
        g.into_any_element()
    }

    fn disclosure_row(&self, key: &str, title: String, count: usize, open: bool, cx: &mut Context<Self>) -> AnyElement {
        div().id(ElementId::Name(format!("{key}-row").into())).flex().items_center().gap(px(6.)).px(px(12.)).py(px(7.)).cursor_pointer()
            .hover(|s| s.bg(theme::hover())).on_click(self.toggle(key.to_string(), cx))
            .child(div().w(px(18.)).flex().justify_center().child(chevron(open)))
            .child(div().text_size(px(12.)).text_color(theme::text()).child(title))
            .child(ui::badge(count.to_string()))
            .into_any_element()
    }
}

fn loupe_code_parent(p: &str) -> String { p.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default() }

fn column_row(c: &lens::DBColumn, table: FactKind) -> gpui_kit::Div {
    let (own, quiet) = ov::column_marks(c, table);
    let notes = ov::column_notes(c, table);
    let nn: SharedString = if c.not_null { "NOT NULL".into() } else { "Nullable".into() };
    div().flex().flex_col().gap(px(1.)).px(px(12.)).py(px(2.5)).w_full()
        .child(div().flex().items_center().gap(px(6.))
            .child(div().w(px(12.)).flex_none().when(own, |d| d.child(status_symbol(c.kind, 10.))))
            .child(div().w(px(10.)).flex_none().when(c.primary, |d| d.child(div().id(ElementId::Name(format!("pk-{}", c.name).into()))
                .tooltip(|w, cx| Tooltip::new("Primary key").build(w, cx)).child(ui::icon("key-round").size(px(9.))))))
            .child(div().truncate().text_size(px(12.)).text_color(if quiet { theme::text_muted() } else { theme::text() }).child(c.name.clone()))
            .when_some(c.references.clone(), |d, r| { let t: SharedString = format!("References {r}").into();
                d.child(div().id(ElementId::Name(format!("ref-{}", c.name).into())).tooltip(move |w, cx| Tooltip::new(t.clone()).build(w, cx))
                    .child(ui::icon("link").size(px(9.)).text_color(theme::text_faint()))) })
            .child(div().flex_1())
            .child(div().id(ElementId::Name(format!("ty-{}", c.name).into())).tooltip(move |w, cx| Tooltip::new(nn.clone()).build(w, cx))
                .child(mono(ov::column_type(c), 11.).truncate())))
        .when(!notes.is_empty(), |d| d.child(caption(notes.join(", ")).pl(px(34.)).line_clamp(2)))
}

fn value_line(f: &LensFact) -> Option<String> {
    (f.before.is_some() || f.after.is_some()).then(|| if f.kind == FactKind::Changed {
        format!("{} → {}", f.before.as_deref().unwrap_or(""), f.after.as_deref().unwrap_or(""))
    } else { f.after.clone().or(f.before.clone()).unwrap_or_default() })
}

fn tag(t: &str, warn: bool) -> gpui_kit::Div {
    div().flex_none().text_size(px(11.)).text_color(if warn { theme::accent() } else { theme::text_muted() }).child(t.to_string())
}

fn dependency_row(f: &LensFact) -> gpui_kit::Div {
    div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(6.)).w_full()
        .child(status_symbol(f.kind, 12.))
        .child(div().truncate().text_size(px(12.5)).text_color(theme::text()).child(f.title.clone()))
        .child(div().flex_1())
        .child(mono(value_line(f).unwrap_or_default(), 11.).truncate())
        .when_some(f.tag.clone(), |d, t| { let w = t == "major" || t == "downgrade"; d.child(tag(&t, w)) })
}

fn config_row(f: &LensFact) -> gpui_kit::Div {
    let parts = lens::components(&f.title);
    let (prefix, last) = match parts.split_last() {
        Some((l, p)) => (p.iter().map(|x| format!("{x}.")).collect::<String>(), l.clone()),
        None => (String::new(), f.title.clone()),
    };
    let text = format!("{prefix}{last}");
    let dim = prefix.len();
    div().flex().flex_col().gap(px(2.)).px(px(10.)).py(px(6.)).w_full()
        .child(div().flex().items_center().gap(px(8.))
            .child(status_symbol(f.kind, 12.))
            .child(div().flex_1().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().font_family(theme::CODE_FONT)
                .text_size(px(11.5)).text_color(theme::text())
                .child(StyledText::new(text).with_highlights([(0..dim, HighlightStyle { color: Some(theme::text_muted()), ..Default::default() })])))
            .when_some(f.tag.clone(), |d, t| { let w = t == "breaking"; d.child(tag(&t, w)) }))
        .map(|d| match value_line(f) {
            Some(v) => d.child(mono(v, 11.).pl(px(22.)).truncate()),
            // Swift showed the detail only without a tag, so "new file · 3 keys" lost its count (§14 #13).
            None => d.when_some(f.detail.clone(), |d, t| d.child(caption(t).pl(px(22.)))),
        })
}

fn code_row(f: &LensFact) -> gpui_kit::Div {
    let warn = f.kind == FactKind::Warning;
    div().flex().items_start().gap(px(8.)).px(px(10.)).py(px(6.)).w_full()
        .child(match f.kind {
            FactKind::Warning => div().w(px(14.)).child(ui::icon("triangle-alert").size(px(11.)).text_color(theme::accent())).into_any_element(),
            FactKind::Info => div().w(px(14.)).child(ui::icon("copy").size(px(11.))).into_any_element(),
            k => status_symbol(k, 12.),
        })
        .child(div().flex().flex_col().gap(px(2.)).flex_1().min_w_0()
            .child(div().truncate().text_size(px(12.)).text_color(theme::text()).when(!warn, |d| d.font_family(theme::CODE_FONT)).child(f.title.clone()))
            .when_some(f.detail.clone(), |d, t| d.child(caption(t).line_clamp(2))))
        .when_some(f.tag.clone(), |d, t| d.child(tag(&t, false)))
}

fn follow_up_row(f: &LensFact) -> gpui_kit::Div {
    let at = f.tag.as_deref().and_then(|t| f.title.find(t).map(|i| i..i + t.len()));
    div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(6.)).w_full()
        .child(div().flex_1().min_w_0().truncate().font_family(theme::CODE_FONT).text_size(px(11.5)).text_color(theme::text_muted())
            .child(StyledText::new(f.title.clone()).with_highlights(at.map(|r| (r, HighlightStyle { color: Some(theme::accent()), ..Default::default() })))))
        .child(faint(ov::location_label(f)).flex_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_input_prefers_unstaged_and_marks_sides() {
        use loupe_core::FileChange as F;
        let mut r = F::new("b.rs", Kind::Renamed, true);
        r.orig_path = Some("a.rs".into());
        let got = changed_paths(&[r, F::new("n.rs", Kind::Untracked, false), F::new("d.rs", Kind::Deleted, false),
                                  F::new("m.rs", Kind::Modified, true), F::new("m.rs", Kind::Modified, false)]);
        let cp = |o: Option<&str>, n: Option<&str>| ChangedPath { old: o.map(Into::into), new: n.map(Into::into) };
        assert_eq!(got, [cp(Some("a.rs"), Some("b.rs")), cp(Some("d.rs"), None), cp(Some("m.rs"), Some("m.rs")), cp(None, Some("n.rs"))]);
    }

    #[test]
    fn inputs_key_like_swift() {
        let c = vec![ChangedPath { old: Some("a".into()), new: Some("a".into()) }];
        assert_eq!(commit_input("/r", "s1", None, c.clone()).key, "s1 1");
        assert_eq!(commit_input("/r", "s1", None, c.clone()).a, Snapshot::Empty);
        let p = pull_input("/r", "mb", "hd", c, "refs/remotes/origin/main", "main");
        assert_eq!(p.key, "mb hd 1 refs/remotes/origin/main");
        assert_eq!(p.target, Some(("refs/remotes/origin/main".into(), "main".into())));
    }
}
