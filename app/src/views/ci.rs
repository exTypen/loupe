//! CI (CI.swift, CIForges.swift): the checks overlay, job detail and log tail. Owner: the CI view agent
//! (docs/gpui/VIEWS.md). The data and network side (features, statuses, runs, jobs, log tailing, formatting) is
//! `loupe_forge::ci`; this file is the views and their polling.
//!
//! - `install` (called once per window by the shell) presents `ChecksOverlay` on `AppEvent::ShowChecks`.
//! - `ChecksModel::shared(app, worktree, sha, pull, cx)` is one commit's checks, shared by the PR header's badge
//!   (`badge`) and the overlay, as the Swift PR model's lazy `ChecksModel` was. The Pulls view calls
//!   `model.update(cx, |m, cx| m.refresh(cx))` when the badge appears and after every PR reload (CI-01, CI-03).
//!
//! Polling: the overlay refreshes every 5 s while anything runs and every 30 s once settled (or empty); a running
//! job's log is tailed every 2 s. Both are tasks owned by their view, so closing the overlay or leaving the detail
//! page drops them, and the `Cancel` they share with the forge client stops a request in flight. Nothing animates.
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Div, ElementId, Entity, FocusHandle, FontWeight, Global,
    Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton, ParentElement as _,
    Render, ScrollStrategy, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::FluentBuilder as _, px, uniform_list,
    component::input::{Input, InputEvent, InputState},
    component::tooltip::Tooltip,
};
use loupe_forge::ci::{Check, CheckState, Checks, CiAction, CiFeatures, JobLog, LogLine, LogNext, LogStyle, duration, parse_iso8601};
use loupe_forge::models::{ActionJob, CheckAnnotation};
use loupe_forge::{Cancel, Client, RemoteRepo};

use crate::shell::keys::BackToList;
use crate::state::{AppEvent, AppModel};
use crate::{theme, ui};

pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Subscription {
    window.subscribe(app, cx, |app, e: &AppEvent, window, cx| {
        let AppEvent::ShowChecks { worktree, sha, pull } = e else { return };
        let model = ChecksModel::shared(&app, worktree, sha, *pull, cx);
        let v = cx.new(|cx| ChecksOverlay::new(model, window, cx));
        app.update(cx, |a, cx| a.present(v.into(), cx));
    })
}

// MARK: - Model

/// One commit's checks (CI.swift `ChecksModel`): the badge and the overlay read the same instance.
pub struct ChecksModel {
    pub app: Entity<AppModel>,
    pub worktree: String,
    pub pull: Option<u64>,
    pub checks: Checks,
    /// One refresh at a time: the badge, PR reloads, the overlay's poll and the refresh button all join it
    /// (§14: the Swift refresh had no in-flight guard).
    refreshing: Option<Shared<Task<()>>>,
    /// Stops a refresh in flight when the model goes away.
    cancel: Cancel,
    /// Waiting for the account's token (`client_for` is None until the Keychain read finishes).
    waiting: bool,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
struct Registry(HashMap<String, WeakEntity<ChecksModel>>);
impl Global for Registry {}

impl Drop for ChecksModel {
    fn drop(&mut self) { self.cancel.cancel() }
}

impl ChecksModel {
    /// The worktree's checks model, switched to `sha` (a new push starts over, keeping the forge's features).
    pub fn shared(app: &Entity<AppModel>, worktree: &str, sha: &str, pull: Option<u64>, cx: &mut App) -> Entity<ChecksModel> {
        let existing = cx.default_global::<Registry>().0.get(worktree).and_then(|w| w.upgrade());
        if let Some(m) = existing {
            m.update(cx, |m, cx| { m.pull = pull; m.set_sha(sha, cx) });
            return m;
        }
        let app = app.clone();
        let m = cx.new(|cx| {
            let subs = vec![cx.observe(&app, |m: &mut ChecksModel, _, cx| if m.waiting && m.source(cx).is_some() { m.waiting = false; m.refresh(cx); })];
            ChecksModel { app: app.clone(), worktree: worktree.into(), pull, checks: Checks::new(sha), refreshing: None, cancel: Cancel::new(), waiting: false, _subs: subs }
        });
        let r = &mut cx.default_global::<Registry>().0;
        r.retain(|_, w| w.upgrade().is_some());
        r.insert(worktree.into(), m.downgrade());
        m
    }

    pub fn set_sha(&mut self, sha: &str, cx: &mut Context<Self>) {
        if self.checks.sha == sha { return }
        self.checks.set_sha(sha);
        self.refreshing = None; // its result is for the old sha and gets dropped
        cx.notify();
    }

    fn source(&self, cx: &App) -> Option<(Client, RemoteRepo)> {
        let a = self.app.read(cx);
        let repo = a.model(&self.worktree)?.read(cx).remote.clone()?;
        Some((a.client_for(&repo, &self.worktree)?.with_cancel(&self.cancel), repo))
    }

    pub fn base_url(&self, cx: &App) -> String { self.source(cx).map(|(c, _)| c.base_url).unwrap_or_default() }
    pub fn github(&self, cx: &App) -> bool { self.source(cx).is_some_and(|(c, _)| c.github) }

    /// True only before anything has loaded (later refreshes show no spinner).
    pub fn loading(&self) -> bool { self.checks.last_updated.is_none() && self.checks.error.is_none() && (self.refreshing.is_some() || self.waiting) }

