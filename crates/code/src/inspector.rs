//! FileInspector.swift's non-UI part: blame for a file's lines and the file's history across renames.

use std::collections::HashMap;

/// A file in a worktree, optionally at a revision: what the Blame and History panel shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileTarget {
    pub worktree: String,
    pub path: String,
    pub rev: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameCommit {
    pub sha: String,
    pub author: String,
    /// Seconds since 1970; 0 when git didn't say.
    pub time: i64,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub sha: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub lines: Vec<BlameLine>,
    pub commits: HashMap<String, BlameCommit>,
}

impl Blame {
    /// The commit of each line (index 0 is line 1): what the diff draws beside the line numbers.
    pub fn shas(&self) -> Vec<&str> {
        self.lines.iter().map(|l| l.sha.as_str()).collect()
    }
}

/// `git blame --porcelain`: each line comes with its commit; a commit's details only on its first appearance.
pub fn parse_blame(out: &str) -> Blame {
    let mut b = Blame::default();
    let mut sha = String::new();
    for l in out.split('\n') {
        if let Some(text) = l.strip_prefix('\t') {
            b.lines.push(BlameLine { sha: sha.clone(), text: text.into() });
            continue;
        }
        let (key, value) = l.split_once(' ').unwrap_or((l, ""));
        if key.is_empty() {
            continue;
        }
        if (key.len() == 40 || key.len() == 64) && key.bytes().all(|c| c.is_ascii_hexdigit()) {
            sha = key.into();
            b.commits.entry(sha.clone()).or_insert_with(|| BlameCommit { sha: sha.clone(), ..Default::default() });
        } else if let Some(c) = b.commits.get_mut(&sha) {
            match key {
                "author" => c.author = value.into(),
                "author-time" => c.time = value.parse().unwrap_or(0),
                "summary" => c.summary = value.into(),
                _ => {}
            }
        }
    }
    b
}

pub fn blame(path: &str, rev: Option<&str>, dir: &str) -> Result<Blame, crate::git::GitError> {
    let mut args = vec!["blame", "--porcelain"];
    args.extend(rev);
    args.extend(["--", path]);
    crate::git::string(&args, dir).map(|o| parse_blame(&o))
}

/// A commit that touched a file, with the file's path in it (renames are followed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    /// `%D` with --decorate=full: "HEAD -> refs/heads/main, tag: refs/tags/v1"
    pub refs: String,
    pub subject: String,
    pub path: String,
}

/// History.swift's log format, as fileLog used it: one record per commit, fields split by 0x1f.
pub const FILE_LOG_FORMAT: &str = "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s";

/// `git log --follow --name-status` output: each commit's fields, then its name-status lines; the last one's last
/// tab-separated field is the file's path in that commit.
pub fn parse_file_log(out: &str, path: &str) -> Vec<FileCommit> {
    out.split('\u{1e}')
        .filter_map(|rec| {
            let mut lines = rec.split('\n').filter(|l| !l.is_empty());
            let f: Vec<&str> = lines.next()?.split('\u{1f}').collect();
            if f.len() < 7 {
                return None;
            }
            let name = lines.last().and_then(|l| l.split('\t').last()).filter(|n| !n.is_empty()).unwrap_or(path);
            Some(FileCommit {
                sha: f[0].into(),
                parents: f[1].split(' ').filter(|p| !p.is_empty()).map(str::to_string).collect(),
                author: f[2].into(),
                email: f[3].into(),
                time: f[4].parse().unwrap_or(0),
                refs: f[5].into(),
                subject: f[6..].join("\u{1f}"),
                path: name.into(),
            })
        })
        .collect()
}

/// Commits touching `path`, newest first, following renames, with the file's path in each.
pub fn file_log(path: &str, dir: &str) -> Result<Vec<FileCommit>, crate::git::GitError> {
    let out = crate::git::string(&["log", "--follow", "-M", "-n", "1000", FILE_LOG_FORMAT, "--decorate=full", "--name-status", "--", path], dir)?;
    Ok(parse_file_log(&out, path))
}

/// `git show` of one commit's change to one file. `whole`: the whole file as context (Full mode, or a side view).
pub fn commit_diff_args<'a>(sha: &'a str, path: &'a str, whole: bool) -> Vec<&'a str> {
    let mut args = vec!["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent"];
    if whole {
        args.push("-U1000000");
    }
    args.extend([sha, "--", path]);
    args
}

/// Which panel the file inspector shows, persisted as the Swift app's "filePanel" key ("", "blame", "history").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilePanel {
    #[default]
    None,
    Blame,
    History,
}

impl FilePanel {
    pub const KEY: &'static str = "filePanel";
    /// "filePanelWidth": the History list's width, 340 by default, dragged within 260…700.
    pub const WIDTH_KEY: &'static str = "filePanelWidth";
    pub const DEFAULT_WIDTH: f64 = 340.0;
    pub const WIDTHS: std::ops::RangeInclusive<f64> = 260.0..=700.0;

    pub fn from_stored(s: &str) -> Self {
        match s {
            "blame" => Self::Blame,
            "history" => Self::History,
            _ => Self::None,
        }
    }
    pub fn stored(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Blame => "blame",
            Self::History => "history",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blame_and_file_log() {
        let sha = "a".repeat(40);
        let out = format!("{sha} 1 1 2\nauthor Ann\nauthor-time 1700000000\nsummary First\nfilename x\n\tline one\n{sha} 2 2\n\tline two\n");
        let b = parse_blame(&out);
        assert_eq!(b.shas(), [sha.as_str(), sha.as_str()]);
        assert_eq!(b.lines[1].text, "line two");
        assert_eq!((b.commits[&sha].author.as_str(), b.commits[&sha].time, b.commits[&sha].summary.as_str()), ("Ann", 1_700_000_000, "First"));
        let log = "\u{1e}abc\u{1f}p1\u{1f}Ann\u{1f}a@x\u{1f}1700000000\u{1f}HEAD -> refs/heads/main\u{1f}Rename it\n\nR100\told.rs\tnew.rs\n\u{1e}def\u{1f}\u{1f}Bob\u{1f}b@x\u{1f}1600000000\u{1f}\u{1f}root\n\nA\told.rs\n";
        let c = parse_file_log(log, "new.rs");
        assert_eq!(c.iter().map(|c| (c.sha.as_str(), c.path.as_str())).collect::<Vec<_>>(), [("abc", "new.rs"), ("def", "old.rs")]);
        assert!(c[1].parents.is_empty() && c[0].subject == "Rename it");
    }

    #[test]
    fn blames_and_follows_file_history_against_real_repo() {
        let dir = std::env::temp_dir().join(format!("loupe-blame-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        let git = |args: &[&str]| crate::git::run(args, d, None, false).unwrap();
        let commit = |m: &str| git(&["-c", "user.name=Ann", "-c", "user.email=a@x", "-c", "commit.gpgsign=false", "commit", "-qm", m]);
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["add", "."]);
        commit("first");
        git(&["mv", "a.txt", "b.txt"]);
        commit("move");
        let b = blame("b.txt", None, d).unwrap();
        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.commits.values().next().unwrap().summary, "first");
        let log = file_log("b.txt", d).unwrap();
        assert_eq!(log.iter().map(|c| (c.subject.as_str(), c.path.as_str())).collect::<Vec<_>>(), [("move", "b.txt"), ("first", "a.txt")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
