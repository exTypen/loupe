//! The PR's Overview tab (PROverview in PullRequestsView.swift): fetch the head, find the merge base and the base
//! branch's tip, then the code view's ChangeOverview over merge-base..head. A fact that points at a file opens it in
//! Files; a branch-row commit opens it in History.
use std::rc::Rc;

use gpui_kit::{AnyElement, AppContext as _, Context, Entity, IntoElement, Task};
use loupe_code::lens::ChangedPath;

use super::detail::PullDetail;
use crate::state::AppEvent;
use crate::ui;
use crate::views::code::{ChangeOverview, overview::{OnOpen, OverviewInput, pull_input}};

#[derive(Default)]
pub struct State {
    key: Option<String>,
    view: Option<Entity<ChangeOverview>>,
    error: Option<String>,
    task: Option<Task<()>>,
}

impl State {
    /// Leaving the PR stops a run; the next showing starts it again.
    pub fn cancel(&mut self) { if self.task.take().is_some() { self.key = None } }
}

fn start(d: &mut PullDetail, cx: &mut Context<PullDetail>) {
    let key = format!("{} {} {}", d.pr.head.sha, d.pr.base.sha, d.files().len());
    if d.overview.key.as_deref() == Some(&key) || d.files().is_empty() { return }
    d.overview.key = Some(key);
    let (pr, local) = (d.pr.clone(), d.local.clone());
    let changes: Vec<ChangedPath> = d.files().iter().map(|f| ChangedPath { old: f.old_path.clone(), new: f.new_path.clone() }).collect();
    d.overview.task = Some(cx.spawn(async move |this, cx| {
        let r = cx.background_executor().spawn(async move {
            super::detail::fetch_head(&pr, &local)?;
            let dir = &local.dir;
            // The base branch as last fetched, which may be further on than the PR's own record of it.
            let tracking = format!("refs/remotes/{}/{}", local.remote, pr.base.ref_);
            let known = loupe_forge::git::string(&["rev-parse", "--verify", "-q", &format!("{tracking}^{{commit}}")], dir, None).is_ok_and(|s| !s.trim().is_empty());
            let target = if known { tracking } else { pr.base.sha.clone() };
            let base = loupe_forge::git::string(&["merge-base", &pr.base.sha, &pr.head.sha], dir, None)?.trim().to_string();
            Ok::<_, loupe_forge::Error>(pull_input(dir, &base, &pr.head.sha, changes, &target, &pr.base.ref_))
        }).await;
        this.update(cx, |d, cx| {
            d.overview.task = None;
            match r {
                Ok(input) => { show(d, input, cx); d.overview.error = None }
                Err(e) => d.overview.error = e.user_message().or(d.overview.error.take()),
            }
            cx.notify();
        }).ok();
    }));
}

fn show(d: &mut PullDetail, input: OverviewInput, cx: &mut Context<PullDetail>) {
    if let Some(v) = &d.overview.view { return v.update(cx, |v, cx| v.set_input(input, cx)) }
    let me = cx.entity().downgrade();
    let on_open: OnOpen = Rc::new(move |p, line, w, cx| { me.update(cx, |d, cx| d.reveal(&p, line, w, cx)).ok(); });
    let (app, worktree) = (d.app.clone(), d.worktree.clone());
    d.overview.view = Some(cx.new(|cx| {
        let mut v = ChangeOverview::new(input, on_open, cx);
        // §14 #14: the branch row's commits open in History.
        v.on_commit(move |sha, _, cx| app.update(cx, |a, cx| a.emit(AppEvent::ShowCommit { worktree: worktree.clone(), sha }, cx)));
        v
    }));
}

pub fn render(d: &mut PullDetail, cx: &mut Context<PullDetail>) -> AnyElement {
    start(d, cx);
    match (&d.overview.view, &d.overview.error) {
        (Some(v), _) => v.clone().into_any_element(),
        (None, Some(e)) => ui::empty_state("triangle-alert", "Can't Read the Pull Request", e.clone()).into_any_element(),
        (None, None) => ui::empty_state("loader", "Fetching the pull request…", "").into_any_element(),
    }
}
