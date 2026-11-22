//! The few read-only git calls code intelligence makes (grep, show, ls-tree, cat-file, diff, blame, log). A local
//! runner so this crate doesn't depend on loupe-core; it follows Git.swift's rules.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

static BINARY: LazyLock<&'static str> = LazyLock::new(|| {
    ["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"]
        .into_iter()
        .find(|p| std::fs::metadata(p).is_ok())
        .unwrap_or("/usr/bin/git")
});

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for GitError {}

/// Runs git in `dir` and returns its stdout. A non-zero exit is an error unless `allow_failure`.
pub fn run(args: &[&str], dir: &str, stdin: Option<&[u8]>, allow_failure: bool) -> Result<Vec<u8>, GitError> {
    let mut cmd = Command::new(*BINARY);
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0") // must not fight other git processes for index.lock
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        // Not piped: a daemon git spawns (fsmonitor) inherits stderr and never closes it, and reading it to the end
        // parked threads forever in the Swift app. These calls only read, so the message isn't needed.
        .stderr(Stdio::null())
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| GitError(format!("git: {e}")))?;
    let writer = stdin.map(|data| {
        let mut pipe = child.stdin.take().unwrap();
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        })
    });
    let out = child.wait_with_output().map_err(|e| GitError(format!("git: {e}")))?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    if !out.status.success() && !allow_failure {
        return Err(GitError(format!("git {} failed ({})", args.first().unwrap_or(&""), out.status)));
    }
    Ok(out.stdout)
}

pub fn string(args: &[&str], dir: &str) -> Result<String, GitError> {
    run(args, dir, None, false).map(|d| String::from_utf8_lossy(&d).into_owned())
}
