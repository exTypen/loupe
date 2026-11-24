//! New Worktree (AddWorktreeSheet in App.swift), Remove Worktree and Prune Stale Worktrees.
//!
//! Fixed against the Swift app (FLOWS.md §14): an existing "origin/x" with a local x checks out x instead of failing
//! on `-b x`; the suggested folder drops the remote's name ("upstream/x" → "<repo>-x") and no longer cuts "origin-"
//! out of the middle of a branch ("fix-origin-check"); Force Remove passes `-f -f`, so a locked worktree goes too;
//! a prune failure shows instead of being swallowed.
use std::path::Path;

use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
    component::{
        IndexPath,
        input::{Input, InputEvent, InputState},
        searchable_list::SearchableVec,
        select::{Select, SelectEvent, SelectState},
    },
};
use loupe_core::git;

use crate::state::AppModel;
use crate::{theme, ui};

/// A branch the sheet offers: "feature/x" (local) or "origin/x" (remote, `local` = "x").
#[derive(Clone, Debug, PartialEq)]
pub struct BranchRef { pub name: String, pub local: Option<String> }

impl BranchRef {
    /// The branch name a worktree for this ref ends up on.
    pub fn local_name(&self) -> &str { self.local.as_deref().unwrap_or(&self.name) }
}

/// Every local and remote branch, newest commit first, without the remotes' HEAD.
pub fn list_branches(repo: &Path) -> Vec<BranchRef> {
    let remotes: Vec<String> = git::string(repo, ["remote"]).unwrap_or_default().lines().map(String::from).collect();
    git::string(repo, ["for-each-ref", "--format=%(refname)", "--sort=-committerdate", "refs/heads", "refs/remotes"])
        .unwrap_or_default().lines().filter(|l| !l.ends_with("/HEAD")).filter_map(|l| parse_ref(l, &remotes)).collect()
}

fn parse_ref(full: &str, remotes: &[String]) -> Option<BranchRef> {
    if let Some(b) = full.strip_prefix("refs/heads/") { return Some(BranchRef { name: b.into(), local: None }) }
    let short = full.strip_prefix("refs/remotes/")?;
    // The longest remote name that prefixes it: a remote may itself contain "/".
    let local = remotes.iter().filter(|r| short.starts_with(&format!("{r}/"))).max_by_key(|r| r.len())
        .map(|r| short[r.len() + 1..].to_string())
        .or_else(|| short.split_once('/').map(|x| x.1.to_string()));
    Some(BranchRef { name: short.into(), local })
}

/// Beside the repository, "<repo>-<branch with / → ->"; "<repo>-worktree" until a branch is typed.
pub fn suggest_path(repo: &str, branch: &str) -> String {
    let p = Path::new(repo);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug = if branch.is_empty() { "worktree".to_string() } else { branch.replace('/', "-") };
    p.parent().unwrap_or(Path::new("/")).join(format!("{name}-{slug}")).to_string_lossy().into_owned()
}

/// What Create runs: `worktree add -b <new> <path> [<base>]`, `worktree add <path> <local>`, or for a remote branch
/// with no local one `worktree add -b <x> <path> <remote>/<x>`.
pub fn add_args(path: &str, new_branch: Option<(&str, &str)>, existing: Option<&BranchRef>, local_exists: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["worktree".into(), "add".into()];
    match (new_branch, existing) {
        (Some((name, base)), _) => {
            a.extend(["-b".into(), name.into(), path.into()]);
            if !base.is_empty() { a.push(base.into()) }
        }
        (None, Some(r)) if r.local.is_some() && !local_exists => a.extend(["-b".into(), r.local_name().into(), path.into(), r.name.clone()]),
        (None, Some(r)) => a.extend([path.into(), r.local_name().into()]),
        (None, None) => {}
    }
    a
}