    /// Starts a refresh, or joins the one already running. None while the account's token isn't loaded yet; the
    /// refresh starts by itself once it is.
    pub fn refresh(&mut self, cx: &mut Context<Self>) -> Option<Shared<Task<()>>> {
        if let Some(t) = &self.refreshing { return Some(t.clone()) }
        // A PR opened by number before its JSON arrived (PC-17) has no head yet: `commits//status` is a 404.
        if self.checks.sha.is_empty() { return None }
        let Some((client, repo)) = self.source(cx) else {
            if !self.waiting { self.waiting = true; cx.notify() }
            return None;
        };
        let mut checks = self.checks.clone();
        let task = cx.spawn(async move |this, cx| {
            let checks = cx.background_executor().spawn(async move { checks.refresh(&client, &repo); checks }).await;
            let _ = this.update(cx, |m, cx| {
                if m.checks.sha != checks.sha { return }
                m.refreshing = None;
                m.checks = checks;
                cx.notify();
            });
        }).shared();
        self.refreshing = Some(task.clone());
        Some(task)
    }

    /// The overlay's poll interval: 5 s while anything moves, 30 s once everything settled. No checks counts as
    /// settled (the Swift app polled an empty commit every 5 s forever, §14).
    fn poll_interval(&self) -> Duration {
        if self.checks.checks.is_empty() { Duration::from_secs(30) } else { self.checks.poll_interval() }
    }
}

/// CI-01: the PR header's badge. Clicking it opens the overlay.
pub fn badge(model: &Entity<ChecksModel>, cx: &App) -> Stateful<Div> {
    let m = model.read(cx);
    let s = m.checks.summary();
    let ev = AppEvent::ShowChecks { worktree: m.worktree.clone(), sha: m.checks.sha.clone(), pull: m.pull };
    let app = m.app.clone();
    let tip = format!("CI checks — {} passed, {} running, {} queued, {} failing", s.passed, s.running, s.queued, s.failing);
    let count = |icon: AnyElement, n: usize, c: Hsla| div().flex().items_center().gap_1().child(icon).child(div().text_color(c).child(n.to_string()));
    div().id("ci-badge").flex().items_center().gap_2().px_2().h(px(22.)).rounded(theme::RADIUS_PILL).cursor_pointer()
        .text_size(px(12.)).hover(|d| d.bg(theme::hover()))
        .map(|d| if s.total == 0 {
            d.child(ui::icon("circle-dashed")).child(div().text_color(theme::text_muted()).child("Checks"))
        } else {
            d.when(s.failing > 0, |d| d.child(count(state_icon(CheckState::Failing), s.failing, theme::red())))
                .when(s.running > 0, |d| d.child(count(state_icon(CheckState::Running), s.running, theme::accent())))
                .when(s.queued > 0, |d| d.child(count(state_icon(CheckState::Pending), s.queued, theme::text_muted())))
                .child(count(state_icon(CheckState::Success), s.passed, theme::green()))
                .child(div().text_color(theme::text_muted()).child("checks"))
        })
        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
        .on_click(move |_, _, cx| app.update(cx, |a, cx| a.emit(ev.clone(), cx)))
}

// MARK: - Overlay

pub struct ChecksOverlay {
    model: Entity<ChecksModel>,
    filter: Entity<InputState>,
    /// The open check's key (`check_keys`); stays set while the check is missing, so it reopens when it returns.
    open: Option<String>,
    detail: Option<Entity<CheckDetail>>,
    focus: FocusHandle,
    focused: bool,
    _poll: Task<()>,
    _subs: Vec<Subscription>,
}

impl ChecksOverlay {
    pub fn new(model: Entity<ChecksModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter checks"));
        let subs = vec![
            cx.observe(&model, |o, _, cx| { o.sync_detail(cx); cx.notify() }),
            cx.subscribe(&filter, |_, _, e: &InputEvent, cx| if let InputEvent::Change = e { cx.notify() }),
        ];
        let m = model.clone();
        let poll = cx.spawn(async move |_, cx| loop {
            let t = m.update(cx, |m, cx| m.refresh(cx));
            if let Some(t) = t { t.await }
            let d = m.read_with(cx, |m, _| m.poll_interval());
            cx.background_executor().timer(d).await;
        });
        Self { model, filter, open: None, detail: None, focus: cx.focus_handle(), focused: false, _poll: poll, _subs: subs }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let app = self.model.read(cx).app.clone();
        app.update(cx, |a, cx| a.dismiss(cx));
    }

    /// Opens the first check whose id contains `text` (the stress driver's "job-01" row); false when none does.
    pub fn open_check_containing(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let key = check_keys(&self.model.read(cx).checks.checks).into_iter().find(|k| k.contains(text));
        let found = key.is_some();
        if found { self.open_check(key, cx) }
        found
    }

    /// The overlay shows the list, not a check's page.
    pub fn showing_list(&self) -> bool { self.detail.is_none() }

    pub fn check_count(&self, cx: &App) -> usize { self.model.read(cx).checks.checks.len() }

    fn open_check(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.open = key;
        self.focused = false; // the filter field may have had focus; keys go to the overlay again
        self.sync_detail(cx);
        cx.notify();
    }

