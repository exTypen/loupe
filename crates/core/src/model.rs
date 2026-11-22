//! The app's non-UI state: spaces, repositories and their worktrees, and per worktree its status, selection, diff
//! and commit box, plus every action on a worktree. Plain structs the UI wraps in gpui entities; the functions that
//! run git block, so the UI calls them on its background executor and applies their results on the main thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diff::FileDiff;
use crate::file_tree::tree_order;
use crate::git::{self, Cancel, Error, FileChange, Kind, Result, Worktree};
use crate::lfs::{LfsCheck, LfsState};
use crate::undo::{self, Recipe, Reset, UndoStack};
use crate::{rebase, submodules};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GitOperation { Merge, CherryPick, Revert, Rebase }

impl GitOperation {
    pub fn command(self) -> &'static str {
        match self { Self::Merge => "merge", Self::CherryPick => "cherry-pick", Self::Revert => "revert", Self::Rebase => "rebase" }
    }
    pub fn verb(self) -> &'static str {
        match self { Self::Merge => "Merging", Self::CherryPick => "Cherry-picking", Self::Revert => "Reverting", Self::Rebase => "Rebasing" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictSide { Current, Incoming }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkAction { Stage, Unstage, Discard }

// MARK: - Spaces and repositories

/// Arc-style spaces: each has its own repos and accounts. Stored as JSON under the "spaces" settings key, as the
/// Swift app did (its Codable shape: id, name, repos, selection).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub repos: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
}

pub const DEFAULT_SPACE: &str = "default";

/// Repositories, their worktrees and the selection, across spaces.
#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub spaces: Vec<Space>,
    pub current_space: String,
    pub worktrees: HashMap<String, Vec<Worktree>>, // repo path → worktrees (main first), across all spaces
    pub selection: Option<String>,                 // selected worktree path
    pub recent: Vec<String>,                       // worktree paths, most recent first
    pub collapsed: HashSet<String>,                // repo paths
}

/// What the app persists, as the settings keys hold it.
#[derive(Clone, Debug, Default)]
pub struct Saved {
    pub spaces: Option<Vec<Space>>,   // "spaces"
    pub repos: Vec<String>,           // "repos": the flat list from before spaces
    pub selection: Option<String>,    // "selection": from before spaces
    pub current_space: Option<String>, // "currentSpace"
    pub recent: Vec<String>,          // "recent"
    pub collapsed: Vec<String>,       // "collapsed"
}

impl AppState {
    /// Before spaces existed there was one flat list; it becomes the "default" space.
    pub fn new(saved: Saved) -> Self {
        let spaces = saved.spaces.filter(|s| !s.is_empty()).unwrap_or_else(|| vec![Space {
            id: DEFAULT_SPACE.into(), name: "Default".into(), repos: saved.repos, selection: saved.selection,
        }]);
        let mut s = AppState {
            spaces,
            current_space: saved.current_space.unwrap_or_else(|| DEFAULT_SPACE.into()),
            worktrees: HashMap::new(),
            selection: None,
            recent: saved.recent,
            collapsed: saved.collapsed.into_iter().collect(),
        };
        s.selection = s.space().selection.clone();
        s
    }

    pub fn spaces_json(&self) -> String { serde_json::to_string(&self.spaces).unwrap_or_default() }

    fn space_index(&self) -> usize { self.spaces.iter().position(|s| s.id == self.current_space).unwrap_or(0) }
    pub fn space(&self) -> &Space { &self.spaces[self.space_index()] }
    /// The current space's repos, in sidebar order.
    pub fn repo_paths(&self) -> &[String] { &self.space().repos }
    pub fn all_repos(&self) -> Vec<String> { self.spaces.iter().flat_map(|s| s.repos.clone()).collect() }

    pub fn repo_containing(&self, worktree: &str) -> Option<&str> {
        self.worktrees.iter().find(|(_, w)| w.iter().any(|w| w.path == worktree)).map(|(r, _)| r.as_str())
    }

    pub fn worktree_with_branch(&self, branch: &str) -> Option<&Worktree> {
        self.worktrees.values().flatten().find(|w| w.branch.as_deref() == Some(branch))
    }

    /// Selects a worktree; returns the one selected before. Only the shown worktree keeps its big, quick-to-rebuild
    /// state: the caller releases the old one's (`WorktreeState::release_heavy_state`) and reloads the new one's.
    pub fn select(&mut self, s: Option<String>) -> Option<String> {
        let old = std::mem::replace(&mut self.selection, s.clone());
        let Some(s) = s else {
            let i = self.space_index();
            self.spaces[i].selection = None;
            return old;
        };
        // Jumping to a worktree in another space (⌘K, PR checkout) takes the sidebar there too.
        let repo = self.repo_containing(&s).unwrap_or(&s).to_string();
        if !self.space().repos.contains(&repo)
            && let Some(sp) = self.spaces.iter().find(|sp| sp.repos.contains(&repo)) { self.current_space = sp.id.clone() }
        let i = self.space_index();
        self.spaces[i].selection = Some(s.clone());
        self.recent.retain(|r| *r != s);
        self.recent.insert(0, s);
        self.recent.truncate(30);
        old
    }

    pub fn switch_space(&mut self, id: &str) {
        if id == self.current_space { return }
        let Some(sel) = self.spaces.iter().find(|s| s.id == id).map(|s| s.selection.clone()) else { return };
        self.current_space = id.into();
        self.select(sel);
    }

    pub fn switch_space_by(&mut self, offset: isize) {
        let i = self.space_index() as isize + offset;
        if i >= 0 && (i as usize) < self.spaces.len() { let id = self.spaces[i as usize].id.clone(); self.switch_space(&id) }
    }

