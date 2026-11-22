//! The History tab (History.swift, InteractiveRebase.swift, Undo.swift's history side). Owner: the History view
//! agent (docs/gpui/VIEWS.md): log + graph, scopes, search, commit detail, commit actions, interactive rebase.
//!
//! Entry point: `HistoryView::new` (the shell makes one and shows it while `AppModel::tab == Tab::History`).
//! Follow `app.selected_model()`; reload only when its `state.head_oid` or `state.refs_version` moved (or on
//! `AppEvent::Refresh`). Serve `AppEvent::ShowCommit` / `AppEvent::ShowBranch`.
use gpui_kit::{Context, Entity, IntoElement, Render, Subscription, Window};

use crate::state::{AppEvent, AppModel};

pub struct HistoryView {
    pub app: Entity<AppModel>,
    _subs: Vec<Subscription>,
}

impl HistoryView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app, window, |_, _, e: &AppEvent, _, _| match e {
                AppEvent::ShowCommit { .. } | AppEvent::ShowBranch { .. } | AppEvent::Refresh => {}
                _ => {}
            }),
        ];
        Self { app, _subs: subs }
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        super::stub("history", "History", "The log of this worktree's branch shows here.")
    }
}
