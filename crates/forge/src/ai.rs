//! Commit messages (and PR titles) written by a local coding-agent CLI, on the user's own subscription: no API key
//! in Loupe. Each CLI runs in its read-only / ask mode with the diff in the prompt, so it needs no tools and can't
//! change the repository.

use crate::settings::Prefs;
use crate::{git, Cancel, Error, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelOption {
    pub id: &'static str,
    pub label: &'static str,
}

const fn m(id: &'static str) -> ModelOption {
    ModelOption { id, label: id }
}
const fn ml(id: &'static str, label: &'static str) -> ModelOption {
    ModelOption { id, label }
}

/// argv for one prompt, and whether the prompt goes on stdin. `out` is a temp file for CLIs that write their final
/// answer there (Codex), so progress output can't leak into the message.
type Invocation = fn(prompt: &str, model: &str, out: &str) -> (Vec<String>, bool);

/// A coding-agent CLI that can answer one prompt and exit.
#[derive(Debug, Clone)]
pub struct Harness {
    pub id: &'static str,
    pub name: &'static str,
    pub binaries: &'static [&'static str],
    /// Suggestions; the field takes any id, empty = the CLI's own default.
    pub models: &'static [ModelOption],
    /// A command that prints one model id per line.
    pub list_models: Option<&'static [&'static str]>,
    pub invocation: Invocation,
    pub reads_output_file: bool,
}

impl PartialEq for Harness {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}
fn flag(f: &str, val: &str) -> Vec<String> {
    if val.is_empty() { vec![] } else { v(&[f, val]) }
}

// Flags checked against each CLI's docs (2026-10); claude and opencode also against their --help.
pub static HARNESSES: &[Harness] = &[
    Harness {
        id: "claude",
        name: "Claude Code",
        binaries: &["claude"],
        models: &[
            ml("claude-haiku-4-5-20251001", "Claude Haiku 4.5 · fastest"),
            ml("claude-sonnet-5", "Claude Sonnet 5"),
            ml("claude-opus-5-5", "Claude Opus 5.5"),
            ml("claude-fable-5-1", "Claude Fable 5.1 · most capable"),
        ],
        list_models: None,
        invocation: |_, m, _| {
            ([v(&["-p", "--tools", "", "--setting-sources", "", "--strict-mcp-config", "--no-session-persistence", "--output-format", "text"]), flag("--model", m)].concat(), true)
        },
        reads_output_file: false,
    },
    Harness {
        id: "codex",
        name: "Codex",
        binaries: &["codex"],
        models: &[m("gpt-6.1-sol"), m("gpt-6-astra"), m("gpt-6-luna")],
        list_models: None,
        invocation: |_, m, out| ([v(&["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-o", out]), flag("-m", m), v(&["-"])].concat(), true),
        reads_output_file: true,
    },
    Harness {
        id: "gemini",
        name: "Gemini CLI",
        binaries: &["gemini"],
        models: &[m("flash"), m("pro"), m("flash-lite"), m("gemini-2.5-flash")],
        list_models: None,
        invocation: |p, m, _| ([v(&["--output-format", "text"]), flag("-m", m), v(&["-p", p])].concat(), false),
        reads_output_file: false,
    },
    Harness {
        id: "copilot",
        name: "GitHub Copilot CLI",
        binaries: &["copilot"],
        models: &[m("claude-sonnet-5"), m("gpt-6.1-sol"), m("gpt-5.3-codex")],
        list_models: None,
        invocation: |p, m, _| ([v(&["-p", p, "-s", "--no-ask-user", "--no-color"]), flag("--model", m)].concat(), false),
        reads_output_file: false,
    },
    Harness {
        id: "cursor",
        name: "Cursor CLI",
        binaries: &["cursor-agent"],
        models: &[m("auto")],
        list_models: Some(&["models"]),
        invocation: |p, m, _| ([v(&["-p", "--output-format", "text", "--mode", "ask", "--trust"]), flag("--model", m), v(&[p])].concat(), false),
        reads_output_file: false,
    },
    Harness {
        id: "opencode",
        name: "opencode",
        binaries: &["opencode"],
        models: &[],
        list_models: Some(&["models"]),
        invocation: |p, m, _| ([v(&["run"]), flag("-m", m), v(&[p])].concat(), false),
        reads_output_file: false,
    },
    Harness {
        id: "qwen",
        name: "Qwen Code",
        binaries: &["qwen"],
        models: &[m("qwen3-coder-plus")],
        list_models: None,
        invocation: |p, m, _| ([v(&["-p", p, "--output-format", "text", "--approval-mode", "plan"]), flag("-m", m)].concat(), false),
        reads_output_file: false,
    },
    Harness { id: "amp", name: "Amp", binaries: &["amp"], models: &[], list_models: None, invocation: |_, _, _| (v(&["-x"]), true), reads_output_file: false },
    Harness {
        id: "droid",
        name: "Factory Droid",
        binaries: &["droid"],
        models: &[m("claude-opus-5-5"), m("gpt-6-sol")],
        list_models: None,
        invocation: |_, m, _| ([v(&["exec", "-o", "text"]), flag("-m", m)].concat(), true),
        reads_output_file: false,
    },
];

