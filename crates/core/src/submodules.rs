//! Submodules: status, diff, actions.

use std::path::Path;

use crate::git::{self, Result};

/// The `<sub>` field of `status --porcelain=v2`: `S<c><m><u>` for a submodule, `N...` for anything else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubmoduleStatus {
    pub commit_changed: bool, // checked out at another commit than the one recorded
    pub modified: bool,       // tracked changes inside it
    pub untracked: bool,      // untracked files inside it
}

impl SubmoduleStatus {
    pub fn parse(field: &str) -> Option<Self> {
        let c = field.as_bytes();
        if c.len() != 4 || c[0] != b'S' { return None }
        Some(SubmoduleStatus { commit_changed: c[1] == b'C', modified: c[2] == b'M', untracked: c[3] == b'U' })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleCommit {
    pub added: bool, // `>` reached by the new commit, `<` only by the old (a rewind)
    pub subject: String,
}

/// `git diff --submodule=log` for one submodule: the commit range and the commits in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmoduleDiff {
    pub old: Option<String>,  // abbreviated; None for a new submodule
    pub new: Option<String>,  // None for a deleted one
    pub note: Option<String>, // "new submodule", "rewind", "commits not present", ...
    pub commits: Vec<SubmoduleCommit>,
    pub modified_content: bool,
    pub untracked_content: bool,
}

impl SubmoduleDiff {
    pub fn parse(text: &str) -> Self {
        let mut d = SubmoduleDiff::default();
        for line in text.split('\n').filter(|l| !l.is_empty()) {
            if let Some(s) = line.strip_prefix("  > ") { d.commits.push(SubmoduleCommit { added: true, subject: s.into() }); continue }
            if let Some(s) = line.strip_prefix("  < ") { d.commits.push(SubmoduleCommit { added: false, subject: s.into() }); continue }
            if !line.starts_with("Submodule ") { continue }
            if line.ends_with(" contains modified content") { d.modified_content = true; continue }
            if line.ends_with(" contains untracked content") { d.untracked_content = true; continue }
            // "Submodule <path> <a>..<b>:" or "... <a>...<b> (rewind):"; the path may hold spaces, the range never does.
            let mut rest = line.strip_suffix(':').unwrap_or(line);
            if rest.ends_with(')')
                && let Some(open) = rest.rfind(" (") {
                    d.note = Some(rest[open + 2..rest.len() - 1].to_string());
                    rest = &rest[..open];
                }
            let Some(range) = rest.split(' ').filter(|s| !s.is_empty()).next_back() else { continue };
            let ends: Vec<&str> = if range.contains("...") { range.split("...").collect() } else { range.split("..").collect() };
            if ends.len() != 2 { continue }
            let sha = |s: &str| (!s.chars().all(|c| c == '0')).then(|| s.to_string());
            d.old = sha(ends[0]);
            d.new = sha(ends[1]);
        }
        d
    }
}

/// `git config -f .gitmodules --get-regexp '^submodule\..*\.path$'`: "submodule.<name>.path <path>" per line.
pub fn paths_from_config(out: &str) -> Vec<String> {
    out.lines().filter_map(|l| l.find(".path ").map(|i| l[i + 6..].to_string())).collect()
}

/// Commits a checkout elsewhere would leave reachable from no branch, tag or remote: work done on the
/// detached HEAD `submodule update` leaves behind. 0 when it isn't checked out.
pub fn stranded_commits(dir: &Path) -> usize {
    if !dir.join(".git").exists() { return 0 }
    git::trimmed(dir, ["rev-list", "--count", "HEAD", "--not", "--branches", "--tags", "--remotes"])
        .ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Submodules listed in .gitmodules that were never checked out. Only a repo with a .gitmodules pays for it: one git
/// call and a stat per submodule.
pub fn uninitialized(path: &Path) -> Vec<String> {
    if !path.join(".gitmodules").exists() { return vec![] }
    let out = git::git(path, ["config", "-f", ".gitmodules", "--get-regexp", r"^submodule\..*\.path$"]).allow_failure().string().unwrap_or_default();
    // A checked-out submodule has a .git (a "gitdir:" file pointing into the superproject's .git/modules).
    paths_from_config(&out).into_iter().filter(|p| !is_checked_out(path, p)).collect()
}

pub fn is_checked_out(path: &Path, sub: &str) -> bool { path.join(sub).join(".git").exists() }

/// The commits between the recorded and the checked-out commit (or before and after, staged), and work inside it.
pub fn diff(path: &Path, sub: &str, staged: bool) -> SubmoduleDiff {
    let mut a = vec!["diff", "--no-color", "--no-ext-diff", "--submodule=log"];
    if staged { a.push("--cached") }
    a.extend(["--", sub]);
    SubmoduleDiff::parse(&git::string(path, a).unwrap_or_default())
}

pub fn init(path: &Path, env: &[(String, String)]) -> Result<()> {
    git::git(path, ["submodule", "update", "--init", "--recursive"]).env(env.iter().map(|(k, v)| (k, v))).run().map(drop)
}

/// Submodules that have commits on a detached HEAD, which an update would leave only in their reflog: ask first.
pub fn stranded(path: &Path, subs: &[String]) -> Vec<(String, usize)> {
    subs.iter().map(|s| (s.clone(), stranded_commits(&path.join(s)))).filter(|s| s.1 > 0).collect()
}

/// Checks the submodules out at the commits this repository records for them.
pub fn update(path: &Path, subs: &[String], env: &[(String, String)]) -> Result<()> {
    let mut a: Vec<&str> = vec!["submodule", "update", "--init", "--recursive", "--"];
    a.extend(subs.iter().map(String::as_str));
    git::git(path, a).env(env.iter().map(|(k, v)| (k, v))).run().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Kind;

    #[test]
    fn parses_submodule_entries_from_status_v2() {
        let raw = "# branch.oid abc\0# branch.head main\0".to_string()
            + "1 .M SCMU 160000 160000 160000 091e5df 091e5df libs/lib\0"
            + "1 M. SC.. 160000 160000 160000 111 222 vendor/other\0"
            + "1 .M N... 100644 100644 100644 aaa bbb src/a.txt\0";
        let files = git::parse_status(raw.as_bytes()).files;
        assert_eq!(files.len(), 3);
        assert!(files[0].path == "libs/lib" && !files[0].staged);
        assert_eq!(files[0].submodule, Some(SubmoduleStatus { commit_changed: true, modified: true, untracked: true }));
        assert!(files[1].path == "vendor/other" && files[1].staged && files[1].submodule.unwrap().commit_changed && !files[1].submodule.unwrap().modified);
        assert_eq!(files[2].submodule, None);
        assert_eq!(SubmoduleStatus::parse("S..U"), Some(SubmoduleStatus { untracked: true, ..Default::default() }));
        assert_eq!(SubmoduleStatus::parse("N..."), None);
    }

    #[test]
    fn parses_submodule_log_diffs() {
        let d = SubmoduleDiff::parse("Submodule libs/my lib contains modified content\nSubmodule libs/my lib 091e5df..51ffb32:\n  > add b\n  > add c\n");
        assert!(d.old.as_deref() == Some("091e5df") && d.new.as_deref() == Some("51ffb32") && d.note.is_none() && d.modified_content && !d.untracked_content);
        assert_eq!(d.commits, [SubmoduleCommit { added: true, subject: "add b".into() }, SubmoduleCommit { added: true, subject: "add c".into() }]);
        let rewind = SubmoduleDiff::parse("Submodule lib 51ffb32...091e5df (rewind):\n  < add b\n");
        assert!(rewind.old.as_deref() == Some("51ffb32") && rewind.new.as_deref() == Some("091e5df") && rewind.note.as_deref() == Some("rewind"));
        assert_eq!(rewind.commits, [SubmoduleCommit { added: false, subject: "add b".into() }]);
        let added = SubmoduleDiff::parse("Submodule lib 0000000...091e5df (new submodule)\n");
        assert!(added.old.is_none() && added.new.as_deref() == Some("091e5df") && added.note.as_deref() == Some("new submodule"));
        let untracked = SubmoduleDiff::parse("Submodule lib contains untracked content\n");
        assert!(untracked.untracked_content && untracked.old.is_none() && untracked.new.is_none());
    }

    #[test]
    fn reads_submodule_paths_from_gitmodules_config() {
        assert_eq!(paths_from_config("submodule.libs/lib.path libs/lib\nsubmodule.x.path vendor/x y\n"), ["libs/lib", "vendor/x y"]);
        assert!(paths_from_config("").is_empty());
    }

    /// Real git: add a submodule, move it to a new commit, stage the pointer; a fresh clone has it uninitialized.
    #[test]
    fn submodule_add_and_pointer_change_against_real_repo() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let (lib, sup, clone) = (root.join("lib"), root.join("super"), root.join("clone"));
        for d in [&lib, &sup] { std::fs::create_dir_all(d).unwrap() }
        let g = |dir: &Path, args: &[&str]| {
            git::git(dir, ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "protocol.file.allow=always", "-c", "init.defaultBranch=main"].iter().chain(args)).run().unwrap();
        };
        g(&lib, &["init", "-q"]);
        std::fs::write(lib.join("a"), "a\n").unwrap();
        g(&lib, &["add", "a"]);
        g(&lib, &["commit", "-qm", "add a"]);
        g(&sup, &["init", "-q"]);
        g(&sup, &["commit", "-q", "--allow-empty", "-m", "init"]);
        g(&sup, &["submodule", "add", "-q", lib.to_str().unwrap(), "libs/lib"]);

        // Just added: a staged new submodule.
        let added = git::status(&sup).unwrap().files.into_iter().find(|f| f.path == "libs/lib").unwrap();
        assert!(added.staged && added.kind == Kind::Added && added.submodule.is_some());
        g(&sup, &["commit", "-qm", "add sub"]);

        // A new commit inside it: the pointer changed, the diff lists the commit.
        let sub = sup.join("libs/lib");
        std::fs::write(lib.join("b"), "b\n").unwrap();
        g(&lib, &["add", "b"]);
        g(&lib, &["commit", "-qm", "add b"]);
        g(&sub, &["fetch", "-q", "origin"]);
        g(&sub, &["checkout", "-q", "FETCH_HEAD"]);
        let moved = git::status(&sup).unwrap().files.into_iter().find(|f| f.path == "libs/lib").unwrap();
        assert!(!moved.staged && moved.submodule.unwrap().commit_changed && !moved.submodule.unwrap().modified);
        let log = diff(&sup, "libs/lib", false);
        assert!(log.commits == [SubmoduleCommit { added: true, subject: "add b".into() }] && log.old.is_some() && log.new.is_some());

        // Staging records the new pointer.
        g(&sup, &["add", "-A", "--", "libs/lib"]);
        let staged: Vec<_> = git::status(&sup).unwrap().files.into_iter().filter(|f| f.path == "libs/lib").collect();
        assert!(staged.len() == 1 && staged[0].staged && staged[0].submodule.is_some());
        g(&sup, &["commit", "-qm", "bump lib"]);

        // Opened as a repository of its own: its worktree is the submodule, not its git dir under .git/modules.
        let real = sub.to_string_lossy().into_owned();
        assert_eq!(git::worktrees(&sub).unwrap().first().map(|w| w.path.clone()), Some(real.clone()));
        // Asked from a linked worktree of the submodule, the main entry is still the submodule, not that worktree.
        g(&sub, &["worktree", "add", "-q", "--detach", root.join("sub-wt").to_str().unwrap()]);
        let from_linked = git::worktrees(&root.join("sub-wt")).unwrap();
        assert!(from_linked.len() == 2 && from_linked[0].path == real);
        // Update to Recorded Commit would strand a commit made on a detached HEAD: it's counted.
        g(&sub, &["commit", "-q", "--allow-empty", "-m", "detached work"]);
        assert_eq!(stranded_commits(&sub), 1);
        g(&sub, &["checkout", "-q", "-B", "work"]);
        assert_eq!(stranded_commits(&sub), 0);
        g(&sub, &["checkout", "-q", "--detach", "HEAD~1"]);

        // A clone has it listed but not checked out, until it's initialized.
        g(&root, &["clone", "-q", sup.to_str().unwrap(), clone.to_str().unwrap()]);
        assert_eq!(uninitialized(&clone), ["libs/lib"]);
        g(&clone, &["submodule", "update", "--init", "--recursive"]);
        assert!(uninitialized(&clone).is_empty());
    }
}
