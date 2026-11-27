//! `DiffView`: one file's diff as a card (header strip, then the code). A gpui `list` lays out only the rows in
//! view, each row draws its line with one shaped text run, and the derived passes (intra-line spans, syntax) run on
//! the background executor and arrive a beat after the text. A 150k-line file opens in a few milliseconds of row
//! building; nothing animates or polls while it sits there.
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::{ScrollableMask, Scrollbar};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, FontWeight,
    HighlightStyle, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment, ListOffset, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task, UnderlineStyle, WeakEntity, Window,
    Bounds, canvas, div, fill, font, img, list, point, px, relative, size,
};

use super::model::*;
use crate::theme;

type Callback<A> = Rc<dyn Fn(A, &mut Window, &mut App)>;
/// Runs on the background executor: syntax spans for every line of a file (loupe-code plugs in here).
pub type Highlighter = Arc<dyn Fn(&FileDiff) -> Syntax + Send + Sync>;

const BLAME_WIDTH: f32 = 210.;
const COMMENT_INSET: f32 = 56.;
const COMMENT_WIDTH: f32 = 620.;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TextPos { row: usize, col: usize }

struct RulerMark { top: f32, height: f32, color: Hsla, row: usize }   // top and height in content pixels

struct Composer { line: usize, input: Entity<TextareaState>, _sub: Subscription }

struct Find {
    input: Entity<InputState>,
    matches: Vec<Match>,
    by_row: HashMap<usize, Vec<usize>>,
    index: usize,
    _sub: Subscription,
}

pub struct DiffView {
    file: Arc<FileDiff>,
    comments: Vec<InlineComment>,
    hunk_actions: Vec<HunkAction>,
    side: DiffSide,          // as chosen; a one-sided file shows as a diff regardless
    layout: Layout,
    blame: Option<Arc<Blame>>,
    blame_rows: HashMap<usize, (Option<String>, bool)>,
    rows: Rc<Vec<Row>>,
    tops: Vec<f32>,          // estimated top of each row, and the total at the end: the ruler and scroll targets
    marks: Vec<Mark>,
    ruler: Rc<Vec<RulerMark>>,
    ruler_bounds: Rc<Cell<Bounds<Pixels>>>,
    highlights: Arc<Highlights>,
    syntax: Option<Arc<Syntax>>,
    highlighter: Option<Highlighter>,
    _highlight_task: Task<()>,
    _syntax_task: Task<()>,
    list: ListState,
    hscroll: ScrollHandle,
    focus: FocusHandle,
    anchor: Option<TextPos>,
    head: Option<TextPos>,
    sel_right: bool,         // split: which side the selection is on
    hover: Option<(usize, Range<usize>)>,   // the identifier under the mouse while ⌘ is held
    menu_at: Option<(usize, isize)>,       // the right-clicked row and column
    find: Option<Find>,
    composer: Option<Composer>,
    change_ruler: bool,
    images: (Option<PathBuf>, Option<PathBuf>),
    max_cols: usize,
    max_digits: usize,
    char_width: Pixels,
    font: EditorFont,
    font_version: u64,
    show_header: bool,
    first_paint: Option<Box<dyn FnOnce()>>,
    _escape: Subscription,

    on_hunk_action: Option<Rc<dyn Fn(HunkAction, BTreeSet<usize>, &mut Window, &mut App)>>,
    on_add_comment: Option<Rc<dyn Fn(Option<u32>, Option<u32>, String, &mut Window, &mut App)>>,
    on_delete_draft: Option<Callback<u64>>,
    can_comment: Option<Rc<dyn Fn(&DiffLine) -> bool>>,
    on_navigate: Option<Rc<dyn Fn(String, CodeAction, &mut Window, &mut App)>>,
    on_mark_viewed: Option<Callback<()>>,
    on_side_change: Option<Rc<dyn Fn(DiffSide, Layout, &mut Window, &mut App)>>,
    header_actions: Option<Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>>,
}

impl DiffView {
    pub fn new(file: Arc<FileDiff>, cx: &mut Context<Self>) -> Self {
        // Escape in the find field or the composer: the text inputs take it for themselves before any listener here
        // sees it, so it's caught on the way in.
        let weak = cx.weak_entity();
        let escape = cx.intercept_keystrokes(move |e, window, cx| {
            if e.keystroke.key != "escape" || e.keystroke.modifiers.modified() { return }
            let handled = weak.update(cx, |v, cx| {
                if !v.focus.contains_focused(window, cx) { return false }
                if v.find.is_some() { v.close_find(window, cx) } else if v.composer.is_some() { v.cancel_comment(window, cx) } else { return false }
                true
            });
            if handled.unwrap_or(false) { cx.stop_propagation() }
        });
        let mut v = Self {
            file: Arc::new(FileDiff::default()),
            comments: vec![],
            hunk_actions: vec![],
            side: DiffSide::Diff,
            layout: Layout::Unified,
            blame: None,
            blame_rows: HashMap::new(),
            rows: Rc::default(),
            tops: vec![0.],
            marks: vec![],
            ruler: Rc::default(),
            ruler_bounds: Rc::default(),
            highlights: Arc::default(),
            syntax: None,
            highlighter: None,
            _highlight_task: Task::ready(()),
            _syntax_task: Task::ready(()),
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            hscroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            anchor: None,
            head: None,
            sel_right: true,
            hover: None,
            menu_at: None,
            find: None,
            composer: None,
            change_ruler: false,
            images: (None, None),
            max_cols: 0,
            max_digits: 3,
            char_width: px(0.),
            font: EditorFont::current(cx),
            font_version: FONT_VERSION.load(std::sync::atomic::Ordering::Relaxed),
            show_header: true,
            first_paint: None,
            _escape: escape,
            on_hunk_action: None,
            on_add_comment: None,
            on_delete_draft: None,
            can_comment: None,
            on_navigate: None,
            on_mark_viewed: None,
            on_side_change: None,
            header_actions: None,
        };
        v.set_file(file, cx);
        v
    }

    // MARK: Inputs

    /// Shows `file`. The same path again (a reload, Before / After loading the whole file) keeps the code at the top of
    /// the view in place, anchored by line number; a new path starts at the top with no selection.
    pub fn set_file(&mut self, file: Arc<FileDiff>, cx: &mut Context<Self>) {
        if file.uid == self.file.uid { return }
        let same_path = file.path() == self.file.path();
        let anchor = if same_path { self.top_line() } else { None };
        let uid = file.uid;
        let cached = cache::get(uid);
        self.file = file;
        self.syntax = None;
        self.highlights = Arc::default();
        match cached.highlights {
            Some(h) => self.highlights = h,
            None => {
                let file = self.file.clone();
                self._highlight_task = cx.spawn(async move |this, cx| {
                    let h = Arc::new(cx.background_spawn(async move { intra_line_highlights(&file) }).await);
                    this.update(cx, |this, cx| {
                        if this.file.uid != uid { return }
                        cache::remember(uid, Some(h.clone()), None);
                        this.highlights = h;
                        cx.notify();
                    }).ok();
                });
            }
        }
        match cached.syntax {
            Some(s) => self.syntax = Some(s),
            None => self.start_syntax(cx),
        }
        if !same_path { self.anchor = None; self.head = None; self.composer = None; self.hscroll.set_offset(point(px(0.), px(0.))) }
        self.rebuild(anchor, cx);
    }

    pub fn file(&self) -> &Arc<FileDiff> { &self.file }

    pub fn set_comments(&mut self, comments: Vec<InlineComment>, cx: &mut Context<Self>) {
        if comments == self.comments { return }
        self.comments = comments;
        self.rebuild(None, cx);
    }