/// How the message should read (`aiStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitStyle {
    Repository,
    Concise,
    Standard,
    Detailed,
    Conventional,
}

impl CommitStyle {
    pub const ALL: [CommitStyle; 5] = [CommitStyle::Repository, CommitStyle::Concise, CommitStyle::Standard, CommitStyle::Detailed, CommitStyle::Conventional];

    pub fn id(self) -> &'static str {
        ["repository", "concise", "standard", "detailed", "conventional"][self as usize]
    }
    pub fn parse(s: &str) -> Self {
        Self::ALL.into_iter().find(|c| c.id() == s).unwrap_or(CommitStyle::Repository)
    }
    pub fn title(self) -> &'static str {
        ["Match the repository", "Concise", "Standard", "Detailed", "Conventional Commits"][self as usize]
    }
    pub fn summary(self) -> &'static str {
        [
            "Follows the style, length and language of the recent commits.",
            "A single summary line, no description.",
            "A summary line, and a short description only when the change needs explaining.",
            "A summary line and a description of what changed and why, as bullet points.",
            "type(scope): summary, as in feat(api): …, with a short description when needed.",
        ][self as usize]
    }
    pub fn instruction(self) -> &'static str {
        [
            "Match the style, language, length and conventions of the recent commit subjects (use prefixes like \"feat:\" only if they do). Add a short description only if the change needs explaining.",
            "Write only a summary line of at most 60 characters, in the imperative mood. No description.",
            "Write a summary line of at most 72 characters in the imperative mood. Add a description of a few short lines only if the change needs explaining.",
            "Write a summary line of at most 72 characters in the imperative mood, then a blank line, then a description: a few bullet points on what changed and why, mentioning anything a reviewer should know.",
            "Use the Conventional Commits format: type(optional scope): summary, with type one of feat, fix, refactor, perf, docs, test, build, ci, chore, style. Summary at most 72 characters, imperative, lowercase after the colon. Add a short description only if the change needs explaining.",
        ][self as usize]
    }
}

/// `aiHarness`: the first installed one.
pub const AUTO: &str = "auto";
pub const OFF: &str = "off";

/// The usual install locations, then the login shell's PATH (nvm, asdf, custom prefixes). Blocking.
pub fn locate(names: &[&str]) -> HashMap<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".into(),
        format!("{home}/.local/bin"),
        format!("{home}/.claude/local"),
        format!("{home}/.bun/bin"),
        format!("{home}/.npm-global/bin"),
        format!("{home}/.opencode/bin"),
        format!("{home}/.cargo/bin"),
        format!("{home}/.amp/bin"),
        format!("{home}/.factory/bin"),
    ];
    let mut found = HashMap::new();
    for n in names {
        if let Some(d) = dirs.iter().find(|d| is_executable(&format!("{d}/{n}"))) { found.insert(n.to_string(), format!("{d}/{n}")); }
    }
    let missing: Vec<&str> = names.iter().copied().filter(|n| !found.contains_key(*n)).collect();
    if missing.is_empty() { return found; }
    let script = missing.iter().map(|n| format!("command -v {n} 2>/dev/null | sed 's|^|{n}=|'")).collect::<Vec<_>>().join("; ");
    if let Ok(o) = Command::new("/bin/zsh").args(["-lc", &script]).stdin(Stdio::null()).stderr(Stdio::null()).output() {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            if let Some((k, v)) = line.split_once('=')
                && v.starts_with('/') { found.insert(k.into(), v.into()); }
        }
    }
    found
}

