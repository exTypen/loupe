//! Helpers for what the shell shows: row titles, paths and the quick switcher's matching. The data itself is
//! `state::AppModel`.
use std::collections::HashMap;

pub use crate::state::Status;
pub use loupe_core::git::Worktree;

/// "main", or "detached @ 1a2b3c4".
pub fn branch_label(w: &Worktree) -> String {
    w.branch.clone().unwrap_or_else(|| format!("detached @ {}", &w.head[..w.head.len().min(7)]))
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

/// The current space's worktrees in sidebar order, for the quick switcher.
pub fn switcher_items(app: &crate::state::AppModel) -> Vec<SwitcherItem> {
    app.repo_paths().iter()
        .flat_map(|r| app.worktrees_of(r).iter().map(|w| SwitcherItem { repo: r.clone(), wt: w.clone() }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: &str) -> Worktree { Worktree { path: path.into(), branch: Some(branch.into()), head: "abc".into(), is_bare: false, is_locked: false, is_prunable: false } }

    #[test]
    fn row_titles_drop_the_repo_prefix() {
        assert_eq!(row_title(&wt("/r/web", "main"), "/r/web"), "Root");
        assert_eq!(row_title(&wt("/r/web-qa", "qa"), "/r/web"), "qa");
        assert_eq!(row_title(&wt("/r/web_x", "x"), "/r/web"), "x");
        assert_eq!(row_title(&wt("/r/web-", "x"), "/r/web"), "web-");
        assert_eq!(row_title(&wt("/r/other", "x"), "/r/web"), "other");
        let detached = Worktree { branch: None, head: "1234567890".into(), ..wt("/r/a", "") };
        assert_eq!(branch_label(&detached), "detached @ 1234567");
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
}
