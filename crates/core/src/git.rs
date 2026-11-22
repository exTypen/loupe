//! Thin wrapper over the git CLI. The CLI (not libgit2) keeps us 100% compatible with worktrees, hooks, credential
//! helpers, signing, and whatever the user's config does.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::diff::{self, FileDiff};
use crate::submodules::SubmoduleStatus;
use crate::undo::UndoRefusal;

// MARK: - Errors

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// git (or the file system) failed; the message is git's stderr when it said something.
    Git(String),
    /// The read was cancelled because whoever asked went away. Not an error to show.
    Cancelled,
    /// An undo that would be unsafe.
    Refused(UndoRefusal),
}

impl Error {
    pub fn msg(s: impl Into<String>) -> Self { Error::Git(s.into()) }

    /// What to show for a failure, or None for a cancellation: a task stopped because its view moved on is not an error.
    pub fn user_message(&self) -> Option<String> {
        match self {
            Error::Cancelled => None,
            e => Some(e.to_string()),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Git(m) => f.write_str(m),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Refused(r) => f.write_str(&r.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Git(e.to_string()) }
}

pub type Result<T> = std::result::Result<T, Error>;

// MARK: - Running git

static TRACE: LazyLock<bool> = LazyLock::new(|| std::env::var_os("LOUPE_TRACE").is_some());

pub fn binary() -> &'static str {
    static BIN: LazyLock<&'static str> = LazyLock::new(|| {
        ["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"]
            .into_iter()
            .find(|p| is_executable(Path::new(p)))
            .unwrap_or("/usr/bin/git")
    });
    &BIN
}

pub(crate) fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Opened from Finder the app's PATH is /usr/bin:/bin:..., so git couldn't find Homebrew's git-lfs (its filters
/// would fail on checkout and pull) or the tools hooks call. A terminal has them; so does git run from here.
pub fn search_path(path: Option<&str>) -> String {
    let mut dirs: Vec<&str> = path.unwrap_or("/usr/bin:/bin:/usr/sbin:/sbin").split(':').filter(|s| !s.is_empty()).collect();
    for d in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if !dirs.contains(&d) { dirs.push(d) }
    }
    dirs.join(":")
}

/// Subcommands that only read: safe to stop halfway.
pub fn is_read_only<S: AsRef<OsStr>>(args: &[S]) -> bool {
    let mut i = 0;
    while i < args.len() && args[i].as_ref().to_string_lossy().starts_with('-') {
        let a = args[i].as_ref();
        i += if a == "-c" || a == "-C" { 2 } else { 1 };
    }
    let Some(cmd) = args.get(i) else { return false };
    let cmd = cmd.as_ref().to_string_lossy();
    ["show", "diff", "log", "grep", "ls-files", "ls-tree", "cat-file", "rev-list", "blame", "diff-tree", "merge-base",
     "for-each-ref", "status", "rev-parse", "show-ref"]
        .contains(&cmd.as_ref())
}

/// Stops the reads it's given to: the view that asked went away. A `git show` of a huge file no longer keeps running
/// and buffering after nobody wants it. Commands that change the repository ignore it and always run to the end.
/// One token can serve many runs (everything a view started); cancelling stops the one running and any later ones.
#[derive(Clone, Default)]
pub struct Cancel(Arc<Mutex<CancelState>>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    pid: Option<i32>,
}

impl Cancel {
    pub fn new() -> Self { Self::default() }

    pub fn cancel(&self) {
        let mut s = self.0.lock().unwrap();
        s.cancelled = true;
        // The pid is cleared before the child is reaped, so this never signals a reused pid.
        if let Some(pid) = s.pid { unsafe { libc::kill(pid, libc::SIGTERM); } }
    }

    pub fn is_cancelled(&self) -> bool { self.0.lock().unwrap().cancelled }
}

/// One git invocation: `git(dir, args).stdin(..).env(..).run()`.
pub struct Git<'a> {
    dir: PathBuf,
    args: Vec<OsString>,
    stdin: Option<&'a [u8]>,
    allow_failure: bool,
    env: Vec<(OsString, OsString)>,
    cancel: Option<&'a Cancel>,
}