impl AppModel {
    /// `git worktree remove [-f -f] <path>` (two: a locked worktree needs both), then the list again; the selection
    /// moves to the repository when it was on the removed one. A failure goes to the window's "Error" alert.
    pub fn remove_worktree_confirmed(&mut self, repo: String, path: String, force: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (r, p) = (repo.clone(), path.clone());
            let res = cx.background_executor().spawn(async move {
                let mut a = vec!["worktree", "remove"];
                if force { a.extend(["-f", "-f"]) }
                a.push(&p);
                git::run(&r, a).map(drop)
            }).await;
            this.update(cx, |m, cx| match res {
                Ok(()) => {
                    loupe_core::model::forget(Path::new(&path));
                    m.refresh_worktrees(repo, cx).detach();
                }
                Err(e) => if let Some(msg) = e.user_message() { m.error = Some(msg); cx.notify() },
            }).ok();
        }).detach();
    }

    /// Header menu › Prune Stale Worktrees: `git worktree prune`, then the list again.
    pub fn prune_worktrees(&mut self, repo: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = repo.clone();
            let res = cx.background_executor().spawn(async move { loupe_core::model::prune_worktrees(Path::new(&r)) }).await;
            this.update(cx, |m, cx| {
                if let Err(e) = res && let Some(msg) = e.user_message() { m.error = Some(msg) }
                m.refresh_worktrees(repo, cx).detach();
            }).ok();
        }).detach();
    }
}

type Picker = Entity<SelectState<SearchableVec<String>>>;

pub struct AddWorktreeSheet {
    app: Entity<AppModel>,
    repo: String,
    initial_base: String,
    new_branch: bool,
    name: Entity<InputState>,
    path: Entity<InputState>,
    path_edited: bool,
    /// A folder the save panel picked, applied on the next render (setting an input's value needs the window).
    pending_path: Option<String>,
    refs: Vec<BranchRef>,
    /// "Based on": HEAD (""), the initial commit when it isn't a branch, then every branch; `base_values` holds what
    /// each row passes to git.
    base: Picker,
    base_values: Vec<String>,
    existing: Picker,
    error: Option<String>,
    working: bool,
    _subs: Vec<Subscription>,
}

/// Opens the sheet for `repo`. `initial_base`: History's "New Worktree from Here" commit, or "".
pub fn open_add_worktree(app: &Entity<AppModel>, repo: String, initial_base: String, window: &mut Window, cx: &mut gpui_kit::App) {
    let a = app.clone();
    let sheet = cx.new(|cx| AddWorktreeSheet::new(a, repo, initial_base, window, cx));
    app.update(cx, |a, cx| a.present(sheet.into(), cx));
}

