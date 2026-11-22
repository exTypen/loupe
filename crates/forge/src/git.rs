//! The few git calls this crate makes (PR diff fallback, PR create, commit message prompt). A local copy of the
//! rules in Git.swift; loupe-core owns the full runner.

use crate::{Cancel, Error, Result};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub const BINARY: &str = "/usr/bin/git";

fn binary() -> &'static str {
    // Homebrew's git when there is one, as Git.swift prefers; Apple's otherwise.
    if std::path::Path::new("/opt/homebrew/bin/git").exists() { "/opt/homebrew/bin/git" } else { BINARY }
}

/// Runs git in `dir` and returns stdout. A cancelled read-only command is killed; commands that change the
/// repository (fetch, push) always run to the end. stderr is drained on its own thread and waited on for 2 s only:
/// a daemon git spawns (fsmonitor) inherits the pipe and never closes it.
pub fn run(args: &[&str], dir: &str, env: &[(String, String)], cancel: Option<&Cancel>) -> Result<Vec<u8>> {
    if let Some(c) = cancel { c.check()?; }
    let mut cmd = Command::new(binary());
    cmd.args(args).current_dir(dir).env("GIT_OPTIONAL_LOCKS", "0").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env { cmd.env(k, v); }
    let mut child = cmd.spawn().map_err(|e| Error::msg(format!("git: {e}")))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        let _ = tx.send(b);
    });
    let killable = matches!(args.first().copied(), Some("diff" | "log" | "rev-list" | "cat-file" | "for-each-ref" | "rev-parse" | "ls-files" | "show" | "ls-tree" | "merge-base"));
    let (otx, orx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        let _ = otx.send(b);
    });
    let out = loop {
        match orx.recv_timeout(Duration::from_millis(50)) {
            Ok(b) => break b,
            Err(mpsc::RecvTimeoutError::Disconnected) => break Vec::new(),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if killable && cancel.is_some_and(|c| c.is_cancelled()) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Cancelled);
                }
            }
        }
    };
    let status = child.wait().map_err(|e| Error::msg(format!("git: {e}")))?;
    let err = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        let msg = String::from_utf8_lossy(if err.is_empty() { &out } else { &err }).trim().to_string();
        Err(Error::msg(if msg.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { msg }))
    }
}

pub fn string(args: &[&str], dir: &str, cancel: Option<&Cancel>) -> Result<String> {
    run(args, dir, &[], cancel).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The branch new work is measured against: the remote's HEAD, else main/master, remote then local.
/// (name as git abbreviates it, oid)
pub fn integration_target(remote: &str, dir: &str) -> Option<(String, String)> {
    for r in [format!("{remote}/HEAD"), format!("{remote}/main"), format!("{remote}/master"), "main".into(), "master".into()] {
        let Ok(out) = string(&["rev-parse", &r, "--abbrev-ref", &r], dir, None) else { continue };
        let f: Vec<&str> = out.lines().collect();
        if f.len() == 2 { return Some((f[1].to_string(), f[0].to_string())); }
    }
    None
}
