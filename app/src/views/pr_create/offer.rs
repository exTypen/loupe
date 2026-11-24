//! Whether to offer a pull request for a worktree's branch (PullCreate.swift's PullCreateBanner.check and
//! `aheadOfDefault`), shared by the Changes banner, the Pull Requests toolbar button and the sidebar menu item.
//!
//! One rule for all three (the Swift app had three: the toolbar ignored `isMerged` and only looked at the loaded
//! list, and the sidebar item checked nothing, FLOWS.md §14): the branch is set, there's a client, it has commits
//! the default branch hasn't, it isn't merged, and the forge has no open PR for it. Checked once per
//! "HEAD refs-version branch client merged" key (vault: 1d10383, "checked once per HEAD"); a key change cancels the
//! check in flight. Nothing polls.
use std::collections::HashMap;

use gpui_kit::{
    App, AnyElement, Entity, FontWeight, Global, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, div, px,
};
use loupe_forge::{Cancel, pull_create, settings};

use crate::state::{AppEvent, WorktreeModel};
use crate::{theme, ui};

/// N commits on `branch` that aren't on `target` (the default branch, without "<remote>/").
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub ahead: usize,
    pub target: String,
    pub branch: String,
}

struct Entry {
    key: String,
    /// None while the check runs.
    result: Option<Option<Offer>>,
    cancel: Option<Cancel>,
    _task: Option<Task<()>>,
}

#[derive(Default)]
struct Offers(HashMap<String, Entry>);
impl Global for Offers {}

/// The offer for this worktree: Some when a PR can be created. Starts the check when its key moved (the model is
/// notified when it lands, so whatever renders from the model renders again); None until then.
pub fn offer(model: &Entity<WorktreeModel>, cx: &mut App) -> Option<Offer> {
    known(model, cx).flatten()
}

/// Some(answer) once checked for the current key; None while unknown. Starts the check when needed.
fn known(model: &Entity<WorktreeModel>, cx: &mut App) -> Option<Option<Offer>> {
    let m = model.read(cx);
    let path = m.path_str();
    let Some(branch) = m.state.branch.clone() else { return Some(None) };
    let (Some(app), Some(remote)) = (m.app.upgrade(), m.remote.clone()) else { return Some(None) };
    let client = app.read(cx).client_for(&remote, &path);
    let key = format!("{} {} {branch} {} {}", m.state.head_oid, m.state.refs_version, client.is_some(), m.state.is_merged);
    let (merged, remote_name) = (m.state.is_merged, m.state.remote_name.clone());
    if !cx.has_global::<Offers>() { cx.set_global(Offers::default()) }
    if let Some(e) = cx.global::<Offers>().0.get(&path) && e.key == key { return e.result.clone() }
    if let Some(c) = cx.global_mut::<Offers>().0.remove(&path).and_then(|e| e.cancel) { c.cancel() }
    let Some(client) = client.filter(|_| !merged) else {
        cx.global_mut::<Offers>().0.insert(path, Entry { key, result: Some(None), cancel: None, _task: None });
        return Some(None);
    };
    let cancel = Cancel::new();
    let (c, p, k, weak) = (cancel.clone(), path.clone(), key.clone(), model.downgrade());
    let task = cx.spawn(async move |cx| {
        let dir = p.clone();
        let r = cx.background_executor().spawn(async move {
            let (ahead, target) = pull_create::ahead_of_default(&dir, &remote_name, &branch)?;
            // Offline or no access: say nothing rather than offer something that will fail.
            match client.with_cancel(&c).open_pull(&remote, &branch) {
                Ok(None) => Some(Offer { ahead, target, branch }),
                _ => None,
            }
        }).await;
        cx.update(|cx| {
            let Some(e) = cx.global_mut::<Offers>().0.get_mut(&p).filter(|e| e.key == k) else { return };
            e.result = Some(r);
            e.cancel = None;
            if let Some(m) = weak.upgrade() { m.update(cx, |_, cx| cx.notify()) }
        });
    });
    cx.global_mut::<Offers>().0.insert(path, Entry { key, result: None, cancel: Some(cancel), _task: Some(task) });
    None
}

/// Check again (Swift's `pushes += 1`): the Create sheet for this worktree closed, or something pushed.
pub fn invalidate(path: &str, cx: &mut App) {
    if !cx.has_global::<Offers>() { return }
    if let Some(c) = cx.global_mut::<Offers>().0.remove(path).and_then(|e| e.cancel) { c.cancel() }
}

/// The Pull Requests toolbar's "Create Pull Request…" (PC-05): the same rule as the banner, so a filtered or paged
/// list, or a merged branch, no longer shows it wrongly.
pub fn can_create(model: &Entity<WorktreeModel>, cx: &mut App) -> bool { offer(model, cx).is_some() }

