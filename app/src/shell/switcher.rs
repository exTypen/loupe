//! ⌘K: go to any worktree in the current space by typing parts of its repo, branch or folder (QuickSwitcher.swift).
use gpui_kit::{
    AnyElement, AppContext as _, Context, DismissEvent, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window, div, uniform_list,
    component::input::{Input, InputEvent, InputState},
};

use super::model::{SwitcherItem, last_component, switcher_items, switcher_matches};
use crate::state::AppModel;
use crate::ui;

pub struct Switcher {
    workspace: Entity<AppModel>,
    input: Entity<InputState>,
    index: usize,
    scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for Switcher {}

impl Switcher {
    pub fn new(workspace: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to worktree…"));
        input.update(cx, |i, cx| i.focus(window, cx));
        let subs = vec![cx.subscribe(&input, |this, _, e: &InputEvent, cx| match e {
            InputEvent::Change => { this.index = 0; cx.notify() }
            InputEvent::PressEnter { .. } => this.open_selected(cx),
            _ => {}
        })];
        let mut s = Self { workspace, input, index: 0, scroll: UniformListScrollHandle::new(), _subs: subs };
        // An empty query lists the current worktree first; preselect the previous one so ⌘K ↩ toggles back.
        let items = s.items(cx);
        let ws = s.workspace.read(cx);
        if items.len() > 1 && ws.selection() == Some(items[0].wt.path.as_str()) { s.index = 1 }
        s
    }

    fn items(&self, cx: &gpui_kit::App) -> Vec<SwitcherItem> {
        let ws = self.workspace.read(cx);
        switcher_matches(&switcher_items(ws), &self.input.read(cx).value(), &ws.recent)
    }

    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.items(cx).len();
        if n == 0 { return }
        self.index = (self.index as isize + d).clamp(0, n as isize - 1) as usize;
        self.scroll.scroll_to_item(self.index, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.items(cx).into_iter().nth(self.index) { self.open(item, cx) }
    }

    fn open(&mut self, item: SwitcherItem, cx: &mut Context<Self>) {
        self.workspace.update(cx, |w, cx| { w.collapsed.remove(&item.repo); w.select(Some(item.wt.path), cx) });
        cx.emit(DismissEvent);
    }
}

impl Render for Switcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items(cx);
        let statuses: Vec<_> = { let ws = self.workspace.read(cx); items.iter().map(|i| ws.status(&i.wt.path)).collect() };
        let index = self.index;
        let me = cx.entity();
        let list = uniform_list("switcher", items.len(), move |range, _, _| {
            range.map(|i| {
                let item = items[i].clone();
                let me = me.clone();
                div().w_full().child(
                    super::worktree_row_body(SharedString::from(format!("sw-{i}")), &item.repo, &item.wt, statuses[i], i == index,
                        Some(last_component(&item.repo).to_string()))
                        .w_full()
                        .on_click(move |_, _, cx| me.update(cx, |s, cx| s.open(item.clone(), cx))))
                    .into_any_element()
            }).collect::<Vec<AnyElement>>()
        }).size_full().track_scroll(&self.scroll);
        let me = cx.entity();
        div().absolute().inset_0()
            .capture_key_down(cx.listener(|s, e: &gpui_kit::KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "up" => s.step(-1, cx),
                    "down" => s.step(1, cx),
                    "escape" => cx.emit(DismissEvent),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(ui::palette::palette(
                Input::new(&self.input).appearance(false),
                list,
                Some(div().flex().gap_3().child("↑↓ to move").child("↩ to open").child("esc to close").into_any_element()),
                move |_, cx| me.update(cx, |_, cx| cx.emit(DismissEvent)),
            ))
    }
}