fn is_executable(p: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn paths_cell() -> &'static Mutex<Option<HashMap<String, String>>> {
    static P: OnceLock<Mutex<Option<HashMap<String, String>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Looks the CLIs up (again: Settings' "Look Again"). Blocking. harness id → executable.
pub fn scan() -> HashMap<String, String> {
    let names: Vec<&str> = HARNESSES.iter().flat_map(|h| h.binaries.iter().copied()).collect();
    let found = locate(&names);
    let p: HashMap<String, String> =
        HARNESSES.iter().filter_map(|h| h.binaries.iter().find_map(|b| found.get(*b)).map(|path| (h.id.to_string(), path.clone()))).collect();
    *paths_cell().lock().unwrap() = Some(p.clone());
    p
}

/// What the last [`scan`] found; None before the first scan finished.
pub fn installed() -> Option<HashMap<String, String>> {
    paths_cell().lock().unwrap().clone()
}

/// The harness in use and its executable, if it's installed.
#[derive(Debug, Clone)]
pub struct Selected {
    pub harness: &'static Harness,
    pub path: String,
    pub model: String,
}

pub fn harness_id(p: &Prefs) -> String {
    p.string("aiHarness").unwrap_or_else(|| AUTO.into())
}

pub fn style(p: &Prefs) -> CommitStyle {
    CommitStyle::parse(&p.string("aiStyle").unwrap_or_default())
}

pub fn extra_instructions(p: &Prefs) -> String {
    p.string("aiInstructions").unwrap_or_default()
}

pub fn model_for(h: &Harness, p: &Prefs) -> String {
    let saved = p.string(&format!("aiModel.{}", h.id));
    if h.id != "claude" { return saved.unwrap_or_default(); }
    // Claude Code's aliases, saved by earlier builds, name no version; show the model they stand for.
    match saved.as_deref() {
        Some("haiku") | None => "claude-haiku-4-5-20251001".into(),
        Some("sonnet") => "claude-sonnet-5".into(),
        Some("opus") => "claude-opus-5-5".into(),
        Some(s) => s.into(),
    }
}

pub fn set_model(h: &Harness, model: &str, p: &mut Prefs) {
    p.set_string(&format!("aiModel.{}", h.id), model.trim());
}

pub fn selected(p: &Prefs, paths: &HashMap<String, String>) -> Option<Selected> {
    let id = harness_id(p);
    if id == OFF { return None; }
    let h = HARNESSES.iter().find(|h| paths.contains_key(h.id) && (id == AUTO || h.id == id))?;
    Some(Selected { harness: h, path: paths[h.id].clone(), model: model_for(h, p) })
}

/// One prompt through the harness, in its read-only mode; the reply cleaned of fences and escapes.
pub fn answer(s: &Selected, prompt: &str, dir: &str, cancel: Option<&Cancel>) -> Result<String> {
    let out = std::env::temp_dir().join(format!("loupe-msg-{}.txt", uuid::Uuid::new_v4().to_string().to_uppercase()));
    let out_s = out.to_string_lossy().into_owned();
    let (args, stdin) = (s.harness.invocation)(prompt, &s.model, &out_s);
    let r = run(&s.path, &args, stdin.then_some(prompt), dir, Duration::from_secs(120), cancel);
    let file = std::fs::read_to_string(&out).ok().filter(|f| !f.is_empty());
    let _ = std::fs::remove_file(&out);
    let mut text = r?;
    if s.harness.reads_output_file
        && let Some(f) = file { text = f; }
    let msg = clean(&text);
    if msg.is_empty() { return Err(Error::msg(format!("{} returned an empty message.", s.harness.name))); }
    Ok(msg)
}

/// A commit message for what's staged, or for everything when nothing is (plus untracked file names). Blocking.
pub fn message(s: Option<&Selected>, p: &Prefs, dir: &str, amend: bool, has_staged: bool, cancel: Option<&Cancel>) -> Result<String> {
    let Some(s) = s else { return Err(Error::msg("No AI tool is set up for commit messages. Pick one in Settings › General.")) };
    answer(s, &prompt(p, dir, amend, has_staged, cancel)?, dir, cancel)
}

fn prompt(p: &Prefs, dir: &str, amend: bool, has_staged: bool, cancel: Option<&Cancel>) -> Result<String> {
    let ok = |args: &[&str]| git::run(args, dir, &[], cancel).is_ok();
    let mut diff_args = vec!["diff", "--no-color", "--no-ext-diff", "-M", "-U2"];
    if amend && ok(&["rev-parse", "--verify", "-q", "HEAD~1"]) {
        diff_args.extend(["--cached", "HEAD~1"]);
    } else if has_staged {
        diff_args.push("--cached");
    } else if ok(&["rev-parse", "--verify", "-q", "HEAD"]) {
        diff_args.push("HEAD");
    }
    let stat_args: Vec<&str> = diff_args.iter().copied().filter(|a| *a != "-U2").chain(["--stat"]).collect();
    let (diff, stat, recent, untracked) = std::thread::scope(|sc| {
        let stat = sc.spawn(|| git::string(&stat_args, dir, cancel));
        let recent = sc.spawn(|| git::string(&["log", "-n", "15", "--format=%s"], dir, cancel).unwrap_or_default());
        let untracked = sc.spawn(|| {
            if has_staged || amend { String::new() } else { git::string(&["ls-files", "--others", "--exclude-standard"], dir, cancel).unwrap_or_default() }
        });
        (git::string(&diff_args, dir, cancel), stat.join().unwrap(), recent.join().unwrap(), untracked.join().unwrap())
    });
    let patch = diff?;
    if patch.is_empty() && untracked.is_empty() { return Err(Error::msg("There are no changes to describe.")); }
    let patch = crate::pull_create::truncated(&patch);
    let extra = extra_instructions(p);
    let extra = extra.trim();
    let extra = if extra.is_empty() { String::new() } else { format!("\n{extra}") };
    let new_files = if untracked.is_empty() { String::new() } else { format!("\nNew files:\n{untracked}") };
    Ok(format!(
        "You write git commit messages. Below are this repository's recent commit subjects, then the change.

{}{extra}

Reply with only the commit message: the summary line, then (if any) a blank line and the description. No quotes, no code fences, no preamble. Don't run tools or look at files; everything you need is here.

Recent subjects:
{recent}
Summary:
{}{new_files}
Diff:
{patch}",
        style(p).instruction(),
        stat?
    ))
}

/// Drops ANSI escapes, code fences and wrapping quotes some CLIs add.
pub fn clean(s: &str) -> String {
    let mut t = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1B}' && it.peek() == Some(&'[') {
            // ESC [ [0-9;?]* letter
            let mut probe = it.clone();
            probe.next();
            while probe.peek().is_some_and(|c| c.is_ascii_digit() || *c == ';' || *c == '?') { probe.next(); }
            if probe.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                probe.next();
                it = probe;
                continue;
            }
        }
        t.push(c);
    }
    let mut t = t.trim().to_string();
    if t.starts_with("```") {
        t = t.split('\n').skip(1).filter(|l| !l.starts_with("```")).collect::<Vec<_>>().join("\n");
    }
    if t.chars().count() > 1 && t.starts_with('"') && t.ends_with('"') { t = t[1..t.len() - 1].to_string(); }
    t.trim().to_string()
}

