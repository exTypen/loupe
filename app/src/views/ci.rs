//! CI (CI.swift, CIForges.swift): the checks overlay, job detail and log tail. Owner: the CI view agent
//! (docs/gpui/VIEWS.md).
//!
//! Entry point: `install` (called once per window by the shell) subscribes to `AppEvent::ShowChecks` and presents
//! `ChecksOverlay` with `AppModel::present`; it closes itself with `AppModel::dismiss`. The PR header's checks badge
//! (Pulls agent) emits the event.
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::state::{AppEvent, AppModel};

pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Subscription {
    window.subscribe(app, cx, |app, e: &AppEvent, _, cx| {
        let AppEvent::ShowChecks { worktree, sha, pull } = e else { return };
        let v = cx.new(|_| ChecksOverlay { app: app.clone(), worktree: worktree.clone(), sha: sha.clone(), pull: *pull });
        app.update(cx, |a, cx| a.present(v.into(), cx));
    })
}

pub struct ChecksOverlay {
    pub app: Entity<AppModel>,
    pub worktree: String,
    pub sha: String,
    pub pull: Option<u64>,
}

impl Render for ChecksOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::overlay_stub(&self.app, "circle-check", "Checks", &format!("Checks for {}", &self.sha[..self.sha.len().min(7)]), cx)
    }
}