    /// Buttons on each hunk header and in the context menu; empty → none (review mode).
    pub fn set_hunk_actions(&mut self, actions: Vec<HunkAction>, cx: &mut Context<Self>) {
        self.hunk_actions = actions;
        cx.notify();
    }

    pub fn set_side(&mut self, side: DiffSide, cx: &mut Context<Self>) {
        if side == self.side { return }
        let anchor = self.top_line();
        self.side = side;
        self.rebuild(anchor, cx);
    }

    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if layout == self.layout { return }
        let anchor = self.top_line();
        self.layout = layout;
        self.anchor = None;
        self.head = None;
        self.rebuild(anchor, cx);
    }

    pub fn set_blame(&mut self, blame: Option<Arc<Blame>>, cx: &mut Context<Self>) {
        self.blame = blame;
        self.blame_rows = self.blame.as_ref().map(|b| blame_labels(&self.file, &self.rows, b)).unwrap_or_default();
        cx.notify();
    }

    /// Marks for the changes beside the scrollbar (PR Full mode); clicking one scrolls to it.
    pub fn set_change_ruler(&mut self, on: bool, cx: &mut Context<Self>) { self.change_ruler = on; cx.notify() }

    pub fn set_show_header(&mut self, on: bool, cx: &mut Context<Self>) { self.show_header = on; cx.notify() }

    /// Images for a binary or LFS file's two sides, already on disk (the integrator finds the LFS object).
    pub fn set_images(&mut self, old: Option<PathBuf>, new: Option<PathBuf>, cx: &mut Context<Self>) { self.images = (old, new); cx.notify() }

    pub fn set_highlighter(&mut self, h: Option<Highlighter>, cx: &mut Context<Self>) {
        self.highlighter = h;
        if self.syntax.is_none() { self.start_syntax(cx) }
    }

    pub fn on_hunk_action(&mut self, f: impl Fn(HunkAction, BTreeSet<usize>, &mut Window, &mut App) + 'static) { self.on_hunk_action = Some(Rc::new(f)) }
    /// Commenting is on while this is set: (new line, old line, body). A removed line comments on the old side.
    pub fn on_add_comment(&mut self, f: impl Fn(Option<u32>, Option<u32>, String, &mut Window, &mut App) + 'static) { self.on_add_comment = Some(Rc::new(f)) }
    pub fn on_delete_draft(&mut self, f: impl Fn(u64, &mut Window, &mut App) + 'static) { self.on_delete_draft = Some(Rc::new(f)) }
    pub fn can_comment(&mut self, f: impl Fn(&DiffLine) -> bool + 'static) { self.can_comment = Some(Rc::new(f)) }
    /// ⌘-click and the context menu's Go to Definition / Find References.
    pub fn on_navigate(&mut self, f: impl Fn(String, CodeAction, &mut Window, &mut App) + 'static) { self.on_navigate = Some(Rc::new(f)) }
    /// The v key (PR review marks the file viewed and moves on).
    pub fn on_mark_viewed(&mut self, f: impl Fn((), &mut Window, &mut App) + 'static) { self.on_mark_viewed = Some(Rc::new(f)) }
    /// The header's Diff / Before / After and Unified / Split: the integrator persists them ("diffSide").
    pub fn on_side_change(&mut self, f: impl Fn(DiffSide, Layout, &mut Window, &mut App) + 'static) { self.on_side_change = Some(Rc::new(f)) }
    /// Extra controls at the end of the header strip (Stage File, Viewed, …).
    pub fn header_actions(&mut self, f: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) { self.header_actions = Some(Rc::new(f)) }
    /// Called once, after the first frame that shows this view (open-time measurement).
    pub fn on_first_paint(&mut self, f: impl FnOnce() + 'static) { self.first_paint = Some(Box::new(f)) }

    pub fn row_count(&self) -> usize { self.rows.len() }
    pub fn list_state(&self) -> &ListState { &self.list }

    // MARK: Rows

    /// A file with only one side (new, deleted, or only additions or removals) always shows as a diff.
    fn shown_side(&self) -> DiffSide {
        if self.file.additions > 0 && self.file.deletions > 0 { self.side } else { DiffSide::Diff }
    }
    fn split(&self) -> bool { self.layout == Layout::Split && self.shown_side() == DiffSide::Diff }

    fn rebuild(&mut self, anchor: Option<(Option<u32>, Option<u32>, f32)>, cx: &mut Context<Self>) {
        let keep = self.list.logical_scroll_top();
        let side = self.shown_side();
        let f = &self.file;
        let rows = build_rows(f, side, self.layout, &self.comments, self.composer.as_ref().map(|c| c.line));
        let (mut max_cols, mut max_no) = (0, 0);
        for l in &f.lines {
            // Byte length bounds the char count; exact enough for the scroll width and much cheaper.
            max_cols = max_cols.max(((l.text.end - l.text.start) as usize).min(MAX_DRAWN_CHARS));
            max_no = max_no.max(l.old_line.unwrap_or(0)).max(l.new_line.unwrap_or(0));
        }
        self.max_cols = max_cols;
        self.max_digits = max_no.to_string().len().max(3);
        let lh = f32::from(self.font.line_height);
        let mut tops = Vec::with_capacity(rows.len() + 1);
        let mut y = 0.;
        for r in &rows {
            tops.push(y);
            y += match *r {
                Row::Hunk(..) => lh + 8.,
                Row::Comment(k) => comment_height(&self.comments[k]),
                Row::Composer(_) => 190.,
                _ => lh,
            };
        }
        tops.push(y);
        self.tops = tops;
        self.marks = change_marks(f, &rows, side);
        // Runs closer than ~a pixel on a tall ruler draw as one mark.
        // In pixels of content: the paint maps them onto the track, which is the view when the content is shorter.
        let total = y.max(1.);
        let mut ruler: Vec<RulerMark> = vec![];
        for m in &self.marks {
            let (top, end) = (self.tops[m.rows.start], self.tops[m.rows.end]);
            let color = if m.removed == 0 { theme::green() } else if m.added == 0 { theme::red() } else { theme::accent() };
            match ruler.last_mut() {
                Some(l) if top - (l.top + l.height) < total * 0.0015 => { l.height = end - l.top; if l.color != color { l.color = theme::accent() } }
                _ => ruler.push(RulerMark { top, height: end - top, color, row: m.rows.start }),
            }
        }
        self.ruler = Rc::new(ruler);
        self.blame_rows = self.blame.as_ref().map(|b| blame_labels(f, &rows, b)).unwrap_or_default();
        self.list.reset_with_uniform_height(rows.len(), self.font.line_height);
        self.rows = Rc::new(rows);
        let target = anchor.and_then(|(old, new, offset)| {
            let r = self.rows.iter().position(|&row| {
                let Some(i) = row_line(row, true) else { return false };
                let l = &self.file.lines[i];
                match (new, l.new_line, old, l.old_line) {
                    (Some(n), Some(ln), ..) => ln >= n,
                    (_, _, Some(o), Some(lo)) => lo >= o,
                    _ => false,
                }
            })?;
            Some((r, offset))
        });
        match target {
            Some((r, offset)) => self.scroll_row_to(r, px(offset)),
            None => self.list.scroll_to(keep),
        }
        self.refresh_find(cx);
        cx.notify();
    }

    fn start_syntax(&mut self, cx: &mut Context<Self>) {
        let Some(h) = self.highlighter.clone() else { return };
        let file = self.file.clone();
        let uid = file.uid;
        // Colours arrive a beat after the text: tokenizing a 150k-line file in the frame would stall the switch.
        self._syntax_task = cx.spawn(async move |this, cx| {
            let s = Arc::new(cx.background_spawn(async move { h(&file) }).await);
            this.update(cx, |this, cx| {
                if this.file.uid != uid { return }
                cache::remember(uid, None, Some(s.clone()));
                this.syntax = Some(s);
                cx.notify();
            }).ok();
        });
    }

    /// The first diff line in view, and how far below the top of the view its row starts.
    fn top_line(&self) -> Option<(Option<u32>, Option<u32>, f32)> {
        let top = self.list.logical_scroll_top();
        let r = (top.item_ix..self.rows.len()).find(|&r| row_line(self.rows[r], true).is_some())?;
        let l = &self.file.lines[row_line(self.rows[r], true)?];
        let offset = self.tops[r] - self.tops[top.item_ix] - f32::from(top.offset_in_item);
        Some((if l.kind == LineKind::Added { None } else { l.old_line }, if l.kind == LineKind::Removed { None } else { l.new_line }, offset))
    }

    /// Scrolls so row `r` starts `below` from the top of the view (by the estimated row heights above it).
    fn scroll_row_to(&self, r: usize, below: Pixels) {
        let want = (self.tops[r] - f32::from(below)).max(0.);
        let k = self.tops.partition_point(|&t| t <= want).saturating_sub(1).min(r);
        self.list.scroll_to(ListOffset { item_ix: k, offset_in_item: px(want - self.tops[k]) });
    }

    fn viewport_height(&self) -> Pixels {
        let h = self.list.viewport_bounds().size.height;
        if h > px(0.) { h } else { px(600.) }
    }

    /// Scrolls `r` to a third of the way down and selects it.
    pub fn show_row(&mut self, r: usize, cx: &mut Context<Self>) {
        if r >= self.rows.len() { return }
        self.scroll_row_to(r, self.viewport_height() / 3.);
        self.anchor = Some(TextPos { row: r, col: 0 });
        self.head = Some(TextPos { row: r, col: self.row_len(r, self.sel_right) });
        cx.notify();
    }

    /// Brings the new-side `line` into view and selects it (a jump from code navigation or the Overview).
    pub fn reveal(&mut self, line: u32, cx: &mut Context<Self>) {
        let f = &self.file;
        let r = self.rows.iter().position(|&row| {
            row_line(row, true).is_some_and(|i| f.lines[i].new_line == Some(line) && f.lines[i].kind != LineKind::Removed)
        });
        if let Some(r) = r { self.sel_right = true; self.show_row(r, cx) }
    }

    /// n / p.
    pub fn jump_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top().item_ix;
        if let Some(r) = jump_target(&self.file, &self.rows, top, forward) {
            self.list.scroll_to(ListOffset { item_ix: r, offset_in_item: px(0.) });
            cx.notify();
        }
    }

    // MARK: Text and geometry

    fn number_width(&self) -> Pixels { self.char_width * self.max_digits as f32 + px(12.) }
    fn blame_width(&self) -> Pixels { if self.blame.is_some() && !self.split() { px(BLAME_WIDTH) } else { px(0.) } }

    /// Where a row's text starts, from the left of its side.
    fn text_x(&self) -> Pixels {
        let nums = if self.split() || self.shown_side() != DiffSide::Diff { 1. } else { 2. };
        self.blame_width() + self.number_width() * nums + px(16.)
    }

    fn content_width(&self) -> Pixels { self.text_x() + self.char_width * self.max_cols as f32 + px(24.) }

    fn display(&self, i: usize) -> String { display_text(self.file.line_text(i)).into_owned() }

    fn row_chars(&self, row: usize, right: bool) -> Vec<char> {
        row_line(self.rows[row], right).map(|i| self.display(i).chars().collect()).unwrap_or_default()
    }

    fn row_len(&self, row: usize, right: bool) -> usize {
        row_line(self.rows[row], right).map_or(0, |i| display_text(self.file.line_text(i)).chars().count())
    }

    /// A window point over a row → (side, column from the text start, possibly negative over the gutter).
    fn column_at(&self, x: Pixels) -> (bool, isize) {
        let b = self.list.viewport_bounds();
        let mut x = x - b.origin.x;
        let mut right = false;
        if self.split() {
            let half = b.size.width / 2.;
            right = x >= half;
            if right { x -= half }
        }
        let col = ((x - self.text_x()) / self.char_width).round() as isize;
        (right, col)
    }

    fn identifier_at(&self, row: usize, right: bool, x: Pixels) -> Option<(Range<usize>, String)> {
        let b = self.list.viewport_bounds();
        let mut x = x - b.origin.x;
        if right { x -= b.size.width / 2. }
        let col = ((x - self.text_x()) / self.char_width).floor() as isize;
        let u = self.row_chars(row, right);
        let r = word_range(&u, col);
        if r.is_empty() || u[r.start].is_ascii_digit() { return None }
        Some((r.clone(), u[r].iter().collect()))
    }

    fn selection(&self) -> Option<(TextPos, TextPos)> {
        let (a, h) = (self.anchor?, self.head?);
        (a != h).then(|| (a.min(h), a.max(h)))
    }

    /// The selected columns on a row, if any. A full-line selection shows its newline (+1).
    fn selected_cols(&self, row: usize, right: bool) -> Option<Range<usize>> {
        let (s, e) = self.selection()?;
        if row < s.row || row > e.row || (self.split() && right != self.sel_right) || row_line(self.rows[row], right).is_none() { return None }
        let a = if row == s.row { s.col } else { 0 };
        let b = if row == e.row { e.col } else { self.row_len(row, right) + 1 };
        (a < b).then_some(a..b)
    }

    fn selected_lines(&self) -> BTreeSet<usize> {
        let Some((s, e)) = self.selection() else { return BTreeSet::new() };
        (s.row..=e.row).filter_map(|r| row_line(self.rows[r], self.sel_right)).collect()
    }

    pub fn selected_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        let mut out = vec![];
        for r in s.row..=e.row {
            if row_line(self.rows[r], self.sel_right).is_none() { continue }
            let u = self.row_chars(r, self.sel_right);
            let a = (if r == s.row { s.col } else { 0 }).min(u.len());
            let b = (if r == e.row { e.col } else { u.len() }).min(u.len());
            // ponytail: copies the displayed text, so tabs come out as four spaces
            out.push(if a < b { u[a..b].iter().collect() } else { String::new() });
        }
        Some(out.join("\n"))
    }

    pub fn copy(&self, cx: &mut App) {
        if let Some(t) = self.selected_text() { cx.write_to_clipboard(ClipboardItem::new_string(t)) }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let n = self.rows.len();
        if n == 0 { return }
        self.anchor = Some(TextPos { row: 0, col: 0 });
        self.head = Some(TextPos { row: n - 1, col: self.row_len(n - 1, self.sel_right) });
        cx.notify();
    }

    // MARK: Mouse

    /// Returns a navigation to run once the view is no longer borrowed.
    fn mouse_down(&mut self, row: usize, e: &MouseDownEvent, cx: &mut Context<Self>) -> Option<(String, CodeAction)> {
        let (right, col) = self.column_at(e.position.x);
        if e.modifiers.platform && e.click_count == 1 && self.on_navigate.is_some() {   // ⌘-click: go to definition
            if let Some((r, w)) = self.identifier_at(row, right, e.position.x) {
                self.sel_right = right;
                self.anchor = Some(TextPos { row, col: r.start });
                self.head = Some(TextPos { row, col: r.end });
                cx.notify();
                return Some((w, CodeAction::Definition));
            }
        }
        // Double-click on the line numbers: write a review comment on that line.
        if e.click_count == 2 && col < 0 && let Some(i) = row_line(self.rows[row], right) {
            self.begin_comment_later(i, cx);
            return None;
        }
        self.sel_right = right;
        let len = self.row_len(row, right);
        let at = TextPos { row, col: (col.max(0) as usize).min(len) };
        match e.click_count {
            2 => {
                let mut r = word_range(&self.row_chars(row, right), col);
                if r.is_empty() && r.start < len { r = r.start..r.start + 1 }
                self.anchor = Some(TextPos { row, col: r.start });
                self.head = Some(TextPos { row, col: r.end });
            }
            n if n >= 3 => { self.anchor = Some(TextPos { row, col: 0 }); self.head = Some(TextPos { row, col: len }) }
            _ => { self.anchor = Some(at); self.head = Some(at) }
        }
        cx.notify();
        None
    }

    fn mouse_move(&mut self, row: usize, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if e.pressed_button == Some(MouseButton::Left) {
            if self.anchor.is_none() { return }
            let (_, col) = self.column_at(e.position.x);
            let len = self.row_len(row, self.sel_right);
            let head = TextPos { row, col: (col.max(0) as usize).min(len) };
            if self.head != Some(head) { self.head = Some(head); cx.notify() }
            return;
        }
        let next = (e.modifiers.platform && self.on_navigate.is_some())
            .then(|| { let (right, _) = self.column_at(e.position.x); self.identifier_at(row, right, e.position.x).map(|(r, _)| (row, r)) })
            .flatten();
        if next != self.hover { self.hover = next; cx.notify() }
    }

    // MARK: Comments

    fn begin_comment_later(&mut self, i: usize, cx: &mut Context<Self>) {
        // The composer's input needs the window; the next frame has it.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let Some(w) = cx.active_window() else { return };
            w.update(cx, |_, window, cx| { this.update(cx, |this, cx| this.begin_comment(i, window, cx)).ok(); }).ok();
        });
    }

    pub fn begin_comment(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_add_comment.is_none() { return }
        let l = &self.file.lines[i];
        if matches!(l.kind, LineKind::Hunk | LineKind::Meta) || !self.can_comment.as_ref().is_none_or(|f| f(l)) { return }
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment"));
        let sub = cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = e { this.submit_comment(window, cx) }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.composer = Some(Composer { line: i, input, _sub: sub });
        self.rebuild(None, cx);
    }

    fn cancel_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.take().is_some() { self.rebuild(None, cx); window.focus(&self.focus, cx) }
    }

    fn submit_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = &self.composer else { return };
        let body = c.input.read(cx).value().trim().to_string();
        if body.is_empty() { return }
        let l = &self.file.lines[c.line];
        let (new, old) = if l.kind == LineKind::Removed { (None, l.old_line) } else { (l.new_line, None) };
        self.cancel_comment(window, cx);
        // Deferred: the callback may update this view (new comments), which is mid-update here.
        if let Some(f) = self.on_add_comment.clone() { window.defer(cx, move |window, cx| f(new, old, body, window, cx)) }
    }

    // MARK: Find

    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in file"));
            let sub = cx.subscribe_in(&input, window, |this, input, e: &InputEvent, _, cx| match e {
                InputEvent::Change => { let q = input.read(cx).value().to_string(); this.search(&q, true, cx) }
                InputEvent::PressEnter { shift, .. } => this.step(if *shift { -1 } else { 1 }, cx),
                _ => {}
            });
            self.find = Some(Find { input, matches: vec![], by_row: HashMap::new(), index: 0, _sub: sub });
        }
        let input = self.find.as_ref().unwrap().input.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() { window.focus(&self.focus, cx); cx.notify() }
    }

    /// After the file or layout changed under an open find bar.
    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &self.find else { return };
        let q = f.input.read(cx).value().to_string();
        self.search(&q, false, cx);
    }

    fn search(&mut self, q: &str, scroll: bool, cx: &mut Context<Self>) {
        let matches = search(&self.file, &self.rows, q);
        // Start at the first match from the top of the view down, as an editor does.
        let top = self.list.logical_scroll_top().item_ix;
        let Some(f) = &mut self.find else { return };
        f.by_row.clear();
        for (k, m) in matches.iter().enumerate() { f.by_row.entry(m.row).or_default().push(k) }
        f.index = matches.iter().position(|m| m.row >= top).unwrap_or(0);
        f.matches = matches;
        if scroll && !f.matches.is_empty() { self.scroll_to_match() }
        cx.notify();
    }

    pub fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else { return };
        if f.matches.is_empty() { return }
        let n = f.matches.len() as isize;
        f.index = ((f.index as isize + d + n) % n) as usize;
        self.scroll_to_match();
        cx.notify();
    }

    /// Brings the current match into view: centred vertically when it's off screen, and sideways past the edge.
    fn scroll_to_match(&self) {
        let Some(f) = &self.find else { return };
        let m = &f.matches[f.index];
        let vb = self.list.viewport_bounds();
        let visible = self.list.bounds_for_item(m.row).is_some_and(|b| b.top() >= vb.top() && b.bottom() <= vb.bottom());
        if !visible { self.scroll_row_to(m.row, self.viewport_height() / 2.) }
        if !self.split() {
            let x = self.text_x() + self.char_width * m.cols.start as f32;
            let w = self.char_width * m.cols.len() as f32;
            let view_w = self.hscroll.bounds().size.width;
            let left = -self.hscroll.offset().x;
            if x < left || x + w > left + view_w { self.hscroll.set_offset(point(-(x - view_w / 3.).max(px(0.)), px(0.))) }
        }
    }

    // MARK: Keys

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let mine = self.focus.is_focused(window);   // n / p / v only when the code has focus, not an input
        match k.key.as_str() {
            "f" if m.platform && !m.shift => self.open_find(window, cx),
            "g" if m.platform && self.find.is_some() => self.step(if m.shift { -1 } else { 1 }, cx),
            "escape" if self.find.is_some() => self.close_find(window, cx),
            "escape" if self.composer.is_some() => self.cancel_comment(window, cx),
            "c" if m.platform && mine => self.copy(cx),
            "a" if m.platform && mine => self.select_all(cx),
            "n" if mine && !m.modified() => self.jump_change(true, cx),
            "p" if mine && !m.modified() => self.jump_change(false, cx),
            "v" if mine && !m.modified() => { if let Some(f) = self.on_mark_viewed.clone() { window.defer(cx, move |window, cx| f((), window, cx)) } }
            _ => return,
        }
        cx.stop_propagation();
    }

    // MARK: Context menu

    fn build_menu(&self, menu: PopupMenu, this: WeakEntity<Self>) -> PopupMenu {
        let mut menu = menu;
        let Some((row, col)) = self.menu_at.filter(|(r, _)| *r < self.rows.len()) else { return menu };
        let right = self.sel_right || !self.split();
        let item = |label: String, f: Rc<dyn Fn(&mut DiffView, &mut Window, &mut Context<DiffView>)>| {
            let this = this.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| { this.update(cx, |v, cx| f(v, window, cx)).ok(); })
        };
        if let Some(nav) = self.on_navigate.clone() {
            let u = self.row_chars(row, right);
            let r = word_range(&u, col);
            if !r.is_empty() && !u[r.start].is_ascii_digit() {
                let w: String = u[r].iter().collect();
                for (label, action) in [("Go to Definition of", CodeAction::Definition), ("Find References to", CodeAction::References)] {
                    let (nav, w2) = (nav.clone(), w.clone());
                    menu = menu.item(PopupMenuItem::new(format!("{label} “{w}”")).on_click(move |_, window, cx| nav(w2.clone(), action, window, cx)));
                }
                menu = menu.separator();
            }
        }
        if let Some(i) = row_line(self.rows[row], right) {
            let mut any = false;
            if self.on_add_comment.is_some() && self.can_comment.as_ref().is_none_or(|f| f(&self.file.lines[i])) {
                menu = menu.item(item("Add Comment…".into(), Rc::new(move |v, window, cx| v.begin_comment(i, window, cx))));
                any = true;
            }
            // Lines: the selection if the click is inside it, else the clicked line if it's a change.
            let changed = |j: usize| matches!(self.file.lines[j].kind, LineKind::Added | LineKind::Removed);
            let selected = self.selected_lines();
            let lines: BTreeSet<usize> = if selected.contains(&i) { selected.into_iter().filter(|&j| changed(j)).collect() } else if changed(i) { [i].into() } else { BTreeSet::new() };
            let hunk: BTreeSet<usize> = self.file.hunks.get(self.file.lines[i].hunk as usize).map(|h| h.lines.clone().collect()).unwrap_or_default();
            if let Some(cb) = self.on_hunk_action.clone() {
                for (set, hunk_menu) in [(lines, false), (hunk, true)] {
                    if set.is_empty() { continue }
                    for &a in &self.hunk_actions {
                        let what = if hunk_menu { "Hunk".to_string() } else if set.len() == 1 { "Line".into() } else { format!("{} Lines", set.len()) };
                        let title = match a { HunkAction::Stage => format!("Stage {what}"), HunkAction::Unstage => format!("Unstage {what}"), HunkAction::Discard => format!("Discard {what}…") };
                        let (cb, set) = (cb.clone(), set.clone());
                        menu = menu.item(PopupMenuItem::new(title).on_click(move |_, window, cx| cb(a, set.clone(), window, cx)));
                        any = true;
                    }
                }
            }
            if any { menu = menu.separator() }
        }
        menu.item(item("Copy".into(), Rc::new(|v, _, cx| v.copy(cx))))
    }

    // MARK: Row rendering

    fn render_row(&self, ix: usize, this: &Entity<Self>) -> AnyElement {
        let lh = self.font.line_height;
        match self.rows[ix] {
            Row::Hunk(li, hi) => self.render_hunk(ix, li, hi),
            Row::Comment(k) => self.render_comment(&self.comments[k], this),
            Row::Composer(i) => self.render_composer(i, this),
            row => {
                let weak = this.downgrade();
                let focus = self.focus.clone();
                let hovering = self.hover.as_ref().is_some_and(|h| h.0 == ix);
                let el = div()
                    .id(("row", ix))
                    .font_family(self.font.family.clone()).text_size(self.font.size).line_height(lh)
                    .h(lh)
                    .w_full()
                    .flex()
                    .cursor(if hovering { CursorStyle::PointingHand } else { CursorStyle::IBeam })
                    .on_mouse_down(MouseButton::Left, {
                        let weak = weak.clone();
                        move |e, window, cx| {
                            window.focus(&focus, cx);
                            let nav = weak.update(cx, |v, cx| v.mouse_down(ix, e, cx).map(|n| (n, v.on_navigate.clone()))).ok().flatten();
                            if let Some(((w, a), Some(f))) = nav { f(w, a, window, cx) }
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let weak = weak.clone();
                        move |e, _, cx| {
                            weak.update(cx, |v, _| {
                                let (right, col) = v.column_at(e.position.x);
                                if v.split() && v.selection().is_none() { v.sel_right = right }
                                v.menu_at = Some((ix, col));
                            }).ok();
                        }
                    })
                    .on_mouse_move(move |e, _, cx| { weak.update(cx, |v, cx| v.mouse_move(ix, e, cx)).ok(); });
                match row {
                    Row::Split(a, b) => el
                        .child(div().w_1_2().h_full().flex().overflow_hidden().child(self.render_side(ix, a, false, true)))
                        .child(div().w(px(1.)).h_full().bg(theme::border()))
                        .child(div().flex_1().h_full().flex().overflow_hidden().child(self.render_side(ix, b, true, true)))
                        .into_any_element(),
                    Row::Line(i) => el.child(self.render_side(ix, Some(i), true, false)).into_any_element(),
                    _ => unreachable!(),
                }
            }
        }
    }

    /// One line: (blame) numbers, the +/− marker, the text over its selection and match backgrounds.
    fn render_side(&self, row: usize, line: Option<usize>, right: bool, split: bool) -> AnyElement {
        let side = self.shown_side();
        let diff = side == DiffSide::Diff;
        let nw = self.number_width();
        let cw = self.char_width;
        let Some(i) = line else {
            // The blank opposite an unpaired change in split.
            return div().size_full().bg(theme::hover()).into_any_element();
        };
        let l = &self.file.lines[i];
        let bg = match l.kind {
            LineKind::Added if diff => Some(theme::added_bg()),
            LineKind::Removed if diff => Some(theme::removed_bg()),
            _ => None,
        };
        let nums: Vec<Option<u32>> = if split { vec![if right { l.new_line } else { l.old_line }] } else {
            match side { DiffSide::Old => vec![l.old_line], DiffSide::New => vec![l.new_line], DiffSide::Diff => vec![l.old_line, l.new_line] }
        };
        let raw = self.file.line_text(i);
        let text = display_text(raw).into_owned();
        let dlen = text.len();
        let to_disp = |b: usize| display_offset(raw, dlen, b);

        // Syntax colours, the intra-line span, the ⌘-hover underline: merged into non-overlapping runs.
        let mut layers: Vec<(Range<usize>, HighlightStyle)> = vec![];
        if let Some(spans) = self.syntax.as_ref().and_then(|s| s.get(i)) {
            for sp in spans {
                let r = to_disp(sp.range.start as usize)..to_disp(sp.range.end as usize);
                if r.start < r.end && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) {
                    layers.push((r, HighlightStyle { color: Some(token_color(sp.kind)), ..Default::default() }));
                }
            }
        }
        if diff && let Some(r) = self.highlights.get(&i) {
            let r = to_disp(r.start)..to_disp(r.end);
            let c = if l.kind == LineKind::Added { theme::added_word() } else { theme::removed_word() };
            if r.start < r.end { layers.push((r, HighlightStyle { background_color: Some(c), ..Default::default() })) }
        }
        if let Some((hr, cols)) = &self.hover && *hr == row && (right || !split) {
            let r = col_to_byte(&text, cols.start)..col_to_byte(&text, cols.end);
            layers.push((r, HighlightStyle { underline: Some(UnderlineStyle { thickness: px(1.), color: Some(theme::blue()), wavy: false }), ..Default::default() }));
        }
        let runs = merge_layers(dlen, &layers);
        let color = if l.kind == LineKind::Meta { theme::text_faint() } else { theme::text() };

        let mut quads: Vec<AnyElement> = vec![];
        if let Some(cols) = self.selected_cols(row, right) {
            quads.push(div().absolute().top_0().h_full().left(cw * cols.start as f32).w(cw * cols.len() as f32).bg(selection_color()).into_any_element());
        }
        if let Some(f) = &self.find && let Some(ks) = f.by_row.get(&row) {
            for &k in ks {
                let m = &f.matches[k];
                if split && m.right != right { continue }
                let c = if k == f.index { theme::find_match_current() } else { theme::find_match() };
                quads.push(div().absolute().top(px(1.)).bottom(px(1.)).rounded(px(2.)).left(cw * m.cols.start as f32 - px(1.)).w(cw * m.cols.len() as f32 + px(2.)).bg(c).into_any_element());
            }
        }

        let mut el = div().relative().h_full().flex_1().flex().items_center().when_some(bg, |d, c| d.bg(c));
        // The diff marks an added or removed line with a bar at its left edge.
        if diff && let Some(bar) = match l.kind { LineKind::Added => Some(theme::green()), LineKind::Removed => Some(theme::red()), _ => None } {
            el = el.child(div().absolute().left_0().top_0().h_full().w(px(2.)).bg(bar));
        }
        if !split && let Some(b) = &self.blame {
            let _ = b;
            let (label, odd) = self.blame_rows.get(&row).cloned().unwrap_or((None, false));
            el = el.child(
                div().w(px(BLAME_WIDTH)).h_full().flex_none().flex().items_center().pl(px(8.)).pr(px(6.)).overflow_hidden()
                    .bg(if odd { theme::hover() } else { theme::card_header() })
                    .font_family(theme::UI_FONT).text_size(px(11.)).text_color(theme::text_faint())
                    .when_some(label, |d, t| d.child(t)),
            );
        }
        for n in nums {
            el = el.child(
                div().w(nw).flex_none().pr(px(6.)).flex().justify_end().text_color(theme::text_faint()).text_size(px(11.))
                    .when_some(n, |d, n| d.child(n.to_string())),
            );
        }
        let marker = match l.kind { LineKind::Added if diff => Some(("+", theme::green())), LineKind::Removed if diff => Some(("−", theme::red())), _ => None };
        el.child(div().w(px(16.)).flex_none().pl(px(4.)).when_some(marker, |d, (m, c)| d.text_color(c).child(m)))
            .child(div().relative().flex_none().h_full().flex().items_center().text_color(color).children(quads).child(StyledText::new(text).with_highlights(runs)))
            .into_any_element()
    }

    fn render_hunk(&self, ix: usize, li: usize, hi: usize) -> AnyElement {
        let header = self.file.line_text(li).to_string();
        let mut row = div().h(self.font.line_height + px(8.)).w_full().font_family(self.font.family.clone()).text_size(self.font.size).flex().items_center().gap(px(8.)).px(px(12.)).bg(theme::hunk_bg())
            .child(div().text_color(theme::text_muted()).text_size(px(11.)).whitespace_nowrap().child(header));
        if let Some(cb) = &self.on_hunk_action {
            for &a in &self.hunk_actions {
                let lines: BTreeSet<usize> = self.file.hunks.get(hi).map(|h| h.lines.clone().collect()).unwrap_or_default();
                let cb = cb.clone();
                let (label, red) = match a { HunkAction::Stage => ("Stage Hunk", false), HunkAction::Unstage => ("Unstage Hunk", false), HunkAction::Discard => ("Discard", true) };
                row = row.child(pill(("hunk", ix * 4 + a as usize), label, false).when(red, |p| p.text_color(theme::red()))
                    .on_click(move |_, window, cx| cb(a, lines.clone(), window, cx)));
            }
        }
        row.into_any_element()
    }

    fn render_comment(&self, c: &InlineComment, this: &Entity<Self>) -> AnyElement {
        let accent = if c.draft_id.is_some() { theme::accent() } else { theme::blue() };
        let author = if c.draft_id.is_some() { format!("{} · Pending", c.author) } else { c.author.clone() };
        let delete = c.draft_id.zip(self.on_delete_draft.clone()).map(|(id, f)| {
            div().id(("del", id as usize)).text_size(px(11.)).text_color(theme::text_faint()).cursor_pointer()
                .hover(|s| s.text_color(theme::red())).child("Delete")
                .on_click(move |_, window, cx| f(id, window, cx))
        });
        let _ = this;
        div().w_full().py(px(5.)).pl(px(COMMENT_INSET)).font_family(theme::UI_FONT).child(
            div().w(px(COMMENT_WIDTH)).rounded(theme::RADIUS_ROW).border_1().border_color(accent.opacity(0.35)).bg(accent.opacity(0.07))
                .px(px(12.)).py(px(7.)).flex().flex_col().gap(px(4.))
                .child(div().flex().justify_between().child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_muted()).child(author)).children(delete))
                .child(div().text_size(px(12.5)).text_color(theme::text()).child(c.body.clone())),
        ).into_any_element()
    }

    fn render_composer(&self, i: usize, this: &Entity<Self>) -> AnyElement {
        let Some(c) = &self.composer else { return div().into_any_element() };
        let l = &self.file.lines[i];
        let label = match (l.kind, l.new_line, l.old_line) {
            (LineKind::Removed, _, o) | (_, None, o) => format!("Line {} (old)", o.unwrap_or(0)),
            (_, Some(n), _) => format!("Line {n}"),
        };
        let (w1, w2) = (this.downgrade(), this.downgrade());
        div().w_full().py(px(6.)).pl(px(COMMENT_INSET)).font_family(theme::UI_FONT).child(
            div().w(px(COMMENT_WIDTH)).rounded(theme::RADIUS_CARD).border_1().border_color(theme::border_strong()).bg(theme::raised())
                .p(px(10.)).flex().flex_col().gap(px(8.))
                .child(div().text_size(px(11.)).text_color(theme::text_muted()).child(format!("{} · {label}", self.file.path())))
                .child(div().h(px(110.)).rounded(theme::RADIUS_ROW).border_1().border_color(theme::border()).bg(theme::canvas()).px(px(8.)).py(px(6.))
                    .text_color(theme::text()).text_size(px(12.5)).child(Textarea::new(&c.input).appearance(false).h_full()))
                .child(div().flex().justify_end().gap(px(8.))
                    .child(pill("composer-cancel", "Cancel", false).on_click(move |_, window, cx| { w1.update(cx, |v, cx| v.cancel_comment(window, cx)).ok(); }))
                    .child(pill("composer-add", "Add Comment  ⌘↩", true).on_click(move |_, window, cx| { w2.update(cx, |v, cx| v.submit_comment(window, cx)).ok(); }))),
        ).into_any_element()
    }

    // MARK: Chrome

    fn render_header(&self, cx: &mut Context<Self>, window: &mut Window) -> AnyElement {
        let f = &self.file;
        let path = f.path().to_string();
        let (dir, name) = match path.rsplit_once('/') { Some((d, n)) => (format!("{d}/"), n.to_string()), None => (String::new(), path.clone()) };
        let renamed = f.old_path.as_ref().filter(|o| f.new_path.is_some() && Some(*o) != f.new_path.as_ref()).cloned();
        let both = f.additions > 0 && f.deletions > 0;
        let seg = |id: &'static str, label: &'static str, on: bool, side: Option<DiffSide>, layout: Option<Layout>, cx: &mut Context<Self>| {
            pill(id, label, on).on_click(cx.listener(move |v, _, window, cx| {
                if let Some(s) = side { v.set_side(s, cx) }
                if let Some(l) = layout { v.set_layout(l, cx) }
                if let Some(f) = v.on_side_change.clone() { let (s, l) = (v.side, v.layout); window.defer(cx, move |window, cx| f(s, l, window, cx)) }
            }))
        };
        div().h(px(38.)).flex_none().flex().items_center().gap(px(8.)).px(px(12.)).bg(theme::card_header())
            .border_b_1().border_color(theme::border()).font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .when_some(renamed, |d, o| d.child(div().text_color(theme::text_muted()).child(format!("{o}  →"))))
            .child(div().flex().min_w_0().overflow_hidden().whitespace_nowrap()
                .child(div().text_color(theme::text_muted()).child(dir))
                .child(div().text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(name)))
            // CB-03: an unchanged file (the code browser's plain files) has no counts to show.
            .when(f.additions + f.deletions > 0, |d| d.child(div().font_family(theme::CODE_FONT).text_size(px(11.)).flex().gap(px(6.))
                .child(div().text_color(theme::green()).child(format!("+{}", f.additions)))
                .child(div().text_color(theme::red()).child(format!("−{}", f.deletions)))))
            .child(div().flex_1())
            // Before / After only mean something when the file has both: a new or deleted file is one side.
            .when(both, |d| d.child(div().flex().gap(px(2.))
                .child(seg("side-diff", "Diff", self.side == DiffSide::Diff, Some(DiffSide::Diff), None, cx))
                .child(seg("side-old", "Before", self.side == DiffSide::Old, Some(DiffSide::Old), None, cx))
                .child(seg("side-new", "After", self.side == DiffSide::New, Some(DiffSide::New), None, cx))))
            .when(self.shown_side() == DiffSide::Diff, |d| d.child(div().flex().gap(px(2.))
                .child(seg("layout-unified", "Unified", self.layout == Layout::Unified, None, Some(Layout::Unified), cx))
                .child(seg("layout-split", "Split", self.layout == Layout::Split, None, Some(Layout::Split), cx))))
            .when_some(self.header_actions.clone(), |d, f| d.child(f(window, cx)))
            .into_any_element()
    }

    fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.find.as_ref()?;
        let q_empty = f.input.read(cx).value().is_empty();
        let count = if f.matches.is_empty() { if q_empty { String::new() } else { "No results".into() } } else { format!("{} of {}", f.index + 1, f.matches.len()) };
        Some(div().h(px(38.)).flex_none().flex().items_center().gap(px(8.)).px(px(10.)).bg(theme::card_header())
            .border_b_1().border_color(theme::border()).font_family(theme::UI_FONT).text_size(px(12.))
            .child(div().w(px(260.)).h(px(26.)).flex().items_center().px(px(8.)).rounded(theme::RADIUS_PILL).border_1().border_color(theme::border_strong())
                .bg(theme::raised()).text_color(theme::text()).child(Input::new(&f.input).appearance(false)))
            .child(div().text_color(theme::text_muted()).font_family(theme::CODE_FONT).text_size(px(11.)).child(count))
            .child(pill("find-prev", "↑", false).on_click(cx.listener(|v, _, _, cx| v.step(-1, cx))))
            .child(pill("find-next", "↓", false).on_click(cx.listener(|v, _, _, cx| v.step(1, cx))))
            .child(div().flex_1())
            .child(pill("find-done", "Done", false).on_click(cx.listener(|v, _, window, cx| v.close_find(window, cx)))).into_any_element())
    }

    /// Painted, not laid out: a 150k-line file has thousands of runs, merged here into what the pixels can show.
    fn render_ruler(&self, cx: &mut Context<Self>) -> AnyElement {
        let ruler = self.ruler.clone();
        let total = self.tops.last().copied().unwrap_or(0.);
        let bounds = self.ruler_bounds.clone();
        let paint_bounds = bounds.clone();
        div().w(px(12.)).h_full().flex_none().relative().bg(theme::card()).border_l_1().border_color(theme::border()).cursor_pointer()
            .child(canvas(move |b, _, _| paint_bounds.set(b), move |b, _, window, _| {
                let scale = b.size.height / px(total.max(f32::from(b.size.height)));
                for m in ruler.iter() {
                    let top = b.top() + px(m.top * scale);
                    let h = px(m.height * scale).max(px(3.));
                    window.paint_quad(fill(Bounds::new(point(b.left() + px(3.), top), size(b.size.width - px(6.), h)), m.color).corner_radii(px(1.)));
                }
            }).size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                let b = bounds.get();
                if b.size.height <= px(0.) { return }
                // Back to content pixels, as the marks are.
                let total = v.tops.last().copied().unwrap_or(0.).max(f32::from(b.size.height));
                let y = (e.position.y - b.top()) / b.size.height * total;
                let slack = 4. * total / f32::from(b.size.height);
                if let Some(m) = v.ruler.iter().find(|m| y >= m.top - slack && y <= m.top + m.height + slack) { v.show_row(m.row, cx) }
            }))
            .into_any_element()
    }

    fn render_placeholder(&self, p: &Placeholder) -> AnyElement {
        let center = |title: &str, sub: Option<&str>| div().flex_1().flex().flex_col().items_center().justify_center().gap(px(6.))
            .child(div().text_color(theme::text_muted()).text_size(px(15.)).child(title.to_string()))
            .when_some(sub.map(str::to_string), |d, s| d.child(div().text_color(theme::text_faint()).text_size(px(12.)).child(s)));
        let image = |p: &Option<PathBuf>| p.clone().map(|p| img(p).max_w(px(360.)).max_h(px(360.)).rounded(px(6.)));
        match p {
            Placeholder::NoContent => center("No Content Changes", Some("Mode change or rename only.")).into_any_element(),
            Placeholder::Binary if self.images.0.is_none() && self.images.1.is_none() => center("Binary File", None).into_any_element(),
            Placeholder::Binary => div().p(px(20.)).flex().gap(px(24.))
                .children([("Before", &self.images.0), ("After", &self.images.1)].into_iter().filter(|(_, p)| p.is_some()).map(|(t, p)| {
                    div().flex().flex_col().gap(px(6.)).child(div().text_color(theme::text_muted()).text_size(px(11.)).child(t)).children(image(p))
                })).into_any_element(),
            Placeholder::Lfs { old, new } => {
                let side = |title: &str, p: &Option<LfsPointer>, im: &Option<PathBuf>| {
                    let mut d = div().flex().flex_col().gap(px(6.)).child(div().text_color(theme::text_muted()).text_size(px(11.)).child(title.to_string()));
                    match p {
                        Some(p) => {
                            d = d.child(div().text_color(theme::text()).text_size(px(17.)).child(format_bytes(p.size)))
                                .child(div().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_muted()).child(p.oid[..12].to_string()));
                            // LF-04: an object that's missing or doesn't decode (CGImageSource failed in Swift) says so.
                            let missing = || div().id("lfs-missing").text_size(px(11.)).text_color(theme::text_faint()).child("Not downloaded")
                                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Pull, or run git lfs fetch, to get this version").build(w, cx))
                                .into_any_element();
                            if let Some(i) = image(im) { d = d.child(gpui_kit::StyledImage::with_fallback(i, missing)) } else if is_image(self.file.path()) { d = d.child(missing()) }
                        }
                        None => d = d.child(div().text_color(theme::text_muted()).child("No file")),
                    }
                    d
                };
                let mut row = div().flex().gap(px(24.)).items_start();
                // A file shown whole (a pointer never downloaded) has the same object on both sides.
                if old == new { row = row.child(side("Object", new, &self.images.1)) } else {
                    if old.is_some() || new.is_none() { row = row.child(side("Before", old, &self.images.0)) }
                    if old.is_some() && new.is_some() { row = row.child(div().pt(px(20.)).text_color(theme::text_muted()).child("→")) }
                    if new.is_some() { row = row.child(side("After", new, &self.images.1)) }
                }
                div().p(px(20.)).flex().flex_col().gap(px(14.))
                    .child(div().flex().items_center().gap(px(6.)).child(crate::ui::icon("hard-drive").text_color(theme::text_muted()))
                        .child(div().text_color(theme::text()).font_weight(FontWeight::SEMIBOLD).child("LFS object")))
                    .child(row).into_any_element()
            }
        }
    }
}