    pub fn add_space(&mut self) -> String {
        let id = new_id();
        self.spaces.push(Space { id: id.clone(), name: format!("Space {}", self.spaces.len() + 1), repos: vec![], selection: None });
        self.switch_space(&id);
        id
    }

    /// Repos go to the neighbouring space. The caller deletes the space's accounts and their tokens.
    pub fn delete_space(&mut self, id: &str) -> bool {
        let Some(i) = self.spaces.iter().position(|s| s.id == id).filter(|_| self.spaces.len() > 1) else { return false };
        let target = self.spaces[if i == 0 { 1 } else { i - 1 }].id.clone();
        let repos = self.spaces[i].repos.clone();
        self.move_repos(&repos, &target);
        if self.current_space == id { self.switch_space(&target) }
        self.spaces.retain(|s| s.id != id);
        true
    }

    /// A repo lives in one space: moving it takes it out of the others.
    pub fn move_repos(&mut self, repos: &[String], id: &str) {
        for s in &mut self.spaces { s.repos.retain(|r| !repos.contains(r)) }
        if let Some(s) = self.spaces.iter_mut().find(|s| s.id == id) { s.repos.extend(repos.iter().cloned()) }
        if let Some(sel) = self.selection.clone() {
            let repo = self.repo_containing(&sel).unwrap_or(&sel).to_string();
            if repos.contains(&repo) && id != self.current_space { let first = self.space().repos.first().cloned(); self.select(first); }
        }
    }

    /// Sidebar drag and drop: puts `repo` just before `target` in the current space.
    pub fn move_repo(&mut self, repo: &str, target: &str) {
        let i = self.space_index();
        let r = &mut self.spaces[i].repos;
        if repo == target || !r.iter().any(|x| x == repo) { return }
        r.retain(|x| x != repo);
        let at = r.iter().position(|x| x == target).unwrap_or(r.len());
        r.insert(at, repo.into());
    }

    /// Opening `main` (a repository's main worktree) here: a repo lives in one space, so opening it again moves it.
    /// The caller then refreshes its worktrees and selects what was opened.
    pub fn add_repo(&mut self, main: &str) {
        if !self.repo_paths().iter().any(|r| r == main) { let cur = self.current_space.clone(); self.move_repos(&[main.into()], &cur) }
    }

    /// Removes a repository; returns its worktrees, which the caller forgets (`forget`).
    pub fn remove_repo(&mut self, path: &str) -> Vec<String> {
        for s in &mut self.spaces { s.repos.retain(|r| r != path) }
        self.worktrees.remove(path).unwrap_or_default().into_iter().map(|w| w.path).collect()
    }

