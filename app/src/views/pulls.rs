//! The Pull Requests tab (PullRequestsView.swift, PullModel.swift, Conversation.swift). Owner: the Pulls view agent
//! (docs/gpui/VIEWS.md): PR list, PR detail (files, viewed, Full mode), conversation, review, checkout, merge.
//!
//! Entry point: `PullsView::new` (shown while `AppModel::tab == Tab::Pulls`). Serves `AppEvent::OpenPull` and
//! `AppEvent::Refresh`; emits `ShowChecks` (the PR header's checks badge), `ShowBranch` after a merge and `CreatePull`.
//!
//! Loading rules (the Swift app's, see loupe_forge::pull): the list loads on showing when its state changed or it's
//! older than 30 s; the open list then polls every 60 s, on activation and on ⌘R. A selected PR replaces the list
//! and stops its poll; going back reloads it at once. Nothing polls while the tab or the worktree isn't showing it,
//! and leaving cancels what runs. A cancelled request is never an error.
pub mod avatar;
mod conversation;
mod detail;
mod ext;
mod markdown;
mod overview;
mod time;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px, uniform_list,
    component::{input::{Input, InputEvent, InputState}, tooltip::Tooltip},
};
use loupe_forge::client::PULL_PAGE;
use loupe_forge::pull::{DetailCache, ListLoad, LocalCheckout, PullList};
use loupe_forge::{Cancel, Client, PullRequest, RemoteRepo};

use crate::shell::keys::OpenSettings;
use crate::state::{AppEvent, AppModel, DiffControls, Tab, WorktreeModel};
use crate::{theme, ui};
use detail::{DetailEvent, PullDetail};

/// What the list needs about the selected worktree, once its remote and account are known.
#[derive(Clone)]
struct Ctx { path: String, remote: RemoteRepo, client: Client, local: LocalCheckout, me: Option<String>, branch: Option<String> }

struct Load { path: String, state: String, cancel: Cancel, _task: Task<()> }

pub struct PullsView {
    app: Entity<AppModel>,
    focus: FocusHandle,
    model: Option<(Entity<WorktreeModel>, Subscription)>,
    /// Per worktree, as the Swift app kept one PullListModel per worktree model: switching keeps each list.
    lists: HashMap<String, PullList>,
    /// The opened PRs of the shown worktree; a worktree switch lets go of them (Model.swift's releaseHeavyState).
    details: DetailCache<(Entity<PullDetail>, Subscription)>,
    details_of: Option<String>,
    mine: bool,
    filter: Entity<InputState>,
    load: Option<Load>,
    again: bool,
    more: Option<(Cancel, Task<()>)>,
    poll: Option<Task<()>>,
    clock: Option<Task<()>>,
    shown: Option<String>,
    _subs: Vec<Subscription>,
}

/// The list's pages: the open list whole, closed / all just page 1. A server whose page is smaller than ours
/// (Forgejo MAX_RESPONSE_ITEMS under 50) still gives the whole open list: the Swift app stopped after its first,
/// short page (FLOWS.md §14).
fn fetch_list(client: &Client, repo: &RemoteRepo, state: &str) -> ListLoad {
    let result = (|| {
        let mut all = client.pulls(repo, state, 1)?;
        let mut p = 1;
        if state == "open" {
            let mut size = PULL_PAGE;
            let mut last = all.len();
            while last > 0 && (last == size || (p == 1 && last < size)) {
                let next = client.pulls(repo, state, p + 1)?;
                if p == 1 && last < size { size = last }
                let seen: HashSet<i64> = all.iter().map(|x| x.id).collect();
                let fresh: Vec<PullRequest> = next.into_iter().filter(|x| !seen.contains(&x.id)).collect();
                if fresh.is_empty() { break }
                last = fresh.len();
                all.extend(fresh);
                p += 1;
            }
        }
        Ok((all, p))
    })();
    ListLoad { state: state.into(), result }
}

