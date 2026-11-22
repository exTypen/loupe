//! Undo of what Loupe did to a worktree.
//!
//! Every undo is made of git objects saved before the action, so nothing is lost: blobs for file contents
//! (`hash-object -w`), a tree for the index (`write-tree`), the commit HEAD was on, a stash's commit.
//!
//! One `UndoStack` per worktree (`UndoStack::for_path`), at most 50 entries. Each entry remembers the branch and HEAD
//! it left; once HEAD moves some other way (a commit in a terminal), the entries above it no longer apply and are
//! dropped. An undo that would rewrite pushed history is refused with a message (and a Revert offer for a single
//! commit); one whose files changed since is refused and dropped.
//!
//! Two ways in:
//!
//! ```ignore
//! // A history-changing command: undo = `reset --keep` to where HEAD was before it, recorded only if the
//! // command finished (no rebase/merge left stopped) and moved HEAD.
//! stack.run(Some(Recipe::Reset(Reset::new("Rebase onto main"))), || git::run(dir, ["rebase", "main"]).map(drop))?;
//! // Files it rewrites: their contents (and the index) come back, if nobody touched them since.
//! stack.run(Some(Recipe::Files { label, paths, index: true, toast: true }), || ...)?;
//! // Anything else: do the work, then record how to take it back.
//! stack.record(label, Restore::Git(vec!["branch".into(), "-m".into(), new, old]), RecordOpts::default());
//! ```
//!
//! A rebase or merge that stops on conflicts is recorded when Loupe continues it (`model::finish_operation`).
//! Calls block (they run git); the stack is safe to share between threads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::git::{self, Error, Result};
use crate::model::GitOperation;

#[derive(Clone, Debug, PartialEq)]
pub struct UndoRefusal {
    pub message: String,
    /// A commit to offer `git revert` for instead.
    pub revert: Option<String>,
    /// The entry can never apply again.
    pub drop: bool,
}

fn refused(message: impl Into<String>, revert: Option<String>, drop: bool) -> Error {
    Error::Refused(UndoRefusal { message: message.into(), revert, drop })
}

/// How an entry is taken back.
#[derive(Clone, Debug)]
pub enum Restore {
    /// `reset --keep` (`--soft` keeps the changes staged) to `before`; None: the first commit, back to unborn.
    Reset { before: Option<String>, soft: bool },
    Files(FileSnapshot),
    /// Takes back what applying the stash changed, then stores the stash again (`store`: sha, message).
    Stash { snapshot: Option<FileSnapshot>, store: Option<(String, String)> },
    /// A git command that takes it back (`switch main`).
    Git(Vec<String>),
    Nothing,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub label: String,     // "Discard of a.txt": the menu says "Undo Discard of a.txt"
    pub head: String,      // `key(branch, oid)` after the action
    pub published: Option<String>, // the commit undoing removes; refused once a remote branch has it
    pub offer_revert: bool, // refused for being pushed → offer `git revert` of `published`
    pub restore: Restore,
    /// A commit's message (and whether it amended): undoing it puts it back in the commit box.
    pub message: Option<(String, bool)>,
}

/// "Discard of a.txt · Undo" after a destructive action, "Undone: …" after an undo. The UI hides it after a few seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub entry: Option<u64>, // None: it reports an undo
}

/// History moved: `reset --keep` (`soft`: `--soft`, keeping the changes staged) to HEAD from before, or `from`.
#[derive(Clone, Debug)]
pub struct Reset {
    pub label: String,
    pub soft: bool,
    pub published: bool,
    pub offer_revert: bool,
    pub from: Option<String>,
    pub message: Option<(String, bool)>,
}

impl Reset {
    pub fn new(label: impl Into<String>) -> Self {
        Reset { label: label.into(), soft: false, published: true, offer_revert: false, from: None, message: None }
    }
}

/// How an action is taken back; `UndoStack::run` saves what's needed before the action and records it after.
#[derive(Clone, Debug)]
pub enum Recipe {
    Reset(Reset),
    /// File contents (and with `index`, the whole index) back to what they were before.
    Files { label: String, paths: Vec<String>, index: bool, toast: bool },
    /// A stash commit is stored again; `applied`: what applying it changed is taken back first.
    Stash { label: String, sha: String, message: String, applied: bool, dropped: bool },
}

