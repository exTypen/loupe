//! The New Pull Request sheet and the "Create a pull request" banner data (PullCreate.swift). Owner: the PR create
//! view agent (docs/gpui/VIEWS.md). The sidebar's "Create Pull Request…" and the Changes banner emit
//! `AppEvent::CreatePull`.
//!
//! Entry point: `install` subscribes to `AppEvent::CreatePull` and presents `PullCreateSheet`. On success: dismiss,
//! then `app.emit(AppEvent::OpenPull { .. })`.
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::state::{AppEvent, AppModel};

pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Subscription {
    window.subscribe(app, cx, |app, e: &AppEvent, _, cx| {
        let AppEvent::CreatePull { worktree } = e else { return };
        let v = cx.new(|_| PullCreateSheet { app: app.clone(), worktree: worktree.clone() });
        app.update(cx, |a, cx| a.present(v.into(), cx));
    })
}

pub struct PullCreateSheet {
    pub app: Entity<AppModel>,
    pub worktree: String,
}

impl Render for PullCreateSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::overlay_stub(&self.app, "git-pull-request", "New Pull Request", &self.worktree, cx)
    }
}