pub fn git<'a, I, S>(dir: impl AsRef<Path>, args: I) -> Git<'a>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Git {
        dir: dir.as_ref().to_path_buf(),
        args: args.into_iter().map(|s| s.as_ref().to_os_string()).collect(),
        stdin: None,
        allow_failure: false,
        env: vec![],
        cancel: None,
    }
}

/// `git <args>` in `dir`; its stdout.
pub fn run<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(dir: impl AsRef<Path>, args: I) -> Result<Vec<u8>> { git(dir, args).run() }

/// `git <args>` in `dir`; its stdout as text.
pub fn string<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(dir: impl AsRef<Path>, args: I) -> Result<String> { git(dir, args).string() }

/// `git <args>` in `dir`; its stdout trimmed of surrounding whitespace.
pub fn trimmed<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(dir: impl AsRef<Path>, args: I) -> Result<String> { git(dir, args).trimmed() }

impl<'a> Git<'a> {
    pub fn stdin(mut self, data: &'a [u8]) -> Self { self.stdin = Some(data); self }
    pub fn allow_failure(mut self) -> Self { self.allow_failure = true; self }
    pub fn cancel(mut self, c: &'a Cancel) -> Self { self.cancel = Some(c); self }
    pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(mut self, vars: impl IntoIterator<Item = (K, V)>) -> Self {
        self.env.extend(vars.into_iter().map(|(k, v)| (k.as_ref().to_os_string(), v.as_ref().to_os_string())));
        self
    }

    pub fn string(self) -> Result<String> { self.run().map(|d| String::from_utf8_lossy(&d).into_owned()) }
    pub fn trimmed(self) -> Result<String> { self.string().map(|s| s.trim().to_string()) }

    pub fn run(self) -> Result<Vec<u8>> {
        let enqueued = Instant::now();
        let killable = self.cancel.filter(|_| is_read_only(&self.args));
        let mut cmd = Command::new(binary());
        cmd.args(&self.args)
            .current_dir(&self.dir)
            .env("GIT_OPTIONAL_LOCKS", "0") // status must not fight other git processes for index.lock
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("PATH", search_path(std::env::var("PATH").ok().as_deref()))
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Launched under the token's lock: a cancel racing the launch either prevents it or sees the pid.
        let mut child = match killable {
            Some(c) => {
                let mut s = c.0.lock().unwrap();
                if s.cancelled { return Err(Error::Cancelled) }
                let child = cmd.spawn()?;
                s.pid = Some(child.id() as i32);
                child
            }
            None => cmd.spawn()?,
        };
        let started = Instant::now();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let stdin_pipe = child.stdin.take();
        let status = std::thread::scope(|scope| {
            if let (Some(mut w), Some(data)) = (stdin_pipe, self.stdin) {
                scope.spawn(move || { let _ = w.write_all(data); });
            }
            // stdout and stderr drained together, so a chatty stderr can't deadlock a big stdout.
            while stdout.is_some() {
                let fds: Vec<i32> = [stdout.as_ref().map(|s| s.as_raw_fd()), stderr.as_ref().map(|s| s.as_raw_fd())]
                    .into_iter().flatten().collect();
                for fd in poll_readable(&fds, -1) {
                    if stdout.as_ref().is_some_and(|s| s.as_raw_fd() == fd) {
                        if !read_some(stdout.as_mut().unwrap(), &mut out) { stdout = None }
                    } else if stderr.as_ref().is_some_and(|s| s.as_raw_fd() == fd)
                        && !read_some(stderr.as_mut().unwrap(), &mut err) { stderr = None }
                }
            }
            if let Some(c) = killable { c.0.lock().unwrap().pid = None }
            let status = child.wait();
            // stderr ends when every holder of the pipe closes it: a daemon git spawned (fsmonitor) inherits it and
            // never does, so after git exits we wait a moment, not forever (that parked ~90 threads and hung quit).
            let deadline = Instant::now() + Duration::from_secs(2);
            while let Some(e) = stderr.as_mut() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() { break }
                if !poll_readable(&[e.as_raw_fd()], left.as_millis().max(1) as i32).is_empty() && !read_some(e, &mut err) {
                    stderr = None;
                }
            }
            status
        })?;
        if *TRACE {
            let a: Vec<_> = self.args.iter().take(3).map(|s| s.to_string_lossy()).collect();
            eprintln!("[git {:.0}ms queued+run] {} ({:.0}ms run)", enqueued.elapsed().as_secs_f64() * 1000.0, a.join(" "),
                      started.elapsed().as_secs_f64() * 1000.0);
        }
        if killable.is_some_and(|c| c.is_cancelled()) { return Err(Error::Cancelled) }
        if !status.success() && !self.allow_failure {
            let msg = String::from_utf8_lossy(&err).trim().to_string();
            return Err(Error::Git(if msg.is_empty() {
                let first = self.args.first().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                format!("git {first} failed ({})", status.code().unwrap_or(-1))
            } else { msg }));
        }
        Ok(out)
    }
}

/// The fds of `fds` ready to read (or at EOF), waiting up to `timeout_ms` (-1: forever).
fn poll_readable(fds: &[i32], timeout_ms: i32) -> Vec<i32> {
    let mut p: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    loop {
        let n = unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, timeout_ms) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue }
        return p.iter().filter(|x| x.revents != 0).map(|x| x.fd).collect();
    }
}

