//! The merge editor and conflicts (MergeEditor.swift, Conflicts.swift, Merge3.swift's UI). Owner: the Merge view
//! agent (docs/gpui/VIEWS.md).
//!
//! `install` presents `MergeEditor` over the window on `AppEvent::OpenMergeEditor`, and also when a conflicted file
//! becomes the selected one (selecting it opens the editor in Swift). The editor shows the operation banner on top
//! (Abort / Continue), Ours, Base and Theirs read-only, the chunk bar and the editable result, or the special
//! conflicts (deleted, one-sided, binary, symlink, submodule). The model is `session::Session`; byte ranges
//! throughout, as core's ConflictText::blocks gives them.
mod session;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, size,
    component::input::{Editor, EditorState, InputEditsEvent},
};
use loupe_core::conflicts::{self, MergeLabels};
use loupe_core::git::Kind;
use loupe_core::merge3::{ConflictKind, ConflictStages};
use loupe_core::model::{self as core_model, ConflictSide, GitOperation};
use loupe_forge::{ai, settings};

pub use session::{Loaded, Role, Session, Side, Take, Tint};
use crate::shell::keys::{NextConflict, PreviousConflict, UseOurs, UseTheirs};
use crate::state::{AppEvent, AppModel, WorktreeModel};
use crate::{theme, ui};

/// LOUPE_FLOWS_MERGE=<path>: a test hook for the flows fixture (like StressDriver's "merge" step): once the selected
/// worktree lists that conflicted path, it gets selected, which opens the editor. LOUPE_FLOWS_MERGE_DO=<steps>, comma
/// separated, then drives it once it's loaded (see `step`), for screenshots of each flow without a mouse.
const TEST_HOOK: &str = "LOUPE_FLOWS_MERGE";
const TEST_STEPS: &str = "LOUPE_FLOWS_MERGE_DO";

pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Subscription {
    let opener = cx.new(|cx| Opener::new(app.clone(), cx));
    window.subscribe(app, cx, move |app, e: &AppEvent, w, cx| {
        let _keep = &opener;
        let AppEvent::OpenMergeEditor { worktree, path } = e else { return };
        open(app, worktree, path, w, cx);
    })
}

fn open(app: Entity<AppModel>, worktree: &str, path: &str, window: &mut Window, cx: &mut App) {
    let showing = app.read(cx).overlay.clone().and_then(|v| v.downcast::<MergeEditor>().ok());
    if showing.is_some_and(|v| { let v = v.read(cx); v.path == path && v.worktree == worktree }) { return }
    let Some(model) = app.read(cx).model(worktree) else { return };
    let v = cx.new(|cx| MergeEditor::new(app.clone(), model, path.to_string(), window, cx));
    let f = v.read(cx).focus.clone();
    app.update(cx, |a, cx| a.present(v.into(), cx));
    window.focus(&f, cx);
}

/// Opens the editor when the selected file is a conflicted one, once per selection.
struct Opener {
    app: Entity<AppModel>,
    model: Option<(Entity<WorktreeModel>, Subscription)>,
    opened: Option<String>,
    hook: Option<String>,
    _sub: Subscription,
}

impl Opener {
    fn new(app: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&app, |o, _, cx| o.follow(cx));
        let mut o = Opener { app, model: None, opened: None, hook: std::env::var(TEST_HOOK).ok().filter(|p| !p.is_empty()), _sub: sub };
        o.follow(cx);
        o
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let m = self.app.read(cx).selected_model();
        if m.as_ref().map(|m| m.entity_id()) == self.model.as_ref().map(|x| x.0.entity_id()) { return }
        self.opened = None;
        self.model = m.map(|m| (m.clone(), cx.observe(&m, |o, m, cx| o.check(m, cx))));
    }

    fn check(&mut self, m: Entity<WorktreeModel>, cx: &mut Context<Self>) {
        if let Some(p) = self.hook.clone() && let Some(f) = m.read(cx).state.conflicted().iter().find(|f| f.path == p) {
            self.hook = None;
            let id = f.id();
            cx.defer(move |cx| m.update(cx, |m, cx| m.select_file(Some(id), cx)));
            return;
        }
        let (sel, wt) = { let m = m.read(cx); (m.state.selected_file().filter(|f| f.kind == Kind::Conflicted).map(|f| f.path.clone()), m.path_str()) };
        if sel == self.opened { return }
        self.opened = sel.clone();
        let Some(path) = sel else { return };
        let app = self.app.clone();
        cx.defer(move |cx| app.update(cx, |a, cx| a.emit(AppEvent::OpenMergeEditor { worktree: wt, path }, cx)));
    }
}

// MARK: - The operation banner

/// Above Changes while an operation is half done (OperationBanner in Conflicts.swift): what it is, what's left, and
/// the way out. None when there's no operation and no conflict. Changes can show it too.
pub fn banner(model: &Entity<WorktreeModel>, cx: &App) -> Option<AnyElement> {
    let s = &model.read(cx).state;
    let n = s.conflicted().len();
    if s.operation.is_none() && n == 0 { return None }
    let (icon, color) = if n > 0 { ("triangle-alert", theme::accent()) } else if s.stopped_to_edit { ("pencil", theme::blue()) } else { ("circle-check", theme::green()) };
    let title = s.operation.map_or("Conflicts".to_string(), |op| format!("{} {}", op.verb(), s.operation_subject));
    let subtitle = if n > 0 { format!("{n} file{} with conflicts. Resolve each one, then continue.", if n == 1 { "" } else { "s" }) }
        else if s.stopped_to_edit { "Stopped to edit this commit. Change it (commit with Amend), then continue.".into() }
        else if s.operation.is_none() { String::new() } else { "All conflicts are resolved.".into() };
    let busy = s.busy.is_some();
    let (m1, m2) = (model.clone(), model.clone());
    Some(div().flex().items_center().gap_2p5().px_3().py_2().flex_none()
        .bg(if n > 0 { theme::accent() } else { theme::green() }.opacity(0.08)).border_b_1().border_color(theme::border())
        .child(ui::icon(icon).text_color(color))
        .child(div().flex().flex_col().flex_1().min_w_0().gap_px()
            .child(tip(div().id("op-title").truncate().font_weight(FontWeight::MEDIUM).text_size(theme::UI_SIZE).text_color(theme::text())
                .child(title.clone()), Some(title)))
            .when(!subtitle.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(subtitle))))
        .when_some(s.operation, |d, op| d
            .child(button("op-abort", format!("Abort {}", abort_name(op)), Some(format!("Go back to how things were before the {}", op.command())), !busy)
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.finish_operation(true, cx).detach())))
            .child(button_primary("op-continue", "Continue".into(),
                Some(if n > 0 { "Resolve the conflicts first".into() } else { format!("Finish the {}", op.command()) }), n == 0 && !busy)
                .on_click(move |_, _, cx| m2.update(cx, |m, cx| m.finish_operation(false, cx).detach()))))
        .into_any_element())
}

