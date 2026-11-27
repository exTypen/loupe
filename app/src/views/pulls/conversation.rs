//! A pull request's conversation as one timeline (Conversation.swift): the description, comments, reviews with their
//! inline comments and pushes, oldest first. A comment straight after another by the same person (within 30 min)
//! joins its card; a push in between starts a new one. Beside it, on a wide window, reviewers and participants.
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::{
    AnyElement, Context, EntityId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    component::{input::Textarea, tooltip::Tooltip},
};
use loupe_forge::pull::ReviewerState;
use loupe_forge::{IssueComment, PrCommit, PullRequest, Review, ReviewComment, User};

use super::avatar::avatar;
use super::detail::PullDetail;
use super::{markdown, time};
use crate::{theme, ui};

const RAIL: f32 = 36.;
const MAX_WIDTH: f32 = 960.;
const JOIN: i64 = 30 * 60;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Opened,
    Comments(Vec<IssueComment>),
    Review(Review, Vec<ReviewComment>),
    Inline(ReviewComment),
    Commits(Vec<PrCommit>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub id: String, pub date: Option<i64>, pub kind: Kind }

fn date(s: Option<&str>) -> Option<i64> { s.and_then(time::parse) }

/// The timeline, in order, with joined runs.
pub fn build(pr: &PullRequest, conversation: &[IssueComment], reviews: &[Review], comments: &[ReviewComment], commits: &[PrCommit]) -> Vec<Entry> {
    let mut raw = vec![];
    // No created_at: still a description card (the Swift app dropped the description, FLOWS.md §14), dated unknown.
    raw.push(Entry { id: "opened".into(), date: date(pr.created_at.as_deref()), kind: Kind::Opened });
    for c in conversation { raw.push(Entry { id: format!("c{}", c.id), date: date(Some(&c.created_at)), kind: Kind::Comments(vec![c.clone()]) }) }
    let mut by_review: HashMap<i64, Vec<ReviewComment>> = HashMap::new();
    for c in comments { if let Some(r) = c.pull_request_review_id { by_review.entry(r).or_default().push(c.clone()) } }
    for r in reviews {
        let inline = by_review.get(&r.id).cloned().unwrap_or_default();
        // A bare "commented" review with nothing in it says nothing.
        if r.body.is_empty() && r.state == "COMMENT" && inline.is_empty() { continue }
        raw.push(Entry { id: format!("r{}", r.id), date: date(r.submitted_at.as_deref()), kind: Kind::Review(r.clone(), inline) });
    }
    let ids: HashSet<i64> = reviews.iter().map(|r| r.id).collect();
    for c in comments.iter().filter(|c| c.pull_request_review_id.is_none_or(|r| !ids.contains(&r))) {
        raw.push(Entry { id: format!("i{}", c.id), date: date(c.created_at.as_deref()), kind: Kind::Inline(c.clone()) });
    }
    for c in commits {
        let d = c.commit.committer.as_ref().and_then(|s| s.date.as_deref()).or(c.commit.author.as_ref().and_then(|s| s.date.as_deref()));
        raw.push(Entry { id: format!("k{}", c.sha), date: date(d), kind: Kind::Commits(vec![c.clone()]) });
    }
    // Stable, and the opening post stays first even when a commit is older than the pull request.
    let mut ix: Vec<usize> = (0..raw.len()).collect();
    ix.sort_by_key(|&i| (!matches!(raw[i].kind, Kind::Opened), raw[i].date.unwrap_or(i64::MIN), i));
    let mut out: Vec<Entry> = vec![];
    for i in ix {
        let e = raw[i].clone();
        if let Some(last) = out.last_mut() {
            match (&mut last.kind, &e.kind) {
                (Kind::Commits(a), Kind::Commits(b)) => { a.extend(b.iter().cloned()); continue }
                (Kind::Comments(a), Kind::Comments(b)) if a.last().and_then(|c| c.user.as_ref().map(|u| &u.login)) == b[0].user.as_ref().map(|u| &u.login)
                    && e.date.unwrap_or(i64::MIN) - date(a.last().map(|c| c.created_at.as_str())).unwrap_or(i64::MIN) < JOIN => {
                    a.extend(b.iter().cloned());
                    continue;
                }
                _ => {}
            }
        }
        out.push(e);
    }
    out
}

