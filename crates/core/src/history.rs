//! History: paged `git log`, stashes, a commit's detail, the graph lanes, blame and a file's log.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diff::{self, FileDiff};
use crate::file_tree::tree_order;
use crate::git::{self, Cancel, Error, Result};
use crate::undo::{Recipe, Reset, UndoStack};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: i64, // unix seconds
    /// From `--decorate=full`: "HEAD -> refs/heads/x", "refs/remotes/origin/x", "tag: refs/tags/v1", "HEAD".
    pub refs: Vec<String>,
    pub subject: String,
    pub selector: Option<String>, // "stash@{2}" for stash entries
}

impl Commit {
    pub fn short(&self) -> &str { &self.sha[..self.sha.len().min(8)] }
}

/// What the History list shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope { Current, All, Branch(String), Stashes }

impl Scope {
    pub fn revs(&self) -> Vec<String> {
        match self {
            Scope::Current => vec!["HEAD".into()],
            Scope::All => ["--branches", "--remotes", "--tags", "HEAD"].map(String::from).to_vec(),
            Scope::Branch(b) => vec![b.clone()],
            Scope::Stashes => vec![],
        }
    }
    pub fn title(&self) -> &str {
        match self { Scope::Current => "Current Branch", Scope::All => "All Branches", Scope::Branch(b) => b, Scope::Stashes => "Stashes" }
    }
}

pub const LOG_FORMAT: &str = "%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";
/// History's first page, and each page after it as the list scrolls.
pub const PAGE: usize = 100;

pub fn log(dir: &Path, revs: &[String], search: &str, skip: usize, limit: usize, cancel: Option<&Cancel>) -> Result<Vec<Commit>> {
    // Topological order keeps a commit below all of its children, which the graph lanes need. With the
    // commit-graph file git keeps, it's as fast as the default order.
    let mut args: Vec<String> = vec!["log".into(), format!("--format={LOG_FORMAT}"), "--decorate=full".into(), "--topo-order".into(),
                                     format!("--skip={skip}"), "-n".into(), limit.to_string()];
    if !search.is_empty() { args.extend(["-i".into(), format!("--grep={search}"), format!("--author={search}")]) } // OR'ed by git
    args.extend(revs.iter().cloned());
    args.push("--".into());
    let mut g = git::git(dir, &args).allow_failure(); // unborn branch (fresh repo) → git log fails; that's just an empty history
    if let Some(c) = cancel { g = g.cancel(c) }
    Ok(parse_log(&g.run()?))
}