#[derive(Clone, Debug, Default)]
pub struct RecordOpts {
    pub published: Option<String>,
    pub offer_revert: bool,
    pub toast: bool,
    pub message: Option<(String, bool)>,
}

/// What an undo took back.
#[derive(Clone, Debug, PartialEq)]
pub struct Undone {
    pub label: String,
    /// Put back in the commit box, unless something was typed there since.
    pub message: Option<(String, bool)>,
}

pub const LIMIT: usize = 50;

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    toast: Option<Toast>,
    in_flight: u32, // an action is between its git command and its record: don't prune
    stamp: u64,     // bumps whenever an action or undo starts or ends
    undoing: bool,
    next_id: u64,
}

pub struct UndoStack {
    pub path: PathBuf,
    state: Mutex<State>,
}

static STACKS: LazyLock<Mutex<HashMap<PathBuf, Arc<UndoStack>>>> = LazyLock::new(Default::default);

/// Holds an action in flight (nothing pruned meanwhile) for as long as it lives.
struct InFlight<'a>(&'a UndoStack);
impl<'a> InFlight<'a> {
    fn new(s: &'a UndoStack) -> Self {
        let mut st = s.state.lock().unwrap();
        st.in_flight += 1;
        st.stamp += 1;
        InFlight(s)
    }
}
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut st = self.0.state.lock().unwrap();
        st.in_flight -= 1;
        st.stamp += 1;
    }
}

impl UndoStack {
    pub fn new(path: impl Into<PathBuf>) -> Self { UndoStack { path: path.into(), state: Mutex::default() } }

    /// The worktree's stack.
    pub fn for_path(path: &Path) -> Arc<UndoStack> {
        STACKS.lock().unwrap().entry(path.to_path_buf()).or_insert_with(|| Arc::new(UndoStack::new(path))).clone()
    }

    pub fn forget(path: &Path) { STACKS.lock().unwrap().remove(path); }

    pub fn title(&self) -> String {
        self.state.lock().unwrap().entries.last().map(|e| format!("Undo {}", e.label)).unwrap_or_else(|| "Undo".into())
    }
    pub fn labels(&self) -> Vec<String> { self.state.lock().unwrap().entries.iter().map(|e| e.label.clone()).collect() }
    pub fn is_empty(&self) -> bool { self.state.lock().unwrap().entries.is_empty() }
    pub fn last_id(&self) -> Option<u64> { self.state.lock().unwrap().entries.last().map(|e| e.id) }
    pub fn toast(&self) -> Option<Toast> { self.state.lock().unwrap().toast.clone() }
    /// The toast's few seconds are over.
    pub fn clear_toast(&self, id: u64) {
        let mut st = self.state.lock().unwrap();
        if st.toast.as_ref().is_some_and(|t| t.id == id) { st.toast = None }
    }
    /// Read before a status: if an action runs meanwhile, that status may predate it and mustn't prune.
    pub fn stamp(&self) -> u64 { self.state.lock().unwrap().stamp }

    // MARK: Recording

    pub fn record(&self, label: impl Into<String>, restore: Restore, opts: RecordOpts) {
        let _f = InFlight::new(self);
        let head = head(&self.path);
        let mut st = self.state.lock().unwrap();
        st.next_id += 1;
        let id = st.next_id;
        let label = label.into();
        st.entries.push(Entry { id, label: label.clone(), head, published: opts.published, offer_revert: opts.offer_revert, restore, message: opts.message });
        if st.entries.len() > LIMIT {
            let n = st.entries.len() - LIMIT;
            st.entries.drain(..n);
        }
        st.next_id += 1;
        st.toast = opts.toast.then(|| Toast { id: st.next_id, text: label, entry: Some(id) });
    }

    /// Runs `op` with nothing pruned meanwhile; `record` inside it sees the state `op` left.
    pub fn track<T>(&self, op: impl FnOnce() -> T) -> T {
        let _f = InFlight::new(self);
        op()
    }