impl AddWorktreeSheet {
    fn new(app: Entity<AppModel>, repo: String, initial_base: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("feature/my-change"));
        let path = cx.new(|cx| InputState::new(window, cx).default_value(suggest_path(&repo, "")));
        let base = cx.new(|cx| SelectState::new(SearchableVec::new(vec!["HEAD".to_string()]), Some(IndexPath::new(0)), window, cx).searchable(true));
        let existing = cx.new(|cx| SelectState::new(SearchableVec::new(Vec::<String>::new()), None, window, cx).searchable(true));
        name.update(cx, |i, cx| i.focus(window, cx));
        let subs = vec![
            cx.subscribe_in(&name, window, |s, _, e: &InputEvent, w, cx| match e {
                InputEvent::Change => s.suggest(w, cx),
                InputEvent::PressEnter { .. } => s.create(cx),
                _ => {}
            }),
            cx.subscribe(&path, |s, _, e: &InputEvent, cx| match e {
                InputEvent::Change => { s.path_edited = true; cx.notify() }
                InputEvent::PressEnter { .. } => s.create(cx),
                _ => {}
            }),
            cx.subscribe_in(&existing, window, |s, _, _: &SelectEvent<SearchableVec<String>>, w, cx| s.suggest(w, cx)),
            cx.subscribe(&base, |_, _, _: &SelectEvent<SearchableVec<String>>, cx| cx.notify()),
        ];
        let dir = repo.clone();
        cx.spawn_in(window, async move |this, cx| {
            let refs = cx.background_executor().spawn(async move { list_branches(Path::new(&dir)) }).await;
            this.update_in(cx, |s, w, cx| s.set_refs(refs, w, cx)).ok();
        }).detach();
        Self { app, repo, initial_base, new_branch: true, name, path, path_edited: false, pending_path: None, refs: vec![], base, base_values: vec![String::new()],
               existing, error: None, working: false, _subs: subs }
    }

    fn set_refs(&mut self, refs: Vec<BranchRef>, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
        let mut labels = vec!["HEAD".to_string()];
        let mut values = vec![String::new()];
        let ib = self.initial_base.clone();
        if !ib.is_empty() && !names.contains(&ib) { labels.push(ib.chars().take(10).collect()); values.push(ib.clone()) }
        labels.extend(names.iter().cloned());
        values.extend(names.iter().cloned());
        let sel = values.iter().position(|v| *v == ib).unwrap_or(0);
        self.base.update(cx, |b, cx| { b.set_items(SearchableVec::new(labels), window, cx); b.set_selected_index(Some(IndexPath::new(sel)), window, cx) });
        self.existing.update(cx, |b, cx| b.set_items(SearchableVec::new(names), window, cx));
        self.base_values = values;
        self.refs = refs;
        cx.notify();
    }

    /// The branch Create would use: the typed name, or the picked existing branch.
    fn branch(&self, cx: &gpui_kit::App) -> Option<String> {
        if self.new_branch {
            Some(self.name.read(cx).value().trim().to_string()).filter(|s| !s.is_empty())
        } else {
            self.existing.read(cx).selected_value().cloned()
        }
    }

    /// The location follows the branch until the user edits it or picks a folder.
    fn suggest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.path_edited {
            let b = self.branch(cx).unwrap_or_default();
            let slug = self.refs.iter().find(|r| r.name == b && !self.new_branch).map_or(b.as_str(), |r| r.local_name()).to_string();
            let p = suggest_path(&self.repo, &slug);
            self.path.update(cx, |i, cx| i.set_value(p, window, cx));
        }
        cx.notify();
    }

    fn set_mode(&mut self, new_branch: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_branch == new_branch { return }
        self.new_branch = new_branch;
        if new_branch { self.name.update(cx, |i, cx| i.focus(window, cx)) }
        self.suggest(window, cx);
    }

    fn choose_location(&mut self, cx: &mut Context<Self>) {
        let cur = self.path.read(cx).value().to_string();
        let p = Path::new(&cur);
        let dir = p.parent().unwrap_or(Path::new("/")).to_path_buf();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        let rx = cx.prompt_for_new_path(&dir, name.as_deref());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            this.update(cx, |s, cx| { s.path_edited = true; s.pending_path = Some(path.to_string_lossy().into_owned()); cx.notify() }).ok();
        }).detach();
    }

    fn can_create(&self, cx: &gpui_kit::App) -> bool {
        !self.working && self.branch(cx).is_some() && !self.path.read(cx).value().trim().is_empty()
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if !self.can_create(cx) { return }
        let Some(branch) = self.branch(cx) else { return };
        let path = self.path.read(cx).value().trim().to_string();
        let base = self.base.read(cx).selected_index(cx).and_then(|i| self.base_values.get(i.row).cloned()).unwrap_or_default();
        let existing = (!self.new_branch).then(|| self.refs.iter().find(|r| r.name == branch).cloned()
            .unwrap_or(BranchRef { name: branch.clone(), local: None }));
        let new_branch = self.new_branch;
        self.working = true;
        self.error = None;
        cx.notify();
        let (repo, app) = (self.repo.clone(), self.app.clone());
        cx.spawn(async move |this, cx| {
            let (r, p) = (repo.clone(), path.clone());
            let res = cx.background_executor().spawn(async move {
                let local_exists = existing.as_ref().is_some_and(|e| e.local.is_some() && git::is_local_branch(Path::new(&r), e.local_name()));
                let a = add_args(&p, new_branch.then_some((branch.as_str(), base.as_str())), existing.as_ref(), local_exists);
                git::run(&r, a).map(drop)
            }).await;
            match res {
                Ok(()) => {
                    let t = app.update(cx, |a, cx| a.refresh_worktrees(repo, cx));
                    t.await;
                    app.update(cx, |a, cx| { a.select(Some(path), cx); a.dismiss(cx) });
                }
                Err(e) => { this.update(cx, |s, cx| { s.working = false; s.error = e.user_message(); cx.notify() }).ok(); }
            }
        }).detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) { self.app.update(cx, |a, cx| a.dismiss(cx)) }
}

fn label(text: &str) -> impl IntoElement {
    div().w(px(90.)).flex_none().text_color(theme::text_muted()).child(text.to_string())
}