/// `git stash list`, newest first. The reflog selector rides in the refs field.
pub fn stashes(dir: &Path) -> Result<Vec<Commit>> {
    let data = git::git(dir, ["stash", "list", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%gd%x1f%s%x1e"]).allow_failure().run()?;
    Ok(parse_log(&data).into_iter().map(|mut c| { c.selector = c.refs.first().cloned(); c.refs.clear(); c }).collect())
}

pub fn parse_log(data: &[u8]) -> Vec<Commit> {
    String::from_utf8_lossy(data).split('\u{1e}').filter_map(|rec| {
        let f: Vec<&str> = rec.trim_matches(['\n', '\r']).split('\u{1f}').collect();
        if f.len() < 7 { return None }
        Some(Commit {
            sha: f[0].into(),
            parents: f[1].split(' ').filter(|p| !p.is_empty()).map(String::from).collect(),
            author: f[2].into(),
            email: f[3].into(),
            date: f[4].parse().unwrap_or(0),
            refs: if f[5].is_empty() { vec![] } else { f[5].split(", ").map(String::from).collect() },
            subject: f[6..].join("\u{1f}"),
            selector: None,
        })
    }).collect()
}

fn sorted(mut d: Vec<FileDiff>) -> Vec<FileDiff> {
    d.sort_by(|a, b| tree_order(a.path(), b.path()));
    d
}

/// Diff of a commit against its first parent (merges included), root commits against the empty tree. In tree order.
pub fn commit_diff(sha: &str, dir: &Path, cancel: Option<&Cancel>) -> Result<Vec<FileDiff>> {
    let mut g = git::git(dir, ["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent", sha]);
    if let Some(c) = cancel { g = g.cancel(c) }
    Ok(sorted(diff::parse_bytes(&g.run()?)))
}

/// A stash's changes, untracked files included (they live in the stash's third parent). In tree order.
pub fn stash_diff(selector: &str, dir: &Path) -> Result<Vec<FileDiff>> {
    Ok(sorted(diff::parse_bytes(&git::run(dir, ["stash", "show", "-p", "--include-untracked", "--no-color", "--no-ext-diff", "-M", selector])?)))
}

/// The selected commit's message and files.
pub fn detail(c: &Commit, dir: &Path, cancel: Option<&Cancel>) -> Result<(String, Vec<FileDiff>)> {
    std::thread::scope(|s| {
        let body = s.spawn(|| {
            let mut g = git::git(dir, ["show", "-s", "--format=%B", &c.sha]);
            if let Some(x) = cancel { g = g.cancel(x) }
            g.trimmed()
        });
        let files = match &c.selector { Some(sel) => stash_diff(sel, dir), None => commit_diff(&c.sha, dir, cancel) };
        Ok((body.join().unwrap()?, files?))
    })
}

/// The whole file at the commit with its changes inline. A new or deleted file's diff already is the whole file: None.
pub fn whole_file(f: &FileDiff, c: &Commit, dir: &Path, cancel: Option<&Cancel>) -> Option<FileDiff> {
    if f.is_binary { return None }
    let (old, new) = (f.old_path.as_deref()?, f.new_path.as_deref()?);
    let mut args: Vec<&str> = match &c.selector {
        Some(sel) => vec!["stash", "show", "-p", "--no-color", "--no-ext-diff", "-M", "-U1000000", sel, "--"],
        None => vec!["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent", "-U1000000", &c.sha, "--"],
    };
    args.push(old);
    if new != old { args.push(new) }
    let mut g = git::git(dir, args);
    if let Some(x) = cancel { g = g.cancel(x) }
    diff::whole_file(diff::parse_bytes(&g.run().ok()?).into_iter().next())
}

/// Local and remote branch names, newest first (the scope menu). Remote HEAD aliases left out.
pub fn branch_names(dir: &Path) -> (Vec<String>, Vec<String>) {
    let names: Vec<String> = git::string(dir, ["for-each-ref", "--format=%(refname)", "--sort=-committerdate", "refs/heads", "refs/remotes"])
        .unwrap_or_default().lines().filter(|l| !l.is_empty() && !l.ends_with("/HEAD")).map(String::from).collect();
    (names.iter().filter_map(|n| n.strip_prefix("refs/heads/").map(String::from)).collect(),
     names.iter().filter_map(|n| n.strip_prefix("refs/remotes/").map(String::from)).collect())
}

/// Shown commits HEAD doesn't contain: these can be cherry-picked. Empty for the current branch.
pub fn not_in_head(dir: &Path, scope: &Scope) -> Result<HashSet<String>> {
    if *scope == Scope::Current || *scope == Scope::Stashes { return Ok(HashSet::new()) }
    let mut args: Vec<String> = vec!["rev-list".into(), "-n".into(), "20000".into()];
    args.extend(scope.revs().into_iter().filter(|r| r != "HEAD"));
    args.extend(["^HEAD".into(), "--".into()]);
    Ok(git::string(dir, &args)?.lines().map(String::from).collect())
}

/// One page of the list: the commits (filtered by `search`, message or author) and whether it was the last.
pub fn fetch_page(dir: &Path, scope: &Scope, search: &str, skip: usize, cancel: Option<&Cancel>) -> Result<(Vec<Commit>, bool)> {
    if *scope == Scope::Stashes {
        let all = stashes(dir)?;
        let s = search.to_lowercase();
        return Ok((if search.is_empty() { all } else { all.into_iter().filter(|c| c.subject.to_lowercase().contains(&s)).collect() }, true));
    }
    let batch = log(dir, &scope.revs(), search, skip, PAGE, cancel)?;
    let exhausted = batch.len() < PAGE;
    Ok((batch, exhausted))
}

/// The graph for the loaded commits, its width in lanes (capped at 12), or nothing when one straight line would say
/// nothing (or the list is filtered). The main line is HEAD's, or the shown branch's own when HEAD isn't in it.
pub fn graph_for(commits: &[Commit], scope: &Scope, head: Option<&str>, filtered: bool) -> (Vec<GraphRow>, usize) {
    if filtered || *scope == Scope::Stashes { return (vec![], 0) }
    let head = if *scope == Scope::Current { commits.first().map(|c| c.sha.as_str()) } else { head };
    let tip = head.filter(|h| commits.iter().any(|c| c.sha == *h)).or(commits.first().map(|c| c.sha.as_str()));
    let g = layout(commits, tip);
    let w = g.iter().map(GraphRow::width).max().unwrap_or(0).min(12);
    if w <= 1 { (vec![], 0) } else { (g, w) }
}

// MARK: - Ref badges

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// The branch HEAD is on.
    Head,
    Tag,
    Remote,
    Local,
    /// Detached HEAD, refs/pull/*, stash...
    Other,
}

/// A `--decorate=full` ref as a badge: its short name and kind.
pub fn parse_ref(r: &str) -> (String, RefKind) {
    if let Some(b) = r.strip_prefix("HEAD -> ") { return (b.replace("refs/heads/", ""), RefKind::Head) }
    if let Some(t) = r.strip_prefix("tag: ") { return (t.replace("refs/tags/", ""), RefKind::Tag) }
    if let Some(x) = r.strip_prefix("refs/remotes/") { return (x.into(), RefKind::Remote) }
    if let Some(x) = r.strip_prefix("refs/heads/") { return (x.into(), RefKind::Local) }
    (r.into(), RefKind::Other)
}

// MARK: - Graph

/// One row of the commit graph: which lanes pass by, which end at the commit (its children above) and which
/// leave it (its parents below). Lanes keep their column for as long as they live, so the lines stay vertical.
/// Lane 0 belongs to the main line (HEAD's first parents), so the branch you're on stays put on the left and the
/// others read as forks off it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GraphRow {
    pub col: usize,
    pub through: Vec<usize>,
    pub into: Vec<usize>,
    pub out: Vec<usize>,
}