    /// The detail page follows the model: a rerun's new attempt (or job id) builds a fresh page and log.
    fn sync_detail(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.open.clone() else { self.detail = None; return };
        let checks = self.model.read(cx).checks.checks.clone();
        let found = check_keys(&checks).into_iter().zip(checks).find(|(k, _)| *k == key).map(|(_, c)| c);
        let Some(check) = found else { self.detail = None; return };
        let ident = detail_ident(&key, &check);
        match &self.detail {
            Some(d) if d.read(cx).ident == ident => d.update(cx, |d, cx| if d.check != check { d.check = check; cx.notify() }),
            _ => {
                let model = self.model.clone();
                self.detail = Some(cx.new(|cx| CheckDetail::new(model, ident, check, cx)));
            }
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let m = self.model.read(cx);
        let c = &m.checks;
        let s = c.summary();
        let loading = m.loading();
        // No endless pulse on the running icon (§14 CI.swift:628), and no checks never reads "All checks passed".
        let (icon, headline): (AnyElement, String) = if s.total == 0 {
            (ui::icon("circle-dashed").size(px(20.)).into_any_element(), if loading { "Loading checks…".into() } else { "No checks".into() })
        } else if s.failing > 0 {
            (ui::icon("octagon-x").size(px(20.)).text_color(theme::red()).into_any_element(), c.headline())
        } else if s.running + s.queued > 0 {
            (div().size(px(16.)).m(px(2.)).rounded_full().bg(theme::accent()).into_any_element(), c.headline())
        } else {
            (ui::icon("badge-check").size(px(20.)).text_color(theme::green()).into_any_element(), c.headline())
        };
        let sha: String = c.sha.chars().take(8).collect();
        let me = cx.entity();
        let me2 = cx.entity();
        div().flex().items_center().gap_3().px(px(14.)).py(px(12.)).flex_none()
            .child(icon)
            .child(div().flex().flex_col().gap_0p5().flex_none()
                .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(headline))
                .child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(format!("{} of {} successful · {sha}", s.passed, s.total))))
            .child(div().w(px(260.)).flex_none().child(Input::new(&self.filter)))
            .child(div().flex_1())
            .map(|d| if let Some(e) = &c.error {
                d.child(div().flex().items_center().gap_1().min_w_0().text_size(px(11.5)).text_color(theme::accent())
                    .child(ui::icon("triangle-alert").text_color(theme::accent())).child(div().truncate().child(e.clone())))
            } else if let Some(n) = c.notice.clone() {
                let tip = n.clone();
                d.child(div().id("ci-notice").flex().items_center().gap_1().min_w_0().max_w(px(520.)).text_size(px(11.5)).text_color(theme::text_muted())
                    .child(ui::icon("info")).child(div().line_clamp(2).child(n))
                    .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx)))
            } else { d })
            .when_some(c.last_updated, |d, t| d.child(div().flex_none().text_size(px(11.5)).text_color(theme::text_faint()).child(updated_label(t.elapsed()))))
            .child(ui::icon_button("ci-refresh", "refresh-cw").tooltip(|w, cx| Tooltip::new("Refresh now").build(w, cx))
                .on_click(move |_, _, cx| me.update(cx, |o, cx| { o.model.update(cx, |m, cx| { m.refresh(cx); }); })))
            .child(ui::pill("ci-done", "Done").on_click(move |_, _, cx| me2.update(cx, |o, cx| o.close(cx))))
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let checks = &m.checks.checks;
        if checks.is_empty() {
            return if m.loading() {
                div().size_full().flex().items_center().justify_center().text_color(theme::text_muted()).child("Loading checks…").into_any_element()
            } else {
                ui::empty_state("circle-dashed", "No Checks", "Nothing has reported a status for this commit.").into_any_element()
            };
        }
        let q = self.filter.read(cx).value().to_lowercase();
        let keys = check_keys(checks);
        let me = cx.entity();
        let mut col = div().id("ci-list").size_full().overflow_y_scroll().px(px(14.)).pb_3().flex().flex_col();
        for state in CheckState::ALL {
            let rows: Vec<(usize, &Check)> = checks.iter().enumerate()
                .filter(|(_, c)| c.state() == state && (q.is_empty() || c.status.context.to_lowercase().contains(&q))).collect();
            if rows.is_empty() { continue }
            col = col.child(div().pt_3().pb_1().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_muted())
                .child(format!("{} · {}", state.title(), rows.len())));
            for (i, c) in rows {
                let key = keys[i].clone();
                let me = me.clone();
                col = col.child(check_row(i, c).on_click(move |_, _, cx| me.update(cx, |o, cx| o.open_check(Some(key.clone()), cx))));
            }
        }
        col.into_any_element()
    }
}

impl Render for ChecksOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        let size = window.viewport_size();
        let w = (size.width - px(64.)).min(px(1500.));
        let h = size.height - px(64.);
        let body: AnyElement = match &self.detail {
            Some(d) => d.clone().into_any_element(),
            None => div().size_full().flex().flex_col().child(self.header(cx)).child(ui::divider()).child(div().flex_1().min_h_0().child(self.list(cx))).into_any_element(),
        };
        let me = cx.entity();
        div().id("ci-overlay").occlude().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&self.focus)
            // Esc: back to the list first, then close. ⌘[ only goes back; the overlay holds focus, so the PR detail
            // behind it never sees it (§14: both bound ⌘[ in the Swift app).
            .capture_key_down(cx.listener(|o, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.key == "escape" {
                    if o.open.is_some() { o.open_check(None, cx) } else { o.close(cx) }
                    cx.stop_propagation();
                } else if k.key == "c" && k.modifiers.platform && let Some(d) = &o.detail
                    && let Some(t) = d.read(cx).selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(t));
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|o, _: &BackToList, _, cx| if o.open.is_some() { o.open_check(None, cx) }))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| me.update(cx, |o, cx| o.close(cx)))
            .child(div().id("ci-card").w(w).h(h).flex().flex_col().overflow_hidden().rounded(px(14.)).border_1().border_color(theme::border())
                .bg(theme::canvas()).shadow_lg().text_size(theme::UI_SIZE).text_color(theme::text())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(body))
    }
}