thread_local! { static MEMO: RefCell<HashMap<EntityId, (u64, Rc<Vec<Entry>>)>> = RefCell::new(HashMap::new()); }

/// Rebuilt only when the pull request's data changed.
fn entries(d: &PullDetail, id: EntityId) -> Rc<Vec<Entry>> {
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if let Some((v, e)) = m.get(&id) && *v == d.data_version { return e.clone() }
        let e = Rc::new(build(&d.pr, &d.conversation, &d.reviews, &d.comments, &d.commits));
        if m.len() > 8 { m.clear() }
        m.insert(id, (d.data_version, e.clone()));
        e
    })
}

fn is_bot(login: &str) -> bool { login.ends_with("[bot]") || login.to_lowercase().ends_with("bot") }

fn tag(text: &str) -> AnyElement {
    div().flex_none().px(px(5.)).rounded_full().border_1().border_color(theme::border_strong()).text_size(px(10.5))
        .text_color(theme::text_muted()).child(text.to_string()).into_any_element()
}

/// Review state → header words, tint and avatar mark. "Requested changes" is red here as in the header badge (the
/// Swift app had it red there and orange here, FLOWS.md §14).
fn review_style(state: &str) -> (&'static str, Option<Hsla>, Option<&'static str>) {
    match state {
        "APPROVED" => ("approved these changes", Some(theme::green()), Some("check")),
        "REQUEST_CHANGES" => ("requested changes", Some(theme::red()), Some("circle-alert")),
        "REQUEST_REVIEW" => ("was asked to review", None, None),
        _ => ("reviewed", None, None),
    }
}

struct Header<'a> { user: &'a User, action: &'a str, date: Option<i64>, author: bool, tint: Option<Hsla>, outdated: bool }

fn header(h: Header, now: i64) -> AnyElement {
    div().flex().items_center().gap_1p5().min_w_0().text_size(px(12.5))
        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(h.user.login.clone()))
        .when(is_bot(&h.user.login), |d| d.child(tag("bot")))
        .when(h.author, |d| d.child(tag("Author")))
        .child(div().min_w_0().truncate().text_color(h.tint.unwrap_or(theme::text_muted())).child(h.action.to_string()))
        .when(h.outdated, |d| d.child(div().id("earlier").child(tag("earlier commit"))
            .tooltip(|w, cx| Tooltip::new("Made on a commit that's no longer the head of this pull request").build(w, cx))))
        .child(div().flex_1())
        .when_some(h.date, |d, t| {
            let full: SharedString = time::full(t).into();
            d.child(div().id(SharedString::from(format!("date-{t}"))).flex_none().text_size(px(11.5)).text_color(theme::text_muted())
                .child(time::relative(t, now)).tooltip(move |w, cx| Tooltip::new(full.clone()).build(w, cx)))
        })
        .into_any_element()
}

fn card(head: AnyElement, tint: Option<Hsla>, body: Vec<AnyElement>) -> AnyElement {
    let accent = tint.unwrap_or(theme::text());
    div().flex().flex_col().flex_1().min_w_0().rounded(px(8.)).border_1()
        .border_color(if tint.is_some() { accent.opacity(0.35) } else { theme::border() }).bg(theme::card())
        .child(div().px_3().py_2().rounded_t(px(8.)).bg(accent.opacity(if tint.is_some() { 0.09 } else { 0.03 }))
            .border_b_1().border_color(theme::border()).child(head))
        .child(div().flex().flex_col().gap_2p5().p_3().children(body))
        .into_any_element()
}