impl GraphRow {
    pub fn width(&self) -> usize {
        self.through.iter().chain(&self.into).chain(&self.out).copied().chain([self.col]).max().unwrap() + 1
    }
}

pub fn layout(commits: &[Commit], tip: Option<&str>) -> Vec<GraphRow> {
    // The main line: first parents from the tip, as far as the loaded commits go.
    let mut by_id: HashMap<&str, &Commit> = HashMap::new();
    for c in commits { by_id.entry(c.sha.as_str()).or_insert(c); }
    let mut main = HashSet::new();
    let mut cur = tip.and_then(|t| by_id.get(t).copied());
    while let Some(c) = cur {
        if !main.insert(c.sha.as_str()) { break }
        cur = c.parents.first().and_then(|p| by_id.get(p.as_str()).copied());
    }
    let reserved = usize::from(!main.is_empty());
    let free = |lanes: &[Option<&str>]| (reserved..lanes.len()).find(|&i| lanes[i].is_none()).unwrap_or(lanes.len());

    let mut lanes: Vec<Option<&str>> = if reserved == 1 { vec![None] } else { vec![] }; // the commit each lane is waiting for
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        // A main-line commit always sits in lane 0; other lanes waiting for it (newer commits on top of it,
        // like origin/main ahead of main) end there.
        let col = if main.contains(c.sha.as_str()) { 0 } else { lanes.iter().position(|l| *l == Some(c.sha.as_str())).unwrap_or_else(|| free(&lanes)) };
        if col == lanes.len() { lanes.push(None) }
        let mut row = GraphRow { col, ..Default::default() };
        for i in 0..lanes.len() {
            match lanes[i] {
                Some(l) if l == c.sha => { row.into.push(i); lanes[i] = None }
                Some(_) => row.through.push(i),
                None => {}
            }
        }
        for (k, p) in c.parents.iter().enumerate() {
            // Join a lane already headed there, except the main line, which keeps lane 0 (the other lane merges into it at p).
            let keeps_main = k == 0 && col == 0 && reserved == 1;
            if !keeps_main
                && let Some(w) = lanes.iter().position(|l| *l == Some(p.as_str())) { row.out.push(w); continue }
            let slot = if k == 0 && lanes[col].is_none() { col } else { free(&lanes) };
            if slot == lanes.len() { lanes.push(None) }
            lanes[slot] = Some(p.as_str());
            row.out.push(slot);
        }
        while lanes.len() > reserved && lanes.last() == Some(&None) { lanes.pop(); }
        rows.push(row);
    }
    rows
}

// MARK: - Actions on the worktree

