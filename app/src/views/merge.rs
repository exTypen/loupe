//! The merge editor and conflicts (MergeEditor.swift, Conflicts.swift, Merge3.swift's UI). Owner: the Merge view
//! agent (docs/gpui/VIEWS.md). The operation banner and conflict rows in Changes emit `AppEvent::OpenMergeEditor`.
//!
//! Entry point: `install` subscribes to `AppEvent::OpenMergeEditor` and presents `MergeEditor` over the window;
//! resolutions go through `WorktreeModel::resolve` / `finish_operation` (or core's conflicts::write_result + a
//! `perform`). Byte ranges, not UTF-16 (core's ConflictText::blocks, Region.range).
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::state::{AppEvent, AppModel};

pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Subscription {
    window.subscribe(app, cx, |app, e: &AppEvent, _, cx| {
        let AppEvent::OpenMergeEditor { worktree, path } = e else { return };
        let v = cx.new(|_| MergeEditor { app: app.clone(), worktree: worktree.clone(), path: path.clone() });
        app.update(cx, |a, cx| a.present(v.into(), cx));
    })
}

pub struct MergeEditor {
    pub app: Entity<AppModel>,
    pub worktree: String,
    pub path: String,
}

impl Render for MergeEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::overlay_stub(&self.app, "git-merge", "Merge Editor", &self.path, cx)
    }
}
