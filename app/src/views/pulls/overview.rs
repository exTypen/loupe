//! The PR's Overview tab (PROverview in PullRequestsView.swift): fetch the head, find the merge base and the base
//! branch's tip, then loupe-code's lenses over merge-base..head. Each fact that points at a file opens it in Files.
//!
//! The shared ChangeOverview view belongs to the Code agent; until it lands this draws the same lens reports as
//! plain cards (title, summary, sections of facts).
use std::sync::Arc;

use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, div, prelude::FluentBuilder as _, px,
};
use loupe_code::lens::{self, ChangedPath, FactKind, LensReport, Snapshot};
use loupe_forge::pull;

use super::detail::PullDetail;
use crate::{theme, ui};

#[derive(Default)]
pub struct State {
    key: Option<String>,
    reports: Option<Arc<Vec<LensReport>>>,
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
            pull::fetch_head(&pr, &local, None)?;
            let dir = &local.dir;
            // The base branch as last fetched, which may be further on than the PR's own record of it.
            let tracking = format!("refs/remotes/{}/{}^{{commit}}", local.remote, pr.base.ref_);
            let tip = loupe_forge::git::string(&["rev-parse", "--verify", "-q", &tracking], dir, None).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let target = tip.unwrap_or_else(|| pr.base.sha.clone());
            let base = loupe_forge::git::string(&["merge-base", &pr.base.sha, &pr.head.sha], dir, None)?.trim().to_string();
            Ok::<_, loupe_forge::Error>(lens::analyze(&changes, &Snapshot::Rev(base), &Snapshot::Rev(pr.head.sha.clone()), Some(&target), dir))
        }).await;
        this.update(cx, |d, cx| {
            d.overview.task = None;
            match r {
                Ok(r) => { d.overview.reports = Some(Arc::new(r)); d.overview.error = None }
                Err(e) => d.overview.error = e.user_message().or(d.overview.error.take()),
            }
            cx.notify();
        }).ok();
    }));
}

fn kind_color(k: FactKind) -> gpui_kit::Hsla {
    match k { FactKind::Added => theme::green(), FactKind::Removed => theme::red(), FactKind::Changed | FactKind::Renamed => theme::blue(),
              FactKind::Warning => theme::accent(), FactKind::Info => theme::text_muted() }
}

pub fn render(d: &mut PullDetail, cx: &mut Context<PullDetail>) -> AnyElement {
    start(d, cx);
    let Some(reports) = d.overview.reports.clone() else {
        return match &d.overview.error {
            Some(e) => ui::empty_state("triangle-alert", "Can't Read the Pull Request", e.clone()).into_any_element(),
            None => ui::empty_state("loader", "Fetching the pull request…", "").into_any_element(),
        };
    };
    let mut n = 0;
    div().id("pr-overview").size_full().overflow_y_scroll().p_5().flex().flex_col().items_center().gap_4()
        .children(reports.iter().map(|r| {
            div().w_full().max_w(px(960.)).child(ui::card()
                .child(ui::card_header(div().flex().gap_2().child(div().font_weight(FontWeight::SEMIBOLD).child(r.title.clone()))
                    .child(div().text_color(theme::text_muted()).child(r.summary.clone())), div()))
                .child(div().flex().flex_col().gap_3().p_3().children(r.sections.iter().map(|s| {
                    div().flex().flex_col().gap_1()
                        .when(!s.title.is_empty(), |d| d.child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_muted()).child(s.title.clone())))
                        .children(s.facts.iter().map(|f| {
                            n += 1;
                            let target = f.path.clone().map(|p| (p, f.line));
                            div().id(SharedString::from(format!("fact-{n}"))).flex().items_center().gap_2().px_1p5().py_0p5().rounded(px(5.)).text_size(px(12.5))
                                .when_some(target, |d, (p, l)| d.cursor_pointer().hover(|s| s.bg(theme::hover()))
                                    .on_click(cx.listener(move |d, _, w, cx| d.reveal(&p, l, w, cx))))
                                .child(ui::badge_tinted(f.kind.label(), kind_color(f.kind)))
                                .child(div().min_w_0().truncate().child(f.title.clone()))
                                .when_some(f.detail.clone(), |d, t| d.child(div().min_w_0().truncate().text_color(theme::text_muted()).child(t)))
                                .when_some(f.path.clone(), |d, p| d.child(div().flex_1()).child(div().flex_none().max_w(px(320.)).text_ellipsis_start()
                                    .whitespace_nowrap().overflow_hidden().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_faint())
                                    .child(match f.line { Some(l) => format!("{p}:{l}"), None => p })))
                        }))
                }))))
        }))
        .into_any_element()
}