/// Model ids from the CLI itself, for the ones that can list them. Blocking (up to 20 s).
pub fn available_models(h: &Harness, path: &str) -> Vec<String> {
    let fallback = || h.models.iter().map(|m| m.id.to_string()).collect();
    let Some(cmd) = h.list_models else { return fallback() };
    let out = run(path, &v(cmd), None, &std::env::temp_dir().to_string_lossy(), Duration::from_secs(20), None).unwrap_or_default();
    let ids: Vec<String> = clean(&out).lines().map(str::trim).filter(|l| !l.is_empty() && !l.contains(' ')).map(str::to_string).collect();
    if ids.is_empty() { fallback() } else { ids }
}

/// Runs a CLI to the end, killed after `timeout` or on cancel. Node-based CLIs need node on PATH; it usually sits
/// next to them.
pub fn run(exe: &str, args: &[String], stdin: Option<&str>, dir: &str, timeout: Duration, cancel: Option<&Cancel>) -> Result<String> {
    let name = std::path::Path::new(exe).file_name().map_or(exe.into(), |n| n.to_string_lossy().into_owned());
    let home = std::env::var("HOME").unwrap_or_default();
    let exe_dir = std::path::Path::new(exe).parent().map_or(String::new(), |p| p.to_string_lossy().into_owned());
    let path = format!("{exe_dir}:/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:{}", std::env::var("PATH").unwrap_or("/usr/bin:/bin".into()));
    let mut child = Command::new(exe)
        .args(args)
        .current_dir(dir)
        .env("PATH", path)
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::msg(format!("{name}: {e}")))?;
    if let (Some(s), Some(mut w)) = (stdin, child.stdin.take()) {
        let s = s.to_string();
        std::thread::spawn(move || {
            let _ = w.write_all(s.as_bytes());
        });
    }
    let reader = |mut r: Box<dyn Read + Send>| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            let _ = tx.send(b);
        });
        rx
    };
    let out = reader(Box::new(child.stdout.take().unwrap()));
    let err = reader(Box::new(child.stderr.take().unwrap()));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| Error::msg(e.to_string()))? { break s; }
        if cancel.is_some_and(|c| c.is_cancelled()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::msg(format!("{name} took longer than {} s and was stopped.", timeout.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // A child the CLI left behind can hold the pipes open: don't wait for it.
    let out = out.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let err = err.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    if status.success() { return Ok(String::from_utf8_lossy(&out).into_owned()); }
    let msg = clean(&String::from_utf8_lossy(if err.is_empty() { &out } else { &err }));
    if msg.is_empty() { return Err(Error::msg(format!("{name} exited with {}", status.code().unwrap_or(-1)))); }
    let chars: Vec<char> = msg.chars().collect();
    Err(Error::msg(chars[chars.len().saturating_sub(600)..].iter().collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_ai_output() {
        assert_eq!(clean("\u{1B}[1mFix the parser\u{1B}[0m\n"), "Fix the parser");
        assert_eq!(clean("```\nAdd login\n\nWith tests\n```"), "Add login\n\nWith tests");
        assert_eq!(clean("\"Bump version\""), "Bump version");
    }

    #[test]
    fn picks_the_selected_harness_and_its_model() {
        let dir = std::env::temp_dir().join(format!("loupe-ai-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Prefs::open_file(dir.join("p.plist"));
        let paths = HashMap::from([("codex".to_string(), "/x/codex".to_string()), ("claude".to_string(), "/x/claude".to_string())]);
        assert_eq!(selected(&p, &paths).unwrap().harness.id, "claude"); // auto: the first installed, in table order
        assert_eq!(selected(&p, &paths).unwrap().model, "claude-haiku-4-5-20251001");
        p.set_string("aiModel.claude", "opus");
        assert_eq!(selected(&p, &paths).unwrap().model, "claude-opus-5-5");
        p.set_string("aiHarness", "codex");
        assert_eq!(selected(&p, &paths).unwrap().harness.id, "codex");
        p.set_string("aiHarness", "gemini");
        assert!(selected(&p, &paths).is_none()); // not installed
        p.set_string("aiHarness", OFF);
        assert!(selected(&p, &paths).is_none());
        let codex = HARNESSES.iter().find(|h| h.id == "codex").unwrap();
        assert_eq!((codex.invocation)("p", "", "/tmp/o").0, ["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-o", "/tmp/o", "-"]);
        assert_eq!(CommitStyle::parse("detailed"), CommitStyle::Detailed);
        assert_eq!(CommitStyle::parse("??"), CommitStyle::Repository);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn runs_a_cli_with_stdin_and_times_out() {
        let tmp = std::env::temp_dir().to_string_lossy().into_owned();
        assert_eq!(run("/bin/cat", &[], Some("hello"), &tmp, Duration::from_secs(5), None).unwrap(), "hello");
        let e = run("/bin/sleep", &v(&["5"]), None, &tmp, Duration::from_millis(200), None).unwrap_err();
        assert!(e.to_string().contains("took longer than"));
        let c = Cancel::new();
        c.cancel();
        assert_eq!(run("/bin/sleep", &v(&["5"]), None, &tmp, Duration::from_secs(5), Some(&c)).unwrap_err(), Error::Cancelled);
        let e = run("/bin/sh", &v(&["-c", "echo nope >&2; exit 3"]), None, &tmp, Duration::from_secs(5), None).unwrap_err();
        assert_eq!(e.to_string(), "nope");
    }

    /// Runs the local `claude` CLI on a scratch repo: cargo test -p loupe-forge -- --ignored writes_a_message
    #[test]
    #[ignore]
    fn writes_a_message_for_the_staged_diff() {
        let dir = std::env::temp_dir().join(format!("loupe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        let id = ["-c", "user.name=t", "-c", "user.email=t@t"];
        std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        git::run(&["init", "-q"], d, &[], None).unwrap();
        git::run(&["add", "."], d, &[], None).unwrap();
        git::run(&[&id[..], &["commit", "-qm", "Add an empty main"]].concat(), d, &[], None).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {\n    println!(\"hello, world\");\n}\n").unwrap();
        git::run(&["add", "."], d, &[], None).unwrap();
        let mut p = Prefs::open_file(dir.join("p.plist"));
        p.set_string("aiHarness", &std::env::var("LOUPE_AI_HARNESS").unwrap_or("claude".into()));
        let paths = scan();
        let msg = message(selected(&p, &paths).as_ref(), &p, d, false, true, None).unwrap();
        println!("message: {msg}");
        assert!(!msg.is_empty() && !msg.contains("```"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