/// "Merge", "Cherry-pick" (Swift's `.capitalized` gave "Cherry-Pick", FLOWS §14), "Revert", "Rebase".
fn abort_name(op: GitOperation) -> &'static str {
    match op { GitOperation::Merge => "Merge", GitOperation::CherryPick => "Cherry-pick", GitOperation::Revert => "Revert", GitOperation::Rebase => "Rebase" }
}

type Button = gpui_kit::Stateful<gpui_kit::Div>;

fn tip(d: Button, help: Option<String>) -> Button {
    match help { Some(h) => d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(h.clone()).build(w, cx)), None => d }
}

fn small(d: Button, enabled: bool) -> Button {
    d.h(px(24.)).px_2p5().text_size(px(12.)).when(!enabled, |d| d.opacity(0.4).cursor_default())
}

/// A small pill that takes clicks only when enabled.
fn button(id: &str, label: String, help: Option<String>, enabled: bool) -> Gated {
    Gated(tip(small(ui::pill(SharedString::from(id.to_string()), label), enabled), help), enabled)
}

fn button_primary(id: &str, label: String, help: Option<String>, enabled: bool) -> Gated {
    Gated(tip(small(ui::pill_primary(SharedString::from(id.to_string()), label, theme::blue()), enabled), help), enabled)
}

struct Gated(Button, bool);
impl Gated {
    fn on_click(self, f: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static) -> Button {
        if self.1 { self.0.on_click(f) } else { self.0 }
    }
}

// MARK: - The editor

struct Panes {
    ours: Entity<EditorState>,
    theirs: Entity<EditorState>,
    base: Entity<EditorState>,
    result: Entity<EditorState>,
}

pub struct MergeEditor {
    pub app: Entity<AppModel>,
    pub worktree: String,
    pub path: String,
    model: Entity<WorktreeModel>,
    focus: FocusHandle,
    loaded: Option<Loaded>,
    s: Option<Session>,
    panes: Option<Panes>,
    survivor: Option<Entity<EditorState>>,
    starts: HashMap<Role, Vec<usize>>,
    show_base: bool,
    known: Option<String>,
    status_version: u64,
    write: Option<Task<()>>,
    write_now: bool,
    loading: Option<Task<()>>,
    reveal: bool,
    suggesting: Option<Task<()>>,
    proposal: Option<(String, usize)>,
    confirm_reset: bool,
    ai: Option<ai::Selected>,
    stepped: bool,
    _subs: Vec<Subscription>,
}

impl MergeEditor {
    pub fn new(app: Entity<AppModel>, model: Entity<WorktreeModel>, path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let worktree = model.read(cx).path_str();
        let status_version = model.read(cx).state.status_version;
        let tab = app.read(cx).tab;
        let subs = vec![
            cx.observe_in(&model, window, |v, m, w, cx| v.model_changed(m, w, cx)),
            cx.observe(&app, move |v, a, cx| {
                // Another worktree or tab selected: the editor goes, as the pane did in Swift.
                let a = a.read(cx);
                if a.selection().map(|s| &s[..]) != Some(&v.worktree[..]) || a.tab != tab { v.close(cx) }
            }),
            // MG-26: whatever closes it, a pending write is flushed first.
            cx.on_release(|v, cx| v.flush_app(cx)),
        ];
        let mut v = MergeEditor {
            app, worktree, path, model, focus: cx.focus_handle(), loaded: None, s: None, panes: None, survivor: None,
            starts: HashMap::new(), show_base: false, known: None, status_version, write: None, write_now: false, loading: None,
            reveal: false, suggesting: None, proposal: None, confirm_reset: false, ai: None, stepped: false, _subs: subs,
        };
        v.load(window, cx);
        cx.spawn(async move |this, cx| {
            let paths = cx.background_executor().spawn(async { ai::installed().unwrap_or_else(ai::scan) }).await;
            this.update(cx, |v, cx| { v.ai = ai::selected(&settings::shared(), &paths); cx.notify() }).ok();
        }).detach();
        v
    }