    /// Runs `op` (the action) and records its undo per `recipe`; None just runs it. Nothing is recorded if `op` fails.
    pub fn run(&self, recipe: Option<Recipe>, op: impl FnOnce() -> Result<()>) -> Result<()> {
        let Some(recipe) = recipe else { return op() };
        let dir = self.path.clone();
        let _f = InFlight::new(self);
        match recipe {
            Recipe::Reset(r) => {
                let before = r.from.clone().or_else(|| oid(&dir));
                op()?;
                let after = oid(&dir);
                let Some(after) = after.filter(|a| Some(a) != before.as_ref()) else { return Ok(()) };
                if operation_in_progress(&dir) { return Ok(()) }
                if before.is_none() && !r.soft { return Ok(()) } // nothing to reset --keep to
                self.record(r.label, Restore::Reset { before, soft: r.soft },
                            RecordOpts { published: r.published.then_some(after), offer_revert: r.offer_revert, toast: false, message: r.message });
            }
            Recipe::Files { label, paths, index, toast } => {
                let mut snap = FileSnapshot::capture(&paths, index, &dir)?;
                op()?;
                snap.seal(&dir)?;
                if snap.is_empty() { return Ok(()) }
                self.record(label, Restore::Files(snap), RecordOpts { toast, ..Default::default() });
            }
            Recipe::Stash { label, sha, message, applied, dropped } => {
                let mut snap = None;
                if applied {
                    let names = git::run(&dir, ["stash", "show", "-z", "--name-only", "--no-renames", "--include-untracked", &sha])?;
                    let names: Vec<String> = names.split(|&b| b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).into_owned()).collect();
                    snap = Some(FileSnapshot::capture(&names, true, &dir)?);
                }
                op()?;
                if let Some(s) = snap.as_mut() { s.seal(&dir)? }
                self.record(label, Restore::Stash { snapshot: snap, store: dropped.then_some((sha, message)) },
                            RecordOpts { toast: dropped, ..Default::default() });
            }
        }
        Ok(())
    }

    // MARK: Undoing

    /// Takes back the newest entry that still applies. `Error::Refused` when that's unsafe; None when another undo or
    /// an action is running.
    pub fn undo(&self) -> Result<Option<Undone>> {
        {
            // A second ⌘Z while the first runs would restore the same entry twice; one during an action would undo the
            // entry below the one that action is about to record.
            let mut st = self.state.lock().unwrap();
            if st.undoing || st.in_flight > 0 { return Ok(None) }
            st.undoing = true;
        }
        let r = {
            let _f = InFlight::new(self);
            self.undo_newest()
        };
        self.state.lock().unwrap().undoing = false;
        r.map(Some)
    }

    fn undo_newest(&self) -> Result<Undone> {
        let head = head(&self.path);
        let e = {
            let mut st = self.state.lock().unwrap();
            drop_unless(&mut st, &head);
            st.entries.last().cloned()
        };
        let Some(e) = e else { return Err(refused("Nothing to undo in this worktree.", None, false)) };
        if let Some(sha) = &e.published {
            let out = git::git(&self.path, ["branch", "-r", "--contains", sha]).allow_failure().string()?;
            if let Some(on) = out.lines().map(str::trim).find(|l| !l.is_empty() && !l.contains(" -> ")) {
                let revert = if e.offer_revert { " Revert it instead: a new commit that takes it back." } else { "" };
                return Err(refused(format!("{} is already on {on}. Undoing it would rewrite pushed history.{revert}", e.label),
                                   e.offer_revert.then(|| sha.clone()), false));
            }
        }
        let r = restore(&e, &self.path);
        let mut st = self.state.lock().unwrap();
        match r {
            Err(Error::Refused(r)) if r.drop => {
                st.entries.retain(|x| x.id != e.id);
                return Err(Error::Refused(r));
            }
            Err(x) => return Err(x),
            Ok(()) => {}
        }
        st.entries.retain(|x| x.id != e.id);
        st.next_id += 1;
        st.toast = Some(Toast { id: st.next_id, text: format!("Undone: {}", e.label), entry: None });
        Ok(Undone { label: e.label, message: e.message })
    }

    /// Status refresh: HEAD (or the branch) moved without Loupe, so the newest entries no longer apply.
    /// `since`: `stamp()` read before the status was; if an action ran meanwhile, that status may predate it.
    pub fn prune(&self, branch: Option<&str>, oid: &str, since: Option<u64>) {
        let mut st = self.state.lock().unwrap();
        if st.in_flight > 0 || since.is_some_and(|s| s != st.stamp) { return }
        drop_unless(&mut st, &key(branch, oid));
    }
}