/// Runs a git command that may stop on conflicts; conflicts aren't an error, they're work for the Changes tab:
/// Ok(true) then.
fn run_may_conflict(dir: &Path, args: &[String], undo: Option<Recipe>) -> Result<bool> {
    match UndoStack::for_path(dir).run(undo, || git::run(dir, args).map(drop)) {
        Ok(()) => Ok(false),
        Err(e) => {
            let unmerged = git::string(dir, ["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            if unmerged.trim().is_empty() { Err(e) } else { Ok(true) }
        }
    }
}

fn mainline(c: &Commit) -> Vec<String> { if c.parents.len() > 1 { vec!["-m".into(), "1".into()] } else { vec![] } }

pub fn cherry_pick(dir: &Path, c: &Commit) -> Result<bool> {
    let mut a = vec!["cherry-pick".to_string()];
    a.extend(mainline(c));
    a.push(c.sha.clone());
    run_may_conflict(dir, &a, Some(Recipe::Reset(Reset { offer_revert: true, ..Reset::new(format!("Cherry-pick of {}", c.short())) })))
}

pub fn revert(dir: &Path, c: &Commit) -> Result<bool> {
    let mut a = vec!["revert".to_string(), "--no-edit".into()];
    a.extend(mainline(c));
    a.push(c.sha.clone());
    run_may_conflict(dir, &a, Some(Recipe::Reset(Reset { offer_revert: true, ..Reset::new(format!("Revert of {}", c.short())) })))
}

pub fn apply_stash(dir: &Path, c: &Commit, pop: bool) -> Result<bool> {
    let Some(s) = &c.selector else { return Err(Error::msg("Not a stash")) };
    let label = if pop { "Pop Stash" } else { "Apply Stash" };
    run_may_conflict(dir, &["stash".into(), if pop { "pop" } else { "apply" }.into(), s.clone()],
                     Some(Recipe::Stash { label: label.into(), sha: c.sha.clone(), message: c.subject.clone(), applied: true, dropped: pop }))
}

pub fn drop_stash(dir: &Path, c: &Commit) -> Result<bool> {
    let Some(s) = &c.selector else { return Err(Error::msg("Not a stash")) };
    run_may_conflict(dir, &["stash".into(), "drop".into(), s.clone()],
                     Some(Recipe::Stash { label: "Drop Stash".into(), sha: c.sha.clone(), message: c.subject.clone(), applied: false, dropped: true }))
}

/// Refused undo's offer: a new commit that takes `sha` back (`-m 1` for a merge).
pub fn revert_sha(dir: &Path, sha: &str) -> Result<()> {
    let merge = git::run(dir, ["rev-parse", "-q", "--verify", &format!("{sha}^2")]).is_ok();
    let mut a = vec!["revert", "--no-edit"];
    if merge { a.extend(["-m", "1"]) }
    a.push(sha);
    git::run(dir, a).map(drop)
}

// MARK: - Blame and a file's history

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlameCommit { pub sha: String, pub author: String, pub date: i64, pub summary: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine { pub sha: String, pub text: String }

/// `git blame --porcelain`: each line comes with its commit; a commit's details only on its first appearance.
pub fn blame(path: &str, rev: Option<&str>, dir: &Path, cancel: Option<&Cancel>) -> Result<(Vec<BlameLine>, HashMap<String, BlameCommit>)> {
    let mut a = vec!["blame", "--porcelain"];
    a.extend(rev);
    a.extend(["--", path]);
    let mut g = git::git(dir, a);
    if let Some(c) = cancel { g = g.cancel(c) }
    let out = g.string()?;
    let (mut lines, mut commits) = (vec![], HashMap::<String, BlameCommit>::new());
    let mut sha = String::new();
    for l in out.split('\n') {
        if let Some(t) = l.strip_prefix('\t') { lines.push(BlameLine { sha: sha.clone(), text: t.into() }); continue }
        let (key, value) = l.split_once(' ').unwrap_or((l, ""));
        if key.is_empty() { continue }
        if (key.len() == 40 || key.len() == 64) && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            sha = key.into();
            commits.entry(sha.clone()).or_insert_with(|| BlameCommit { sha: sha.clone(), author: String::new(), date: 0, summary: String::new() });
        } else if let Some(c) = commits.get_mut(&sha) {
            match key {
                "author" => c.author = value.into(),
                "author-time" => c.date = value.parse().unwrap_or(0),
                "summary" => c.summary = value.into(),
                _ => {}
            }
        }
    }
    Ok((lines, commits))
}

/// Commits touching `path`, following renames, with the file's path in each.
pub fn file_log(path: &str, dir: &Path) -> Result<Vec<(Commit, String)>> {
    let format = format!("--format=%x1e{}", LOG_FORMAT.replace("%x1e", ""));
    let out = git::string(dir, ["log", "--follow", "-M", "-n", "1000", &format, "--decorate=full", "--name-status", "--", path])?;
    Ok(out.split('\u{1e}').filter_map(|rec| {
        let lines: Vec<&str> = rec.split('\n').filter(|l| !l.is_empty()).collect();
        let c = parse_log(lines.first()?.as_bytes()).into_iter().next()?;
        let name = lines.iter().skip(1).last().map(|l| l.rsplit('\t').next().unwrap_or("").to_string()).unwrap_or_else(|| path.into());
        Some((c, if name.is_empty() { path.into() } else { name }))
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log() {
        let raw = "abc\u{1f}p1 p2\u{1f}Ann\u{1f}a@x\u{1f}1700000000\u{1f}HEAD -> refs/heads/main, tag: refs/tags/v1\u{1f}Merge it\u{1e}\ndef\u{1f}\u{1f}Bob\u{1f}b@x\u{1f}1600000000\u{1f}\u{1f}root\u{1e}\n";
        let c = parse_log(raw.as_bytes());
        assert!(c.len() == 2 && c[0].parents == ["p1", "p2"] && c[0].refs.len() == 2 && c[1].parents.is_empty() && c[1].subject == "root");
        assert_eq!(parse_ref(&c[0].refs[0]), ("main".into(), RefKind::Head));
        assert_eq!(parse_ref(&c[0].refs[1]), ("v1".into(), RefKind::Tag));
        assert_eq!(parse_ref("refs/remotes/origin/x"), ("origin/x".into(), RefKind::Remote));
    }

    fn c(s: &str, p: &[&str]) -> Commit {
        Commit { sha: s.into(), parents: p.iter().map(|x| x.to_string()).collect(), author: String::new(), email: String::new(), date: 0, refs: vec![], subject: String::new(), selector: None }
    }
    fn row(col: usize, through: &[usize], into: &[usize], out: &[usize]) -> GraphRow {
        GraphRow { col, through: through.to_vec(), into: into.to_vec(), out: out.to_vec() }
    }

    #[test]
    fn lays_out_graph_lanes() {
        // M merges F into A; F branches off B.   M─┬─A─B─R
        //                                           └─F─┘
        let rows = layout(&[c("M", &["A", "F"]), c("A", &["B"]), c("F", &["B"]), c("B", &["R"]), c("R", &[])], Some("M"));
        assert_eq!(rows[0], row(0, &[], &[], &[0, 1]));
        assert_eq!(rows[1], row(0, &[1], &[0], &[0]));
        assert_eq!(rows[2], row(1, &[0], &[1], &[0])); // F joins the lane already waiting for B
        assert_eq!(rows[3], row(0, &[], &[0], &[0]));
        assert!(rows[4].out.is_empty());
        // The newest commit is on a side branch X off B: the main line still gets lane 0, X takes lane 1.
        let side = layout(&[c("X", &["B"]), c("M", &["B"]), c("B", &[])], Some("M"));
        assert_eq!(side.iter().map(|r| r.col).collect::<Vec<_>>(), [1, 0, 0]);
        assert!(side[0].out == [1] && side[1].through == [1] && side[2].into == [0, 1]);
        // origin/main two commits ahead of main (M): they run in lane 1 and end in M, which stays in lane 0.
        let ahead = layout(&[c("O2", &["O1"]), c("O1", &["M"]), c("M", &["B"]), c("B", &[])], Some("M"));
        assert_eq!(ahead.iter().map(|r| r.col).collect::<Vec<_>>(), [1, 1, 0, 0]);
        assert!(ahead[2].into == [1] && ahead[2].out == [0]);
    }

    #[test]
    fn blames_and_follows_file_history_against_real_repo() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let id = ["-c", "user.name=alice", "-c", "user.email=a@x"];
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git::run(dir, ["init", "-q"]).unwrap();
        git::run(dir, ["add", "."]).unwrap();
        git::run(dir, id.iter().chain(&["commit", "-qm", "first"])).unwrap();
        git::run(dir, ["mv", "a.txt", "b.txt"]).unwrap();
        std::fs::write(dir.join("b.txt"), "one\nTWO\n").unwrap();
        git::run(dir, id.iter().chain(&["commit", "-qam", "rename and change"])).unwrap();
        let (lines, commits) = blame("b.txt", None, dir, None).unwrap();
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one", "TWO"]);
        assert!(commits[&lines[0].sha].summary == "first" && commits[&lines[1].sha].summary == "rename and change");
        assert_eq!(commits[&lines[0].sha].author, "alice");
        let log = file_log("b.txt", dir).unwrap();
        assert_eq!(log.iter().map(|l| l.0.subject.as_str()).collect::<Vec<_>>(), ["rename and change", "first"]);
        assert_eq!(log.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(), ["b.txt", "a.txt"]); // followed across the rename

        let (page, exhausted) = fetch_page(dir, &Scope::Current, "", 0, None).unwrap();
        assert!(page.len() == 2 && exhausted);
        assert!(page[0].refs.iter().any(|r| r.starts_with("HEAD -> refs/heads/")));
        let (msg, files) = detail(&page[0], dir, None).unwrap();
        assert_eq!(msg, "rename and change");
        assert_eq!(files.len(), 1);
    }
}