    /// Takes in a fresh `worktree list` for `repo`. Returns the worktrees that are gone (forget their models, watchers
    /// and undo history); the selected one included, whose selection moves to the repository: leaving it on a gone path
    /// made the detail view build a fresh model (git + watcher on a missing folder) for it again, and its watcher
    /// refreshed the list, forgetting it again.
    pub fn apply_worktrees(&mut self, repo: &str, list: Vec<Worktree>) -> Vec<String> {
        let wts: Vec<Worktree> = list.into_iter().filter(|w| !w.is_bare).collect();
        let gone: Vec<String> = self.worktrees.get(repo).into_iter().flatten()
            .filter(|old| !wts.iter().any(|w| w.path == old.path)).map(|w| w.path.clone()).collect();
        for g in &gone {
            forget(Path::new(g));
            if self.selection.as_deref() == Some(g) { self.select(Some(repo.into())); }
        }
        self.worktrees.insert(repo.into(), wts);
        gone
    }
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let (a, b) = (h.finish(), std::collections::hash_map::RandomState::new().build_hasher().finish());
    format!("{:08X}-{:04X}-{:04X}-{:04X}-{:012X}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFFF, b >> 48, b & 0xFFFF_FFFF_FFFF)
}

/// A worktree that's gone: its undo stack and last rewrite. (The UI drops its model and watcher.)
pub fn forget(path: &Path) {
    UndoStack::forget(path);
    rebase::forget_rewrite(path);
}

/// The main worktree of the repository `dir` is in: adding a linked worktree adds its repo.
pub fn main_worktree(dir: &Path) -> Result<String> {
    let common = git::common_dir(dir).ok_or_else(|| Error::msg(format!("{} is not a git repository.", dir.display())))?;
    Ok(common.strip_suffix("/.git").map(String::from).unwrap_or_else(|| dir.to_string_lossy().into_owned()))
}

pub fn add_worktree(repo: &Path, path: &str, branch: &str, base: &str, new_branch: bool) -> Result<()> {
    let mut a = vec!["worktree", "add"];
    if new_branch {
        a.extend(["-b", branch, path]);
        if !base.is_empty() { a.push(base) }
    } else { a.extend([path, branch]) }
    git::run(repo, a).map(drop)
}

/// New Worktree with an existing branch: "origin/x" with no local x makes a local x tracking it.
pub fn add_worktree_for_branch(repo: &Path, path: &str, branch: &str, base: &str, new_branch: bool, branches: &[String]) -> Result<()> {
    if !new_branch && branch.contains('/') && branches.iter().any(|b| b == branch) && !git::is_local_branch(repo, branch) {
        let local = branch.split_once('/').unwrap().1;
        return add_worktree(repo, path, local, branch, true);
    }
    add_worktree(repo, path, branch, base, new_branch)
}

/// Where a new worktree for `branch` goes: beside the repository, "<repo>-<branch>".
pub fn suggest_worktree_path(repo: &str, branch: &str) -> String {
    let p = Path::new(repo);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug = if branch.is_empty() { "worktree".to_string() } else { branch.replace('/', "-").replace("origin-", "") };
    p.parent().unwrap_or(Path::new("/")).join(format!("{name}-{slug}")).to_string_lossy().into_owned()
}

pub fn remove_worktree(repo: &Path, path: &str, force: bool) -> Result<()> {
    let mut a = vec!["worktree", "remove"];
    if force { a.push("--force") }
    a.push(path);
    git::run(repo, a)?;
    forget(Path::new(path));
    Ok(())
}

pub fn prune_worktrees(repo: &Path) -> Result<()> { git::run(repo, ["worktree", "prune"]).map(drop) }

/// Fetches refs/pull/N/head and checks it out in a sibling worktree "<repo>-pr-N" on branch pr/N. The caller first
/// selects an existing worktree on pr/N if there is one (`AppState::worktree_with_branch`).
pub fn checkout_pr_worktree(repo: &Path, remote: &str, number: u64, env: &[(String, String)]) -> Result<String> {
    let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dest = repo.parent().unwrap_or(Path::new("/")).join(format!("{name}-pr-{number}")).to_string_lossy().into_owned();
    git::git(repo, ["fetch", remote, &format!("+refs/pull/{number}/head:refs/remotes/{remote}/pr/{number}")]).env(env.iter().map(|(k, v)| (k, v))).run()?;
    git::run(repo, ["worktree", "add", "-B", &format!("pr/{number}"), &dest, &format!("{remote}/pr/{number}")])?;
    Ok(dest)
}

// MARK: - Per-worktree state

#[derive(Clone, Debug)]
pub struct WorktreeState {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head_oid: String, // changes on commit/checkout/reset → history reloads
    pub ahead: u32,
    pub behind: u32,
    files: Vec<FileChange>,
    // Split once per status change: the file list and sidebar read these many times per frame.
    staged: Vec<FileChange>,
    unstaged: Vec<FileChange>,
    conflicted: Vec<FileChange>,
    listed: Vec<FileChange>, // file list order: conflicted, staged, unstaged, each in tree order
    change_count: usize,
    pub stash_count: usize,
    pub is_merged: bool, // the branch's work is already on the default branch
    merged_key: String,
    pub operation: Option<GitOperation>, // a merge, cherry-pick, revert or rebase waiting to be continued or aborted
    pub operation_subject: String,       // the commit it's applying
    pub stopped_to_edit: bool,           // an interactive rebase's "edit" stop: amend, then continue
    git_dir: Option<String>,
    pub selected_file_id: Option<String>,        // the file whose diff is shown
    pub selected_file_ids: HashSet<String>,      // the file list's selection (⌘/⇧-click, ⌘A); bulk actions use it
    pub diff: Option<FileDiff>,
    pub commit_message: String,
    pub amend: bool,
    pub busy: Option<String>, // Some → an operation is running (its label)
    pub error: Option<String>,
    pub remote_name: String,
    pub refs_version: u64,   // bumps after fetch / pull / push
    pub status_version: u64, // bumps on every status refresh
    pub lfs: LfsState,
    pub uninitialized_submodules: Vec<String>,
    pub submodule_banner_hidden: bool, // "Not Now": until the app quits
}

/// What a status refresh needs from the state, to run off the main thread.
#[derive(Clone, Debug)]
pub struct StatusQuery {
    pub path: PathBuf,
    pub git_dir: Option<String>,
    pub merged_key: String,
    pub remote_name: String,
    pub lfs: LfsState,
}

/// Everything one status refresh read.
#[derive(Clone, Debug)]
pub struct StatusSnapshot {
    pub status: git::Status,
    pub stash_count: usize,
    pub git_dir: Option<String>,
    pub operation: Option<GitOperation>,
    pub operation_subject: String,
    pub stopped_to_edit: bool,
    pub lfs: LfsCheck,
    pub uninitialized_submodules: Vec<String>,
    /// The merged check's key and answer when it had to run again; None: nothing it depends on moved.
    pub merged: Option<(String, bool)>,
}

impl WorktreeState {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        WorktreeState {
            path: path.into(), branch: None, head_oid: String::new(), ahead: 0, behind: 0,
            files: vec![], staged: vec![], unstaged: vec![], conflicted: vec![], listed: vec![], change_count: 0,
            stash_count: 0, is_merged: false, merged_key: String::new(), operation: None, operation_subject: String::new(),
            stopped_to_edit: false, git_dir: None, selected_file_id: None, selected_file_ids: HashSet::new(), diff: None,
            commit_message: String::new(), amend: false, busy: None, error: None, remote_name: "origin".into(),
            refs_version: 0, status_version: 0, lfs: LfsState::default(), uninitialized_submodules: vec![],
            submodule_banner_hidden: false,
        }
    }

    pub fn files(&self) -> &[FileChange] { &self.files }
    pub fn staged(&self) -> &[FileChange] { &self.staged }
    pub fn unstaged(&self) -> &[FileChange] { &self.unstaged }
    pub fn conflicted(&self) -> &[FileChange] { &self.conflicted }
    pub fn listed(&self) -> &[FileChange] { &self.listed }
    pub fn change_count(&self) -> usize { self.change_count }

    pub fn set_files(&mut self, files: Vec<FileChange>) {
        self.staged = files.iter().filter(|f| f.staged).cloned().collect();
        self.unstaged = files.iter().filter(|f| !f.staged && f.kind != Kind::Conflicted).cloned().collect();
        self.conflicted = files.iter().filter(|f| f.kind == Kind::Conflicted).cloned().collect();
        let sorted = |v: &[FileChange]| { let mut v = v.to_vec(); v.sort_by(|a, b| tree_order(&a.path, &b.path)); v };
        self.listed = [sorted(&self.conflicted), sorted(&self.staged), sorted(&self.unstaged)].concat();
        self.change_count = files.iter().map(|f| f.path.as_str()).collect::<HashSet<_>>().len();
        self.files = files;
    }