fn check_row(i: usize, c: &Check) -> Stateful<Div> {
    let ctx = &c.status.context;
    let parts: Vec<&str> = ctx.split(" / ").collect();
    let (name, wf) = if parts.len() > 1 { (parts[1..].join(" / "), Some(parts[0].to_string())) } else { (ctx.clone(), None) };
    let tip = ctx.clone() + &c.status.description.as_ref().map(|d| format!("\n{d}")).unwrap_or_default();
    let mut caption = div().flex().items_center().gap(px(10.)).text_size(px(11.5)).text_color(theme::text_muted()).min_w_0().overflow_hidden();
    if let Some(d) = c.status.description.as_ref().filter(|d| !d.is_empty()) { caption = caption.child(div().truncate().child(d.clone())) }
    if let Some(j) = &c.job {
        if let Some(r) = j.runs_on.as_ref().filter(|r| !r.is_empty()) { caption = caption.child(labelled("server", r.join(", "))) }
        if let Some(a) = j.attempt.filter(|a| *a > 1) { caption = caption.child(labelled("rotate-ccw", format!("attempt {a}"))) }
        if let Some(n) = j.needs.as_ref().filter(|n| !n.is_empty()) { caption = caption.child(labelled("corner-down-right", format!("needs {}", n.join(", ")))) }
    } else if c.status.target_url.is_some() {
        caption = caption.child(labelled("square-arrow-out-up-right", "external".into()));
    }
    if let Some(t) = c.status.updated_at.as_deref().and_then(parse_iso8601) { caption = caption.child(div().flex_none().child(ago_named(now() - t))) }
    div().id(ElementId::Name(format!("ci-row-{i}").into())).flex().items_center().gap(px(10.)).py_1p5().px_1().border_b_1().border_color(theme::border())
        .cursor_pointer().hover(|d| d.bg(theme::hover()))
        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
        .child(div().w(px(16.)).flex().justify_center().child(state_icon(c.state())))
        .child(div().flex().flex_col().gap(px(3.)).flex_1().min_w_0()
            .child(div().flex().items_center().gap_1p5().min_w_0()
                .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(name))
                .when_some(wf, |d, wf| d.child(div().truncate().text_color(theme::text_muted()).child(wf))))
            .child(caption))
        .child(ui::icon("chevron-right").text_color(theme::text_faint()))
}

fn labelled(icon: &str, text: String) -> Div {
    div().flex().flex_none().items_center().gap_1().child(ui::icon(icon).size(px(11.))).child(text)
}

fn state_icon(s: CheckState) -> AnyElement {
    match s {
        CheckState::Failing => ui::icon("circle-x").text_color(theme::red()).into_any_element(),
        // Static: a job can run for an hour, and an endless animation redraws the window every frame (f310e64).
        CheckState::Running => div().size(px(14.)).flex().flex_none().items_center().justify_center()
            .child(div().size(px(10.)).rounded_full().bg(theme::accent())).into_any_element(),
        CheckState::Pending => ui::icon("circle-dashed").into_any_element(),
        CheckState::Success => ui::icon("circle-check").text_color(theme::green()).into_any_element(),
        CheckState::Skipped => ui::icon("circle-minus").text_color(theme::text_faint()).into_any_element(),
    }
}

// MARK: - Detail page

pub struct CheckDetail {
    model: Entity<ChecksModel>,
    /// key + job id + attempt: a rerun's new attempt builds a fresh page.
    ident: String,
    check: Check,
    log: Option<JobLog>,
    /// Indices into `log.lines` that aren't inside a folded group, rebuilt when `log.version` moves.
    shown: Vec<usize>,
    shown_version: Option<u64>,
    shown_len: usize,
    widest: usize,
    scroll: UniformListScrollHandle,
    /// Selected lines (anchor, head) in `log.lines`; a problem jump selects its line.
    selection: Option<(usize, usize)>,
    show_problems: bool,
    busy: Option<CiAction>,
    action_error: Option<String>,
    annotations: Vec<CheckAnnotation>,
    cancel: Cancel,
    _log: Option<Task<()>>,
    _task: Option<Task<()>>,
}

impl Drop for CheckDetail {
    fn drop(&mut self) { self.cancel.cancel() }
}

impl CheckDetail {
    fn new(model: Entity<ChecksModel>, ident: String, check: Check, cx: &mut Context<Self>) -> Self {
        let mut d = Self {
            model, ident, check, log: None, shown: vec![], shown_version: None, shown_len: 0, widest: 0,
            scroll: UniformListScrollHandle::new(), selection: None, show_problems: true, busy: None, action_error: None,
            annotations: vec![], cancel: Cancel::new(), _log: None, _task: None,
        };
        let source = d.model.read(cx).source(cx);
        if let Some((client, repo)) = source {
            let client = client.with_cancel(&d.cancel);
            if let Some(job) = d.check.job.clone() {
                let features = d.model.read(cx).checks.features.clone().unwrap_or_else(|| CiFeatures::detect(client.github, None));
                d.show_problems = CheckState::from_job(&job.status) == CheckState::Failing;
                d.log = Some(JobLog::new(job, features));
                d._log = Some(Self::tail(client, repo, cx));
            } else if let Some(run) = d.check.status.check_run.clone().filter(|r| r.annotations.unwrap_or(0) > 0) {
                // A third-party check run's annotations (an Actions job's are its log's ##[error] lines).
                d._task = Some(cx.spawn(async move |this, cx| {
                    let a = cx.background_executor().spawn(async move { client.check_run_annotations(&repo, run.id).unwrap_or_default() }).await;
                    let _ = this.update(cx, |d, cx| { d.annotations = a; cx.notify() });
                }));
            }
        }
        d
    }

