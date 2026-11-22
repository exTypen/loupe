//! The Pull Requests tab (PullRequestsView.swift, PullModel.swift, Conversation.swift). Owner: the Pulls view agent
//! (docs/gpui/VIEWS.md): PR list, PR detail (files, viewed, Full mode), conversation, review, checkout, merge.
//!
//! Entry point: `PullsView::new` (shown while `AppModel::tab == Tab::Pulls`). The forge client for the selected
//! worktree: `app.read(cx).client_for(remote, &path)` with `remote = model.read(cx).remote`. Serve
//! `AppEvent::OpenPull` and `AppEvent::Refresh`. After a merge: `app.emit(AppEvent::ShowBranch { .. })`.
use gpui_kit::{Context, Entity, IntoElement, Render, Subscription, Window};

use crate::state::{AppEvent, AppModel};

pub struct PullsView {
    pub app: Entity<AppModel>,
    _subs: Vec<Subscription>,
}

impl PullsView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&app, window, |_, _, e: &AppEvent, _, _| if let AppEvent::OpenPull { .. } | AppEvent::Refresh = e {}),
        ];
        Self { app, _subs: subs }
    }
}

impl Render for PullsView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        super::stub("git-pull-request", "Pull Requests", "This repository's pull requests show here.")
    }
}