fn card_row(user: &User, mark: Option<(&'static str, Hsla)>, content: AnyElement, me: EntityId, cx: &mut gpui_kit::App) -> AnyElement {
    div().flex().items_start().gap_3().py_2()
        .child(div().w(px(RAIL - 4.)).flex_none().flex().justify_center().pt_0p5().relative().child(avatar(user, 32., me, cx))
            .when_some(mark, |d, (icon, c)| d.child(div().absolute().bottom(px(-4.)).right(px(-2.)).size(px(15.)).rounded_full().bg(c)
                .border_2().border_color(theme::canvas()).flex().items_center().justify_center()
                .child(ui::icon(icon).size(px(8.)).text_color(gpui_kit::white())))))
        .child(content)
        .into_any_element()
}

/// The last lines of a review comment's hunk: the code it points at.
fn hunk_tail(hunk: &str) -> AnyElement {
    let lines: Vec<&str> = hunk.split('\n').filter(|l| !l.starts_with("@@")).collect();
    let tail = &lines[lines.len().saturating_sub(4)..];
    div().flex().flex_col().py_1().bg(theme::canvas()).font_family(theme::CODE_FONT).text_size(px(11.))
        .children(tail.iter().map(|l| {
            let mark = l.chars().next().unwrap_or(' ');
            let (bg, c, m) = match mark { '+' => (Some(theme::added_bg()), theme::green(), "+"), '-' => (Some(theme::removed_bg()), theme::red(), "−"), _ => (None, theme::text_faint(), " ") };
            div().flex().gap_1p5().px_2p5().when_some(bg, |d, b| d.bg(b))
                .child(div().w(px(8.)).flex_none().text_color(c).child(m))
                .child(div().whitespace_nowrap().overflow_hidden().text_color(theme::text()).child(l.get(1..).unwrap_or("").to_string()))
        }))
        .into_any_element()
}

fn inline_comment(c: &ReviewComment, head: &str, d: &PullDetail, cx: &mut Context<PullDetail>) -> AnyElement {
    let line = if c.position > 0 { c.position } else { c.original_position };
    let (path, open_line) = (c.path.clone(), (c.position > 0).then_some(c.position as usize));
    let me = cx.entity_id();
    let user = c.user.clone().unwrap_or(User { login: "?".into(), avatar_url: None });
    let _ = d;
    div().flex().flex_col().rounded(px(6.)).border_1().border_color(theme::border()).overflow_hidden()
        .child(div().id(SharedString::from(format!("ic-{}", c.id))).flex().items_center().gap_1p5().px_2p5().py_1p5().cursor_pointer()
            .hover(|s| s.bg(theme::hover()))
            .tooltip(|w, cx| Tooltip::new("Open in Files").build(w, cx))
            .on_click(cx.listener(move |d, _, w, cx| d.reveal(&path, open_line, w, cx)))
            .child(ui::file_icon(&c.path))
            .child(div().min_w_0().text_ellipsis_start().whitespace_nowrap().overflow_hidden().font_family(theme::CODE_FONT).text_size(px(11.)).child(c.path.clone()))
            .when(line > 0, |d| d.child(div().flex_none().text_size(px(11.)).text_color(theme::text_muted()).child(format!("line {line}"))))
            .when(c.commit_id.as_deref().is_some_and(|s| s != head), |d| d.child(tag("outdated")))
            .child(div().flex_1())
            .child(ui::icon("arrow-up-right").size(px(11.)).text_color(theme::text_faint())))
        .when_some(c.diff_hunk.as_deref().filter(|h| !h.is_empty()), |d, h| d.child(div().border_t_1().border_color(theme::border()).child(hunk_tail(h))))
        .child(div().flex().items_start().gap_2().p_2p5().border_t_1().border_color(theme::border())
            .child(avatar(&user, 18., me, cx))
            .child(div().flex().flex_col().gap_0p5().flex_1().min_w_0()
                .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(12.5)).child(user.login.clone()))
                .child(markdown::render(&c.body, false))))
        .into_any_element()
}