    /// The Swift poll loop: the first read, then (while the job runs) the job and a Range tail every 2 s; network
    /// and formatting off the main thread.
    fn tail(client: Client, repo: RemoteRepo, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| loop {
            let Ok(Some(req)) = this.update(cx, |d, _| d.log.as_mut().map(|l| l.poll_request())) else { return };
            let (c, r) = (client.clone(), repo.clone());
            let resp = cx.background_executor().spawn(async move { req.fetch(&c, &r) }).await;
            let Ok(Some(next)) = this.update(cx, |d, cx| {
                let n = d.log.as_mut().map(|l| l.apply(resp));
                cx.notify();
                n
            }) else { return };
            match next {
                LogNext::Done => return,
                LogNext::Again(t) => cx.background_executor().timer(t).await,
            }
        })
    }

    fn load_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.log.as_ref().and_then(|l| l.earlier_request()) else { return };
        let Some((client, repo)) = self.model.read(cx).source(cx) else { return };
        let client = client.with_cancel(&self.cancel);
        self._task = Some(cx.spawn(async move |this, cx| {
            let resp = cx.background_executor().spawn(async move { req.fetch(&client, &repo) }).await;
            let _ = this.update(cx, |d, cx| { if let Some(l) = d.log.as_mut() { l.apply(resp); } cx.notify() });
        }));
    }

    fn perform(&mut self, a: CiAction, job: ActionJob, cx: &mut Context<Self>) {
        let Some((client, repo)) = self.model.read(cx).source(cx) else { return };
        let client = client.with_cancel(&self.cancel);
        let mut checks = self.model.read(cx).checks.clone();
        self.busy = Some(a);
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let (err, checks) = cx.background_executor().spawn(async move { let e = checks.perform(a, &job, &client, &repo); (e, checks) }).await;
            let _ = this.update(cx, |d, cx| {
                d.busy = None;
                d.action_error = err;
                d.model.update(cx, |m, cx| if m.checks.sha == checks.sha { m.checks = checks; cx.notify() });
                cx.notify();
            });
        }));
    }

    fn sync_lines(&mut self) {
        let Some(log) = &self.log else { return };
        if self.shown_version == Some(log.version) { return }
        let grew = log.lines.len() > self.shown_len;
        // Follow the tail only from the bottom (or on the first fill); a fold toggle never scrolls.
        let follow = self.shown_len == 0 || self.scroll.is_scrolled_to_end().unwrap_or(true);
        self.shown = visible_lines(&log.lines, &log.expanded);
        self.widest = self.shown.iter().enumerate().max_by_key(|(_, i)| log.lines[**i].text.len()).map_or(0, |(p, _)| p);
        self.shown_version = Some(log.version);
        self.shown_len = log.lines.len();
        if grew && follow { self.scroll.scroll_to_bottom() }
    }

    fn jump(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(l) = self.log.as_mut() else { return };
        l.unfold(index);
        self.sync_lines();
        if let Ok(p) = self.shown.binary_search(&index) { self.scroll.scroll_to_item(p, ScrollStrategy::Center) }
        self.selection = Some((index, index));
        cx.notify();
    }

    fn click_line(&mut self, index: usize, shift: bool, cx: &mut Context<Self>) {
        let Some(l) = self.log.as_mut() else { return };
        if let Some(g) = l.lines.get(index).and_then(|x| x.header) {
            l.toggle_group(g);
        } else {
            self.selection = Some(match self.selection { Some((a, _)) if shift => (a, index), _ => (index, index) });
        }
        cx.notify();
    }

    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (a, b) = (a.min(b), a.max(b));
        let log = self.log.as_ref()?;
        Some(self.shown.iter().filter(|i| (a..=b).contains(*i)).map(|&i| display(&log.lines[i], &log.expanded).into_owned()).collect::<Vec<_>>().join("\n"))
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let m = self.model.read(cx);
        let forge = m.checks.features.as_ref().map_or("Forgejo", |f| f.name());
        let job = self.log.as_ref().map(|l| &l.job).or(self.check.job.as_ref()).cloned();
        let state = self.log.as_ref().map_or(self.check.state(), |l| CheckState::from_job(&l.job.status));
        let actions = match (&m.checks.features, &job) {
            (Some(f), Some(j)) => offered(f, m.checks.run(j.run_id).and_then(|r| r.status.as_deref()), &j.status),
            _ => vec![],
        };
        let url = self.check.web_url(&m.base_url(cx));
        let mut buttons = div().flex().items_center().gap_2().flex_none();
        for a in actions {
            let j = job.clone().unwrap_or_default();
            let label: SharedString = if self.busy == Some(a) { "Working…".into() } else { a.title().into() };
            let icon = match a { CiAction::RerunJob => "refresh-cw", CiAction::Cancel => "circle-stop", _ => "refresh-ccw-dot" };
            let mut b = ui::pill(ElementId::Name(format!("ci-act-{a:?}").into()), "").child(ui::icon(icon)).child(label)
                .tooltip(move |w, cx| Tooltip::new(a.help()).build(w, cx));
            if self.busy.is_none() { b = b.on_click(cx.listener(move |d, _, _, cx| d.perform(a, j.clone(), cx))) } else { b = b.opacity(0.5) }
            buttons = buttons.child(b);
        }
        if self.log.is_some() {
            buttons = buttons.child(ui::pill("ci-copy", "Copy Log").on_click(cx.listener(|d, _, _, cx| {
                if let Some(l) = &d.log { cx.write_to_clipboard(ClipboardItem::new_string(copy_text(l))) }
            })));
        }
        if let Some(u) = url { buttons = buttons.child(ui::pill("ci-web", format!("Open in {forge}")).on_click(move |_, _, cx| cx.open_url(&u))) }
        let info = job.map(|j| {
            let steps = j.steps.as_deref().unwrap_or(&[]);
            let dur = duration(steps.first().and_then(|s| s.started.as_deref()), steps.last().and_then(|s| s.stopped.as_deref()), now());
            div().flex().items_center().gap(px(10.)).text_size(px(11.5)).text_color(theme::text_muted())
                .child(format!("Job “{}”", j.name))
                .when_some(j.runs_on.filter(|r| !r.is_empty()), |d, r| d.child(labelled("server", r.join(", "))))
                .when_some(j.attempt.filter(|a| *a > 1), |d, a| d.child(format!("attempt {a}")))
                .when_some(dur, |d, t| d.child(labelled("timer", t)))
        });
        div().flex().flex_col().gap(px(6.)).p_3().flex_none()
            .child(div().flex().items_center().gap_2()
                .child(div().id("ci-back").flex().items_center().gap_1().px_1().rounded(px(5.)).cursor_pointer().text_color(theme::blue())
                    .hover(|d| d.bg(theme::hover())).child(ui::icon("chevron-left").text_color(theme::blue())).child("Checks")
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(BackToList), cx)))
                .child(state_icon(state))
                .child(div().flex_1().min_w_0().line_clamp(2).text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(self.check.status.context.clone()))
                .child(buttons))
            .when_some(self.check.status.description.clone().filter(|d| !d.is_empty()), |d, t| d.child(div().text_color(theme::text_muted()).child(t)))
            .when_some(self.action_error.clone(), |d, e| d.child(div().flex().items_center().gap_1().text_color(theme::red())
                .child(ui::icon("triangle-alert").text_color(theme::red())).child(e)))
            .children(info)
    }

    fn body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_lines();
        let Some(log) = &self.log else { return self.no_log(cx) };
        let github = self.model.read(cx).github(cx);
        let mut col = div().size_full().flex().flex_col();
        if let Some(steps) = log.job.steps.as_ref().filter(|s| !s.is_empty()) {
            let n = now();
            col = col.child(div().id("ci-steps").max_h(px(150.)).overflow_y_scroll().flex_none().px_3().py_2().flex().flex_col().gap(px(3.))
                .children(steps.iter().map(|s| {
                    let skipped = s.status == "skipped";
                    div().flex().items_center().gap_2()
                        .child(div().w(px(14.)).flex().justify_center().child(state_icon(CheckState::from_job(&s.status))))
                        .child(div().flex_1().truncate().text_color(if skipped { theme::text_faint() } else { theme::text() }).child(s.name.clone()))
                        .when_some((!skipped).then(|| duration(s.started.as_deref(), s.stopped.as_deref(), n)).flatten(), |d, t|
                            d.child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(t)))
                })))
                .child(ui::divider());
        }
        if log.waiting_for_log {
            let text = if CheckState::from_job(&log.job.status) == CheckState::Pending { "Waiting for a runner to pick up this job…" }
                else if github { "GitHub makes a job's log available once the job has finished; the steps above update meanwhile." }
                else { "Waiting for the job's log…" };
            // A static clock, not a spinner: the wait can last minutes.
            col = col.child(div().flex().items_center().gap_2().px_3().py_2().flex_none().text_color(theme::text_muted())
                .child(ui::icon("clock")).child(text)).child(ui::divider());
        }
        if !log.problems.is_empty() {
            let open = self.show_problems;
            col = col.child(div().px_3().py(px(6.)).flex_none().flex().flex_col().gap_1()
                .child(div().id("ci-problems").flex().items_center().gap_1().cursor_pointer().text_color(theme::red())
                    .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }))
                    .child(ui::icon("triangle-alert").text_color(theme::red()))
                    .child(format!("{} problem line(s) — click to jump", log.problems.len()))
                    .on_click(cx.listener(|d, _, _, cx| { d.show_problems = !d.show_problems; cx.notify() })))
                .when(open, |d| d.child(div().id("ci-problem-list").max_h(px(140.)).overflow_y_scroll().flex().flex_col().gap(px(2.))
                    .children(log.problems.iter().take(200).enumerate().map(|(i, p)| {
                        let index = p.index;
                        div().id(ElementId::Name(format!("ci-p-{i}").into())).cursor_pointer().line_clamp(2)
                            .font_family(theme::CODE_FONT).text_size(px(11.)).text_color(if p.warning { theme::accent() } else { theme::red() })
                            .hover(|d| d.bg(theme::hover()))
                            .child(clean(&p.line).into_owned())
                            .on_click(cx.listener(move |d, _, _, cx| d.jump(index, cx)))
                    })))))
                .child(ui::divider());
        }
        if log.truncated_bytes > 0 {
            // Says what's missing: after a live-tail trim it isn't "the last 16 MB" (§14 CI.swift:778).
            col = col.child(div().flex().items_center().gap_2().px_3().py_1().flex_none().text_size(px(11.5)).text_color(theme::text_muted())
                .child(format!("The first {} of the log aren't shown.", size_label(log.truncated_bytes)))
                .child(ui::pill("ci-full", "Load Full Log").h(px(22.)).on_click(cx.listener(|d, _, _, cx| d.load_earlier(cx)))));
        }
        let sel = self.selection.map(|(a, b)| (a.min(b), a.max(b)));
        let list = uniform_list("ci-log", self.shown.len(), cx.processor(move |d: &mut CheckDetail, range: std::ops::Range<usize>, _, cx| {
            let Some(log) = &d.log else { return vec![] };
            range.map(|p| {
                let i = d.shown[p];
                let l = &log.lines[i];
                let selected = sel.is_some_and(|(a, b)| (a..=b).contains(&i));
                let text = display(l, &log.expanded);
                // A line the runner coloured ("\x1b[31merror\x1b[0m: …") reads as a problem once its escapes are gone.
                let style = if l.style == LogStyle::Plain && l.text.contains('\x1b') && loupe_forge::ci::is_problem(&text) { LogStyle::Problem } else { l.style };
                let (color, bold) = style_color(style);
                div().id(ElementId::Integer(i as u64)).h(px(16.)).px_2().whitespace_nowrap().text_color(color)
                    .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(selected, |d| d.bg(theme::find_match()))
                    .when(l.header.is_some(), |d| d.cursor_pointer())
                    .child(text.into_owned())
                    .on_click(cx.listener(move |d, e: &gpui_kit::ClickEvent, _, cx| d.click_line(i, e.modifiers().shift, cx)))
                    .into_any_element()
            }).collect()
        }))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(self.widest))
        .track_scroll(&self.scroll)
        .flex_1().min_h_0().py_2().font_family(theme::CODE_FONT).text_size(px(11.5)).bg(theme::card());
        col = col.child(list);
        if let Some(e) = &log.error { col = col.child(div().p(px(6.)).flex_none().text_size(px(11.5)).text_color(theme::red()).child(e.clone())) }
        col.into_any_element()
    }

    fn no_log(&self, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let forge = m.checks.features.as_ref().map_or("Forgejo", |f| f.name());
        let run = self.check.status.check_run.as_ref();
        if let Some(run) = run.filter(|r| r.summary.as_ref().is_some_and(|s| !s.is_empty()) || !self.annotations.is_empty()) {
            return div().id("ci-output").size_full().overflow_y_scroll().p(px(14.)).flex().flex_col().gap_3()
                .when_some(run.summary.clone().filter(|s| !s.is_empty()), |d, s| d.child(gpui_kit::component::text::TextView::markdown("ci-summary", s)))
                .when(!self.annotations.is_empty(), |d| d
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Annotations · {}", self.annotations.len())))
                    .children(self.annotations.iter().map(|a| {
                        let (icon, color) = match a.annotation_level.as_deref() {
                            Some("failure") => ("circle-x", theme::red()),
                            Some("warning") => ("triangle-alert", theme::accent()),
                            _ => ("info", theme::text_muted()),
                        };
                        div().flex().gap_2().child(ui::icon(icon).text_color(color)).child(div().flex().flex_col().gap_0p5()
                            .child(div().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_muted())
                                .child(a.path.clone() + &a.start_line.map(|l| format!(":{l}")).unwrap_or_default()))
                            .when_some(a.title.clone().filter(|t| !t.is_empty()), |d, t| d.child(div().font_weight(FontWeight::MEDIUM).child(t)))
                            .child(a.message.clone()))
                    })))
                .into_any_element();
        }
        let actions_job = run.is_some_and(|r| r.app.as_deref() == Some("github-actions"));
        let (title, text) = if actions_job {
            ("Job Not Loaded", m.checks.notice.clone().unwrap_or_else(|| "GitHub hasn't listed this job yet; it shows here once it does.".into()))
        } else {
            ("External Status", format!("Reported by a service outside {forge} Actions, so there's no log to show here."))
        };
        let url = self.check.web_url(&m.base_url(cx));
        ui::empty_state("square-arrow-out-up-right", title, text)
            .when_some(url, |d, u| d.child(ui::pill("ci-details", "Open Details").on_click(move |_, _, cx| cx.open_url(&u))))
            .into_any_element()
    }
}