impl gpui_kit::Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus.clone() }
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Settings › Editor changed the font: re-measure and re-lay the rows (their heights follow the line height).
        let v = FONT_VERSION.load(std::sync::atomic::Ordering::Relaxed);
        if v != self.font_version {
            self.font_version = v;
            self.font = EditorFont::current(cx);
            self.char_width = px(0.);
            self.rebuild(None, cx);
        }
        if self.char_width == px(0.) {
            let ts = window.text_system();
            let id = ts.resolve_font(&font(self.font.family.clone()));
            self.char_width = ts.advance(id, self.font.size, 'm').map(|s| s.width).unwrap_or(px(7.2));
        }
        if let Some(f) = self.first_paint.take() { window.on_next_frame(move |_, _| f()) }

        let header = self.show_header.then(|| self.render_header(cx, window));
        let find = self.render_find(cx);
        let card = div().id("diff-card").key_context("DiffView").track_focus(&self.focus)
            .on_key_down(cx.listener(|v, e, window, cx| v.key_down(e, window, cx)))
            .on_modifiers_changed(cx.listener(|v, e: &gpui_kit::ModifiersChangedEvent, _, cx| {
                if !e.modifiers.platform && v.hover.take().is_some() { cx.notify() }
            }))
            .size_full().flex().flex_col().overflow_hidden()
            .bg(theme::card()).border_1().border_color(theme::border()).rounded(theme::RADIUS_CARD)
            .font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .children(header).children(find);

        if let Some(p) = placeholder(&self.file) { return card.child(self.render_placeholder(&p)) }

        let this = cx.entity();
        let rows = self.rows.clone();
        let split = self.split();
        let width = self.content_width();
        let body_list = list(self.list.clone(), move |ix, _, cx| {
            if ix >= rows.len() { return div().into_any_element() }
            this.read(cx).render_row(ix, &this)
        })
        .when(split, |l| l.w_full())
        .when(!split, |l| l.w(width).min_w(relative(1.)))
        .h_full();

        let weak = cx.weak_entity();
        let body = div().id("diff-body").relative().flex_1().min_h_0().min_w_0()
            .child(div().id("diff-h").size_full().overflow_x_hidden().track_scroll(&self.hscroll).child(body_list))
            .when(!split, |d| d.child(ScrollableMask::new(Axis::Horizontal, &self.hscroll).id("diff-h-wheel")))
            .child(div().absolute().inset_0().child(Scrollbar::vertical(&self.list).viewport_from_layout()))
            .when(!split, |d| d.child(Scrollbar::horizontal(&self.hscroll)))
            .context_menu(move |menu, _, cx| match weak.upgrade() { Some(v) => v.read(cx).build_menu(menu, v.downgrade()), None => menu });
        card.child(div().flex_1().min_h_0().flex().child(body).when(self.change_ruler, |d| d.child(self.render_ruler(cx))))
    }
}