fn row(e: &Entry, d: &PullDetail, now: i64, cx: &mut Context<PullDetail>) -> AnyElement {
    let me = cx.entity_id();
    let pr = &d.pr;
    let unknown = || User { login: "?".into(), avatar_url: None };
    match &e.kind {
        Kind::Opened => {
            let body = pr.body.as_deref().filter(|b| !b.trim().is_empty())
                .map(|b| markdown::render(b, false))
                .unwrap_or_else(|| div().italic().text_color(theme::text_muted()).child("No description provided.").into_any_element());
            let h = header(Header { user: &pr.user, action: "opened this pull request", date: e.date, author: false, tint: None, outdated: false }, now);
            card_row(&pr.user, None, card(h, None, vec![body]), me, cx)
        }
        Kind::Comments(cs) => {
            let user = cs[0].user.clone().unwrap_or_else(unknown);
            let h = header(Header { user: &user, action: "commented", date: e.date, author: user.login == pr.user.login, tint: None, outdated: false }, now);
            let mut body = vec![];
            for (k, c) in cs.iter().enumerate() {
                if k > 0 {
                    body.push(div().flex().items_center().gap_2().child(div().flex_1().h(px(1.)).bg(theme::border()))
                        .child(div().text_size(px(10.5)).text_color(theme::text_faint()).child(time::parse(&c.created_at).map(time::clock).unwrap_or_default()))
                        .into_any_element());
                }
                body.push(markdown::render(&c.body, false));
            }
            card_row(&user, None, card(h, None, body), me, cx)
        }
        Kind::Review(r, inline) => {
            let user = r.user.clone().unwrap_or_else(unknown);
            let (action, tint, icon) = review_style(&r.state);
            let outdated = r.commit_id.as_deref().is_some_and(|c| c != pr.head.sha);
            let h = header(Header { user: &user, action, date: e.date, author: user.login == pr.user.login, tint, outdated }, now);
            let mut body = vec![];
            if !r.body.trim().is_empty() { body.push(markdown::render(&r.body, false)) }
            for c in inline { body.push(inline_comment(c, &pr.head.sha, d, cx)) }
            card_row(&user, icon.zip(tint), card(h, tint, body), me, cx)
        }
        Kind::Inline(c) => {
            let user = c.user.clone().unwrap_or_else(unknown);
            let h = header(Header { user: &user, action: "commented on a line", date: e.date, author: user.login == pr.user.login, tint: None, outdated: false }, now);
            let body = vec![inline_comment(c, &pr.head.sha, d, cx)];
            card_row(&user, None, card(h, None, body), me, cx)
        }
        Kind::Commits(cs) => {
            let last = cs.last();
            let who = last.and_then(|c| c.author.clone()).unwrap_or_else(|| User {
                login: last.and_then(|c| c.commit.author.as_ref()?.name.clone()).unwrap_or_else(|| "?".into()), avatar_url: None });
            div().flex().items_start().gap_3().py_2()
                .child(div().w(px(RAIL - 4.)).flex_none().flex().justify_center()
                    .child(div().size(px(22.)).rounded_full().bg(theme::canvas()).border_1().border_color(theme::border_strong())
                        .flex().items_center().justify_center().child(ui::icon("git-commit-horizontal").size(px(12.)))))
                .child(div().flex().flex_col().gap_1().flex_1().min_w_0().pt_0p5()
                    .child(div().flex().items_center().gap_1p5().text_size(px(12.5))
                        .child(avatar(&who, 16., me, cx))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(who.login.clone()))
                        .child(div().text_color(theme::text_muted()).child(if cs.len() == 1 { "added a commit".to_string() } else { format!("added {} commits", cs.len()) }))
                        .child(div().flex_1())
                        .when_some(e.date, |d, t| d.child(div().text_size(px(11.5)).text_color(theme::text_muted()).child(time::relative(t, now)))))
                    .children(cs.iter().map(|c| {
                        let msg: SharedString = c.commit.message.clone().into();
                        div().id(SharedString::from(format!("commit-{}", c.sha))).flex().gap_2().text_size(px(12.5))
                            .tooltip(move |w, cx| Tooltip::new(msg.clone()).build(w, cx))
                            .child(div().flex_none().font_family(theme::CODE_FONT).text_size(px(11.5)).text_color(theme::text_muted()).child(c.sha.chars().take(7).collect::<String>()))
                            .child(div().min_w_0().truncate().child(c.commit.message.lines().next().unwrap_or("").to_string()))
                    })))
                .into_any_element()
        }
    }
}

