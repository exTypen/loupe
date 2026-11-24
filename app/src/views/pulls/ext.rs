//! What the PR views need from the app model beyond its shared API (state/ is frozen, so these live here).
use std::path::Path;

use gpui_kit::Context;

use crate::state::{AppEvent, AppModel};

impl AppModel {
    /// PD-28 Check Out: selects this repository's worktree already on `pr/N`, or checks the PR out in a sibling
    /// worktree "<repo>-pr-N" and selects it. Only this repository's worktrees count: the Swift app searched every
    /// repository, so PR #5 of one repo could select another repo's `pr/5` worktree (FLOWS.md §14).
    pub fn checkout_pull_in_repo(&mut self, worktree: &str, number: u64, cx: &mut Context<Self>) {
        let repo = self.repo_containing(worktree).unwrap_or(worktree).to_string();
        let branch = format!("pr/{number}");
        if let Some(w) = self.worktrees_of(&repo).iter().find(|w| w.branch.as_deref() == Some(&branch)).map(|w| w.path.clone()) {
            return self.select(Some(w), cx);
        }
        let Some(wm) = self.model(worktree) else { return };
        let (remote, env) = { let w = wm.read(cx); (w.state.remote_name.clone(), self.git_auth(w.remote.as_ref(), worktree)) };
        wm.update(cx, |w, cx| {
            let r = repo.clone();
            let t = w.perform(&format!("Checking out PR #{number}"), move |_| loupe_core::model::checkout_pr_worktree(Path::new(&r), &remote, number, &env), cx);
            cx.spawn(async move |w, cx| {
                let Some(dest) = t.await else { return };
                let Some(app) = w.update(cx, |w, _| w.app.clone()).ok() else { return };
                let Ok(t) = app.update(cx, |a, cx| a.refresh_worktrees(repo, cx)) else { return };
                t.await;
                app.update(cx, |a, cx| if Path::new(&dest).exists() { a.select(Some(dest), cx) }).ok();
            }).detach();
        });
    }

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