impl Render for CheckDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.header(cx);
        let body = self.body(cx);
        div().size_full().flex().flex_col().child(header).child(ui::divider()).child(div().flex_1().min_h_0().child(body))
    }
}

// MARK: - Helpers

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) }

/// A stable id per row: the context, numbered when several statuses share it (a GitHub status and a check run with
/// one name, matrix jobs), so each opens its own page (§14: the Swift id was the bare context).
fn check_keys(checks: &[Check]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    checks.iter().map(|c| {
        let n = seen.entry(c.id()).or_default();
        *n += 1;
        if *n == 1 { c.id().to_string() } else { format!("{}#{}", c.id(), n) }
    }).collect()
}

fn detail_ident(key: &str, c: &Check) -> String {
    format!("{key}/{}.{}", c.job.as_ref().map_or(0, |j| j.id), c.job.as_ref().and_then(|j| j.attempt).unwrap_or(0))
}

/// The buttons to offer. Reruns of any kind need the whole run finished: the Swift app showed "Re-run Failed Jobs"
/// while a run with an early failure was still going, which the forge refuses (§14 CIForges.swift:57).
fn offered(f: &CiFeatures, run: Option<&str>, job: &str) -> Vec<CiAction> {
    let run_done = CheckState::from_job(run.unwrap_or(job)).is_done();
    f.actions(run, job).into_iter().filter(|a| *a != CiAction::RerunFailed || run_done).collect()
}