    fn dir(&self) -> PathBuf { PathBuf::from(&self.worktree) }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.flush(cx);
        let me = cx.entity_id();
        let app = self.app.clone();
        cx.defer(move |cx| app.update(cx, |a, cx| if a.overlay.as_ref().is_some_and(|o| o.entity_id() == me) { a.dismiss(cx) }));
    }

    // MARK: Loading

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.write = None;
        let (dir, path) = (self.dir(), self.path.clone());
        self.loading = Some(cx.spawn_in(window, async move |this, cx| {
            let l = cx.background_executor().spawn(async move { session::load(&dir, &path) }).await;
            this.update_in(cx, |v, w, cx| v.apply(l, w, cx)).ok();
        }));
    }

    fn apply(&mut self, mut l: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = None;
        self.known = l.disk.clone();
        self.s = l.session.take();
        let make = |t: String, ro: bool, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| {
            let mut e = EditorState::new(window, cx).line_number(false).folding(false).soft_wrap(false).default_value(t);
            e.set_readonly(ro, cx);
            e
        });
        if let Some(s) = &self.s {
            let m = s.merge.as_ref().expect("a text session has its merge");
            let text = |lines: &[String]| lines.iter().map(|l| format!("{l}\n")).collect::<String>();
            let (o, t, b) = (text(&m.ours), text(&m.theirs), text(&m.base));
            self.starts = HashMap::from([(Role::Ours, session::line_starts(&o)), (Role::Theirs, session::line_starts(&t)),
                (Role::Base, session::line_starts(&b)), (Role::Result, session::line_starts(&s.text))]);
            let r = s.text.clone();
            match &self.panes {
                // A reload (an outside change, Reset…) loses the result's undo history, as in Swift.
                Some(p) => for (e, t) in [(p.ours.clone(), o), (p.theirs.clone(), t), (p.base.clone(), b), (p.result.clone(), r)] { e.update(cx, |e, cx| e.set_value(t, window, cx)) },
                None => {
                    let p = Panes { ours: make(o, true, window, cx), theirs: make(t, true, window, cx), base: make(b, true, window, cx), result: make(r, false, window, cx) };
                    self._subs.push(cx.subscribe_in(&p.result, window, |v, _, e: &InputEditsEvent, _, cx| v.edited(e, cx)));
                    self._subs.push(cx.observe(&p.result, |v, e, cx| v.caret_moved(e, cx)));
                    self.panes = Some(p);
                }
            }
            self.reveal = true;
        } else if let Some(t) = l.survivor.clone() {
            self.starts = HashMap::from([(Role::Ours, session::line_starts(&t))]);
            self.survivor = Some(make(t, true, window, cx));
        }
        self.loaded = Some(l);
        cx.notify();
    }

    /// Re-reads everything when the file or its stages changed behind our back (a terminal, the merge tool).
    fn reload_if_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded.is_none() || self.loading.is_some() { return }
        let (dir, path, known) = (self.dir(), self.path.clone(), self.known.clone());
        let stages = self.loaded.as_ref().map(|l| l.stages.clone()).unwrap_or_default();
        self.loading = Some(cx.spawn_in(window, async move |this, cx| {
            let (disk, st) = cx.background_executor().spawn(async move {
                (std::fs::read(dir.join(&path)).ok().and_then(|d| String::from_utf8(d).ok()), ConflictStages::read(&dir, &path))
            }).await;
            this.update_in(cx, |v, w, cx| {
                v.loading = None;
                // Empty stages: resolved elsewhere, the editor is about to go.
                if (disk != known || st != stages) && !st.entries.is_empty() { v.load(w, cx) }
            }).ok();
        }));
    }

    fn model_changed(&mut self, m: Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) {
        let (version, conflicted) = { let s = &m.read(cx).state; (s.status_version, s.conflicted().iter().any(|f| f.path == self.path)) };
        if version == self.status_version { cx.notify(); return }
        self.status_version = version;
        // Resolved (Mark Resolved, a whole side, elsewhere): the file left Conflicts, so the editor goes.
        if !conflicted && self.loaded.is_some() { self.close(cx); return }
        self.reload_if_changed(window, cx);
        cx.notify();
    }

    // MARK: Editing

    fn edited(&mut self, e: &InputEditsEvent, cx: &mut Context<Self>) {
        let Some(s) = &mut self.s else { return };
        for (edit, rope) in e.edits() { s.edited(edit.start_byte, edit.old_end_byte, edit.new_end_byte, rope.to_string()) }
        self.starts.insert(Role::Result, session::line_starts(&s.text));
        if std::mem::take(&mut self.write_now) {
            self.commit(cx);
        } else {
            self.write = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(350)).await;
                this.update(cx, |v, cx| v.commit(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// The caret moved into another conflict: it becomes current.
    fn caret_moved(&mut self, e: Entity<EditorState>, cx: &mut Context<Self>) {
        let at = e.read(cx).selected_range().start;
        if let Some(s) = &mut self.s && let Some(k) = s.conflict_at(at) && k != s.current { s.current = k; cx.notify() }
    }

    /// Writes the result (atomically, keeping the file's mode) if it differs from what's on disk.
    fn commit(&mut self, cx: &mut Context<Self>) { if let Some(e) = self.write_result() { self.model.update(cx, |m, cx| m.set_error(e, cx)) } }

    fn write_result(&mut self) -> Option<String> {
        self.write = None;
        let text = self.s.as_ref()?.file_text();
        if self.known.as_deref() == Some(text.as_str()) { return None }
        match write_atomic(&self.dir().join(&self.path), &text) {
            Ok(()) => { self.known = Some(text); None }
            Err(e) => Some(e.to_string()),
        }
    }

    fn flush(&mut self, cx: &mut Context<Self>) { if self.write.is_some() { self.commit(cx) } }

    fn flush_app(&mut self, cx: &mut App) {
        if self.write.is_some() && let Some(e) = self.write_result() { self.model.update(cx, |m, cx| m.set_error(e, cx)) }
    }

    /// Before git rewrites the file (Reset, a whole side): a write still waiting would land on top of it.
    fn discard_pending_write(&mut self) { self.write = None }

    fn replace(&mut self, r: Option<(Range<usize>, String)>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((range, text)), Some(p)) = (r, &self.panes) else { cx.notify(); return };
        // Through the text view, so ⌘Z takes it back; the file is written at once.
        self.write_now = true;
        p.result.update(cx, |e, cx| { e.set_selected_range(range, cx); e.replace(text, window, cx) });
        cx.notify();
    }

    fn take(&mut self, t: Take, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &mut self.s else { return };
        let r = s.take(t, s.current);
        self.replace(r, window, cx);
    }

    fn pick(&mut self, side: Side, editor: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) {
        let sel = editor.read(cx).selected_range();
        if !sel.is_empty() { return }
        let role = if side == Side::Ours { Role::Ours } else { Role::Theirs };
        let Some(line) = self.starts.get(&role).map(|st| session::line_at(st, sel.start)) else { return };
        let Some(s) = &mut self.s else { return };
        let r = s.click(side, line);
        self.replace(r, window, cx);
    }

    // MARK: Navigation

    fn go(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(k) = self.s.as_ref().and_then(|s| s.go(delta)) { self.focus_conflict(k, window, cx) }
    }

    /// Makes conflict `k` current, scrolls the result so it sits a quarter of the way down with the caret at its start,
    /// and scrolls Ours, Theirs and Base to its chunk.
    fn focus_conflict(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(s), Some(p)) = (&mut self.s, &self.panes) else { return };
        if k >= s.conflict_count() { return }
        s.current = k;
        let r = s.regions[s.conflicts[k]].range.clone();
        let row = self.starts.get(&Role::Result).map_or(0, |st| session::line_at(st, r.start));
        let res = p.result.clone();
        res.update(cx, |e, cx| e.set_selected_range(r.start..r.start, cx));
        scroll_to_row(&res, row, cx);
        for (role, e) in [(Role::Ours, &p.ours), (Role::Theirs, &p.theirs), (Role::Base, &p.base)] {
            if let Some(lr) = s.chunk_lines(role) { scroll_to_row(e, lr.start, cx) }
        }
        res.update(cx, |e, cx| e.focus(window, cx));
        cx.notify();
    }

    // MARK: Actions

    fn resolve(&mut self, side: Option<ConflictSide>, cx: &mut Context<Self>) {
        if side.is_some() { self.discard_pending_write() } else { self.flush(cx) }
        let path = self.path.clone();
        self.model.update(cx, |m, cx| m.resolve(path, side, cx).detach());
    }

    fn delete_file(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.model.update(cx, |m, cx| m.perform("Resolving", move |p| core_model::resolve_deleting(p, &path), cx).detach());
    }

    fn use_submodule(&mut self, sha: String, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.model.update(cx, |m, cx| m.perform("Resolving", move |p| conflicts::resolve_submodule(p, &path, &sha), cx).detach());
    }

    /// Reset… (`git checkout -m`), then one reload and the first conflict (Swift loaded twice, FLOWS §14).
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        self.discard_pending_write();
        let path = self.path.clone();
        let t = self.model.update(cx, |m, cx| m.perform("Resetting", move |p| conflicts::reset_conflict(p, &path), cx));
        cx.spawn_in(window, async move |this, cx| {
            if t.await.is_none() { return }
            this.update_in(cx, |v, w, cx| { v.loading = None; v.load(w, cx) }).ok();
        }).detach();
        cx.notify();
    }

    fn merge_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, gui)) = self.loaded.as_ref().and_then(|l| l.merge_tool.clone()) else { return };
        self.flush(cx);
        let path = self.path.clone();
        let t = self.model.update(cx, |m, cx| m.perform(&format!("Merging in {name}"), move |p| conflicts::run_merge_tool(p, &path, gui), cx));
        cx.spawn_in(window, async move |this, cx| {
            t.await;
            this.update_in(cx, |v, w, cx| v.reload_if_changed(w, cx)).ok();
        }).detach();
    }

    fn suggest(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.s else { return };
        let k = s.current;
        let (Some(prompt), Some(h)) = (s.prompt(&self.path, k), self.ai.clone()) else {
            self.model.update(cx, |m, cx| m.set_error("No AI tool is set up. Pick one in Settings › General.".to_string(), cx));
            return;
        };
        let dir = self.worktree.clone();
        self.suggesting = Some(cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { suggest(&h, &prompt, &dir) }).await;
            this.update(cx, |v, cx| {
                v.suggesting = None;
                match r {
                    Ok(text) => v.proposal = Some((text, k)),
                    Err(e) => v.model.update(cx, |m, cx| m.set_error(e, cx)),
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    /// "Use Suggestion" replaces the conflict it was asked for, even if another one is current now.
    fn accept_proposal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, k)) = self.proposal.take() else { return };
        let Some(s) = &mut self.s else { return };
        let Some((range, _)) = s.take(Take::Ours, k) else { return };
        let body: String = loupe_core::merge3::lines(&text).iter().map(|l| format!("{l}\n")).collect();
        self.replace(Some((range, body)), window, cx);
    }

    /// One LOUPE_FLOWS_MERGE_DO step: a button or key of the editor, by name.
    fn step(&mut self, st: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (name, arg) = st.split_once(':').unwrap_or((st, ""));
        match name {
            "ours" => self.take(Take::Ours, window, cx),
            "theirs" => self.take(Take::Theirs, window, cx),
            "ot" => self.take(Take::OursThenTheirs, window, cx),
            "to" => self.take(Take::TheirsThenOurs, window, cx),
            "base" => self.take(Take::Base, window, cx),
            "restore" => self.take(Take::Markers, window, cx),
            "next" => self.go(1, window, cx),
            "prev" => self.go(-1, window, cx),
            "unresolved" => if let Some(k) = self.s.as_ref().and_then(|s| s.next_unresolved()) { self.focus_conflict(k, window, cx) },
            "showbase" => { self.show_base = !self.show_base; cx.notify() }
            "pick" | "picktheirs" => {
                let side = if name == "pick" { Side::Ours } else { Side::Theirs };
                if let Some(s) = &mut self.s && let Ok(line) = arg.parse() { let r = s.click(side, line); self.replace(r, window, cx) }
            }
            "type" => if let Some(p) = &self.panes { p.result.update(cx, |e, cx| e.insert(arg.replace("\\n", "\n"), window, cx)) },
            "suggest" => self.suggest(cx),
            "accept" => self.accept_proposal(window, cx),
            "reset?" => { self.confirm_reset = true; cx.notify() }
            "reset" => self.reset(window, cx),
            "tool" => self.merge_tool(window, cx),
            "resolve" => self.resolve(None, cx),
            "useours" => self.resolve(Some(ConflictSide::Current), cx),
            "usetheirs" => self.resolve(Some(ConflictSide::Incoming), cx),
            "abort" => self.model.update(cx, |m, cx| m.finish_operation(true, cx).detach()),
            "continue" => self.model.update(cx, |m, cx| m.finish_operation(false, cx).detach()),
            "close" => self.close(cx),
            _ => {}
        }
    }

    fn labels(&self, cx: &App) -> MergeLabels {
        let s = &self.model.read(cx).state;
        MergeLabels::new(s.operation, s.branch.as_deref(), &s.operation_subject)
    }
}

