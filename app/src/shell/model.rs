//! What the shell shows: spaces → repositories → worktrees, the selection and the recent list (AppModel in
//! Model.swift). The data here is fake; the integrator fills `worktrees` and `status` from loupe-core and
//! persists `spaces`, `current_space`, `recent` and `collapsed` under the Swift app's defaults keys.
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
    pub locked: bool,
}

impl Worktree {
    pub fn name(&self) -> &str { last_component(&self.path) }
    /// "main", or "detached @ 1a2b3c4".
    pub fn branch_label(&self) -> String {
        self.branch.clone().unwrap_or_else(|| format!("detached @ {}", &self.head[..self.head.len().min(7)]))
    }
}

/// What a worktree's row shows besides its name; WorktreeModel's fields in Swift.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    pub changes: usize,
    pub ahead: usize,
    pub behind: usize,
    pub merged: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub repos: Vec<String>,
    pub selection: Option<String>,
}

pub const DEFAULT_SPACE: &str = "default";

#[derive(Clone, Debug)]
pub struct Workspace {
    pub spaces: Vec<Space>,
    pub current_space: String,
    pub worktrees: HashMap<String, Vec<Worktree>>, // repo path → worktrees, main first
    pub status: HashMap<String, Status>,            // worktree path → status
    selection: Option<String>,
    pub recent: Vec<String>,                        // worktree paths, most recent first
    pub collapsed: HashSet<String>,                 // repo paths
}

pub fn last_component(path: &str) -> &str { path.trim_end_matches('/').rsplit('/').next().unwrap_or(path) }

pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&h) => format!("~{}", &path[h.len()..]),
        _ => path.into(),
    }
}