fn visible_lines(lines: &[LogLine], expanded: &HashSet<u32>) -> Vec<usize> {
    (0..lines.len()).filter(|&i| lines[i].group.is_none_or(|g| expanded.contains(&g))).collect()
}

/// ANSI escapes stripped and a progress bar's lone CRs resolved to what a terminal would end on (§14: the Swift
/// view showed raw escape codes).
fn clean(s: &str) -> Cow<'_, str> {
    if !s.contains(['\x1b', '\r']) { return Cow::Borrowed(s) }
    let s = s.trim_end_matches('\r');
    let s = s.rsplit('\r').next().unwrap_or("");
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' { out.push(c); continue }
        match it.next() {
            Some('[') => { for c in it.by_ref() { if ('\x40'..='\x7e').contains(&c) { break } } }
            Some(']') => { while let Some(c) = it.next() { if c == '\x07' || c == '\x1b' && it.next_if_eq(&'\\').is_some() { break } } }
            _ => {}
        }
    }
    Cow::Owned(out)
}

fn display<'a>(l: &'a LogLine, expanded: &HashSet<u32>) -> Cow<'a, str> {
    let t = clean(&l.text);
    match l.header {
        Some(g) if expanded.contains(&g) => Cow::Owned(t.replacen('▸', "▾", 1)),
        _ => t,
    }
}

