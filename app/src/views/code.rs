//! The code browser (CodeBrowser.swift, CodeIndex.swift, Lenses.swift, ChangeOverview.swift): Full mode's worktree
//! tree and file pane, go to definition / references, outline, tabs, the change overview and its lenses. Owner: the
//! Code view agent (docs/gpui/VIEWS.md).
//!
//! Entry points: `CodeBrowser::new(app, model, window, cx)`, embedded by Changes (Full mode, Overview) and by
//! Pulls / History for a commit or PR (they pass the revision). It uses ui::tabs (tab bar, nav controls, results
//! panel), diff::DiffView (on_navigate) and loupe_code (index, lens, overview).
use gpui_kit::{Context, Entity, IntoElement, Render, Window};

use crate::state::{AppModel, WorktreeModel};

pub struct CodeBrowser {
    pub app: Entity<AppModel>,
    pub model: Entity<WorktreeModel>,
}

impl CodeBrowser {
    pub fn new(app: Entity<AppModel>, model: Entity<WorktreeModel>, _: &mut Window, _: &mut Context<Self>) -> Self { Self { app, model } }
}

impl Render for CodeBrowser {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        super::stub("code", "Code", "The worktree as an IDE: files, outline, definitions.")
    }
}
