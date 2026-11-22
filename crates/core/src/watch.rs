//! Watching a worktree (and its git dir) for changes that matter to what Loupe shows.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

use crate::git;

/// What a batch of file events means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    /// Status, diff and views are out of date: refresh (the app debounces this by 250 ms).
    pub status: bool,
    /// A directory directly under .git/worktrees/ appeared or vanished: a worktree was added or removed (possibly from
    /// outside Loupe). Refresh the repository's worktree list.
    pub worktrees: bool,
}

/// Object-store churn (fetch, gc, LFS downloads), a submodule's too under .git/modules/, and lock files don't
/// change what we display.
pub fn is_noise(p: &str) -> bool {
    if p.ends_with(".lock") || p.contains("/.git/objects/") || p.contains("/.git/lfs/") { return true }
    let Some(i) = p.find("/.git/modules/") else { return false };
    let inner = &p[i + 14..];
    inner.contains("/objects/") || inner.contains("/lfs/")
}

/// Decides whether file events can change what one worktree shows. Most can't: nix-direnv, build output and agents'
/// scratch files land in git-ignored directories, and the main worktree's folder holds every linked worktree
/// (.worktree/…), whose edits are theirs. Refreshing for those ran git status, stash, diff, LFS and submodule
/// checks and recomputed the views about once a second while anything worked in any worktree.
pub struct StatusFilter {
    pub path: String,
    pub git_dir: Option<String>,
    /// Other worktrees of the repository: those inside this one are skipped.
    pub worktrees: Vec<String>,
    ignored_dirs: HashSet<String>, // worktree-relative folders git ignores ("a/b/")
}

impl StatusFilter {
    pub fn new(path: impl Into<String>, git_dir: Option<String>) -> Self {
        StatusFilter { path: path.into(), git_dir, worktrees: vec![], ignored_dirs: HashSet::new() }
    }

    pub fn affects_status(&mut self, changed: &[String]) -> bool {
        let root = format!("{}/", self.path);
        let nested: Vec<String> = self.worktrees.iter().map(|w| format!("{w}/")).filter(|w| *w != root && w.starts_with(&root)).collect();
        let mut unknown: Vec<String> = vec![]; // worktree-relative paths git has to judge
        for p in changed {
            if p.ends_with("/.gitignore") || p.ends_with("/info/exclude") { self.ignored_dirs.clear(); return true }
            if let Some(g) = &self.git_dir
                && (p == g || p.starts_with(&format!("{g}/"))) {
                    if p.starts_with(&format!("{g}/worktrees/")) { continue } // another worktree's index and HEAD
                    return true; // this worktree's index, HEAD, refs, stash
                }
            let Some(rel) = p.strip_prefix(&root) else { return true };
            if nested.iter().any(|n| p.starts_with(n)) { continue }
            if self.ignored_dirs.iter().any(|d| rel.starts_with(d.as_str())) { continue }
            unknown.push(rel.to_string());
        }
        if unknown.is_empty() { return false }
        // One check-ignore for the paths and their folders. A folder git ignores is remembered (when it holds no
        // tracked file), so the next event under it needs no git at all.
        let mut queries = HashSet::new();
        for r in &unknown {
            queries.insert(r.clone());
            let mut d = parent(r);
            while !d.is_empty() { queries.insert(format!("{d}/")); d = parent(d); }
        }
        let input = queries.into_iter().collect::<Vec<_>>().join("\0");
        let Ok(out) = git::git(&self.path, ["check-ignore", "-z", "--stdin"]).stdin(input.as_bytes()).allow_failure().run() else { return true };
        let ignored: HashSet<String> = out.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
        for d in ignored.iter().filter(|d| d.ends_with('/')) {
            if self.ignored_dirs.contains(d) { continue }
            let tracked = git::run(&self.path, ["ls-files", "-z", "--", d]).is_ok_and(|o| !o.is_empty());
            if !tracked { self.ignored_dirs.insert(d.clone()); }
        }
        unknown.iter().any(|r| {
            if ignored.contains(r) { return false }
            let mut d = parent(r);
            while !d.is_empty() {
                if ignored.contains(&format!("{d}/")) { return false }
                d = parent(d);
            }
            true
        })
    }
}

fn parent(p: &str) -> &str { p.rfind('/').map(|i| &p[..i]).unwrap_or("") }

/// FSEvents on a worktree and its git dir (a linked worktree's index lives under the main repo's
/// .git/worktrees/<name>, outside the worktree). Events are coalesced for 150 ms, filtered, and `on_change` is called
/// on the watcher's own thread only when something matters. Dropping the watcher stops it.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
    filter: Arc<Mutex<StatusFilter>>,
}