fn participants(entries: &[Entry], pr: &PullRequest) -> Vec<String> {
    let mut s = BTreeSet::new();
    for e in entries {
        match &e.kind {
            Kind::Opened => { s.insert(pr.user.login.clone()); }
            Kind::Comments(cs) => s.extend(cs.iter().filter_map(|c| c.user.as_ref().map(|u| u.login.clone()))),
            Kind::Review(r, i) => { s.extend(r.user.as_ref().map(|u| u.login.clone())); s.extend(i.iter().filter_map(|c| c.user.as_ref().map(|u| u.login.clone()))) }
            Kind::Inline(c) => s.extend(c.user.as_ref().map(|u| u.login.clone())),
            Kind::Commits(cs) => s.extend(cs.iter().filter_map(|c| c.author.as_ref().map(|u| u.login.clone()))),
        }
    }
    s.into_iter().collect()
}

/// The rail the avatars sit on: drawn per row (rows touch), so the scroll view's children stay the entries the
/// sidebar jumps to.
fn rail() -> gpui_kit::Div {
    div().absolute().top_0().bottom_0().left(px(20. + RAIL / 2. - 1.)).w(px(2.)).bg(theme::border())
}

pub fn render(d: &mut PullDetail, window: &mut Window, cx: &mut Context<PullDetail>) -> AnyElement {
    let me = cx.entity_id();
    let es = entries(d, me);
    let now = time::now();
    // Child index of each entry in the scroll view (day markers sit between), for the sidebar's jumps.
    let mut children: Vec<AnyElement> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    for (i, e) in es.iter().enumerate() {
        if i == 0 || !time::same_day(es[i - 1].date, e.date) {
            children.push(div().w_full().max_w(px(MAX_WIDTH)).px_5().py_2p5().relative().child(rail()).child(div().flex().ml(px(RAIL / 2. - 20.))
                .child(div().px_2().rounded_full().bg(theme::canvas()).border_1().border_color(theme::border())
                    .text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_muted()).child(time::day_marker(e.date, now))))
                .into_any_element());
        }
        at.insert(e.id.clone(), children.len());
        children.push(div().w_full().max_w(px(MAX_WIDTH)).px_5().relative().child(rail()).child(row(e, d, now, cx)).into_any_element());
    }
    children.push(div().h(px(16.)).into_any_element());
    // It opens at the newest, and a posted comment shows: scroll to the end (applied in this frame's prepaint,
    // after the new rows are laid out).
    if !d.conv_at_bottom {
        d.conv_at_bottom = true;
        d.conv_scroll.scroll_to_bottom();
    }
    let sending = d.composer.sending;
    let can_send = !sending && !d.composer.input.read(cx).value().trim().is_empty();
    let timeline = div().flex().flex_col().flex_1().min_w_0().h_full()
        .child(div().id("conversation").track_scroll(&d.conv_scroll).flex().flex_col().items_center().flex_1().min_h_0().overflow_y_scroll().pt_4()
            .children(children))
        .child(ui::divider())
        .child(div().w_full().flex().justify_center().child(div().w_full().max_w(px(MAX_WIDTH)).px_5().py_3().flex().flex_col().gap_2()
            .child(div().min_h(px(64.)).max_h(px(220.)).px_2p5().py_2().rounded(px(8.)).bg(theme::card()).border_1().border_color(theme::border_strong())
                .child(Textarea::new(&d.composer.input).appearance(false).h(px(56.))))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().text_size(px(11.5)).text_color(theme::text_faint()).child("Markdown · ⌘↩ to send"))
                .when(sending, |d| d.child(ui::icon("loader").text_color(theme::text_faint())))
                .child(ui::pill_primary("comment-send", "Comment", theme::blue()).when(!can_send, |d| d.opacity(0.4))
                    .when(can_send, |d| d.on_click(cx.listener(|d, _, _, cx| d.post_comment(cx))))))));
    // Past a readable line length the room goes to what a reviewer checks beside the thread (960 + 290).
    let wide = window.viewport_size().width - px(260.) >= px(MAX_WIDTH + 290.);
    if !wide { return div().flex().size_full().child(timeline).into_any_element() }
    let states = d.reviewer_states();
    let users = d.users();
    let latest: HashMap<String, usize> = es.iter().filter_map(|e| match &e.kind {
        Kind::Review(r, _) => r.user.as_ref().map(|u| (u.login.clone(), at[&e.id])), _ => None }).collect();
    let user = |l: &str| users.get(l).cloned().unwrap_or(User { login: l.into(), avatar_url: None });
    let section = |title: &str| div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_muted()).child(title.to_string());
    let mut reviewers = div().flex().flex_col().gap_2().child(section("Reviewers"));
    if states.is_empty() { reviewers = reviewers.child(div().text_color(theme::text_muted()).child("No reviews yet")) }
    for (login, s) in &states {
        let target = latest.get(login).copied();
        let (label, icon, color) = match s {
            ReviewerState::Approved => ("Approved", "circle-check", theme::green()),
            ReviewerState::ChangesRequested => ("Changes", "circle-alert", theme::red()),
            ReviewerState::Pending => ("Pending", "circle-dashed", theme::text_muted()),
        };
        let h = d.conv_scroll.clone();
        let tip: SharedString = format!("Go to {login}'s review").into();
        reviewers = reviewers.child(div().id(SharedString::from(format!("rv-{login}"))).flex().items_center().gap_2().text_size(px(12.5))
            .when_some(target, |d, t| d.cursor_pointer().hover(|s| s.bg(theme::hover())).on_click(move |_, _, cx| { h.scroll_to_top_of_item(t); cx.refresh_windows() })
                .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx)))
            .when(target.is_none(), |d| d.opacity(0.6))
            .child(avatar(&user(login), 20., me, cx))
            .child(div().flex_1().min_w_0().truncate().child(login.clone()))
            .child(div().flex().items_center().gap_1().text_color(color).child(ui::icon(icon).text_color(color)).child(label)));
    }
    let people = participants(&es, &d.pr);
    let sidebar = div().id("conv-sidebar").w(px(290.)).flex_none().h_full().overflow_y_scroll().border_l_1().border_color(theme::border()).p_4()
        .flex().flex_col().gap(px(22.))
        .child(reviewers)
        .child(div().flex().flex_col().gap_2().child(section("Participants"))
            .child(div().flex().flex_wrap().gap_1p5().children(people.iter().map(|l| avatar(&user(l), 28., me, cx)))));
    div().flex().size_full().child(timeline).child(sidebar).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use loupe_forge::{CommitDetail, Signature};

    fn u(l: &str) -> Option<User> { Some(User { login: l.into(), avatar_url: None }) }
    fn c(id: i64, who: &str, at: &str) -> IssueComment { IssueComment { id, user: u(who), body: format!("c{id}"), created_at: at.into() } }

    #[test]
    fn orders_joins_and_keeps_the_opened_card_first() {
        let pr = PullRequest { created_at: Some("2026-11-20T10:00:00Z".into()), ..Default::default() };
        let conv = [c(1, "bob", "2026-11-20T11:00:00Z"), c(2, "bob", "2026-11-20T11:10:00Z"), c(3, "bob", "2026-11-20T12:00:00Z")];
        let commit = PrCommit { sha: "abc".into(), commit: CommitDetail { message: "m".into(), author: None,
            committer: Some(Signature { name: None, date: Some("2026-11-19T09:00:00Z".into()) }) }, author: u("alice") };
        let empty_review = Review { id: 9, user: u("carol"), state: "COMMENT".into(), ..Default::default() };
        let orphan = ReviewComment { id: 5, path: "a".into(), pull_request_review_id: Some(77), created_at: Some("2026-11-20T13:00:00Z".into()), ..Default::default() };
        let e = build(&pr, &conv, &[empty_review], &[orphan], &[commit]);
        let ids: Vec<&str> = e.iter().map(|e| e.id.as_str()).collect();
        // The commit is older than the PR but the opened card stays first; 11:00 and 11:10 join, 12:00 doesn't
        // (50 min after the last of the run); the empty COMMENT review is left out; the orphan inline stands alone.
        assert_eq!(ids, ["opened", "kabc", "c1", "c3", "i5"]);
        assert!(matches!(&e[2].kind, Kind::Comments(cs) if cs.len() == 2));
        assert_eq!(participants(&e, &pr), ["", "alice", "bob"]);
    }
}