fn drop_unless(st: &mut State, head: &str) {
    if st.entries.last().is_none_or(|e| e.head == head) { return }
    while st.entries.last().is_some_and(|e| e.head != head) { st.entries.pop(); }
    if let Some(t) = st.toast.as_ref().and_then(|t| t.entry)
        && !st.entries.iter().any(|e| e.id == t) { st.toast = None }
}

fn restore(e: &Entry, dir: &Path) -> Result<()> {
    match &e.restore {
        Restore::Reset { before: Some(b), soft } => git::run(dir, ["reset", if *soft { "--soft" } else { "--keep" }, b]).map(drop),
        Restore::Reset { before: None, .. } => git::run(dir, ["update-ref", "-d", "HEAD"]).map(drop), // the first commit: back to unborn
        Restore::Files(s) => s.restore(dir, &e.label),
        Restore::Stash { snapshot, store } => {
            let put_back = || match store {
                Some((sha, message)) => git::run(dir, ["stash", "store", "-m", message, sha]).map(drop),
                None => Ok(()),
            };
            if let Some(s) = snapshot {
                match s.restore(dir, &e.label) {
                    Err(Error::Refused(mut r)) if store.is_some() => {
                        put_back()?; // the files can't go back, but the stash still can
                        r.message += " The stash is back in the list.";
                        return Err(Error::Refused(r));
                    }
                    r => r?,
                }
            }
            put_back()
        }
        Restore::Git(args) => git::run(dir, args).map(drop),
        Restore::Nothing => Ok(()),
    }
}

// MARK: Git state

pub fn key(branch: Option<&str>, oid: &str) -> String {
    format!("{} {}", branch.unwrap_or("HEAD"), if oid == "(initial)" { "" } else { oid })
}

pub fn oid(dir: &Path) -> Option<String> {
    git::git(dir, ["rev-parse", "-q", "--verify", "HEAD"]).allow_failure().trimmed().ok().filter(|s| !s.is_empty())
}

pub fn head(dir: &Path) -> String {
    let b = git::git(dir, ["symbolic-ref", "-q", "--short", "HEAD"]).allow_failure().trimmed().unwrap_or_default();
    key((!b.is_empty()).then_some(b.as_str()), &oid(dir).unwrap_or_default())
}

pub const OPERATION_FILES: [&str; 5] = ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"];

/// A rebase, merge, cherry-pick or revert stopped partway (an interactive rebase's "edit" exits 0 there).
pub fn operation_in_progress(dir: &Path) -> bool {
    git::git_dir(dir).is_some_and(|d| OPERATION_FILES.iter().any(|f| Path::new(&d).join(f).exists()))
}

/// Where HEAD was before `op` started, read while it's stopped: a rebase keeps it in orig-head; the others
/// haven't committed yet, so it's HEAD.
pub fn pre_operation_head(op: GitOperation, dir: &Path) -> Option<String> {
    if op == GitOperation::Rebase {
        let d = git::git_dir(dir)?;
        return ["rebase-merge", "rebase-apply"].iter()
            .find_map(|s| std::fs::read_to_string(Path::new(&d).join(s).join("orig-head")).ok())
            .map(|s| s.trim().to_string());
    }
    oid(dir)
}

impl GitOperation {
    pub fn undo_label(self) -> &'static str {
        match self { GitOperation::Merge => "Merge", GitOperation::CherryPick => "Cherry-pick", GitOperation::Revert => "Revert", GitOperation::Rebase => "Rebase" }
    }
}

/// "Stage a.txt" for one file, "Stage 3 Files" for more.
pub fn files_label(verb: &str, paths: &[&str]) -> String {
    if paths.len() == 1 { format!("{verb} {}", paths[0].rsplit('/').next().unwrap_or(paths[0])) } else { format!("{verb} {} Files", paths.len()) }
}

// MARK: - File snapshot

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content { Blob { sha: String, executable: bool }, Link(String) }

#[derive(Clone, Debug)]
pub struct SnapFile {
    pub path: String,
    pub before: Option<Content>, // None: no file
    pub after: Option<Content>,
}

/// Some files' contents (and optionally the index) before and after an action, kept as git objects.
#[derive(Clone, Debug, Default)]
pub struct FileSnapshot {
    pub files: Vec<SnapFile>,
    pub index_before: Option<String>,
    pub index_after: Option<String>,
}

impl FileSnapshot {
    pub fn is_empty(&self) -> bool { self.files.is_empty() && self.index_before == self.index_after }