impl PullsView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter pull requests"));
        let subs = vec![
            cx.observe_in(&app, window, |v, _, w, cx| v.sync(w, cx)),
            cx.subscribe(&filter, |_, _, e: &InputEvent, cx| if matches!(e, InputEvent::Change) { cx.notify() }),
            cx.subscribe_in(&app, window, |v, _, e: &AppEvent, w, cx| match e {
                AppEvent::OpenPull { worktree, number } => v.open_pull(worktree, *number as i64, w, cx),
                AppEvent::Refresh => v.refresh(cx),
                _ => {}
            }),
            cx.observe_window_activation(window, |v, w, cx| if w.is_window_active() { v.refresh(cx) }),
        ];
        let mine = loupe_forge::settings::shared().bool("pullsMine");
        let mut v = Self {
            app, focus: cx.focus_handle(), model: None, lists: HashMap::new(), details: DetailCache::default(), details_of: None,
            mine, filter, load: None, again: false, more: None, poll: None, clock: None, shown: None, _subs: subs,
        };
        v.sync(window, cx);
        v
    }

    fn ctx(&self, cx: &gpui_kit::App) -> Option<Ctx> {
        let m = self.model.as_ref()?.0.read(cx);
        let remote = m.remote.clone()?;
        let path = m.path_str();
        let a = self.app.read(cx);
        let client = a.client_for(&remote, &path)?;
        let me = a.account_for(&remote, &path).map(|a| a.username.clone()).filter(|u| !u.is_empty());
        let local = LocalCheckout { dir: path.clone(), remote: m.state.remote_name.clone(), git_auth: a.git_auth(Some(&remote), &path) };
        Some(Ctx { path, remote, client, local, me, branch: m.state.branch.clone() })
    }

    fn list(&mut self, path: &str) -> &mut PullList { self.lists.entry(path.to_string()).or_default() }

    /// The shown worktree's list length (the stress driver waits for the whole open list).
    pub fn list_count(&self) -> usize { self.shown.as_ref().and_then(|p| self.lists.get(p)).map_or(0, |l| l.pulls.len()) }

    fn selected_detail(&self) -> Option<Entity<PullDetail>> {
        let n = self.lists.get(self.shown.as_ref()?)?.selected?;
        self.details.details.get(&n).map(|(d, _)| d.clone())
    }

    /// Follows the selection, the tab and the account: starts what a newly shown list needs, stops what nothing
    /// shows any more.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.app.read(cx).selected_model();
        if m.as_ref().map(|m| m.entity_id()) != self.model.as_ref().map(|x| x.0.entity_id()) {
            self.model = m.map(|m| (m.clone(), cx.observe_in(&m, window, |v, _, w, cx| v.sync(w, cx))));
        }
        let path = self.model.as_ref().map(|m| m.0.read(cx).path_str());
        // Another worktree: its opened PRs go (they hold big diffs); the lists stay.
        if self.details_of.is_some() && path != self.details_of {
            self.hide(cx);
            self.shown = None;
            for (d, _) in self.details.details.values() { d.update(cx, |d, cx| d.hide(cx)) }
            self.details = DetailCache::default();
            self.details_of = None;
        }
        let ctx = self.ctx(cx).filter(|_| self.app.read(cx).tab == Tab::Pulls);
        let now = ctx.as_ref().map(|c| c.path.clone());
        if now != self.shown {
            self.hide(cx);
            self.shown = now;
            if let Some(c) = &ctx { self.show(c, window, cx) }
        }
        cx.notify();
    }

    fn show(&mut self, c: &Ctx, window: &mut Window, cx: &mut Context<Self>) {
        self.details_of = Some(c.path.clone());
        let list = self.list(&c.path);
        list.restore_cache(&c.remote); // instant list from the last session; the fetch replaces it
        let load = list.should_load_on_show();
        if let Some(d) = self.selected_detail() {
            d.update(cx, |d, cx| { d.show(cx); d.focus(window, cx) });
            return;
        }
        // The list, not a PR: no PR controls in the toolbar.
        self.app.update(cx, |a, cx| a.set_controls(Tab::Pulls, DiffControls::default(), cx));
        if load { self.load(cx) }
        self.start_poll(cx);
        self.tick(cx);
        self.focus.focus(window, cx);
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        self.clock = None;
        if let Some(l) = self.load.take() { l.cancel.cancel() }
        if let Some((c, _)) = self.more.take() { c.cancel() }
        if let Some(d) = self.selected_detail() { d.update(cx, |d, cx| d.hide(cx)) }
    }

    /// Only the open list auto-refreshes: closed / all are paged and refreshing would throw away scrolled pages.
    fn start_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(loupe_forge::pull::POLL).await;
            let alive = this.update(cx, |v, cx| {
                let Some(p) = v.shown.clone() else { return };
                if v.list(&p).state == "open" { v.load(cx) }
            });
            if alive.is_err() { return }
        }));
    }

    /// ⌘R and app activation: the open PR, else the list.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.shown.is_none() { return }
        match self.selected_detail() {
            Some(d) => d.update(cx, |d, cx| d.load(cx)),
            None => self.load(cx),
        }
    }

    /// One load per state at a time; asked again meanwhile, it runs once more after. A load of a state no longer
    /// shown is stopped.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.ctx(cx) else { return };
        let list = self.list(&c.path);
        if list.selected.is_some() { return }
        let state = list.state.clone();
        if let Some(l) = &self.load {
            if l.path == c.path && l.state == state { self.again = true; return }
            l.cancel.cancel();
        }
        self.again = false;
        let cancel = Cancel::new();
        let client = c.client.with_cancel(&cancel);
        let (repo, st, path) = (c.remote.clone(), state.clone(), c.path.clone());
        let task = cx.spawn(async move |this, cx| {
            let r2 = repo.clone();
            let r = cx.background_executor().spawn(async move { fetch_list(&client, &r2, &st) }).await;
            this.update(cx, |v, cx| {
                v.load = None;
                let list = v.list(&path);
                list.apply(r, &repo);
                if std::mem::take(&mut v.again) && v.shown.as_deref() == Some(path.as_str()) { v.load(cx) }
                if v.shown.is_some() { v.tick(cx) }
                cx.notify();
            }).ok();
        });
        self.load = Some(Load { path: c.path, state, cancel, _task: task });
        cx.notify();
    }

    /// The next page of closed / all, on scrolling to the end of what's loaded (whichever section the last row is
    /// in: the Swift app only asked from the "All" section, FLOWS.md §14).
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.ctx(cx) else { return };
        if self.load.is_some() || self.more.is_some() { return }
        let Some((state, page)) = self.list(&c.path).more() else { return };
        let cancel = Cancel::new();
        let client = c.client.with_cancel(&cancel);
        let (repo, path) = (c.remote.clone(), c.path.clone());
        let task = cx.spawn(async move |this, cx| {
            let st = state.clone();
            let r = cx.background_executor().spawn(async move { client.pulls(&repo, &st, page) }).await;
            this.update(cx, |v, cx| { v.more = None; v.list(&path).apply_more(&state, page, r); cx.notify() }).ok();
        });
        self.more = Some((cancel, task));
        cx.notify();
    }

    /// "Updated 2 minutes ago" changes once a minute: re-render only then, not on a 5 s timer.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() { return }
        }));
    }

    fn set_state(&mut self, s: &str, cx: &mut Context<Self>) {
        let Some(p) = self.shown.clone() else { return };
        let list = self.list(&p);
        if list.state == s { return }
        list.state = s.into();
        // The old state's PRs go: under the new picker value they'd read as its result, and a failed load would
        // leave them there with the error hidden (FLOWS.md §14).
        list.pulls.clear();
        list.error = None;
        if let Some((c, _)) = self.more.take() { c.cancel() }
        self.load(cx);
    }

    fn set_mine(&mut self, on: bool, cx: &mut Context<Self>) {
        self.mine = on;
        loupe_forge::settings::shared().set_bool("pullsMine", on);
        cx.notify();
    }

    // MARK: Opening a PR

    fn detail(&mut self, c: &Ctx, pr: PullRequest, window: &mut Window, cx: &mut Context<Self>) -> Entity<PullDetail> {
        let n = pr.number;
        let selected = self.list(&c.path).selected;
        if let Some((d, _)) = self.details.details.get(&n) { let d = d.clone(); self.details.get_or_insert_with(n, selected, || unreachable!()); return d }
        let d = cx.new(|cx| PullDetail::new(self.app.clone(), c.path.clone(), pr, c.client.clone(), c.remote.clone(), c.local.clone(), window, cx));
        let sub = cx.subscribe_in(&d, window, |v, d, e: &DetailEvent, w, cx| match e {
            DetailEvent::Back => v.back(w, cx),
            DetailEvent::Merged { base } => {
                // Done: no more polling a merged PR. Then the branch it went into, the merge on top.
                let (wt, base) = (d.read(cx).worktree.clone(), base.clone());
                v.back(w, cx);
                v.app.update(cx, |a, cx| a.show_merged(&wt, &base, cx));
            }
        });
        self.details.get_or_insert_with(n, selected, || (d.clone(), sub));
        d
    }

    fn open(&mut self, pr: PullRequest, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.ctx(cx) else { return };
        let n = pr.number;
        if let Some(l) = self.load.take() { l.cancel.cancel() }
        if let Some((m, _)) = self.more.take() { m.cancel() }
        self.poll = None;
        self.list(&c.path).selected = Some(n);
        let d = self.detail(&c, pr, window, cx);
        // The list's filter field had the keys; the detail takes them (⌘[, ⇧⌘V, ⌥↓).
        d.update(cx, |d, cx| { d.show(cx); d.focus(window, cx) });
        cx.notify();
    }

    /// From PR create (and anything else that emits OpenPull): the PR may not be in the list yet.
    fn open_pull(&mut self, worktree: &str, n: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
        let Some(c) = self.ctx(cx).filter(|c| c.path == worktree) else { return };
        if self.list(&c.path).selected == Some(n) { return }
        if let Some(d) = self.selected_detail() { d.update(cx, |d, cx| d.hide(cx)) }
        let pr = self.list(&c.path).pulls.iter().find(|p| p.number == n).cloned()
            .or_else(|| self.details.details.get(&n).map(|(d, _)| d.read(cx).pr.clone()))
            .unwrap_or(PullRequest { number: n, state: "open".into(), title: format!("#{n}"), ..Default::default() });
        self.open(pr, window, cx);
    }

    /// Back to the list: it reloads at once (not subject to the 30 s rule) and polls again.
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.shown.clone() else { return };
        if let Some(d) = self.selected_detail() { d.update(cx, |d, cx| d.hide(cx)) }
        self.list(&p).selected = None;
        self.details.trim(None);
        self.app.update(cx, |a, cx| a.set_controls(Tab::Pulls, DiffControls::default(), cx));
        self.load(cx);
        self.start_poll(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    // MARK: Rendering

    fn row(&self, pr: &PullRequest, local: bool, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity_id();
        let (icon, color) = if pr.merged == Some(true) { ("git-merge", theme::purple()) } else if pr.state == "closed" { ("circle-x", theme::red()) } else { ("git-pull-request", theme::green()) };
        let refs: SharedString = format!("{} → {}", pr.head.ref_, pr.base.ref_).into();
        let (r1, r2) = (refs.clone(), refs);
        let p = pr.clone();
        div().id(SharedString::from(format!("pr-{}", pr.number))).w_full().flex().items_start().gap_2p5().h(px(52.)).px_2p5().py_1p5()
            .rounded(theme::RADIUS_ROW).cursor_pointer().hover(|s| s.bg(theme::hover()))
            .on_click(cx.listener(move |v, _, w, cx| v.open(p.clone(), w, cx)))
            .child(div().pt_0p5().child(avatar::avatar(&pr.user, 28., me, cx)))
            .child(div().flex().flex_col().gap_0p5().flex_1().min_w_0()
                .child(div().flex().items_center().gap_1p5().min_w_0()
                    .child(ui::icon(icon).text_color(color))
                    .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(pr.title.clone())))
                .child(div().flex().w_full().items_center().gap_1p5().min_w_0().text_size(px(11.5)).text_color(theme::text_muted())
                    .child(div().flex_none().child(format!("#{}", pr.number)))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(pr.user.login.clone()))
                    .when(pr.draft == Some(true), |d| d.child(ui::badge("Draft")))
                    .child(div().id("head").min_w_0().truncate().font_family(theme::CODE_FONT).text_size(px(11.))
                        .text_color(if local { theme::accent() } else { theme::text_muted() }).child(pr.head.ref_.clone())
                        .tooltip(move |w, cx| Tooltip::new(r1.clone()).build(w, cx)))
                    .when(local, |d| d.child(div().id("local").child(ui::icon("git-branch").size(px(12.)).text_color(theme::accent()))
                        .tooltip(|w, cx| Tooltip::new("Checked out in a local worktree").build(w, cx))))
                    .child(div().id("base").flex_none().max_w(px(160.)).truncate().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_faint())
                        .child(format!("→ {}", pr.base.ref_)).tooltip(move |w, cx| Tooltip::new(r2.clone()).build(w, cx)))
                    .child(div().flex_1())
                    .when_some(pr.comments.filter(|&c| c > 0), |d, c| d.child(div().flex().flex_none().items_center().gap_0p5()
                        .child(ui::icon("message-square").size(px(12.))).child(c.to_string())))))
            .into_any_element()
    }

    fn browser(&mut self, c: &Ctx, cx: &mut Context<Self>) -> AnyElement {
        let q = self.filter.read(cx).value().to_string();
        let ql = q.to_lowercase();
        let mine = self.mine;
        let list = self.lists.entry(c.path.clone()).or_default().clone();
        // Opened by me only means something with a username: without one it can't hide everything (FLOWS.md §14).
        let visible: Vec<&PullRequest> = list.pulls.iter()
            .filter(|p| !mine || c.me.as_ref().is_none_or(|me| p.user.login.eq_ignore_ascii_case(me)))
            .filter(|p| q.is_empty() || p.title.to_lowercase().contains(&ql) || format!("#{}", p.number).contains(&q) || p.user.login.to_lowercase().contains(&ql))
            .collect();
        // PRs from this worktree's branch first, then other local worktrees', then the rest.
        let others: HashSet<String> = {
            let a = self.app.read(cx);
            let repo = a.repo_containing(&c.path).unwrap_or(&c.path).to_string();
            a.worktrees_of(&repo).iter().filter_map(|w| w.branch.clone()).collect()
        };
        let here = |p: &PullRequest| Some(&p.head.ref_) == c.branch.as_ref();
        enum Row { Header(&'static str), Pull(PullRequest, bool), More }
        let current: Vec<&PullRequest> = visible.iter().copied().filter(|p| here(p)).collect();
        let local: Vec<&PullRequest> = visible.iter().copied().filter(|p| !here(p) && others.contains(&p.head.ref_)).collect();
        let rest: Vec<&PullRequest> = visible.iter().copied().filter(|p| !here(p) && !others.contains(&p.head.ref_)).collect();
        let mut rows: Vec<Row> = vec![];
        if !current.is_empty() { rows.push(Row::Header("This Branch")); rows.extend(current.iter().map(|p| Row::Pull((*p).clone(), true))) }
        if !local.is_empty() { rows.push(Row::Header("Other Worktrees")); rows.extend(local.iter().map(|p| Row::Pull((*p).clone(), true))) }
        if !rows.is_empty() && !rest.is_empty() { rows.push(Row::Header("All")) }
        rows.extend(rest.iter().map(|p| Row::Pull((*p).clone(), false)));
        if self.more.is_some() { rows.push(Row::More) }
        let loading = self.load.as_ref().is_some_and(|l| l.path == c.path);
        let wants_more = list.state != "open" && list.more().is_some() && !loading;
        // PC-05 / PR-19: pr_create's rule (the forge's open PR for the branch, merged excluded), once per HEAD.
        let can_create = self.model.as_ref().map(|m| m.0.clone()).is_some_and(|m| crate::views::pr_create::can_create(&m, cx));
        let state_ix = match list.state.as_str() { "closed" => 1, "all" => 2, _ => 0 };
        let this = cx.entity();
        let me_tip: SharedString = c.me.as_ref().map_or("Add a Forgejo account to filter by author".into(), |m| format!("Only pull requests opened by {m}")).into();
        let (wt, app, branch) = (c.path.clone(), self.app.clone(), c.branch.clone().unwrap_or_default());
        let updated = list.last_updated.map(|t| match t.elapsed().as_secs() / 60 {
            0 => "Updated just now".to_string(),
            1 => "Updated 1 minute ago".into(),
            m if m < 60 => format!("Updated {m} minutes ago"),
            m => format!("Updated {} hours ago", m / 60),
        });
        let stale_error = list.error.clone().filter(|_| !list.pulls.is_empty());
        let header = div().flex().items_center().gap_2().px_2p5().py_2().flex_none().border_b_1().border_color(theme::border())
            .child(ui::segmented("pr-state", &["Open", "Closed", "All"], state_ix, move |i, _, cx| this.update(cx, |v, cx| v.set_state(["open", "closed", "all"][i], cx))))
            .child(ui::pill("pr-mine", "Opened by me").when(mine, |d| d.bg(theme::selected()).border_color(theme::blue()))
                .when(c.me.is_none(), |d| d.opacity(0.4))
                .when(c.me.is_some(), |d| d.on_click(cx.listener(move |v, _, _, cx| v.set_mine(!mine, cx))))
                .tooltip(move |w, cx| Tooltip::new(me_tip.clone()).build(w, cx)))
            .child(div().w(px(320.)).h(px(28.)).px_2().flex().items_center().rounded(px(7.)).bg(theme::raised())
                .border_1().border_color(theme::border()).child(Input::new(&self.filter).appearance(false)))
            .when(can_create, |d| d.child({
                let tip: SharedString = format!("Create a pull request for {branch}").into();
                // Swift: a bordered "+ Create Pull Request…" button, not a bare icon (PC-05).
                ui::pill("pr-create", "Create Pull Request…").flex_row_reverse().child(ui::icon("plus")).tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    .on_click(move |_, _, cx| app.update(cx, |a, cx| a.emit(AppEvent::CreatePull { worktree: wt.clone() }, cx)))
            }))
            .child(div().flex_1())
            // A refresh that failed while PRs show: say so (the Swift app hid it, FLOWS.md §14).
            .when_some(stale_error, |d, e| d.child(div().id("pr-list-error").flex().items_center().gap_1().text_size(px(11.5)).text_color(theme::accent())
                .child(ui::icon("triangle-alert").text_color(theme::accent())).child("Couldn't refresh")
                .tooltip(move |w, cx| Tooltip::new(e.clone()).build(w, cx))))
            .map(|d| if loading { d.child(ui::icon("loader").text_color(theme::text_faint())) }
                     else { d.when_some(updated, |d, u| d.child(div().text_size(px(11.5)).text_color(theme::text_faint()).child(u))) })
            .child(ui::icon_button("pr-refresh", "refresh-cw").tooltip(|w, cx| Tooltip::new("Refresh (⌘R)").build(w, cx))
                .on_click(cx.listener(|v, _, _, cx| v.load(cx))))
            .child(div().text_color(theme::text_muted()).child(format!("{}/{}", c.remote.owner, c.remote.name)));
        let body = if loading && list.pulls.is_empty() {
            ui::empty_state("loader", "", "").into_any_element()
        } else if let Some(e) = list.error.clone().filter(|_| list.pulls.is_empty()) {
            ui::empty_state("triangle-alert", "Couldn't Load", e).into_any_element()
        } else if visible.is_empty() {
            ui::empty_state("git-pull-request", if mine && c.me.is_some() { "No Pull Requests by You" } else { "No Pull Requests" }, "").into_any_element()
        } else {
            let n = rows.len();
            let rows = std::rc::Rc::new(rows);
            let this = cx.entity();
            uniform_list("pr-list", n, move |range, _, cx| {
                if wants_more && range.end + 3 >= n {
                    let t = this.downgrade();
                    cx.defer(move |cx| { t.update(cx, |v, cx| v.load_more(cx)).ok(); });
                }
                range.map(|i| match &rows[i] {
                    Row::Header(t) => div().h(px(52.)).flex().items_end().pb_1().child(ui::section_header(*t, None)).into_any_element(),
                    Row::Pull(p, local) => this.update(cx, |v, cx| v.row(p, *local, cx)),
                    Row::More => div().h(px(52.)).flex().items_center().justify_center().child(ui::icon("loader").text_color(theme::text_faint())).into_any_element(),
                }).collect::<Vec<_>>()
            }).flex_1().px_2().into_any_element()
        };
        div().flex().flex_col().size_full().child(header).child(div().flex().flex_col().flex_1().min_h_0().child(body)).into_any_element()
    }
}

impl Render for PullsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.shown.is_some() && self.selected_detail().is_none() && window.focused(cx).is_none() { self.focus.focus(window, cx) }
        let root = div().id("pulls").track_focus(&self.focus).key_context("Pulls").size_full().bg(theme::canvas());
        let Some(m) = self.model.as_ref().map(|m| m.0.clone()) else { return root.into_any_element() };
        if let Some(c) = self.ctx(cx) {
            if let Some(d) = self.selected_detail() { return root.child(d).into_any_element() }
            return root.child(self.browser(&c, cx)).into_any_element();
        }
        let (remote, detected) = { let m = m.read(cx); (m.remote.clone(), m.remote_detected) };
        if !detected || !self.app.read(cx).tokens_loaded {
            return root.child(ui::empty_state("loader", "", "")).into_any_element();
        }
        match remote {
            Some(r) => {
                let kind = if r.host == "github.com" { "GitHub" } else { "Forgejo" };
                root.child(ui::empty_state("user-plus", format!("Connect {}", r.host),
                    format!("Add a {kind} account for {} to review pull requests for {}/{}.", r.host, r.owner, r.name))
                    .child(ui::pill_primary("pr-open-settings", "Open Settings…", theme::blue())
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(OpenSettings), cx))))
                    .into_any_element()
            }
            None => root.child(ui::empty_state("unplug", "No Remote", "This repository has no remote that looks like a Forgejo or GitHub repo.")).into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock forge on a thread: answers `pulls?page=N` with `pages[N-1]` PRs (the server's own page size).
    fn serve(pages: Vec<usize>) -> String {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut id = 0;
            for s in l.incoming() {
                let mut s = s.unwrap();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let page: usize = req.split("page=").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).unwrap_or(1);
                let count = pages.get(page - 1).copied().unwrap_or(0);
                let items: Vec<String> = (0..count).map(|_| { id += 1; format!(r#"{{"id":{id},"number":{id},"title":"t","state":"open","user":{{"login":"a"}},"head":{{"ref":"h","sha":"s"}},"base":{{"ref":"main","sha":"b"}},"html_url":"","updated_at":""}}"#) }).collect();
                let body = format!("[{}]", items.join(","));
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn open_list_pages_past_a_server_capped_below_fifty() {
        let base = serve(vec![30, 30, 7]);
        let client = Client::new(&base, "t", false);
        let repo = RemoteRepo::parse(&format!("{base}/o/r.git")).unwrap();
        assert_eq!(fetch_list(&client, &repo, "open").result.unwrap().0.len(), 67);
        let base = serve(vec![12]);
        let client = Client::new(&base, "t", false);
        assert_eq!(fetch_list(&client, &repo, "open").result.unwrap().0.len(), 12);
    }
}