    pub fn selected_file(&self) -> Option<&FileChange> {
        self.selected_file_id.as_ref().and_then(|id| self.files.iter().find(|f| f.id() == *id))
    }
    pub fn selected_files(&self) -> Vec<FileChange> { self.files.iter().filter(|f| self.selected_file_ids.contains(&f.id())).cloned().collect() }

    /// Selects one file; the caller loads its diff (`load_diff`).
    pub fn select(&mut self, id: Option<String>) {
        self.selected_file_ids = id.iter().cloned().collect();
        self.selected_file_id = id;
    }

    pub fn status_query(&self) -> StatusQuery {
        StatusQuery { path: self.path.clone(), git_dir: self.git_dir.clone(), merged_key: self.merged_key.clone(),
                      remote_name: self.remote_name.clone(), lfs: self.lfs.clone() }
    }

    /// Takes in a refresh. Returns the file whose diff to (re)load, None when nothing is selected (the diff is
    /// cleared then). A refresh always reloads it: `set_diff` keeps the old one when nothing changed.
    pub fn apply_status(&mut self, s: StatusSnapshot) -> Option<FileChange> {
        self.status_version += 1;
        self.branch = s.status.branch.clone();
        self.head_oid = s.status.oid.clone();
        self.ahead = s.status.ahead;
        self.behind = s.status.behind;
        self.stash_count = s.stash_count;
        if s.git_dir.is_some() { self.git_dir = s.git_dir }
        self.operation = s.operation;
        self.operation_subject = s.operation_subject;
        self.stopped_to_edit = s.stopped_to_edit;
        self.lfs.apply(s.lfs);
        self.uninitialized_submodules = s.uninitialized_submodules;
        if s.status.files != self.files { self.set_files(s.status.files) }
        if self.selected_file().is_none() {
            // Keep the same path selected when it moves between staged/unstaged.
            let p = self.selected_file_id.as_ref().map(|id| id[2..].to_string());
            self.selected_file_id = self.files.iter().find(|f| Some(&f.path) == p.as_ref()).or(self.files.first()).map(FileChange::id);
        }
        let ids: HashSet<String> = self.files.iter().map(FileChange::id).collect();
        self.selected_file_ids.retain(|i| ids.contains(i));
        if self.selected_file_ids.is_empty() { self.selected_file_ids.extend(self.selected_file_id.clone()) }
        if let Some((key, merged)) = s.merged {
            self.merged_key = key;
            self.is_merged = merged;
        }
        let f = self.selected_file().cloned();
        if f.is_none() { self.diff = None }
        f
    }

    /// The loaded diff, unless it shows what is already shown: that keeps the table (and its scroll and selection)
    /// when nothing changed on disk. True when it was replaced.
    pub fn set_diff(&mut self, d: Option<FileDiff>) -> bool {
        if let (Some(new), Some(old)) = (&d, &self.diff)
            && old.path() == new.path() && old.header_lines == new.header_lines && old.lines.len() == new.lines.len()
                && old.lines.iter().zip(&new.lines).all(|(a, b)| old.text(a) == new.text(b)) {
                return false;
            }
        self.diff = d;
        true
    }

    /// The sidebar moved to another worktree: let go of what's big and quick to rebuild (the shown diff). The status
    /// list, selections, tabs and scopes stay, so coming back looks the same.
    pub fn release_heavy_state(&mut self) { self.diff = None }

    /// After a fetch, pull or push: remote refs moved without moving HEAD; History reloads on its next showing.
    pub fn bump_refs(&mut self) { self.refs_version += 1 }
}

/// One status refresh, off the main thread: status, stash count, an operation in progress, LFS, submodules, and
/// whether the branch is merged (only rechecked when the branch or its target moved).
pub fn read_status(q: &StatusQuery) -> Result<StatusSnapshot> {
    let dir = q.path.as_path();
    let stack = UndoStack::for_path(dir);
    let undo_stamp = stack.stamp();
    let status = git::status(dir)?;
    stack.prune(status.branch.as_deref(), &status.oid, Some(undo_stamp));
    let stash_count = git::string(dir, ["stash", "list", "--format=%H"]).map(|s| s.lines().filter(|l| !l.is_empty()).count()).unwrap_or(0);
    let git_dir = q.git_dir.clone().or_else(|| git::git_dir(dir));
    let (operation, operation_subject, stopped_to_edit) = git_dir.as_deref().map(|g| detect_operation(dir, Path::new(g))).unwrap_or_default();
    let lfs = q.lfs.check(dir);
    let uninitialized_submodules = submodules::uninitialized(dir);
    let merged = detect_merged(dir, status.branch.as_deref(), &status.oid, &q.remote_name, &q.merged_key);
    Ok(StatusSnapshot { status, stash_count, git_dir, operation, operation_subject, stopped_to_edit, lfs, uninitialized_submodules, merged })
}

/// Runs on every status refresh, so the check itself only happens when the branch or its target moved.
fn detect_merged(dir: &Path, branch: Option<&str>, head: &str, remote: &str, last_key: &str) -> Option<(String, bool)> {
    let target = branch.and_then(|b| git::integration_target(remote, dir).filter(|t| t.0 != b && t.0 != format!("{remote}/{b}")));
    let (Some(branch), Some((name, oid))) = (branch, target) else { return Some((String::new(), false)) };
    let key = format!("{head}{oid}");
    if key == last_key { return None }
    let merged = git::is_merged(branch, &name, dir);
    Some((key, merged))
}