    pub fn capture(paths: &[String], index: bool, dir: &Path) -> Result<Self> {
        let mut unique: Vec<String> = paths.to_vec();
        unique.sort();
        unique.dedup();
        let before = contents(&unique, true, dir)?;
        Ok(FileSnapshot {
            files: unique.into_iter().zip(before).map(|(path, before)| SnapFile { path, before, after: None }).collect(),
            index_before: if index { tree(dir) } else { None },
            index_after: None,
        })
    }

    /// Records the state the action left, which undo checks is still there.
    pub fn seal(&mut self, dir: &Path) -> Result<()> {
        let paths: Vec<String> = self.files.iter().map(|f| f.path.clone()).collect();
        for (f, a) in self.files.iter_mut().zip(contents(&paths, false, dir)?) { f.after = a }
        self.files.retain(|f| f.before != f.after);
        if self.index_before.is_some() { self.index_after = tree(dir) }
        Ok(())
    }

    pub fn restore(&self, dir: &Path, label: &str) -> Result<()> {
        let paths: Vec<String> = self.files.iter().map(|f| f.path.clone()).collect();
        let now = contents(&paths, false, dir)?;
        // A directory reads as "no file": one made since at a path that had none must not be deleted below.
        let changed: Vec<&str> = self.files.iter().zip(&now)
            .filter(|(f, n)| f.after != **n || is_directory(&dir.join(&f.path)))
            .map(|(f, _)| f.path.as_str()).collect();
        let index_moved = self.index_before != self.index_after;
        let index_now = if index_moved { tree(dir) } else { self.index_after.clone() };
        if !changed.is_empty() || index_now != self.index_after {
            let what = match changed.len() { 0 => "The staged changes".to_string(), 1 => changed[0].to_string(), n => format!("{n} files") };
            return Err(refused(format!("{what} changed since {label}, so undoing it would overwrite that. Nothing was changed."), None, true));
        }
        for f in &self.files {
            let p = dir.join(&f.path);
            if std::fs::symlink_metadata(&p).is_ok() { std::fs::remove_file(&p)? }
            match &f.before {
                None => {}
                Some(Content::Link(target)) => {
                    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)? }
                    std::os::unix::fs::symlink(target, &p)?;
                }
                Some(Content::Blob { sha, executable }) => {
                    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)? }
                    std::fs::write(&p, git::run(dir, ["cat-file", "blob", sha])?)?;
                    if *executable {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
                    }
                }
            }
        }
        // -m keeps the stat cache of unchanged files. It refuses ("not uptodate") a path whose index entry differs
        // from both the tree and the file on disk, as after staging some lines of a file with other edits: then a
        // plain read-tree, which only costs the next status a re-stat.
        if index_moved
            && let Some(t) = &self.index_before
                && git::run(dir, ["read-tree", "-m", t]).is_err() { git::run(dir, ["read-tree", t])?; }
        Ok(())
    }
}

/// Each path's content; `write` stores blobs. Raw bytes (`--no-filters`): an LFS or CRLF filter must not
/// turn the saved copy into something else. Directories (submodules) are skipped as absent.
fn contents(paths: &[String], write: bool, dir: &Path) -> Result<Vec<Option<Content>>> {
    use std::os::unix::fs::PermissionsExt;
    let mut result = vec![None; paths.len()];
    let mut regular = vec![];
    for (i, p) in paths.iter().enumerate() {
        let full = dir.join(p);
        if let Ok(t) = std::fs::read_link(&full) { result[i] = Some(Content::Link(t.to_string_lossy().into_owned())); continue }
        if std::fs::metadata(&full).is_ok_and(|m| !m.is_dir()) { regular.push(i) }
    }
    if regular.is_empty() { return Ok(result) }
    // Paths on stdin: a discard of thousands of files would overflow the argument list.
    let list: String = regular.iter().map(|&i| format!("{}\n", paths[i])).collect();
    let mut args = vec!["hash-object", "--no-filters", "--stdin-paths"];
    if write { args.push("-w") }
    let out = git::git(dir, args).stdin(list.as_bytes()).string()?;
    for (&i, sha) in regular.iter().zip(out.lines()) {
        let executable = std::fs::metadata(dir.join(&paths[i])).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
        result[i] = Some(Content::Blob { sha: sha.to_string(), executable });
    }
    Ok(result)
}