impl Watcher {
    /// Blocks for one `rev-parse` (the git dir): call it off the main thread.
    pub fn new(path: &Path, on_change: impl Fn(Change) + Send + 'static) -> notify::Result<Self> {
        let p = path.to_string_lossy().into_owned();
        let git_dir = git::git_dir(path);
        let mut roots = vec![p.clone()];
        if let Some(g) = git_dir.as_ref().filter(|g| !g.starts_with(&p)) { roots.push(g.clone()) }
        let filter = Arc::new(Mutex::new(StatusFilter::new(p, git_dir)));
        // FSEvents reports real paths (/private/var/... for /var/...): map them back onto the roots as given.
        let aliases: Vec<(String, String)> = roots.iter()
            .filter_map(|r| std::fs::canonicalize(r).ok().map(|c| (c.to_string_lossy().into_owned(), r.clone())))
            .filter(|(c, r)| c != r)
            .collect();
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let mut inner = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
            if let Ok(e) = e { let _ = tx.send(e.paths); }
        })?;
        for r in &roots { inner.watch(Path::new(r), RecursiveMode::Recursive)? }
        let f = filter.clone();
        std::thread::Builder::new().name("loupe-watch".into()).spawn(move || {
            // Ends when the watcher (and with it the sender) is dropped.
            while let Ok(first) = rx.recv() {
                let mut batch = first;
                let until = Instant::now() + Duration::from_millis(150);
                loop {
                    match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                        Ok(more) => batch.extend(more),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                let mut changed: Vec<String> = batch.into_iter().map(|p| {
                    let s = p.to_string_lossy().into_owned();
                    aliases.iter().find_map(|(c, r)| s.strip_prefix(c.as_str()).filter(|rest| rest.is_empty() || rest.starts_with('/')).map(|rest| format!("{r}{rest}"))).unwrap_or(s)
                }).filter(|p| !is_noise(p)).collect();
                changed.sort();
                changed.dedup();
                if changed.is_empty() { continue }
                let worktrees = changed.iter().any(|p| parent(p).ends_with("/.git/worktrees"));
                let status = f.lock().unwrap().affects_status(&changed);
                if status || worktrees { on_change(Change { status, worktrees }) }
            }
        }).map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(Watcher { _inner: inner, filter })
    }

    /// The repository's worktrees, so events inside nested ones are left to them.
    pub fn set_worktrees(&self, paths: Vec<String>) { self.filter.lock().unwrap().worktrees = paths }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn ignores_object_store_churn_including_submodules() {
        assert!(is_noise("/r/.git/objects/ab/cdef"));
        assert!(is_noise("/r/.git/lfs/tmp/x"));
        assert!(is_noise("/r/.git/modules/libs/lib/objects/ab/cd"));
        assert!(is_noise("/r/.git/index.lock"));
        assert!(!is_noise("/r/.git/modules/libs/lib/index"));
        assert!(!is_noise("/r/src/objects/model.swift"));
    }

    #[test]
    fn ignored_folders_and_nested_worktrees_do_not_refresh() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().canonicalize().unwrap();
        git::run(&dir, ["init", "-q"]).unwrap();
        std::fs::write(dir.join(".gitignore"), ".direnv/\nbuild/\n").unwrap();
        let p = dir.to_string_lossy().into_owned();
        let mut f = StatusFilter::new(p.clone(), git::git_dir(&dir));
        f.worktrees = vec![p.clone(), format!("{p}/.worktree/feat")];
        assert!(!f.affects_status(&[format!("{p}/.direnv/flake-profile")]));
        assert!(!f.affects_status(&[format!("{p}/build/out/a.o")])); // remembered: no git this time
        assert!(!f.affects_status(&[format!("{p}/.worktree/feat/src/a.rs")]));
        assert!(!f.affects_status(&[format!("{p}/.git/worktrees/feat/index")]));
        assert!(f.affects_status(&[format!("{p}/src/a.rs")]));
        assert!(f.affects_status(&[format!("{p}/.git/index")]));
        assert!(f.affects_status(&[format!("{p}/.gitignore")]));
        assert!(f.affects_status(&["/elsewhere/x".to_string()]));
    }

    /// Events still arrive, and dropping watchers while files change is fine.
    #[test]
    fn fires_and_survives_teardown() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_path_buf();
        git::run(&dir, ["init", "-q"]).unwrap();
        let fired = Arc::new(AtomicUsize::new(0));
        let f2 = fired.clone();
        let w = Watcher::new(&dir, move |c| { if c.status { f2.fetch_add(1, Ordering::SeqCst); } }).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        for k in 0..20 {
            std::fs::write(dir.join(format!("f{k}")), "x").unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(50)) }
        assert!(fired.load(Ordering::SeqCst) > 0);
        assert!(fired.load(Ordering::SeqCst) < 20, "coalesced"); // 20 writes 30 ms apart: a handful of batches
        for k in 0..50 {
            let t = Watcher::new(&dir, |_| {}).unwrap();
            std::fs::write(dir.join(format!("g{k}")), "y").unwrap();
            drop(t);
        }
        drop(w);
    }
}