/// The sidebar worktree menu's "Create Pull Request…" (SH-20/PC-06). A menu is built synchronously, so while the
/// answer isn't known yet it shows as in Swift (branch and client); once known it's hidden when there's nothing to
/// create or a PR is already open.
pub fn show_menu_item(model: &Entity<WorktreeModel>, cx: &mut App) -> bool {
    let m = model.read(cx);
    let (Some(_), Some(app), Some(remote)) = (m.state.branch.as_ref(), m.app.upgrade(), m.remote.as_ref()) else { return false };
    if app.read(cx).client_for(remote, &m.path_str()).is_none() { return false }
    known(model, cx).is_none_or(|o| o.is_some())
}

// MARK: - The Changes banner (PC-01..04)

const DISMISSED: &str = "prBannerDismissed"; // "<path>\t<branch>" lines, the Swift key

fn dismissed(key: &str) -> bool {
    settings::shared().string(DISMISSED).is_some_and(|s| s.split('\n').any(|l| l == key))
}

fn set_dismissed(key: &str, on: bool) {
    let mut p = settings::shared();
    let list = with_line(&p.string(DISMISSED).unwrap_or_default(), key, on);
    p.set_string(DISMISSED, &list);
}

/// The newline-separated list with `key` added (once, at the end) or removed.
fn with_line(list: &str, key: &str, on: bool) -> String {
    let mut lines: Vec<&str> = list.split('\n').filter(|l| !l.is_empty() && *l != key).collect();
    if on { lines.push(key) }
    lines.join("\n")
}

/// Opening the sheet for a dismissed branch brings its banner back: the Swift dismissal was permanent with no way
/// to undo it (§14, PullCreate.swift:403), and asking for a PR is the clearest sign the offer is wanted.
pub(super) fn undismiss(path: &str, branch: &str) {
    let key = format!("{path}\t{branch}");
    if dismissed(&key) { set_dismissed(&key, false) }
}

/// The "Create a pull request" banner the Changes tab shows above its list (instead of the operation banner, which
/// wins while a merge or rebase runs or there are conflicts). None when there's nothing to offer.
///
/// The Swift banner never appeared: its `.task` hung off an empty `Group`, which SwiftUI never shows, so the check
/// never ran (no request reached the forge). This one runs.
pub fn banner(model: &Entity<WorktreeModel>, cx: &mut App) -> Option<AnyElement> {
    let o = offer(model, cx)?;
    let m = model.read(cx);
    let path = m.path_str();
    let key = format!("{path}\t{}", o.branch);
    if dismissed(&key) { return None }
    let app = m.app.upgrade()?;
    let n = o.ahead;
    let text = format!("{} on {} that {} on {}", if n == 1 { "1 commit".into() } else { format!("{n} commits") }, o.branch,
                       if n == 1 { "isn't" } else { "aren't" }, o.target);
    let tip: SharedString = format!("{} → {}", o.branch, o.target).into();
    let weak = model.downgrade();
    Some(div().flex().flex_col().flex_none()
        .child(div().flex().items_center().gap(px(10.)).px_3().py_2().bg(theme::blue().opacity(0.08)).text_size(px(12.5))
            .child(ui::icon("git-pull-request-arrow").text_color(theme::blue()))
            .child(div().id("pr-banner-text").flex_1().min_w_0().truncate().text_color(theme::text()).child(text)
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)))
            .child(ui::pill_primary("pr-banner-create", "Create Pull Request…", theme::blue()).h(px(24.)).font_weight(FontWeight::MEDIUM)
                .on_click(move |_, _, cx| app.update(cx, |a, cx| a.emit(AppEvent::CreatePull { worktree: path.clone() }, cx))))
            .child(ui::icon_button("pr-banner-dismiss", "x").size(px(22.))
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Don't offer a pull request for this branch again").build(w, cx))
                .on_click(move |_, _, cx| {
                    set_dismissed(&key, true);
                    if let Some(m) = weak.upgrade() { m.update(cx, |_, cx| cx.notify()) }
                })))
        .child(ui::divider())
        .into_any_element())
}

#[cfg(test)]
mod tests {
    #[test]
    fn dismissed_lines() {
        let k = "/r/app\tfeat/a";
        assert_eq!(super::with_line("", k, true), k);
        assert_eq!(super::with_line("/r/b\tx", k, true), format!("/r/b\tx\n{k}"));
        assert_eq!(super::with_line(&format!("{k}\n/r/b\tx"), k, true), format!("/r/b\tx\n{k}"));
        assert_eq!(super::with_line(&format!("/r/b\tx\n{k}"), k, false), "/r/b\tx");
    }
}