/// One read into `buf`; false at EOF (or a read error, which ends the stream the same way).
fn read_some(r: &mut impl Read, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => { buf.extend_from_slice(&chunk[..n]); return true }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
}

/// Makes git authenticate to a space's server with the space's token instead of whatever credential helper the user
/// has (osxkeychain holds one login per host, which is what spaces are meant to get around). Only HTTPS remotes ask
/// for credentials; SSH remotes keep using the user's keys.
pub fn auth_env(base_url: &str, username: &str, token: &str) -> Vec<(String, String)> {
    let key = format!("credential.{base_url}.helper");
    vec![
        ("GIT_CONFIG_COUNT".into(), "2".into()),
        ("GIT_CONFIG_KEY_0".into(), key.clone()),
        ("GIT_CONFIG_VALUE_0".into(), String::new()), // empty resets the helper list for this URL
        ("GIT_CONFIG_KEY_1".into(), key),
        ("GIT_CONFIG_VALUE_1".into(),
         r#"!f() { test "$1" = get && echo username="$LOUPE_GIT_USER" && echo password="$LOUPE_GIT_TOKEN"; }; f"#.into()),
        ("LOUPE_GIT_USER".into(), username.into()),
        ("LOUPE_GIT_TOKEN".into(), token.into()),
    ]
}

pub fn git_dir(dir: &Path) -> Option<String> {
    trimmed(dir, ["rev-parse", "--path-format=absolute", "--git-dir"]).ok().filter(|s| !s.is_empty())
}

pub fn common_dir(dir: &Path) -> Option<String> {
    trimmed(dir, ["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok().filter(|s| !s.is_empty())
}

/// `remote -v`: each remote's name and fetch URL.
pub fn remotes(dir: &Path) -> Vec<(String, String)> {
    let out = string(dir, ["remote", "-v"]).unwrap_or_default();
    out.lines()
        .filter(|l| l.ends_with("(fetch)"))
        .filter_map(|l| {
            let mut parts = l.split(['\t', ' ']).filter(|s| !s.is_empty());
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect()
}

// MARK: - Worktrees

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Worktree {
    pub path: String,
    pub head: String,
    pub branch: Option<String>, // short name, None when detached
    pub is_bare: bool,
    pub is_locked: bool,
    pub is_prunable: bool,
}

impl Worktree {
    pub fn name(&self) -> &str { self.path.rsplit('/').next().unwrap_or(&self.path) }
}

pub fn worktrees(dir: &Path) -> Result<Vec<Worktree>> {
    let mut list = parse_worktrees(&string(dir, ["worktree", "list", "--porcelain"])?);
    // A submodule opened as a repo lists its git dir (<super>/.git/modules/<name>) as its main worktree. Its
    // core.worktree names the real one; asking from `dir` alone would give a linked worktree's own folder.
    if let Some(first) = list.first().map(|w| w.path.clone()).filter(|p| p.contains("/.git/modules/"))
        && let Ok(top) = trimmed(dir, [format!("--git-dir={first}").as_str(), "rev-parse", "--show-toplevel"])
            && !top.is_empty() { list[0].path = top }
    Ok(list)
}

pub fn parse_worktrees(s: &str) -> Vec<Worktree> {
    let mut result = vec![];
    let mut cur: Option<Worktree> = None;
    for line in s.split('\n') {
        if line.is_empty() {
            result.extend(cur.take());
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match (key, cur.as_mut()) {
            ("worktree", _) => cur = Some(Worktree { path: value.into(), ..Default::default() }),
            ("HEAD", Some(c)) => c.head = value.into(),
            ("branch", Some(c)) => c.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into()),
            ("bare", Some(c)) => c.is_bare = true,
            ("locked", Some(c)) => c.is_locked = true,
            ("prunable", Some(c)) => c.is_prunable = true,
            _ => {}
        }
    }
    result.extend(cur);
    result
}

/// The branch finished work lands on, and its tip. The remote's view first: a local main is often behind it.
pub fn integration_target(remote: &str, dir: &Path) -> Option<(String, String)> {
    for r in [format!("{remote}/HEAD"), format!("{remote}/main"), format!("{remote}/master"), "main".into(), "master".into()] {
        let out = string(dir, ["rev-parse", r.as_str(), "--abbrev-ref", r.as_str()]).unwrap_or_default();
        let f: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        if f.len() == 2 { return Some((f[1].into(), f[0].into())) }
    }
    None
}

/// Whether the branch's work is already on `target`, however it got there.
pub fn is_merged(branch: &str, target: &str, dir: &Path) -> bool {
    let out = |args: &[&str]| trimmed(dir, args).ok();
    let Some(ahead) = out(&["rev-list", "--count", &format!("{target}..{branch}")]) else { return false };
    if ahead != "0" {
        // Squashed or rebased: its commits aren't on target, but merging them would change nothing.
        let Some(merged) = out(&["merge-tree", "--write-tree", "--no-messages", target, branch]) else { return false }; // conflicts fail
        return Some(merged) == out(&["rev-parse", &format!("{target}^{{tree}}")]);
    }
    // No commits of its own: a branch just cut from target looks the same as one merged, unless it was ever committed on.
    // ponytail: a PR checked out and merged with a merge commit, never committed on locally, reads as not merged; reflog is the only local signal.
    out(&["reflog", "show", "--format=%gs", &format!("refs/heads/{branch}")])
        .is_some_and(|s| s.lines().any(|l| l.starts_with("commit")))
}

// MARK: - Status

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind { Modified, Added, Deleted, Renamed, Copied, TypeChanged, Untracked, Conflicted }

impl Kind {
    pub fn from_char(c: char) -> Kind {
        match c {
            'A' => Kind::Added,
            'D' => Kind::Deleted,
            'R' => Kind::Renamed,
            'C' => Kind::Copied,
            'T' => Kind::TypeChanged,
            '?' => Kind::Untracked,
            'U' => Kind::Conflicted,
            _ => Kind::Modified,
        }
    }
    pub fn letter(self) -> char {
        match self {
            Kind::Modified => 'M', Kind::Added => 'A', Kind::Deleted => 'D', Kind::Renamed => 'R',
            Kind::Copied => 'C', Kind::TypeChanged => 'T', Kind::Untracked => '?', Kind::Conflicted => 'U',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileChange {
    pub path: String,
    pub orig_path: Option<String>,
    pub kind: Kind,
    pub staged: bool,
    /// A submodule, not a file: its recorded commit or its contents changed.
    pub submodule: Option<SubmoduleStatus>,
}

impl FileChange {
    pub fn new(path: impl Into<String>, kind: Kind, staged: bool) -> Self {
        FileChange { path: path.into(), orig_path: None, kind, staged, submodule: None }
    }
    /// "S:path" or "W:path": the same path can be both staged and unstaged.
    pub fn id(&self) -> String { format!("{}{}", if self.staged { "S:" } else { "W:" }, self.path) }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub branch: Option<String>,
    pub oid: String,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
}

pub fn status(dir: &Path) -> Result<Status> {
    Ok(parse_status(&run(dir, ["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"])?))
}

/// Parses `git status --porcelain=v2 -z --branch`. Each file can yield a staged and an unstaged entry.
pub fn parse_status(data: &[u8]) -> Status {
    let mut s = Status::default();
    let parts: Vec<String> = data.split(|&b| b == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
    let mut i = 0;
    while i < parts.len() {
        let e = &parts[i];
        i += 1;
        let Some(t) = e.chars().next() else { continue };
        match t {
            '#' => {
                if let Some(v) = e.strip_prefix("# branch.oid ") { s.oid = v.into() }
                if let Some(h) = e.strip_prefix("# branch.head ") { s.branch = (h != "(detached)").then(|| h.into()) }
                if let Some(ab) = e.strip_prefix("# branch.ab ") {
                    let ab: Vec<&str> = ab.split(' ').filter(|x| !x.is_empty()).collect();
                    if ab.len() == 2 {
                        s.ahead = ab[0].get(1..).and_then(|x| x.parse().ok()).unwrap_or(0);
                        s.behind = ab[1].get(1..).and_then(|x| x.parse().ok()).unwrap_or(0);
                    }
                }
            }
            '1' | '2' => {
                // 1 XY sub mH mI mW hH hI path   |   2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let n = if t == '1' { 9 } else { 10 };
                let f: Vec<&str> = e.splitn(n, ' ').collect();
                if f.len() < n { continue }
                let xy: Vec<char> = f[1].chars().collect();
                if xy.len() < 2 { continue }
                let path = f[n - 1].to_string();
                let mut orig = None;
                if t == '2' && i < parts.len() {
                    orig = Some(parts[i].clone());
                    i += 1;
                }
                let sub = SubmoduleStatus::parse(f[2]);
                if xy[0] != '.' {
                    s.files.push(FileChange { path: path.clone(), orig_path: orig.clone(), kind: Kind::from_char(xy[0]), staged: true, submodule: sub });
                }
                if xy[1] != '.' {
                    s.files.push(FileChange { path, orig_path: if xy[1] == 'R' { orig } else { None }, kind: Kind::from_char(xy[1]), staged: false, submodule: sub });
                }
            }
            'u' => {
                let f: Vec<&str> = e.splitn(11, ' ').collect();
                if f.len() >= 11 { s.files.push(FileChange::new(f[10], Kind::Conflicted, false)) }
            }
            '?' => s.files.push(FileChange::new(e.get(2..).unwrap_or(""), Kind::Untracked, false)),
            _ => {}
        }
    }
    s
}

/// The diff of one entry of the status list. `context` lines around changes; a huge number gives the whole file.
pub fn diff_for(file: &FileChange, dir: &Path, context: usize, cancel: Option<&Cancel>) -> Result<Option<FileDiff>> {
    let u = format!("-U{context}");
    let args: Vec<&str> = if file.kind == Kind::Untracked {
        vec!["diff", "--no-color", "--no-ext-diff", "--no-index", &u, "--", "/dev/null", &file.path]
    } else {
        let mut a = vec!["diff", "--no-color", "--no-ext-diff", "-M", &u];
        if file.staged { a.push("--cached") }
        a.extend(["--", &file.path]);
        a.extend(file.orig_path.as_deref());
        a
    };
    let mut g = git(dir, &args);
    if let Some(c) = cancel { g = g.cancel(c) }
    // --no-index exits 1 when files differ.
    if file.kind == Kind::Untracked { g = g.allow_failure() }
    Ok(diff::parse_bytes(&g.run()?).into_iter().next())
}

/// Local branches (with the worktree each is checked out in) and remote branches, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branches {
    pub local: Vec<String>,
    pub checked_out: HashMap<String, String>, // local branch → worktree path holding it
    /// "origin/x" for remote branches with no local x (those are hidden: picking one makes the local branch).
    pub remote: Vec<String>,
}

pub fn branches(dir: &Path) -> Branches {
    // lstrip=2, not refname:short: short names refs/remotes/origin/HEAD plain "origin".
    let refs = |ns: &str| -> Vec<(String, String)> {
        string(dir, ["for-each-ref", "--format=%(refname:lstrip=2)%09%(worktreepath)", "--sort=-committerdate", ns])
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (a, b) = l.split_once('\t').unwrap_or((l, ""));
                (a.to_string(), b.to_string())
            })
            .collect()
    };
    let heads = refs("refs/heads");
    let local: Vec<String> = heads.iter().map(|h| h.0.clone()).collect();
    let checked_out = heads.into_iter().filter(|h| !h.1.is_empty()).collect();
    let remote = refs("refs/remotes")
        .into_iter()
        .map(|r| r.0)
        .filter(|b| !b.ends_with("/HEAD") && !local.iter().any(|l| Some(l.as_str()) == b.split_once('/').map(|x| x.1)))
        .collect();
    Branches { local, checked_out, remote }
}

/// All branch names, local and remote, newest first (the base picker of New Worktree).
pub fn all_branch_names(dir: &Path) -> Vec<String> {
    string(dir, ["for-each-ref", "--format=%(refname:short)", "--sort=-committerdate", "refs/heads", "refs/remotes"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
        .map(String::from)
        .collect()
}

pub fn is_local_branch(dir: &Path, b: &str) -> bool {
    run(dir, ["show-ref", "--verify", "--quiet", &format!("refs/heads/{b}")]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_v2() {
        let raw = "# branch.head main\0# branch.ab +2 -1\01 M. N... 100644 100644 100644 a b src/x.swift\01 .M N... 100644 100644 100644 a b y z.txt\02 R. N... 100644 100644 100644 a b R100 new.txt\0old.txt\0? untracked.md\0";
        let s = parse_status(raw.as_bytes());
        assert!(s.branch.as_deref() == Some("main") && s.ahead == 2 && s.behind == 1);
        let paths: Vec<_> = s.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/x.swift", "y z.txt", "new.txt", "untracked.md"]);
        assert!(s.files[0].staged && !s.files[1].staged);
        assert!(s.files[2].orig_path.as_deref() == Some("old.txt") && s.files[2].kind == Kind::Renamed);
        assert_eq!(s.files[3].kind, Kind::Untracked);
    }

    #[test]
    fn parses_worktrees() {
        let w = parse_worktrees("worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r-feat\nHEAD def\ndetached\nlocked\n\n");
        assert!(w.len() == 2 && w[0].branch.as_deref() == Some("main") && w[1].branch.is_none() && w[1].is_locked);
    }

    #[test]
    fn finds_homebrew_for_git_even_from_finder() {
        assert_eq!(search_path(Some("/usr/bin:/bin")), "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin");
        assert_eq!(search_path(Some("/opt/homebrew/bin:/usr/bin")), "/opt/homebrew/bin:/usr/bin:/usr/local/bin");
    }

    #[test]
    fn read_only_detection() {
        assert!(is_read_only(&["show", "HEAD:a"]));
        assert!(is_read_only(&["-c", "core.quotepath=off", "diff", "--no-color"]));
        assert!(!is_read_only(&["commit", "-m", "x"]));
        assert!(!is_read_only(&["-c", "core.editor=true", "rebase", "--continue"]));
    }

    #[test]
    fn cancelled_before_launch_never_runs() {
        let c = Cancel::new();
        c.cancel();
        assert_eq!(git(std::env::temp_dir(), ["status"]).cancel(&c).run(), Err(Error::Cancelled));
    }

    #[test]
    fn cancelling_stops_a_running_read() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), ["init", "-q"]).unwrap();
        let c = Cancel::new();
        let c2 = c.clone();
        // A read-only command kept busy by more input than it gets through before the cancel.
        let input = b"HEAD\n".repeat(4_000_000);
        let t = std::thread::spawn(move || git(dir.path(), ["cat-file", "--batch-check"]).cancel(&c2).stdin(&input).run());
        std::thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        c.cancel();
        assert_eq!(t.join().unwrap(), Err(Error::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn writes_are_not_killed() {
        let c = Cancel::new();
        c.cancel();
        let dir = tempfile::tempdir().unwrap();
        // `init` changes the repository: it runs although the token is cancelled.
        assert!(git(dir.path(), ["init", "-q"]).cancel(&c).run().is_ok());
    }

    #[test]
    fn stderr_held_open_by_a_daemon_does_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), ["init", "-q"]).unwrap();
        // An alias that leaves a background process holding stderr, as fsmonitor's daemon does.
        let start = Instant::now();
        let r = run(dir.path(), ["-c", "alias.bg=!sleep 30 1>/dev/null 2>&2 & echo ok", "bg"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&r).trim(), "ok");
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }
}