/// One prompt through the commit-message AI tool, in its read-only mode. An empty reply is an error: Swift resolved
/// the conflict to nothing (FLOWS §14).
fn suggest(s: &ai::Selected, prompt: &str, dir: &str) -> Result<String, String> {
    let out = std::env::temp_dir().join(format!("loupe-merge-{}.txt", std::process::id()));
    let out_s = out.to_string_lossy().into_owned();
    let (args, stdin) = (s.harness.invocation)(prompt, &s.model, &out_s);
    let r = ai::run(&s.path, &args, stdin.then_some(prompt), dir, Duration::from_secs(120), None);
    let file = std::fs::read_to_string(&out).ok().filter(|f| !f.is_empty());
    let _ = std::fs::remove_file(&out);
    let mut text = r.map_err(|e| e.user_message().unwrap_or_else(|| e.to_string()))?;
    if s.harness.reads_output_file && let Some(f) = file { text = f }
    let text = session::clean_suggestion(&text);
    if text.trim().is_empty() { return Err(format!("{} suggested nothing for this conflict.", s.harness.name)) }
    Ok(text)
}

/// Atomically (a temp file renamed over it), keeping the file's permissions: a script stays executable.
fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok().map(|m| m.permissions().mode());
    let tmp = path.with_file_name(format!(".{}.loupe-merge", path.file_name().map_or("f".into(), |n| n.to_string_lossy())));
    std::fs::write(&tmp, text)?;
    if let Some(m) = mode { std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m))? }
    std::fs::rename(&tmp, path)
}