/// A row is a worktree: its folder leads, minus the repo-name prefix the header already shows ("web-qa" under
/// "web" reads "qa"). The main worktree is the repo's own folder, so its name would only repeat the header.
pub fn row_title(wt: &Worktree, repo: &str) -> String {
    if wt.path == repo { return "Root".into() }
    let (r, name) = (last_component(repo), wt.name());
    for sep in ['-', '_', '.'] {
        if let Some(rest) = name.strip_prefix(r).and_then(|x| x.strip_prefix(sep)) {
            if !rest.is_empty() { return rest.into() }
        }
    }
    name.into()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherItem { pub repo: String, pub wt: Worktree }

/// Every query token must appear in "repo branch dir"; matches ordered most-recently-used first, then
/// sidebar order.
pub fn switcher_matches(all: &[SwitcherItem], query: &str, recent: &[String]) -> Vec<SwitcherItem> {
    let rank: HashMap<&str, usize> = recent.iter().enumerate().rev().map(|(i, p)| (p.as_str(), i)).collect();
    let q = query.to_lowercase();
    let tokens: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
    let mut hits: Vec<(usize, &SwitcherItem)> = all.iter().enumerate()
        .filter(|(_, i)| {
            let hay = format!("{} {} {}", last_component(&i.repo), i.wt.branch.as_deref().unwrap_or(""), i.wt.name()).to_lowercase();
            tokens.iter().all(|t| hay.contains(t))
        })
        .collect();
    hits.sort_by_key(|(n, i)| (rank.get(i.wt.path.as_str()).copied().unwrap_or(usize::MAX), *n));
    hits.into_iter().map(|(_, i)| i.clone()).collect()
}

impl Workspace {
    pub fn new(spaces: Vec<Space>, current_space: String) -> Self {
        let mut w = Self { spaces, current_space, worktrees: HashMap::new(), status: HashMap::new(), selection: None,
                           recent: vec![], collapsed: HashSet::new() };
        if w.spaces.is_empty() { w.spaces.push(Space { id: DEFAULT_SPACE.into(), name: "Default".into(), repos: vec![], selection: None }) }
        w.selection = w.space().selection.clone();
        w
    }

    fn space_index(&self) -> usize { self.spaces.iter().position(|s| s.id == self.current_space).unwrap_or(0) }
    pub fn space(&self) -> &Space { &self.spaces[self.space_index()] }
    pub fn repo_paths(&self) -> &[String] { &self.space().repos }
    pub fn worktrees_of(&self, repo: &str) -> &[Worktree] { self.worktrees.get(repo).map_or(&[], |v| v.as_slice()) }
    pub fn status(&self, path: &str) -> Status { self.status.get(path).copied().unwrap_or_default() }
    pub fn selection(&self) -> Option<&str> { self.selection.as_deref() }
    pub fn selected_worktree(&self) -> Option<&Worktree> {
        let s = self.selection.as_deref()?;
        self.worktrees.values().flatten().find(|w| w.path == s)
    }

    pub fn repo_containing(&self, worktree: &str) -> Option<&str> {
        self.worktrees.iter().find(|(_, v)| v.iter().any(|w| w.path == worktree)).map(|(k, _)| k.as_str())
    }

    /// Uncommitted changes summed over a repo's worktrees: a collapsed header still shows it.
    pub fn change_count(&self, repo: &str) -> usize {
        self.worktrees_of(repo).iter().map(|w| self.status(&w.path).changes).sum()
    }

    /// The selected worktree. Jumping to one in another space (⌘K, PR checkout) takes the sidebar there too;
    /// each space remembers its own selection; the recent list keeps 30.
    pub fn select(&mut self, sel: Option<String>) {
        self.selection = sel.clone();
        let Some(s) = sel else {
            let i = self.space_index();
            self.spaces[i].selection = None;
            return;
        };
        let repo = self.repo_containing(&s).unwrap_or(&s).to_string();
        if !self.space().repos.contains(&repo) {
            if let Some(sp) = self.spaces.iter().find(|sp| sp.repos.contains(&repo)) { self.current_space = sp.id.clone() }
        }
        let i = self.space_index();
        self.spaces[i].selection = Some(s.clone());
        self.recent.retain(|r| *r != s);
        self.recent.insert(0, s);
        self.recent.truncate(30);
    }

    pub fn switch_space_to(&mut self, id: &str) {
        if id == self.current_space { return }
        let Some(sp) = self.spaces.iter().find(|s| s.id == id) else { return };
        let sel = sp.selection.clone();
        self.current_space = id.into();
        self.select(sel);
    }

    /// Bounds-checked: the space list can change under a swipe.
    pub fn switch_space_by(&mut self, offset: isize) {
        let i = self.space_index() as isize + offset;
        if i >= 0 && (i as usize) < self.spaces.len() { let id = self.spaces[i as usize].id.clone(); self.switch_space_to(&id) }
    }

    pub fn add_space(&mut self) -> String {
        let id = format!("{:X}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        self.spaces.push(Space { id: id.clone(), name: format!("Space {}", self.spaces.len() + 1), repos: vec![], selection: None });
        self.switch_space_to(&id);
        id
    }

    /// Its repos go to the neighbouring space. (Its accounts and tokens are the forge's to delete.)
    pub fn delete_space(&mut self, id: &str) {
        let Some(i) = self.spaces.iter().position(|s| s.id == id) else { return };
        if self.spaces.len() < 2 { return }
        let target = self.spaces[if i == 0 { 1 } else { i - 1 }].id.clone();
        let repos = self.spaces[i].repos.clone();
        self.move_repos(&repos, &target);
        if self.current_space == id { self.switch_space_to(&target) }
        self.spaces.retain(|s| s.id != id);
    }

    pub fn move_repos(&mut self, repos: &[String], to: &str) {
        for sp in &mut self.spaces { sp.repos.retain(|r| !repos.contains(r)) }
        if let Some(sp) = self.spaces.iter_mut().find(|s| s.id == to) { sp.repos.extend(repos.iter().cloned()) }
        if let Some(s) = self.selection.clone() {
            let repo = self.repo_containing(&s).unwrap_or(&s).to_string();
            if repos.contains(&repo) && to != self.current_space { let first = self.repo_paths().first().cloned(); self.select(first) }
        }
    }

    /// Sidebar drag and drop: puts `repo` just before `target` in the current space.
    pub fn move_repo(&mut self, repo: &str, before: &str) {
        if repo == before || !self.repo_paths().iter().any(|r| r == repo) { return }
        let i = self.space_index();
        let r = &mut self.spaces[i].repos;
        r.retain(|x| x != repo);
        let at = r.iter().position(|x| x == before).unwrap_or(r.len());
        r.insert(at, repo.into());
    }

    pub fn remove_repo(&mut self, repo: &str) {
        for sp in &mut self.spaces { sp.repos.retain(|r| r != repo) }
        if self.selection.as_deref().is_some_and(|s| self.repo_containing(s) == Some(repo)) { self.select(None) }
        self.worktrees.remove(repo);
    }

    /// The current space's worktrees in sidebar order, for the quick switcher.
    pub fn switcher_items(&self) -> Vec<SwitcherItem> {
        self.repo_paths().iter()
            .flat_map(|r| self.worktrees_of(r).iter().map(|w| SwitcherItem { repo: r.clone(), wt: w.clone() }))
            .collect()
    }

    /// Opened from the File menu or dropped on the window. ponytail: no `--git-common-dir` normalisation or
    /// worktree listing here; loupe-core's addRepo replaces this.
    pub fn add_repo(&mut self, path: &str) {
        if self.spaces.iter().any(|s| s.repos.iter().any(|r| r == path)) { return }
        let i = self.space_index();
        self.spaces[i].repos.push(path.into());
        self.worktrees.entry(path.into()).or_insert_with(|| vec![Worktree { path: path.into(), branch: None, head: String::new(), locked: false }]);
    }

    /// A few repositories in two spaces, for running the shell before the real models are wired in.
    pub fn demo() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/me".into());
        let p = |s: &str| format!("{home}/{s}");
        let wt = |path: &str, branch: Option<&str>| Worktree { path: p(path), branch: branch.map(Into::into), head: "fde8251c0a".into(), locked: false };
        let mut w = Workspace::new(vec![
            Space { id: DEFAULT_SPACE.into(), name: "Main".into(), repos: vec![p("loupe"), p("synchronicity"), p("devallet")], selection: Some(p("loupe-gpui")) },
            Space { id: "work".into(), name: "Work".into(), repos: vec![p("brolyz"), p("fleetdriver")], selection: None },
        ], DEFAULT_SPACE.into());
        w.worktrees.insert(p("loupe"), vec![
            wt("loupe", Some("main")), wt("loupe-gpui", Some("gpui")), wt("loupe-gpui-shell", Some("gpui-shell")),
            wt("loupe-memtest", Some("memtest")),
        ]);
        w.worktrees.insert(p("synchronicity"), vec![wt("synchronicity", Some("main")), wt("synchronicity-perps-fix", Some("fix/perps-funding")),
            Worktree { locked: true, ..wt("synchronicity-audit", None) }]);
        w.worktrees.insert(p("devallet"), vec![wt("devallet", Some("main"))]);
        w.worktrees.insert(p("brolyz"), vec![wt("brolyz", Some("qa")), wt("brolyz-release", Some("release/2.4"))]);
        w.worktrees.insert(p("fleetdriver"), vec![wt("fleetdriver", Some("main"))]);
        for (path, s) in [
            ("loupe", Status { changes: 3, ..Default::default() }),
            ("loupe-gpui", Status { changes: 12, ahead: 4, ..Default::default() }),
            ("loupe-gpui-shell", Status { ahead: 1, behind: 2, ..Default::default() }),
            ("loupe-memtest", Status { merged: true, ..Default::default() }),
            ("synchronicity-perps-fix", Status { changes: 5, behind: 7, ..Default::default() }),
            ("brolyz-release", Status { changes: 1, ..Default::default() }),
        ] { w.status.insert(p(path), s); }
        w.collapsed.insert(p("devallet"));
        w.recent = vec![p("loupe-gpui"), p("loupe")];
        w.selection = Some(p("loupe-gpui"));
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: &str) -> Worktree { Worktree { path: path.into(), branch: Some(branch.into()), head: "abc".into(), locked: false } }

    #[test]
    fn row_titles_drop_the_repo_prefix() {
        assert_eq!(row_title(&wt("/r/web", "main"), "/r/web"), "Root");
        assert_eq!(row_title(&wt("/r/web-qa", "qa"), "/r/web"), "qa");
        assert_eq!(row_title(&wt("/r/web_x", "x"), "/r/web"), "x");
        assert_eq!(row_title(&wt("/r/web-", "x"), "/r/web"), "web-");
        assert_eq!(row_title(&wt("/r/other", "x"), "/r/web"), "other");
        let detached = Worktree { branch: None, head: "1234567890".into(), ..wt("/r/a", "") };
        assert_eq!(detached.branch_label(), "detached @ 1234567");
    }

    // QuickSwitcherTests.swift
    #[test]
    fn switcher_filters_by_every_token_and_orders_by_recent() {
        let items = vec![
            SwitcherItem { repo: "/r/web".into(), wt: wt("/r/web", "main") },
            SwitcherItem { repo: "/r/web".into(), wt: wt("/r/web-qa", "qa") },
            SwitcherItem { repo: "/r/api".into(), wt: wt("/r/api", "main") },
        ];
        let paths = |v: Vec<SwitcherItem>| v.into_iter().map(|i| i.wt.path).collect::<Vec<_>>();
        assert_eq!(paths(switcher_matches(&items, "", &[])), ["/r/web", "/r/web-qa", "/r/api"]);
        assert_eq!(paths(switcher_matches(&items, "", &["/r/api".into(), "/r/web-qa".into()])), ["/r/api", "/r/web-qa", "/r/web"]);
        assert_eq!(paths(switcher_matches(&items, "WEB  main", &[])), ["/r/web"]);
        assert_eq!(paths(switcher_matches(&items, "qa", &[])), ["/r/web-qa"]);
        assert!(switcher_matches(&items, "nothing", &[]).is_empty());
    }

    #[test]
    fn selecting_in_another_space_follows_it_and_records_recent() {
        let mut w = Workspace::demo();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/me".into());
        let target = format!("{home}/brolyz-release");
        w.select(Some(target.clone()));
        assert_eq!(w.current_space, "work");
        assert_eq!(w.space().selection.as_deref(), Some(target.as_str()));
        assert_eq!(w.recent[0], target);
        // Switching back restores that space's own selection.
        w.switch_space_to(DEFAULT_SPACE);
        assert_eq!(w.selection(), Some(format!("{home}/loupe-gpui").as_str()));
        for i in 0..40 { w.select(Some(format!("/x/{i}"))) }
        assert_eq!(w.recent.len(), 30);
    }

    #[test]
    fn spaces_add_move_delete() {
        let mut w = Workspace::new(vec![], DEFAULT_SPACE.into());
        w.add_repo("/a"); w.add_repo("/b"); w.add_repo("/a");
        assert_eq!(w.repo_paths(), ["/a", "/b"]);
        w.move_repo("/b", "/a");
        assert_eq!(w.repo_paths(), ["/b", "/a"]);
        let id = w.add_space();
        assert_eq!(w.current_space, id);
        w.move_repos(&["/a".into()], &id);
        assert_eq!(w.space().repos, ["/a"]);
        w.delete_space(&id);
        assert_eq!(w.spaces.len(), 1);
        assert_eq!(w.repo_paths(), ["/b", "/a"]);
        w.delete_space(DEFAULT_SPACE); // the last space stays
        assert_eq!(w.spaces.len(), 1);
        w.switch_space_by(5);
        assert_eq!(w.current_space, DEFAULT_SPACE);
    }
}