/// The operation waiting in the git dir, the commit it's applying ("abc1234 subject"), and whether it's an edit stop.
pub fn detect_operation(dir: &Path, git_dir: &Path) -> (Option<GitOperation>, String, bool) {
    let has = |n: &str| git_dir.join(n).exists();
    let op = if has("rebase-merge") || has("rebase-apply") { Some(GitOperation::Rebase) }
        else if has("CHERRY_PICK_HEAD") { Some(GitOperation::CherryPick) }
        else if has("REVERT_HEAD") { Some(GitOperation::Revert) }
        else if has("MERGE_HEAD") { Some(GitOperation::Merge) }
        else { None };
    let edit = op == Some(GitOperation::Rebase) && has("rebase-merge/amend"); // written only for an edit stop
    let head = op.and_then(|o| match o {
        GitOperation::CherryPick => Some("CHERRY_PICK_HEAD"),
        GitOperation::Revert => Some("REVERT_HEAD"),
        GitOperation::Merge => Some("MERGE_HEAD"),
        GitOperation::Rebase => has("rebase-merge/stopped-sha").then_some("rebase-merge/stopped-sha"),
    });
    let subject = head.and_then(|h| std::fs::read_to_string(git_dir.join(h)).ok())
        .map(|sha| git::trimmed(dir, ["log", "-1", "--format=%h %s", sha.trim()]).unwrap_or_default())
        .unwrap_or_default();
    (op, subject, edit)
}

/// The diff of a status entry. Full mode (and Before / After, which show the file itself): the whole file with its
/// changes inline; a new or deleted file's diff already is the whole file.
pub fn load_diff(dir: &Path, f: &FileChange, full: bool, cancel: Option<&Cancel>) -> Result<Option<FileDiff>> {
    let full = full && !matches!(f.kind, Kind::Untracked | Kind::Added | Kind::Deleted);
    let d = git::diff_for(f, dir, if full { 1_000_000 } else { 3 }, cancel)?;
    Ok(if full { crate::diff::whole_file(d) } else { d })
}

// MARK: - Actions on a worktree
// Each blocks and records its undo. The UI shows `busy` while one runs, puts an error in `error`, then refreshes.

fn paths(fs: &[FileChange]) -> Vec<&str> { fs.iter().map(|f| f.path.as_str()).collect() }

fn with_orig(fs: &[FileChange]) -> Vec<String> {
    fs.iter().flat_map(|f| std::iter::once(f.path.clone()).chain(f.orig_path.clone())).collect()
}

pub fn stage(dir: &Path, fs: &[FileChange]) -> Result<()> {
    let label = undo::files_label("Stage", &paths(fs));
    UndoStack::for_path(dir).run(Some(Recipe::Files { label, paths: vec![], index: true, toast: false }), || {
        git::run(dir, ["add", "-A", "--"].into_iter().chain(paths(fs))).map(drop)
    })
}

pub fn unstage(dir: &Path, fs: &[FileChange]) -> Result<()> {
    // restore --staged fails on an unborn branch; rm --cached handles the initial commit.
    let has_head = git::run(dir, ["rev-parse", "--verify", "HEAD"]).is_ok();
    let label = undo::files_label("Unstage", &paths(fs));
    UndoStack::for_path(dir).run(Some(Recipe::Files { label, paths: vec![], index: true, toast: false }), || {
        if has_head { git::run(dir, ["restore", "--staged", "--"].iter().map(|s| s.to_string()).chain(with_orig(fs))).map(drop) }
        else { git::run(dir, ["rm", "--cached", "-r", "-q", "--"].into_iter().chain(paths(fs))).map(drop) }
    })
}

pub fn discard(dir: &Path, fs: &[FileChange]) -> Result<()> {
    let label = undo::files_label("Discard of", &paths(fs));
    UndoStack::for_path(dir).run(Some(Recipe::Files { label, paths: with_orig(fs), index: false, toast: true }), || {
        let tracked: Vec<&str> = fs.iter().filter(|f| f.kind != Kind::Untracked).map(|f| f.path.as_str()).collect();
        if !tracked.is_empty() { git::run(dir, ["restore", "--worktree", "--"].into_iter().chain(tracked))?; }
        // Untracked files go to the Trash, not oblivion.
        for f in fs.iter().filter(|f| f.kind == Kind::Untracked) { trash(&dir.join(&f.path))? }
        Ok(())
    })
}

/// Moves a file to the Trash.
pub fn trash(p: &Path) -> Result<()> {
    // ponytail: a rename into ~/.Trash (no Finder "Put Back"); another volume goes through Finder.
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| Error::msg("No home folder"))?;
    let bin = home.join(".Trash");
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dest = bin.join(&name);
    let mut n = 2;
    while std::fs::symlink_metadata(&dest).is_ok() { dest = bin.join(format!("{name} {n}")); n += 1 }
    match std::fs::rename(p, &dest) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let script = format!("tell application \"Finder\" to delete POSIX file \"{}\"", p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""));
            let ok = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).output().is_ok_and(|o| o.status.success());
            if ok { Ok(()) } else { Err(Error::msg(format!("Couldn't move {name} to the Trash"))) }
        }
        Err(e) => Err(e.into()),
    }
}