fn scroll_to_row(e: &Entity<EditorState>, row: usize, cx: &mut App) {
    e.update(cx, |e, cx| {
        let lh = e.line_height().unwrap_or(theme::CODE_LINE_HEIGHT);
        let h = e.input_bounds().size.height;
        let y = (lh * row as f32 - h / 4.).max(px(0.));
        e.set_scroll_offset(point(px(0.), -y), cx);
    });
}

fn tint_color(t: Tint) -> Hsla {
    // ponytail: theme.rs has no teal or orange; amber stands in for orange and grey for teal (requested).
    match t {
        Tint::Ours => theme::blue().opacity(0.13),
        Tint::OursPicked => theme::blue().opacity(0.32),
        Tint::Theirs => theme::purple().opacity(0.13),
        Tint::TheirsPicked => theme::purple().opacity(0.32),
        Tint::Both => theme::text_muted().opacity(0.12),
        Tint::Changed => theme::text_muted().opacity(0.15),
        Tint::Conflict => theme::accent().opacity(0.16),
        Tint::ConflictCurrent => theme::accent().opacity(0.30),
        Tint::Resolved => theme::green().opacity(0.12),
        Tint::ResolvedCurrent => theme::green().opacity(0.25),
    }
}

/// Whole-line fills behind an editor's text (TintLayoutManager): rows, colour, and whether it's a 2 pt bar (a
/// conflict resolved to nothing). Painted from the editor's last layout, so it follows its scrolling.
fn tint_layer(editor: Entity<EditorState>, starts: Vec<usize>, tints: Vec<(Range<usize>, Hsla, bool)>) -> impl IntoElement {
    canvas(|_, _, _| (), move |bounds: Bounds<gpui_kit::Pixels>, _, window, cx| {
        let e = editor.read(cx);
        let (Some(vr), Some(lh)) = (e.visible_row_range(), e.line_height()) else { return };
        let first = starts.get(vr.start).copied().unwrap_or(0);
        let Some(b) = e.range_to_bounds(&(first..first)) else { return };
        let y = |row: usize| b.origin.y + lh * (row as f32 - vr.start as f32);
        for (rows, color, bar) in &tints {
            if *bar {
                if rows.start + 1 < vr.start || rows.start > vr.end { continue }
                window.paint_quad(fill(Bounds::new(point(bounds.origin.x, y(rows.start) - px(1.)), size(bounds.size.width, px(2.))), color.opacity((color.a * 3.).min(1.))));
                continue;
            }
            let (a, z) = (rows.start.max(vr.start), rows.end.min(vr.end + 1));
            if a >= z { continue }
            window.paint_quad(fill(Bounds::new(point(bounds.origin.x, y(a)), size(bounds.size.width, lh * (z - a) as f32)), *color));
        }
    }).absolute().inset_0()
}

impl Focusable for MergeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus.clone() }
}