fn copy_text(l: &JobLog) -> String {
    let mut s = String::new();
    for line in &l.lines {
        s += &display(line, &l.expanded);
        s.push('\n');
    }
    s
}

fn style_color(s: LogStyle) -> (Hsla, bool) {
    match s {
        LogStyle::Plain => (theme::text(), false),
        LogStyle::Header => (theme::text_muted(), true),
        LogStyle::Error => (theme::red(), true),
        LogStyle::Warning => (theme::accent(), false),
        LogStyle::Notice => (theme::blue(), false),
        LogStyle::Debug => (theme::text_faint(), false),
        LogStyle::Command => (theme::text_muted(), false),
        LogStyle::Star => (theme::blue(), true),
        LogStyle::Success => (theme::green(), false),
        LogStyle::Problem => (theme::red(), false),
    }
}

fn size_label(bytes: usize) -> String {
    if bytes >= 1_000_000 { format!("{:.1} MB", bytes as f64 / 1e6) } else { format!("{} KB", bytes.div_ceil(1000)) }
}

/// "Updated 12 s ago". Recomputed whenever the header renders (every poll), not ticking every second: a 1 Hz timer
/// kept the Swift window redrawing (§14 CI.swift:639).
fn updated_label(d: Duration) -> String {
    let s = d.as_secs();
    if s < 5 { "Updated just now".into() } else if s < 60 { format!("Updated {s} s ago") } else { format!("Updated {} min ago", s / 60) }
}

/// "2 minutes ago", like Swift's `.relative(presentation: .named)`.
fn ago_named(secs: i64) -> String {
    let unit = |n: i64, u: &str| format!("{n} {u}{} ago", if n == 1 { "" } else { "s" });
    match secs.max(0) {
        s if s < 60 => unit(s, "second"),
        s if s < 3600 => unit(s / 60, "minute"),
        s if s < 86_400 => unit(s / 3600, "hour"),
        s if s < 2 * 86_400 => "yesterday".into(),
        s => unit(s / 86_400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loupe_forge::models::CommitStatus;

    fn check(ctx: &str, status: &str) -> Check {
        Check { status: CommitStatus { context: ctx.into(), status: status.into(), ..Default::default() }, job: None }
    }

    #[test]
    fn duplicate_contexts_get_their_own_keys() {
        let cs = [check("ci / test", "success"), check("lint", "failure"), check("ci / test", "failure")];
        assert_eq!(check_keys(&cs), ["ci / test", "lint", "ci / test#2"]);
    }

    #[test]
    fn rerun_failed_waits_for_the_run_to_finish() {
        let gh = CiFeatures::detect(true, None);
        assert!(!offered(&gh, Some("running"), "failure").contains(&CiAction::RerunFailed));
        assert!(offered(&gh, Some("failure"), "failure").contains(&CiAction::RerunFailed));
        let fj = CiFeatures::detect(false, Some("17.2.0+gitea-1.22.0"));
        assert_eq!(offered(&fj, Some("failure"), "failure"), [CiAction::RerunJob, CiAction::RerunAll]);
        assert_eq!(offered(&fj, Some("running"), "running"), [CiAction::Cancel]);
    }

    #[test]
    fn strips_ansi_and_resolves_carriage_returns() {
        assert_eq!(clean("plain"), "plain");
        assert_eq!(clean("\x1b[31merror\x1b[0m: x"), "error: x");
        assert_eq!(clean("10%\r55%\r100%"), "100%");
        assert_eq!(clean("line\r"), "line");
        assert_eq!(clean("\x1b]0;title\x07ok"), "ok");
    }

    #[test]
    fn folded_groups_hide_their_bodies() {
        let mut fold = loupe_forge::ci::Fold::default();
        let lines = loupe_forge::ci::format_log("a\n##[group]G\nb\nc\n##[endgroup]\nd\n", &mut fold);
        let mut open = HashSet::new();
        assert_eq!(visible_lines(&lines, &open), [0, 1, 4]);
        assert_eq!(display(&lines[1], &open), "▸ G");
        open.insert(lines[1].header.unwrap());
        assert_eq!(visible_lines(&lines, &open), [0, 1, 2, 3, 4]);
        assert_eq!(display(&lines[1], &open), "▾ G");
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago_named(1), "1 second ago");
        assert_eq!(ago_named(150), "2 minutes ago");
        assert_eq!(ago_named(90_000), "yesterday");
        assert_eq!(updated_label(Duration::from_secs(2)), "Updated just now");
        assert_eq!(updated_label(Duration::from_secs(75)), "Updated 1 min ago");
        assert_eq!(size_label(1_234_567), "1.2 MB");
    }
}