/// Stage, unstage or discard just these diff lines (indices into `d.lines`): a hunk or a selection. `d` must be the
/// diff of `f` (the pane may briefly show the previous file's diff while the next loads; never act on a stale one).
pub fn apply_lines(dir: &Path, d: &FileDiff, f: &FileChange, action: HunkAction, lines: &HashSet<usize>) -> Result<()> {
    if d.path() != f.path && d.old_path.as_deref() != Some(&f.path) { return Ok(()) }
    if f.kind == Kind::Untracked {
        return if action == HunkAction::Stage { stage(dir, std::slice::from_ref(f)) } else { discard(dir, std::slice::from_ref(f)) };
    }
    let Some(text) = d.patch_selecting(lines, action != HunkAction::Stage) else { return Ok(()) };
    let args: &[&str] = match action {
        HunkAction::Stage => &["apply", "--cached", "--whitespace=nowarn", "-"],
        HunkAction::Unstage => &["apply", "--cached", "--reverse", "--whitespace=nowarn", "-"],
        HunkAction::Discard => &["apply", "--reverse", "--whitespace=nowarn", "-"],
    };
    let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
    let recipe = match action {
        HunkAction::Discard => Recipe::Files { label: format!("Discard in {name}"), paths: vec![f.path.clone()], index: false, toast: true },
        HunkAction::Stage => Recipe::Files { label: format!("Stage in {name}"), paths: vec![], index: true, toast: false },
        HunkAction::Unstage => Recipe::Files { label: format!("Unstage in {name}"), paths: vec![], index: true, toast: false },
    };
    UndoStack::for_path(dir).run(Some(recipe), || git::git(dir, args).stdin(text.as_bytes()).run().map(drop))
}

/// Commits the staged changes (hooks and signing run as in a terminal). Undo keeps the changes staged and puts the
/// message back. The caller clears the commit box on success.
pub fn commit(dir: &Path, message: &str, amend: bool) -> Result<()> {
    let msg = message.trim();
    if msg.is_empty() { return Ok(()) }
    let r = Reset { soft: true, offer_revert: !amend, message: Some((msg.to_string(), amend)), ..Reset::new(if amend { "Amend" } else { "Commit" }) };
    UndoStack::for_path(dir).run(Some(Recipe::Reset(r)), || {
        let mut a = vec!["commit", "-F", "-"];
        if amend { a.push("--amend") }
        git::git(dir, a).stdin(msg.as_bytes()).run().map(drop)
    })
}

pub fn last_commit_message(dir: &Path) -> Option<String> { git::trimmed(dir, ["log", "-1", "--format=%B"]).ok() }

/// `git stash push`: everything, or just `paths`. Untracked files only go in with `untracked`.
pub fn stash(dir: &Path, message: &str, untracked: bool, paths: &[String]) -> Result<()> {
    let msg = message.trim();
    let mut a: Vec<&str> = vec!["stash", "push"];
    if untracked { a.push("--include-untracked") }
    if !msg.is_empty() { a.extend(["-m", msg]) }
    if !paths.is_empty() { a.push("--"); a.extend(paths.iter().map(String::as_str)) }
    git::run(dir, a).map(drop)
}

/// `git <op> --continue` with the prepared message, or `--abort`. Continuing records the whole operation for Undo,
/// from where HEAD was before it started.
pub fn finish_operation(dir: &Path, op: GitOperation, abort: bool) -> Result<()> {
    let start = if abort { None } else { undo::pre_operation_head(op, dir) };
    let recipe = start.map(|s| Recipe::Reset(Reset {
        offer_revert: op != GitOperation::Merge && op != GitOperation::Rebase, from: Some(s), ..Reset::new(op.undo_label())
    }));
    UndoStack::for_path(dir).run(recipe, || {
        git::git(dir, ["-c", "core.editor=true", op.command(), if abort { "--abort" } else { "--continue" }]).env([("GIT_EDITOR", "true")]).run().map(drop)
    })
}

/// Resolves a conflicted file wholesale: one side, or (None) "it's fixed as it is on disk".
pub fn resolve(dir: &Path, path: &str, side: Option<ConflictSide>) -> Result<()> {
    if let Some(side) = side {
        // The side may have deleted the file: then resolving means deleting it.
        if git::run(dir, ["checkout", if side == ConflictSide::Current { "--ours" } else { "--theirs" }, "--", path]).is_err() {
            return git::run(dir, ["rm", "-q", "--", path]).map(drop);
        }
    }
    git::run(dir, ["add", "--", path]).map(drop)
}

/// Resolves a conflict by deleting the file (delete/modify conflicts).
pub fn resolve_deleting(dir: &Path, path: &str) -> Result<()> { git::run(dir, ["rm", "-q", "--", path]).map(drop) }

/// fetch / pull / push with the space's credentials. A pull moves HEAD: undo goes back (the pulled commits stay on
/// the remote). The caller bumps `refs_version` and refreshes the repository's worktrees.
pub fn remote_op(dir: &Path, args: &[&str], env: &[(String, String)]) -> Result<()> {
    let recipe = (args.first() == Some(&"pull")).then(|| Recipe::Reset(Reset { published: false, ..Reset::new("Pull") }));
    UndoStack::for_path(dir).run(recipe, || git::git(dir, args).env(env.iter().map(|(k, v)| (k, v))).run().map(drop))
}

/// Switches the worktree to `b`, recorded for Undo (back to the branch, or the detached commit, it was on).
/// A remote "origin/x" with no local x makes a new local x tracking it. Conflicting changes, or a branch checked out
/// in another worktree: git refuses and says why. (A branch checked out elsewhere: the UI selects that worktree.)
pub fn switch_branch(dir: &Path, b: &str, remote: bool, current: Option<&str>, head_oid: &str) -> Result<()> {
    let args: Vec<String> = match b.split_once('/').filter(|_| remote) {
        Some((_, local)) => vec!["switch".into(), "-c".into(), local.into(), "--track".into(), b.into()],
        None => vec!["switch".into(), b.into()],
    };
    let back: Vec<String> = match current {
        Some(c) => vec!["switch".into(), c.into()],
        None => vec!["switch".into(), "--detach".into(), head_oid.into()],
    };
    let stack = UndoStack::for_path(dir);
    stack.track(|| {
        git::run(dir, &args)?;
        stack.record(format!("Switch to {b}"), undo::Restore::Git(back), Default::default());
        Ok(())
    })
}

