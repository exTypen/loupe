//! What the PR views need from the app model beyond its shared API (state/ is frozen, so these live here).
use gpui_kit::Context;

use crate::state::{AppEvent, AppModel};

impl AppModel {
    /// PD-30: after a merge on the server, fetch the branch it went into (busy "Fetching <base>"; a failure shows
    /// the Git alert) and open that branch's History with its newest commit, the merge, selected.
    pub fn show_merged(&mut self, worktree: &str, base: &str, cx: &mut Context<Self>) {
        let Some(wm) = self.model(worktree) else { return };
        let remote = wm.read(cx).state.remote_name.clone();
        let t = wm.update(cx, |w, cx| w.remote_op(&format!("Fetching {base}"), vec!["fetch".into(), remote.clone(), base.into()], cx));
        let (worktree, branch) = (worktree.to_string(), format!("{remote}/{base}"));
        cx.spawn(async move |this, cx| {
            t.await;
            this.update(cx, |a, cx| a.emit(AppEvent::ShowBranch { worktree, branch }, cx)).ok();
        }).detach();
    }
}