impl MergeEditor {
    fn pane(&self, role: Role, title: String, detail: String, dot: Hsla, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.panes.as_ref().expect("panes exist for a text conflict");
        let e = match role { Role::Ours => p.ours.clone(), Role::Theirs => p.theirs.clone(), _ => p.base.clone() };
        let s = self.s.as_ref().expect("a text session");
        let tints = s.pane_tints(role).into_iter().map(|(r, t)| (r, tint_color(t), false)).collect();
        let starts = self.starts.get(&role).cloned().unwrap_or_default();
        let show_base = self.show_base;
        let side = match role { Role::Ours => Some(Side::Ours), Role::Theirs => Some(Side::Theirs), _ => None };
        let e2 = e.clone();
        div().flex().flex_col().flex_1().min_w_0().h_full()
            .child(div().flex().items_center().gap_1p5().px_2().h(px(26.)).flex_none().bg(theme::card_header()).border_b_1().border_color(theme::border())
                .child(ui::dot(dot))
                .child(div().flex_none().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title))
                .child(tip(div().id(SharedString::from(format!("detail-{role:?}"))).flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(theme::text_muted())
                    .child(detail.clone()), Some(detail)))
                .when(role == Role::Ours, |d| d.child(
                    tip(div().id("base-toggle").flex().items_center().h(px(18.)).px_1p5().rounded(px(5.)).cursor_pointer().text_size(px(11.))
                        .border_1().border_color(theme::border_strong())
                        .map(|d| if show_base { d.bg(theme::selected()).text_color(theme::text()) } else { d.text_color(theme::text_muted()) })
                        .child("Base"), Some("Show the common ancestor between the two sides".into()))
                        .on_click(cx.listener(|v, _, _, cx| { v.show_base = !v.show_base; cx.notify() })))))
            .child(div().id(SharedString::from(format!("pane-{role:?}"))).relative().flex_1().min_h_0().overflow_hidden().bg(theme::card())
                .child(tint_layer(e.clone(), starts, tints))
                .child(Editor::new(&e).appearance(false).h_full())
                // A plain click (no drag, no selection) on a conflict line picks it (PickTextView): read after the
                // editor has placed the caret.
                .when_some(side, |d, side| d.capture_any_mouse_up(cx.listener(move |_, ev: &gpui_kit::MouseUpEvent, w, cx| {
                    if ev.button == MouseButton::Left && ev.click_count == 1 {
                        let e = e2.clone();
                        cx.defer_in(w, move |v, w, cx| v.pick(side, &e, w, cx));
                    }
                }))))
    }

    fn header(&self, labels: &MergeLabels, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.loaded.as_ref();
        let kind = l.map(|l| l.kind());
        let text = l.is_some_and(|l| l.is_text());
        let one_sided = matches!(kind, Some(ConflictKind::DeletedByUs | ConflictKind::DeletedByThem | ConflictKind::AddedByUs | ConflictKind::AddedByThem));
        let busy = self.model.read(cx).state.busy.is_some();
        let s = self.s.as_ref();
        let (n, unresolved, current, markers_left) = s.map_or((0, 0, 0, true), |s| (s.conflict_count(), s.unresolved_count(), s.current, s.markers_left));
        let tool = l.and_then(|l| l.merge_tool.clone());
        div().flex().items_center().gap_2().px_3().h(px(38.)).flex_none().bg(theme::sidebar()).border_b_1().border_color(theme::border())
            .text_size(theme::UI_SIZE)
            .child(ui::icon("triangle-alert").text_color(theme::accent()))
            .child(div().min_w_0().flex_shrink(1.).truncate().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(self.path.clone()))
            .when(kind == Some(ConflictKind::BothAdded), |d| d.child(div().flex_none().text_color(theme::text_muted()).child("added on both sides")))
            .when(text && n > 0, |d| d
                .child(tip(ui::icon_button("prev-conflict", "chevron-up").size(px(22.)), Some("Previous conflict ⌥⌘↑".into()))
                    .on_click(cx.listener(|v, _, w, cx| v.go(-1, w, cx))))
                .child(tip(ui::icon_button("next-conflict", "chevron-down").size(px(22.)), Some("Next conflict ⌥⌘↓".into()))
                    .on_click(cx.listener(|v, _, w, cx| v.go(1, w, cx))))
                .child(div().flex_none().text_color(theme::text()).child(format!("Conflict {} of {n}", (current + 1).min(n))))
                .child(if unresolved > 0 { div().flex_none().text_color(theme::text_muted()).child(format!("· {unresolved} left")).into_any_element() }
                    else { div().flex().flex_none().items_center().gap_1().text_color(theme::green()).child(ui::icon("circle-check").text_color(theme::green())).child("all resolved").into_any_element() }))
            .child(div().flex_1())
            // Only text conflicts get the whole-file buttons here: binary, symlink, submodule and one-sided conflicts
            // have their choices in the body, and `checkout -m` can't bring a delete/modify back (FLOWS §14).
            .when(text && !one_sided, |d| d
                .child(button("use-ours-file", format!("Use {}", labels.ours), Some(format!("Take the whole file from {} (git checkout --ours) and mark it resolved", labels.ours.to_lowercase())), !busy)
                    .on_click(cx.listener(|v, _, _, cx| v.resolve(Some(ConflictSide::Current), cx))))
                .child(button("use-theirs-file", format!("Use {}", labels.theirs), Some(format!("Take the whole file from {} (git checkout --theirs) and mark it resolved", labels.theirs.to_lowercase())), !busy)
                    .on_click(cx.listener(|v, _, _, cx| v.resolve(Some(ConflictSide::Incoming), cx))))
                .child(button("reset", "Reset…".into(), Some("Throw away the edits to this file and bring its conflicts back (git checkout -m)".into()), !busy)
                    .on_click(cx.listener(|v, _, _, cx| { v.confirm_reset = true; cx.notify() })))
                .child(button("merge-tool", "Merge Tool".into(), Some(tool.as_ref().map_or("Set merge.tool (or merge.guitool) in your git config to a GUI merge tool to use it here".into(), |t| format!("Open the file in {} (git mergetool)", t.0))), tool.is_some() && !busy)
                    .on_click(cx.listener(|v, _, w, cx| v.merge_tool(w, cx))))
                .child(button_primary("mark-resolved", "Mark Resolved".into(), Some(if markers_left { "Resolve every conflict first: the result still has conflict markers".into() } else { "Stage the result (git add)".into() }), !markers_left && !busy)
                    .on_click(cx.listener(|v, _, _, cx| v.resolve(None, cx)))))
            .child(tip(ui::icon_button("merge-close", "x").size(px(24.)), Some("Close (Esc)".into())).on_click(cx.listener(|v, _, _, cx| v.close(cx))))
    }

    fn chunk_bar(&self, labels: &MergeLabels, cx: &mut Context<Self>) -> AnyElement {
        let s = self.s.as_ref().expect("a text session");
        let n = s.conflict_count();
        let mut bar = div().flex().items_center().gap_1p5().px_2p5().h(px(32.)).flex_none().bg(theme::sidebar())
            .border_y_1().border_color(theme::border()).text_size(px(11.5))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child("Result"));
        if n == 0 {
            return bar.child(div().text_color(theme::text_muted()).child("· merged without conflicts; edit it freely")).child(div().flex_1()).into_any_element();
        }
        let (o, t) = (labels.ours.clone(), labels.theirs.clone());
        bar = bar.child(div().text_color(theme::text_muted()).child(format!("· Conflict {}", s.current + 1)))
            .child(tip(small(ui::pill("take-ours", format!("Use {o}")), true), Some("⌥⌘←".into())).on_click(cx.listener(|v, _, w, cx| v.take(Take::Ours, w, cx))))
            .child(tip(small(ui::pill("take-theirs", format!("Use {t}")), true), Some("⌥⌘→".into())).on_click(cx.listener(|v, _, w, cx| v.take(Take::Theirs, w, cx))))
            .child(small(ui::pill("take-ot", format!("{o}, Then {t}")), true).on_click(cx.listener(|v, _, w, cx| v.take(Take::OursThenTheirs, w, cx))))
            .child(small(ui::pill("take-to", format!("{t}, Then {o}")), true).on_click(cx.listener(|v, _, w, cx| v.take(Take::TheirsThenOurs, w, cx))))
            .child(button("take-base", "Use Base".into(), Some("The lines as they were before either side changed them".into()), s.can_use_base(s.current))
                .on_click(cx.listener(|v, _, w, cx| v.take(Take::Base, w, cx))))
            .child(button("take-markers", "Restore".into(), Some("Put the conflict markers back".into()), true)
                .on_click(cx.listener(|v, _, w, cx| v.take(Take::Markers, w, cx))));
        if let Some(h) = &self.ai {
            bar = bar.child(if self.suggesting.is_some() {
                div().flex().items_center().gap_1().px_2().text_color(theme::text_muted()).child(ui::icon("loader")).child("Suggesting…").into_any_element()
            } else {
                tip(small(ui::pill("suggest", "Suggest"), true).child(ui::icon("sparkles").size(px(12.))), Some(format!("Ask {} for a merge of this conflict; you review it before it goes in", h.harness.name)))
                    .on_click(cx.listener(|v, _, _, cx| v.suggest(cx))).into_any_element()
            });
        }
        bar.child(div().flex_1())
            .child(div().truncate().text_color(theme::text_faint()).child("Click lines above to pick them"))
            .when(s.unresolved_count() > 0, |d| d.child(small(ui::pill("next-unresolved", "Next Unresolved"), true)
                .on_click(cx.listener(|v, _, w, cx| if let Some(k) = v.s.as_ref().and_then(|s| s.next_unresolved()) { v.focus_conflict(k, w, cx) }))))
            .into_any_element()
    }

    fn proposal_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (text, _) = self.proposal.as_ref()?;
        Some(div().absolute().top(px(34.)).left(px(120.)).w(px(548.)).p_3p5().flex().flex_col().gap_2()
            .bg(theme::raised()).border_1().border_color(theme::border_strong()).rounded(theme::RADIUS_CARD).shadow_lg()
            .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(13.)).text_color(theme::text()).child("Suggested merge"))
            .child(div().id("proposal-text").w(px(520.)).h(px(260.)).overflow_scroll().p_1p5().rounded(px(6.)).bg(theme::card())
                .font_family(theme::CODE_FONT).text_size(theme::CODE_SIZE).text_color(theme::text()).whitespace_nowrap()
                .children(text.split('\n').map(|l| div().min_h(theme::CODE_LINE_HEIGHT).child(l.to_string()))))
            .child(div().text_size(px(11.5)).text_color(theme::text_muted()).child("Check it before you use it: it replaces this conflict in the result, and ⌘Z takes it back."))
            .child(div().flex().gap_2().justify_end()
                .child(small(ui::pill("proposal-discard", "Discard"), true).on_click(cx.listener(|v, _, _, cx| { v.proposal = None; cx.notify() })))
                .child(small(ui::pill_primary("proposal-use", "Use Suggestion", theme::blue()), true).on_click(cx.listener(|v, _, w, cx| v.accept_proposal(w, cx)))))
            .into_any_element())
    }

    fn editor(&self, labels: &MergeLabels, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.panes.as_ref().expect("panes exist for a text conflict");
        let s = self.s.as_ref().expect("a text session");
        let starts = self.starts.get(&Role::Result).cloned().unwrap_or_default();
        let tints = s.result_tints().into_iter().map(|(r, t)| {
            let a = session::line_at(&starts, r.start);
            let rows = if r.is_empty() { a..a } else { a..session::line_at(&starts, r.end - 1) + 1 };
            (rows, tint_color(t), r.is_empty())
        }).collect();
        div().flex().flex_col().flex_1().min_h_0()
            .child(div().flex().flex_1().min_h_0()
                .child(self.pane(Role::Ours, labels.ours.clone(), labels.ours_detail.clone(), theme::blue(), cx))
                .when(self.show_base, |d| d.child(div().w(px(1.)).h_full().bg(theme::border()))
                    .child(self.pane(Role::Base, "Base".into(), "the common ancestor".into(), theme::text_muted(), cx)))
                .child(div().w(px(1.)).h_full().bg(theme::border()))
                .child(self.pane(Role::Theirs, labels.theirs.clone(), labels.theirs_detail.clone(), theme::purple(), cx)))
            .child(self.chunk_bar(labels, cx))
            .child(div().relative().flex_1().min_h_0().overflow_hidden().bg(theme::card())
                .child(tint_layer(p.result.clone(), starts, tints))
                .child(Editor::new(&p.result).appearance(false).h_full())
                .children(self.proposal_card(cx)))
            .when(s.edited_outside, |d| d.child(div().px_3().py_1().flex_none().bg(theme::sidebar()).border_t_1().border_color(theme::border())
                .text_size(px(11.5)).text_color(theme::text_muted())
                .child("This file was changed outside Loupe, so only its remaining conflict markers are tracked.")))
    }

    /// Conflicts that aren't about lines: a side deleted the file, it's binary, it's a submodule, or only one side has it.
    fn special(&self, labels: &MergeLabels, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.loaded.as_ref().expect("loaded");
        let kind = l.kind();
        let size = |n: u8| l.sizes.get(&n).map_or("none".to_string(), |b| byte_count(*b));
        let (icon, title, detail) = match kind {
            ConflictKind::Submodule => ("package", "Submodule conflict".to_string(), "Both sides moved the submodule to a different commit. Pick the commit to keep.".to_string()),
            ConflictKind::DeletedByUs => ("trash", format!("Deleted on {}, changed on {}", labels.ours, labels.theirs), "Keep the changed file, or delete it as the other side did.".into()),
            ConflictKind::DeletedByThem => ("trash", format!("Changed on {}, deleted on {}", labels.ours, labels.theirs), "Keep the changed file, or delete it as the other side did.".into()),
            ConflictKind::AddedByUs | ConflictKind::AddedByThem => ("file-question-mark",
                format!("Only on {}", if kind == ConflictKind::AddedByUs { &labels.ours } else { &labels.theirs }),
                "The other side doesn't have this path, usually because it renamed or deleted the file. Keep it or delete it.".into()),
            _ if l.symlink => ("file-symlink", "Symbolic link conflict".into(), "The two sides point the link at different targets. Keep one side.".into()),
            _ if l.binary => ("file-image", "Binary file conflict".into(), format!("Binary files can't be merged line by line. Keep one side: {} {}, {} {}.", labels.ours, size(2), labels.theirs, size(3))),
            _ => ("file-question-mark", "Conflict".into(), "Keep one side.".into()),
        };
        let mut buttons = div().flex().gap_2().pt_1();
        match kind {
            ConflictKind::Submodule => for st in [2u8, 3] {
                if let Some((_, sha)) = l.stages.entries.get(&st).cloned() {
                    let name = if st == 2 { &labels.ours } else { &labels.theirs };
                    let sha2 = sha.clone();
                    buttons = buttons.child(tip(ui::pill(SharedString::from(format!("sub-{st}")), format!("Use {name} · {}", &sha[..sha.len().min(8)])), Some(format!("Record the submodule at {sha}")))
                        .on_click(cx.listener(move |v, _, _, cx| v.use_submodule(sha2.clone(), cx))));
                }
            },
            ConflictKind::DeletedByUs | ConflictKind::DeletedByThem | ConflictKind::AddedByUs | ConflictKind::AddedByThem => {
                let side = if matches!(kind, ConflictKind::DeletedByThem | ConflictKind::AddedByUs) { ConflictSide::Current } else { ConflictSide::Incoming };
                buttons = buttons
                    .child(ui::pill_primary("keep-file", "Keep File", theme::blue()).on_click(cx.listener(move |v, _, _, cx| v.resolve(Some(side), cx))))
                    .child(ui::pill_primary("delete-file", "Delete File", theme::red()).on_click(cx.listener(|v, _, _, cx| v.delete_file(cx))));
            }
            _ => {
                buttons = buttons
                    .child(ui::pill("use-ours", format!("Use {}", labels.ours)).on_click(cx.listener(|v, _, _, cx| v.resolve(Some(ConflictSide::Current), cx))))
                    .child(ui::pill("use-theirs", format!("Use {}", labels.theirs)).on_click(cx.listener(|v, _, _, cx| v.resolve(Some(ConflictSide::Incoming), cx))));
            }
        }
        let survivor = self.survivor.clone().filter(|_| kind != ConflictKind::Submodule && !l.binary);
        div().flex().flex_col().flex_1().min_h_0()
            .child(div().flex().flex_col().items_center().gap_2p5().p_6().flex_none()
                .child(ui::icon(icon).size(px(28.)).text_color(theme::accent()))
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title))
                .child(div().max_w(px(460.)).text_center().text_color(theme::text_muted()).child(detail))
                .child(buttons))
            .child(match survivor {
                Some(e) => div().flex_1().min_h_0().border_t_1().border_color(theme::border()).bg(theme::card()).child(Editor::new(&e).appearance(false).h_full()).into_any_element(),
                None => div().flex_1().into_any_element(),
            })
    }

    fn reset_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path).to_string();
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .child(ui::card().w(px(380.)).p_4().gap_3().bg(theme::raised())
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(format!("Bring back the conflicts in {name}?")))
                .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Your edits to the result are lost."))
                .child(div().flex().justify_end().gap_2()
                    .child(ui::pill("reset-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| { v.confirm_reset = false; cx.notify() })))
                    .child(ui::pill_primary("reset-confirm", "Reset File", theme::red()).on_click(cx.listener(|v, _, w, cx| v.reset(w, cx))))))
    }
}