/// Estimated height of a comment row (the list measures the real one when it shows).
fn comment_height(c: &InlineComment) -> f32 {
    let lines: usize = c.body.lines().map(|l| l.chars().count() / 90 + 1).sum::<usize>().max(1);
    46. + 17. * lines as f32
}

/// Splits overlapping style layers into the sorted, disjoint runs StyledText wants; later layers win per field.
fn merge_layers(len: usize, layers: &[(Range<usize>, HighlightStyle)]) -> Vec<(Range<usize>, HighlightStyle)> {
    if layers.is_empty() { return vec![] }
    let mut cuts: Vec<usize> = layers.iter().flat_map(|(r, _)| [r.start.min(len), r.end.min(len)]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = vec![];
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut s = HighlightStyle::default();
        let mut any = false;
        for (r, l) in layers {
            if r.start <= a && b <= r.end {
                any = true;
                s.color = l.color.or(s.color);
                s.background_color = l.background_color.or(s.background_color);
                s.underline = l.underline.or(s.underline);
            }
        }
        if any { out.push((a..b, s)) }
    }
    out
}

/// The code font every diff draws with: Settings › Editor's font, size and line height.
#[derive(Clone)]
struct EditorFont { family: gpui_kit::SharedString, size: Pixels, line_height: Pixels }