/// Undo of the newest action. A refused undo comes back as `Error::Refused` (with a Revert offer when it's one
/// pushed commit: `history::revert_sha`).
pub fn undo_last(dir: &Path) -> Result<Option<undo::Undone>> { UndoStack::for_path(dir).undo() }

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.name", "t"], &["config", "user.email", "t@t"], &["config", "commit.gpgsign", "false"]] {
            git::run(d.path(), a).unwrap();
        }
        d
    }
    fn commit_all(dir: &Path, m: &str) {
        git::run(dir, ["add", "."]).unwrap();
        git::run(dir, ["commit", "-qm", m]).unwrap();
    }

    #[test]
    fn tells_merged_branches_from_new_ones() {
        let d = repo();
        let dir = d.path();
        let c = |name: &str| { std::fs::write(dir.join(name), name).unwrap(); commit_all(dir, name) };
        let g = |a: &[&str]| { git::run(dir, a).unwrap(); };
        c("base");
        g(&["branch", "fresh"]);
        g(&["switch", "-qc", "squashed"]); c("s1"); c("s2");
        g(&["switch", "-q", "main"]);
        g(&["switch", "-qc", "fastForwarded"]); c("ff");
        g(&["switch", "-q", "main"]);
        g(&["switch", "-qc", "open"]); c("o");
        g(&["switch", "-q", "main"]);
        g(&["merge", "-q", "--squash", "squashed"]);
        g(&["commit", "-qm", "squash"]);
        c("later"); // target moved on after the squash
        g(&["merge", "-q", "--no-edit", "fastForwarded"]);
        let (target, _) = git::integration_target("origin", dir).unwrap();
        assert_eq!(target, "main");
        assert!(git::is_merged("squashed", &target, dir));
        assert!(git::is_merged("fastForwarded", &target, dir));
        assert!(!git::is_merged("fresh", &target, dir));
        assert!(!git::is_merged("open", &target, dir));
    }

    #[test]
    fn stages_single_hunk_against_real_repo() {
        let d = repo();
        let dir = d.path();
        let file = dir.join("f.txt");
        let mut lines: Vec<String> = (1..=60).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        commit_all(dir, "init");
        lines[2] = "CHANGED 3".into();
        lines[50] = "CHANGED 51".into();
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();

        let change = git::status(dir).unwrap().files.remove(0);
        let diff = git::diff_for(&change, dir, 3, None).unwrap().unwrap();
        assert_eq!(diff.hunks.len(), 2);
        git::git(dir, ["apply", "--cached", "-"]).stdin(diff.patch_for_hunk(1).as_bytes()).run().unwrap();

        let after = git::status(dir).unwrap().files;
        assert_eq!(after.len(), 2); // same file: staged + unstaged
        let staged = git::diff_for(after.iter().find(|f| f.staged).unwrap(), dir, 3, None).unwrap().unwrap();
        assert!(staged.hunks.len() == 1 && staged.lines.iter().any(|l| staged.text(l) == "CHANGED 51"));
        git::git(dir, ["apply", "--cached", "--reverse", "-"]).stdin(staged.patch_for_hunk(0).as_bytes()).run().unwrap();
        assert!(git::status(dir).unwrap().files.iter().all(|f| !f.staged));
    }

    #[test]
    fn stages_and_discards_single_lines_against_real_repo() {
        let d = repo();
        let dir = d.path();
        let file = dir.join("f.txt");
        let original: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, original.join("\n") + "\n").unwrap();
        commit_all(dir, "init");
        // One hunk, four changes: line 4 and 5 replaced, two lines added after line 7.
        let mut edited = original.clone();
        edited[3] = "FOUR".into();
        edited[4] = "FIVE".into();
        edited.splice(7..7, ["NEW A".to_string(), "NEW B".to_string()]);
        std::fs::write(&file, edited.join("\n") + "\n").unwrap();
        let diff_of = |staged: bool| {
            let f = git::status(dir).unwrap().files.into_iter().find(|f| f.staged == staged).unwrap();
            (git::diff_for(&f, dir, 3, None).unwrap().unwrap(), f)
        };
        let index = |d: &FileDiff, kind, text: &str| d.lines.iter().position(|l| l.kind == kind && d.text(l) == text).unwrap();
        use crate::diff::LineKind::*;

        // Stage only "+NEW A" (through the action, so with its undo): the index gets that line and nothing else.
        let (d, f) = diff_of(false);
        assert_eq!(d.hunks.len(), 1);
        apply_lines(dir, &d, &f, HunkAction::Stage, &HashSet::from([index(&d, Added, "NEW A")])).unwrap();
        let mut expected = original.clone();
        expected.insert(7, "NEW A".into());
        assert_eq!(git::string(dir, ["show", ":f.txt"]).unwrap(), expected.join("\n") + "\n");

        // Discard only "-line 4" / "+FOUR" from the working tree: line 4 comes back, the other edits stay.
        let (d, f) = diff_of(false);
        apply_lines(dir, &d, &f, HunkAction::Discard, &HashSet::from([index(&d, Removed, "line 4"), index(&d, Added, "FOUR")])).unwrap();
        let mut on_disk = edited.clone();
        on_disk[3] = "line 4".into();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), on_disk.join("\n") + "\n");

        // Unstage "+NEW A" again.
        let (d, f) = diff_of(true);
        apply_lines(dir, &d, &f, HunkAction::Unstage, &HashSet::from([index(&d, Added, "NEW A")])).unwrap();
        assert!(git::status(dir).unwrap().files.iter().all(|f| !f.staged));
        // Nothing selected that is a change → no patch.
        assert_eq!(d.patch_selecting(&HashSet::new(), false), None);
        // And the discard can be undone (the unstage above is the newest entry, then it).
        assert!(undo_last(dir).unwrap().is_some());
        let undone = undo_last(dir).unwrap().unwrap();
        assert_eq!(undone.label, "Discard in f.txt");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), edited.join("\n") + "\n");
    }

    #[test]
    fn a_status_refresh_selects_and_keeps_the_path() {
        let d = repo();
        let dir = d.path();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        commit_all(dir, "init");
        std::fs::write(dir.join("a.txt"), "2").unwrap();
        std::fs::write(dir.join("b.txt"), "new").unwrap();
        let mut st = WorktreeState::new(dir);
        let snap = read_status(&st.status_query()).unwrap();
        let f = st.apply_status(snap).unwrap();
        assert_eq!(f.id(), "W:a.txt");
        assert_eq!(st.listed().len(), 2);
        assert!(st.operation.is_none() && st.stash_count == 0);
        let d1 = load_diff(dir, &f, false, None).unwrap();
        assert!(st.set_diff(d1));
        assert!(!st.set_diff(load_diff(dir, &f, false, None).unwrap())); // same content: the shown diff stays
        stage(dir, &[f]).unwrap();
        let f = st.apply_status(read_status(&st.status_query()).unwrap()).unwrap();
        assert_eq!(f.id(), "S:a.txt"); // moved to staged, still selected
        assert_eq!(st.staged().len(), 1);
    }

    #[test]
    fn detects_a_stopped_cherry_pick() {
        let d = repo();
        let dir = d.path();
        std::fs::write(dir.join("f"), "a\n").unwrap();
        commit_all(dir, "base");
        git::run(dir, ["switch", "-qc", "side"]).unwrap();
        std::fs::write(dir.join("f"), "side\n").unwrap();
        commit_all(dir, "side change");
        let side = git::trimmed(dir, ["rev-parse", "HEAD"]).unwrap();
        git::run(dir, ["switch", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f"), "main\n").unwrap();
        commit_all(dir, "main change");
        let stopped = crate::history::cherry_pick(dir, &crate::history::parse_log(
            git::run(dir, ["log", "-1", &format!("--format={}", crate::history::LOG_FORMAT), &side]).unwrap().as_slice())[0]).unwrap();
        assert!(stopped);
        let (op, subject, edit) = detect_operation(dir, Path::new(&git::git_dir(dir).unwrap()));
        assert!(op == Some(GitOperation::CherryPick) && subject.ends_with("side change") && !edit);
        resolve(dir, "f", Some(ConflictSide::Incoming)).unwrap();
        finish_operation(dir, GitOperation::CherryPick, false).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "side\n");
        assert_eq!(UndoStack::for_path(dir).title(), "Undo Cherry-pick");
    }

    #[test]
    fn worktrees_removed_outside_are_forgotten() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("r");
        let wt = base.path().join("r-wt");
        std::fs::create_dir_all(&dir).unwrap();
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.name", "t"], &["config", "user.email", "t@t"], &["commit", "-q", "--allow-empty", "-m", "one"]] {
            git::run(&dir, a).unwrap();
        }
        git::run(&dir, ["worktree", "add", "-q", "-b", "x", wt.to_str().unwrap()]).unwrap();
        let real = git::trimmed(&dir, ["rev-parse", "--show-toplevel"]).unwrap();
        let mut app = AppState::new(Saved { repos: vec![real.clone()], ..Default::default() });
        app.apply_worktrees(&real, git::worktrees(Path::new(&real)).unwrap());
        let linked = app.worktrees[&real].iter().find(|w| w.path != real).unwrap().path.clone();
        app.select(Some(linked.clone()));
        let _ = UndoStack::for_path(Path::new(&linked));
        git::run(Path::new(&real), ["worktree", "remove", "--force", &linked]).unwrap();
        let gone = app.apply_worktrees(&real, git::worktrees(Path::new(&real)).unwrap());
        assert_eq!(gone, [linked]);
        assert_eq!(app.worktrees[&real].len(), 1);
        assert_eq!(app.selection.as_deref(), Some(real.as_str())); // the selection moved to the repository
    }

    #[test]
    fn spaces_keep_repos_in_one_place() {
        let mut app = AppState::new(Saved { repos: vec!["/r/a".into(), "/r/b".into()], selection: Some("/r/a".into()), ..Default::default() });
        assert_eq!(app.selection.as_deref(), Some("/r/a"));
        let s2 = app.add_space();
        assert_eq!(app.current_space, s2);
        app.add_repo("/r/b"); // opening it here moves it
        assert_eq!(app.spaces[0].repos, ["/r/a"]);
        assert_eq!(app.repo_paths(), ["/r/b"]);
        app.select(Some("/r/a".into())); // jumping into another space takes the sidebar there
        assert_eq!(app.current_space, DEFAULT_SPACE);
        assert_eq!(app.recent[0], "/r/a");
        assert!(app.delete_space(&s2));
        assert_eq!(app.spaces.len(), 1);
        assert_eq!(app.spaces[0].repos, ["/r/a", "/r/b"]);
        app.move_repo("/r/b", "/r/a");
        assert_eq!(app.repo_paths(), ["/r/b", "/r/a"]);
        let json = app.spaces_json();
        let back: Vec<Space> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, app.spaces);
        // The Swift app's JSON reads back.
        let swift: Vec<Space> = serde_json::from_str(r#"[{"id":"default","name":"Default","repos":["/x"],"selection":"/x"},{"id":"A","name":"Work","repos":[]}]"#).unwrap();
        assert!(swift[1].selection.is_none());
    }

    #[test]
    fn suggests_a_sibling_worktree_path() {
        assert_eq!(suggest_worktree_path("/src/loupe", "origin/feat/x"), "/src/loupe-feat-x");
        assert_eq!(suggest_worktree_path("/src/loupe", ""), "/src/loupe-worktree");
    }
}