fn field_row() -> gpui_kit::Div { div().flex().items_center().gap_3().min_h(px(32.)) }

impl Render for AddWorktreeSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(p) = self.pending_path.take() { self.path.update(cx, |i, cx| i.set_value(p, window, cx)) }
        let enabled = self.can_create(cx);
        let mode = ui::segmented("wt-mode", &["New branch", "Existing branch"], (!self.new_branch) as usize, {
            let me = cx.entity();
            move |i, w, cx| me.update(cx, |s, cx| s.set_mode(i == 0, w, cx))
        });
        let body = div().flex().flex_col().gap_2().p_4()
            .child(field_row().child(label("Branch")).child(mode))
            .map(|d| if self.new_branch {
                d.child(field_row().child(label("Name")).child(div().flex_1().child(Input::new(&self.name))))
                 .child(field_row().child(label("Based on")).child(div().flex_1().child(Select::new(&self.base).id("wt-base").menu_max_h(px(260.)))))
            } else {
                d.child(field_row().child(label("Branch")).child(div().flex_1().child(Select::new(&self.existing).id("wt-existing").placeholder("Choose…").menu_max_h(px(260.)))))
            })
            .child(field_row().child(label("Location")).child(div().flex_1().min_w_0().child(Input::new(&self.path)))
                .child(ui::pill("wt-choose", "Choose…").on_click(cx.listener(|s, _, _, cx| s.choose_location(cx)))))
            .when_some(self.error.clone(), |d, e| d.child(div().text_size(px(12.5)).text_color(theme::red()).child(e)));
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .capture_key_down(cx.listener(|s, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" { s.cancel(cx); cx.stop_propagation() }))
            .child(ui::card().w(px(480.)).bg(theme::sidebar())
                .child(ui::card_header(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("New Worktree"), div()))
                .child(body)
                .child(ui::card_footer(div().flex().w_full().justify_end().gap_2()
                    .child(ui::pill("wt-cancel", "Cancel").on_click(cx.listener(|s, _, _, cx| s.cancel(cx))))
                    .child(ui::pill_primary("wt-create", SharedString::from(if self.working { "Creating…" } else { "Create" }), theme::blue())
                        .when(!enabled, |d| d.opacity(0.4))
                        .when(enabled, |d| d.on_click(cx.listener(|s, _, _, cx| s.create(cx))))))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_path_keeps_origin_inside_names() {
        assert_eq!(suggest_path("/r/app", ""), "/r/app-worktree");
        assert_eq!(suggest_path("/r/app", "feature/x"), "/r/app-feature-x");
        // Swift cut "origin-" anywhere: "fix-origin-check" became "fix-check".
        assert_eq!(suggest_path("/r/app", "fix-origin-check"), "/r/app-fix-origin-check");
    }

    #[test]
    fn remote_refs_know_their_local_name() {
        let remotes = vec!["origin".to_string(), "up/stream".to_string()];
        assert_eq!(parse_ref("refs/heads/main", &remotes), Some(BranchRef { name: "main".into(), local: None }));
        assert_eq!(parse_ref("refs/remotes/origin/feature/x", &remotes).unwrap().local_name(), "feature/x");
        assert_eq!(parse_ref("refs/remotes/up/stream/x", &remotes).unwrap().local_name(), "x");
    }

    #[test]
    fn create_arguments() {
        let s = |v: Vec<String>| v.join(" ");
        assert_eq!(s(add_args("/p", Some(("n", "")), None, false)), "worktree add -b n /p");
        assert_eq!(s(add_args("/p", Some(("n", "abc")), None, false)), "worktree add -b n /p abc");
        let remote = BranchRef { name: "origin/x".into(), local: Some("x".into()) };
        assert_eq!(s(add_args("/p", None, Some(&remote), false)), "worktree add -b x /p origin/x");
        // Swift ran `-b x` here too and failed with "a branch named 'x' already exists".
        assert_eq!(s(add_args("/p", None, Some(&remote), true)), "worktree add /p x");
        let local = BranchRef { name: "dev".into(), local: None };
        assert_eq!(s(add_args("/p", None, Some(&local), false)), "worktree add /p dev");
    }
}