fn is_directory(p: &Path) -> bool { std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) }

fn tree(dir: &Path) -> Option<String> { git::trimmed(dir, ["write-tree"]).ok() } // None with conflicts in the index

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway repo with one commit of a.txt ("one\n").
    fn repo() -> tempfile::TempDir {
        let d = empty_repo();
        write("one\n", "a.txt", d.path());
        git::run(d.path(), ["add", "a.txt"]).unwrap();
        git::run(d.path(), ["commit", "-q", "-m", "first"]).unwrap();
        d
    }
    fn empty_repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.name", "t"], &["config", "user.email", "t@t"], &["config", "commit.gpgsign", "false"]] {
            git::run(d.path(), a).unwrap();
        }
        d
    }
    fn write(s: &str, name: &str, dir: &Path) { std::fs::write(dir.join(name), s).unwrap() }
    fn read(name: &str, dir: &Path) -> Option<String> { std::fs::read_to_string(dir.join(name)).ok() }
    fn g(dir: &Path, args: &[&str]) -> String { git::trimmed(dir, args).unwrap() }
    fn files(label: &str, paths: &[&str], index: bool, toast: bool) -> Option<Recipe> {
        Some(Recipe::Files { label: label.into(), paths: paths.iter().map(|s| s.to_string()).collect(), index, toast })
    }
    fn refusal(r: Result<Option<Undone>>) -> UndoRefusal {
        match r { Err(Error::Refused(r)) => r, other => panic!("expected a refusal, got {other:?}") }
    }

    #[test]
    fn discard_undo_restores_tracked_and_untracked() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        write("new\n", "n.txt", dir);
        let stack = UndoStack::new(dir);
        stack.run(files("Discard of 2 Files", &["a.txt", "n.txt"], false, true), || {
            git::run(dir, ["restore", "--worktree", "--", "a.txt"])?;
            std::fs::remove_file(dir.join("n.txt"))?;
            Ok(())
        }).unwrap();
        assert!(read("a.txt", dir).as_deref() == Some("one\n") && read("n.txt", dir).is_none());
        assert!(stack.title() == "Undo Discard of 2 Files" && stack.toast().is_some());
        stack.undo().unwrap().unwrap();
        assert_eq!(read("a.txt", dir).as_deref(), Some("two\n"));
        assert_eq!(read("n.txt", dir).as_deref(), Some("new\n"));
        assert!(stack.is_empty());
    }

    #[test]
    fn discard_undo_refuses_when_the_file_changed_since() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        let stack = UndoStack::new(dir);
        stack.run(files("Discard in a.txt", &["a.txt"], false, true), || git::run(dir, ["restore", "--worktree", "--", "a.txt"]).map(drop)).unwrap();
        write("edited later\n", "a.txt", dir);
        refusal(stack.undo());
        assert_eq!(read("a.txt", dir).as_deref(), Some("edited later\n")); // never overwritten
        assert!(stack.is_empty()); // and the undo is gone for good
    }

    #[test]
    fn stage_undo_restores_the_index() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        let stack = UndoStack::new(dir);
        stack.run(files("Stage a.txt", &[], true, false), || git::run(dir, ["add", "a.txt"]).map(drop)).unwrap();
        assert_eq!(g(dir, &["diff", "--cached", "--name-only"]), "a.txt");
        stack.undo().unwrap();
        assert_eq!(g(dir, &["diff", "--cached", "--name-only"]), "");
        assert_eq!(read("a.txt", dir).as_deref(), Some("two\n"));
    }

    #[test]
    fn commit_undo_keeps_changes_staged() {
        let d = repo();
        let dir = d.path();
        let first = g(dir, &["rev-parse", "HEAD"]);
        write("two\n", "a.txt", dir);
        git::run(dir, ["add", "a.txt"]).unwrap();
        let stack = UndoStack::new(dir);
        let r = Reset { soft: true, offer_revert: true, message: Some(("second".into(), false)), ..Reset::new("Commit") };
        stack.run(Some(Recipe::Reset(r)), || git::run(dir, ["commit", "-q", "-m", "second"]).map(drop)).unwrap();
        assert_ne!(g(dir, &["rev-parse", "HEAD"]), first);
        let undone = stack.undo().unwrap().unwrap();
        assert_eq!(g(dir, &["rev-parse", "HEAD"]), first);
        assert_eq!(g(dir, &["diff", "--cached", "--name-only"]), "a.txt");
        assert_eq!(undone.message, Some(("second".into(), false)));
    }

    #[test]
    fn amend_undo_restores_the_previous_commit() {
        let d = repo();
        let dir = d.path();
        let first = g(dir, &["rev-parse", "HEAD"]);
        write("two\n", "a.txt", dir);
        git::run(dir, ["add", "a.txt"]).unwrap();
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Reset(Reset { soft: true, ..Reset::new("Amend") })), || git::run(dir, ["commit", "-q", "--amend", "-m", "first, amended"]).map(drop)).unwrap();
        stack.undo().unwrap();
        assert_eq!(g(dir, &["rev-parse", "HEAD"]), first);
        assert_eq!(g(dir, &["diff", "--cached", "--name-only"]), "a.txt");
    }

    #[test]
    fn undo_of_a_pushed_commit_is_refused_with_a_revert_offer() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        git::run(dir, ["add", "a.txt"]).unwrap();
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Reset(Reset { soft: true, offer_revert: true, ..Reset::new("Commit") })), || git::run(dir, ["commit", "-q", "-m", "second"]).map(drop)).unwrap();
        let head = g(dir, &["rev-parse", "HEAD"]);
        git::run(dir, ["update-ref", "refs/remotes/origin/main", &head]).unwrap(); // as if pushed
        let r = refusal(stack.undo());
        assert!(r.revert.as_deref() == Some(head.as_str()) && r.message.contains("origin/main"));
        assert_eq!(g(dir, &["rev-parse", "HEAD"]), head);
        assert_eq!(stack.labels().len(), 1); // still there: Revert is offered instead
    }

    #[test]
    fn stash_drop_undo_stores_it_again() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        git::run(dir, ["stash", "push", "-q", "-m", "wip"]).unwrap();
        let sha = g(dir, &["rev-parse", "stash@{0}"]);
        let subject = g(dir, &["log", "-1", "--format=%s", &sha]);
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Stash { label: "Drop Stash".into(), sha: sha.clone(), message: subject, applied: false, dropped: true }),
                  || git::run(dir, ["stash", "drop", "-q", "stash@{0}"]).map(drop)).unwrap();
        assert_eq!(g(dir, &["stash", "list"]), "");
        stack.undo().unwrap();
        assert_eq!(g(dir, &["stash", "list", "--format=%H %gs"]), format!("{sha} On main: wip"));
    }

    #[test]
    fn stash_pop_undo_takes_back_the_changes_and_the_stash() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        write("u\n", "u.txt", dir);
        git::run(dir, ["stash", "push", "-q", "--include-untracked", "-m", "wip"]).unwrap();
        let sha = g(dir, &["rev-parse", "stash@{0}"]);
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Stash { label: "Pop Stash".into(), sha: sha.clone(), message: "On main: wip".into(), applied: true, dropped: true }),
                  || git::run(dir, ["stash", "pop", "-q", "stash@{0}"]).map(drop)).unwrap();
        assert!(read("a.txt", dir).as_deref() == Some("two\n") && read("u.txt", dir).as_deref() == Some("u\n"));
        stack.undo().unwrap();
        assert!(read("a.txt", dir).as_deref() == Some("one\n") && read("u.txt", dir).is_none());
        assert_eq!(g(dir, &["stash", "list", "--format=%H"]), sha);
    }

    #[test]
    fn cherry_pick_undo_resets_to_the_head_before() {
        let d = repo();
        let dir = d.path();
        let base = g(dir, &["rev-parse", "HEAD"]);
        git::run(dir, ["switch", "-q", "-c", "side"]).unwrap();
        write("side\n", "s.txt", dir);
        git::run(dir, ["add", "s.txt"]).unwrap();
        git::run(dir, ["commit", "-q", "-m", "side"]).unwrap();
        let pick = g(dir, &["rev-parse", "HEAD"]);
        git::run(dir, ["switch", "-q", "main"]).unwrap();
        write("dirty but unrelated\n", "a.txt", dir); // reset --keep keeps local changes
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Reset(Reset { offer_revert: true, ..Reset::new("Cherry-pick of side") })), || git::run(dir, ["cherry-pick", &pick]).map(drop)).unwrap();
        assert_eq!(read("s.txt", dir).as_deref(), Some("side\n"));
        stack.undo().unwrap();
        assert_eq!(g(dir, &["rev-parse", "HEAD"]), base);
        assert!(read("s.txt", dir).is_none());
        assert_eq!(read("a.txt", dir).as_deref(), Some("dirty but unrelated\n"));
    }

    #[test]
    fn entries_are_dropped_once_head_moves_elsewhere() {
        let d = repo();
        let dir = d.path();
        write("two\n", "a.txt", dir);
        let stack = UndoStack::new(dir);
        stack.run(files("Discard of a.txt", &["a.txt"], false, true), || git::run(dir, ["restore", "--worktree", "--", "a.txt"]).map(drop)).unwrap();
        stack.prune(Some("main"), &g(dir, &["rev-parse", "HEAD"]), None);
        assert_eq!(stack.labels().len(), 1);
        git::run(dir, ["commit", "-q", "--allow-empty", "-m", "elsewhere"]).unwrap(); // a terminal commit
        stack.prune(Some("main"), &g(dir, &["rev-parse", "HEAD"]), None);
        assert!(stack.is_empty() && stack.toast().is_none());
    }

    #[test]
    fn stack_is_bounded() {
        let d = repo();
        let stack = UndoStack::new(d.path());
        for i in 0..LIMIT + 5 { stack.record(format!("Step {i}"), Restore::Nothing, RecordOpts::default()) }
        let l = stack.labels();
        assert!(l.len() == LIMIT && l[0] == "Step 5");
    }

    #[test]
    fn discard_undo_never_deletes_a_directory_made_since() {
        let d = repo();
        let dir = d.path();
        write("new\n", "n", dir);
        let stack = UndoStack::new(dir);
        stack.run(files("Discard of n", &["n"], false, true), || Ok(std::fs::remove_file(dir.join("n"))?)).unwrap();
        std::fs::create_dir_all(dir.join("n")).unwrap();
        write("keep\n", "n/inside.txt", dir);
        refusal(stack.undo());
        assert_eq!(read("n/inside.txt", dir).as_deref(), Some("keep\n"));
    }

    #[test]
    fn a_status_read_from_before_an_action_does_not_prune() {
        let d = repo();
        let dir = d.path();
        let before = g(dir, &["rev-parse", "HEAD"]);
        let stack = UndoStack::new(dir);
        let stamp = stack.stamp();
        write("two\n", "a.txt", dir);
        git::run(dir, ["add", "a.txt"]).unwrap();
        stack.run(Some(Recipe::Reset(Reset { soft: true, ..Reset::new("Commit") })), || git::run(dir, ["commit", "-q", "-m", "second"]).map(drop)).unwrap();
        stack.prune(Some("main"), &before, Some(stamp)); // the status was read before the commit
        assert_eq!(stack.labels().len(), 1);
        stack.prune(Some("main"), &before, Some(stack.stamp())); // a fresh read showing HEAD moved elsewhere
        assert!(stack.is_empty());
    }

    #[test]
    fn stash_pop_undo_handles_non_ascii_names() {
        let d = repo();
        let dir = d.path();
        write("é\n", "café.txt", dir);
        git::run(dir, ["stash", "push", "-q", "--include-untracked", "-m", "wip"]).unwrap();
        let sha = g(dir, &["rev-parse", "stash@{0}"]);
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Stash { label: "Pop Stash".into(), sha: sha.clone(), message: "On main: wip".into(), applied: true, dropped: true }),
                  || git::run(dir, ["stash", "pop", "-q"]).map(drop)).unwrap();
        stack.undo().unwrap();
        assert!(read("café.txt", dir).is_none());
        assert_eq!(g(dir, &["stash", "list", "--format=%H"]), sha);
    }

    #[test]
    fn undo_of_the_first_commit_leaves_the_branch_unborn_and_staged() {
        let d = empty_repo();
        let dir = d.path();
        write("one\n", "a.txt", dir);
        git::run(dir, ["add", "a.txt"]).unwrap();
        let stack = UndoStack::new(dir);
        stack.run(Some(Recipe::Reset(Reset { soft: true, ..Reset::new("Commit") })), || git::run(dir, ["commit", "-q", "-m", "first"]).map(drop)).unwrap();
        stack.undo().unwrap();
        assert_eq!(oid(dir), None);
        assert_eq!(g(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(g(dir, &["diff", "--cached", "--name-only"]), "a.txt");
    }
}