static EDITOR_FONT: std::sync::Mutex<Option<(String, f32, f32)>> = std::sync::Mutex::new(None);
static FONT_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Sets the code font of every DiffView (editorFont / editorFontSize, and the row height in points:
/// `EditorSettings::line_height()`). An empty or uninstalled family draws with SF Mono where it's a family, else the
/// app's code font, as the Swift app fell back to the system monospaced font. Call `cx.refresh_windows()` after.
pub fn set_editor_font(family: &str, size: f32, line_height: f32) {
    let new = Some((family.to_string(), size, line_height));
    let mut f = EDITOR_FONT.lock().unwrap();
    if *f != new { *f = new; FONT_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
}

impl EditorFont {
    fn current(cx: &App) -> Self {
        let Some((family, size, lh)) = EDITOR_FONT.lock().unwrap().clone() else {
            return Self { family: theme::CODE_FONT.into(), size: theme::CODE_SIZE, line_height: theme::CODE_LINE_HEIGHT };
        };
        // ponytail: the family list is read once; a font installed while Loupe runs shows after a restart.
        static FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let fams = FAMILIES.get_or_init(|| cx.text_system().all_font_names());
        let has = |f: &str| fams.iter().any(|x| x == f);
        let family = if !family.is_empty() && has(&family) { family } else if has("SF Mono") { "SF Mono".into() } else { theme::CODE_FONT.into() };
        Self { family: family.into(), size: px(size), line_height: px(lh) }
    }
}

/// The selected code theme's token colors (0xRRGGBB, TokenKind order), shared by every diff. Zero: not set yet.
static TOKEN_RGB: [std::sync::atomic::AtomicU32; 9] = [const { std::sync::atomic::AtomicU32::new(0) }; 9];

/// Sets the syntax colors every DiffView draws with (loupe_code's CodeTheme::tokens); windows redraw on their next
/// frame, so call `cx.refresh_windows()` after a change.
pub fn set_token_colors(rgb: [u32; 9]) {
    for (a, v) in TOKEN_RGB.iter().zip(rgb) { a.store(v | 0xFF00_0000, std::sync::atomic::Ordering::Relaxed) }
}

fn token_color(kind: u8) -> Hsla {
    let v = TOKEN_RGB.get(kind as usize).map_or(0, |a| a.load(std::sync::atomic::Ordering::Relaxed));
    if v != 0 { return gpui_kit::rgb(v & 0xFF_FFFF).into() }
    match kind {
        0 => theme::purple(),           // keyword
        1 => theme::green(),            // string
        2 => theme::text_faint(),       // comment
        3 => theme::accent(),           // number
        4 => theme::blue(),             // type
        5 => theme::text(),             // function
        6 => theme::purple(),           // macro
        7 => theme::accent(),           // attribute
        _ => theme::text_muted(),       // property
    }
}

fn selection_color() -> Hsla { theme::selection() }

/// A pill button; `on` is the selected segment.
fn pill(id: impl Into<gpui_kit::ElementId>, label: &'static str, on: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    div().id(id).h(px(22.)).px(px(10.)).flex().items_center().rounded(theme::RADIUS_PILL).border_1()
        .font_family(theme::UI_FONT).text_size(px(11.5)).whitespace_nowrap().cursor_pointer()
        .border_color(if on { theme::border_strong() } else { theme::border() })
        .bg(if on { theme::selected() } else { theme::raised() })
        .text_color(if on { theme::text() } else { theme::text_muted() })
        .hover(|s| s.bg(theme::selected()).text_color(theme::text()))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_merge_into_disjoint_runs() {
        let c = |r: Range<usize>| (r, HighlightStyle { color: Some(theme::blue()), ..Default::default() });
        let b = (2..6, HighlightStyle { background_color: Some(theme::red()), ..Default::default() });
        let out = merge_layers(10, &[c(0..4), b]);
        assert_eq!(out.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(), vec![0..2, 2..4, 4..6]);
        assert!(out[1].1.color.is_some() && out[1].1.background_color.is_some());
        assert!(out[2].1.color.is_none());
    }
}