/// ByteCountFormatter's .file style: decimal units.
fn byte_count(b: usize) -> String {
    match b {
        0..1000 => format!("{b} bytes"),
        1000..1_000_000 => format!("{:.0} KB", b as f64 / 1e3),
        _ => format!("{:.1} MB", b as f64 / 1e6),
    }
}

impl Render for MergeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The panes are laid out now: scroll them to the first conflict (MergeEditor's `.task { focus }`).
        if self.reveal && self.panes.as_ref().is_some_and(|p| p.result.read(cx).line_height().is_some()) {
            self.reveal = false;
            let k = self.s.as_ref().map_or(0, |s| s.current);
            cx.defer_in(window, move |v, w, cx| v.focus_conflict(k, w, cx));
            if let Some(steps) = std::env::var(TEST_STEPS).ok().filter(|_| !self.stepped) {
                self.stepped = true;
                cx.spawn_in(window, async move |this, cx| {
                    for st in steps.split(',') {
                        cx.background_executor().timer(Duration::from_millis(700)).await;
                        if this.update_in(cx, |v, w, cx| v.step(st, w, cx)).is_err() { break }
                    }
                }).detach();
            }
        }
        let labels = self.labels(cx);
        let body = match &self.loaded {
            None => div().flex_1().child(ui::empty_state("loader", "Loading…", "")).into_any_element(),
            Some(l) if l.is_text() && self.s.is_some() => self.editor(&labels, cx).into_any_element(),
            Some(_) => self.special(&labels, cx).into_any_element(),
        };
        let busy = self.model.read(cx).state.busy.clone();
        // Below the title bar (42 pt), which stays usable; the editor covers the sidebar and Changes.
        div().id("merge-editor").track_focus(&self.focus).key_context("MergeEditor").absolute().top(px(42.)).left_0().right_0().bottom_0()
            .border_t_1().border_color(theme::border()).flex().flex_col().bg(theme::canvas())
            .font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .on_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    if v.proposal.take().is_some() || std::mem::take(&mut v.confirm_reset) { cx.notify() } else { v.close(cx) }
                }
            }))
            .on_action(cx.listener(|v, _: &NextConflict, w, cx| v.go(1, w, cx)))
            .on_action(cx.listener(|v, _: &PreviousConflict, w, cx| v.go(-1, w, cx)))
            .on_action(cx.listener(|v, _: &UseOurs, w, cx| v.take(Take::Ours, w, cx)))
            .on_action(cx.listener(|v, _: &UseTheirs, w, cx| v.take(Take::Theirs, w, cx)))
            .children(banner(&self.model, cx))
            .child(self.header(&labels, cx))
            .child(body)
            .when(self.confirm_reset, |d| d.child(self.reset_dialog(cx)))
            .when_some(busy, |d, b| d.child(div().absolute().bottom_3().right_4().px_3().py_1().rounded(theme::RADIUS_PILL)
                .bg(theme::raised()).border_1().border_color(theme::border()).text_size(px(12.)).text_color(theme::text_muted()).child(format!("{b}…"))))
    }
}
