//! "New Pull Request": defaults from the branch's commits, the forge's template, AI-written title and body, push
//! then create. And the Changes banner's "commits on this branch that aren't on the default branch".

use crate::models::PullRequest;
use crate::{ai, git, Cancel, Client, Error, RemoteRepo, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub body: String,
}

/// `git log` format [`parse_log`] reads.
pub const LOG_FORMAT: &str = "--format=%H%x1f%s%x1f%b%x1e";

pub fn parse_log(s: &str) -> Vec<Commit> {
    s.split('\u{1e}')
        .filter(|r| !r.is_empty())
        .filter_map(|rec| {
            let f: Vec<&str> = rec.splitn(3, '\u{1f}').collect();
            let sha = f[0].trim();
            if sha.is_empty() || f.len() < 2 { return None; }
            Some(Commit { sha: sha.into(), subject: f[1].into(), body: f.get(2).map_or("", |b| b.trim()).into() })
        })
        .collect()
}

/// One commit: its subject. More: the branch name read as words ("feat/add-login_page" → "Add login page").
pub fn title(commits: &[Commit], branch: &str) -> String {
    if commits.len() == 1 { commits[0].subject.clone() } else { humanize(branch) }
}

pub fn humanize(branch: &str) -> String {
    let last = branch.split('/').rfind(|s| !s.is_empty()).unwrap_or(branch);
    let words = last.replace(['-', '_'], " ").split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let mut c = words.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// The template the forge itself would use: its own folder first, then the others', then docs/ and the root.
pub fn template_paths(github: bool) -> Vec<String> {
    let dirs: &[&str] = if github { &[".github", ".forgejo", ".gitea"] } else { &[".forgejo", ".gitea", ".github"] };
    dirs.iter()
        .chain(&["docs", ""])
        .flat_map(|d| ["pull_request_template.md", "PULL_REQUEST_TEMPLATE.md"].map(|f| if d.is_empty() { f.to_string() } else { format!("{d}/{f}") }))
        .collect()
}

pub fn template(dir: &str, github: bool) -> Option<String> {
    template_paths(github).iter().find_map(|p| {
        let s = std::fs::read_to_string(std::path::Path::new(dir).join(p)).ok()?;
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    })
}

/// The template when there is one, else a lone commit's description.
pub fn body(commits: &[Commit], template: Option<&str>) -> String {
    match template {
        Some(t) => t.into(),
        None if commits.len() == 1 => commits[0].body.clone(),
        None => String::new(),
    }
}

pub(crate) fn truncated(diff: &str) -> String {
    const LIMIT: usize = 60_000;
    if diff.len() <= LIMIT { return diff.into(); }
    diff.chars().take(LIMIT).collect::<String>() + "\n[diff truncated]"
}

pub fn prompt(commits: &[Commit], base: &str, branch: &str, stat: &str, diff: &str, template: Option<&str>, extra: &str) -> String {
    let patch = truncated(diff);
    let log = commits
        .iter()
        .map(|c| {
            if c.body.is_empty() { c.subject.clone() } else { format!("{}\n{}", c.subject, c.body.split('\n').map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")) }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let extra = extra.trim();
    let tpl = template.map_or(String::new(), |t| format!("\nFill in this pull request template, keeping its headings and checkboxes:\n{t}\n"));
    let extra = if extra.is_empty() { String::new() } else { format!("\n{extra}") };
    format!(
        "You write pull request titles and descriptions. Below are the commits of branch {branch}, which is to be merged into {base}, then the change.

Write a title of at most 72 characters that says what the pull request does, in the style of the commit subjects. Then a description in Markdown for a reviewer: what changed and why, and anything they should look at closely. Be brief; don't list every file.{tpl}{extra}

Reply with only the title on the first line, a blank line, then the description. No quotes, no code fences around the reply, no preamble. Don't run tools or look at files; everything you need is here.

Commits:
{log}

Summary:
{stat}
Diff:
{patch}"
    )
}

/// First line: the title (a leading "# " or "Title:" dropped); the rest: the description.
pub fn split_reply(s: &str) -> (String, String) {
    let (first, rest) = s.split_once('\n').map_or((s, None), |(a, b)| (a, Some(b)));
    let mut t = first.trim_matches([' ', '\t']).to_string();
    for p in ["# ", "Title:", "title:"] {
        if let Some(x) = t.strip_prefix(p) { t = x.trim_matches([' ', '\t']).to_string(); }
    }
    if t.chars().count() > 1 && t.starts_with('"') && t.ends_with('"') { t = t[1..t.len() - 1].to_string(); }
    (t, rest.map_or(String::new(), |b| b.trim().to_string()))
}

/// The sheet's state. Commits, title and body follow the base until the user edits them.
#[derive(Debug, Clone, Default)]
pub struct PullCreate {
    pub branch: String,
    /// Remote branch names, newest first, the default branch first.
    pub bases: Vec<String>,
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
    /// The newest 100.
    pub commits: Vec<Commit>,
    /// All of them.
    pub total: usize,
    pub template: Option<String>,
    pub needs_push: bool,
    /// The server says one is open already: offer it.
    pub existing: Option<PullRequest>,
    auto_title: String,
    auto_body: String,
}

/// What [`PullCreate::branch_state`] found for a base.
#[derive(Debug, Clone)]
pub struct BaseState {
    pub base: String,
    pub commits: Vec<Commit>,
    pub total: usize,
    pub needs_push: bool,
}

impl PullCreate {
    /// Blocking: bases, the default one, the template and an already open PR. Err with the message to show when
    /// there's no account or HEAD is detached (the caller passes those it knows).
    pub fn load(client: &Client, repo: &RemoteRepo, dir: &str, remote_name: &str, branch: &str) -> Result<Self> {
        if branch.is_empty() { return Err(Error::msg("HEAD is detached; switch to a branch first.")); }
        let template = template(dir, client.github);
        let out = git::string(&["for-each-ref", "--format=%(refname:lstrip=3)", "--sort=-committerdate", &format!("refs/remotes/{remote_name}")], dir, client.cancel())
            .unwrap_or_default();
        let mut names: Vec<String> = out.lines().filter(|n| *n != "HEAD" && *n != branch && !n.is_empty()).map(str::to_string).collect();
        let def = match client.default_branch(repo) {
            Ok(d) => Some(d),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => git::integration_target(remote_name, dir).map(|(n, _)| n.strip_prefix(&format!("{remote_name}/")).map_or(n.clone(), str::to_string)),
        };
        if let Some(d) = &def
            && d != branch && !names.contains(d) { names.insert(0, d.clone()); }
        names.truncate(200);
        let base = def.filter(|d| names.contains(d)).or_else(|| names.first().cloned()).unwrap_or_default();
        let existing = client.open_pull(repo, branch).ok().flatten();
        Ok(Self { branch: branch.into(), bases: names, base, template, existing, ..Default::default() })
    }

    /// Blocking: the commits on HEAD that aren't on `remote/base`, capped at 100 (an unrelated base like gh-pages
    /// would otherwise list the whole history), and whether HEAD still needs pushing.
    pub fn branch_state(dir: &str, remote_name: &str, base: &str, branch: &str, cancel: Option<&Cancel>) -> BaseState {
        let range = format!("{remote_name}/{base}..HEAD");
        let (log, count) = std::thread::scope(|s| {
            let count = s.spawn(|| git::string(&["rev-list", "--count", &range], dir, cancel));
            (git::string(&["log", "-n", "100", LOG_FORMAT, &range], dir, cancel), count.join().unwrap())
        });
        let commits = parse_log(&log.unwrap_or_default());
        let n: usize = count.unwrap_or_default().trim().parse().unwrap_or(0);
        // Already on the remote (or behind it): nothing to push.
        let needs_push = git::run(&["merge-base", "--is-ancestor", "HEAD", &format!("{remote_name}/{branch}")], dir, &[], cancel).is_err();
        BaseState { base: base.into(), total: n.max(commits.len()), commits, needs_push }
    }

    /// Ignored when the base changed again meanwhile (that call fills in).
    pub fn apply_base(&mut self, s: BaseState) {
        if s.base != self.base { return; }
        self.commits = s.commits;
        self.total = s.total;
        let t = title(&self.commits, &self.branch);
        let b = body(&self.commits, self.template.as_deref());
        if self.title == self.auto_title { self.title = t.clone(); }
        if self.body == self.auto_body { self.body = b.clone(); }
        self.auto_title = t;
        self.auto_body = b;
        self.needs_push = s.needs_push;
    }

    pub fn can_create(&self) -> bool {
        !self.base.is_empty() && !self.commits.is_empty() && !self.title.trim().is_empty()
    }

    pub fn create_label(&self) -> &'static str {
        if self.needs_push { "Push and Create" } else { "Create Pull Request" }
    }

    /// Blocking: the title and description written by the selected AI tool. (title, body); an empty title keeps
    /// the current one.
    pub fn write_with_ai(&self, dir: &str, remote_name: &str, extra: &str, harness: &ai::Selected, cancel: Option<&Cancel>) -> Result<(String, String)> {
        let range = format!("{remote_name}/{}...HEAD", self.base);
        let args = ["diff", "--no-color", "--no-ext-diff", "-M", &range];
        let (diff, stat) = std::thread::scope(|s| {
            let stat = s.spawn(|| git::string(&[&args[..], &["--stat"]].concat(), dir, cancel));
            (git::string(&[&args[..], &["-U2"]].concat(), dir, cancel), stat.join().unwrap())
        });
        let p = prompt(&self.commits, &self.base, &self.branch, &stat?, &diff?, self.template.as_deref(), extra);
        Ok(split_reply(&ai::answer(harness, &p, dir, cancel)?))
    }

    /// Blocking: pushes the branch when the remote doesn't have its head yet (with the space's credentials), then
    /// opens the pull request. On [`Error::PullExists`] look the open one up with [`Client::open_pull`] and offer it.
    pub fn create(&mut self, client: &Client, repo: &RemoteRepo, dir: &str, remote_name: &str, git_auth: &[(String, String)]) -> Result<PullRequest> {
        if self.needs_push {
            git::run(&["push", "--set-upstream", remote_name, "HEAD"], dir, git_auth, None)?;
            self.needs_push = false;
        }
        client.create_pull(repo, &crate::client::new_pull(&self.branch, &self.base, &self.title, &self.body, self.draft, client.github))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(sha: &str, s: &str, b: &str) -> Commit {
        Commit { sha: sha.into(), subject: s.into(), body: b.into() }
    }

    #[test]
    fn pull_title_from_one_commit_or_the_branch() {
        let one = vec![c("a", "Fix the login race", "Details.")];
        assert_eq!(title(&one, "fix/login"), "Fix the login race");
        assert_eq!(body(&one, None), "Details.");
        assert_eq!(body(&one, Some("## Why")), "## Why");
        let two = [one.clone(), vec![c("b", "Test it", "")]].concat();
        assert_eq!(title(&two, "feat/add-login_page"), "Add login page");
        assert_eq!(body(&two, None), "");
        assert_eq!(humanize("JIRA-12--tidy"), "JIRA 12 tidy");
        assert_eq!(humanize("main"), "Main");
    }

    #[test]
    fn parses_commit_log() {
        let log = "aaa\u{1f}First\u{1f}\u{1e}\nbbb\u{1f}Second\u{1f}Line one\nLine two\n\u{1e}\n";
        let cs = parse_log(log);
        assert_eq!(cs.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(), ["aaa", "bbb"]);
        assert!(cs[0].body.is_empty() && cs[1].subject == "Second" && cs[1].body == "Line one\nLine two");
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn finds_the_pull_request_template_the_forge_would_use() {
        let dir = std::env::temp_dir().join(format!("loupe-tpl-{}", uuid::Uuid::new_v4()));
        let write = |p: &str, s: &str| {
            let u = dir.join(p);
            std::fs::create_dir_all(u.parent().unwrap()).unwrap();
            std::fs::write(u, s).unwrap();
        };
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        assert_eq!(template(d, true), None);
        write("docs/pull_request_template.md", "docs");
        assert_eq!(template(d, false).as_deref(), Some("docs"));
        write(".github/pull_request_template.md", "github\n");
        write(".gitea/PULL_REQUEST_TEMPLATE.md", "gitea");
        assert_eq!(template(d, true).as_deref(), Some("github"));
        assert_eq!(template(d, false).as_deref(), Some("gitea"));
        write(".forgejo/pull_request_template.md", "   "); // empty: skipped
        assert_eq!(template(d, false).as_deref(), Some("gitea"));
        assert_eq!(template_paths(false)[0], ".forgejo/pull_request_template.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_the_ai_reply() {
        assert_eq!(split_reply("Add login\n\n- does x\n- does y"), ("Add login".into(), "- does x\n- does y".into()));
        assert_eq!(split_reply("# \"Add login\""), ("Add login".into(), "".into()));
        assert_eq!(split_reply("Title: Add login\nBody"), ("Add login".into(), "Body".into()));
        let p = prompt(&[c("a", "S", "B")], "main", "feat/x", "1 file", &"x".repeat(70_000), Some("## Why"), "");
        assert!(p.contains("[diff truncated]") && p.contains("## Why") && p.contains("S\n    B"));
    }

    #[test]
    fn title_and_body_follow_the_base_until_edited() {
        let mut m = PullCreate { branch: "feat/x".into(), base: "main".into(), ..Default::default() };
        let st = |base: &str, cs: Vec<Commit>| BaseState { base: base.into(), total: cs.len(), commits: cs, needs_push: true };
        m.apply_base(st("main", vec![c("a", "One", "Why")]));
        assert_eq!((m.title.as_str(), m.body.as_str()), ("One", "Why"));
        m.title = "Mine".into();
        m.apply_base(st("main", vec![c("a", "One", ""), c("b", "Two", "")]));
        assert_eq!((m.title.as_str(), m.body.as_str()), ("Mine", ""));
        m.apply_base(st("dev", vec![])); // a stale answer for another base
        assert_eq!(m.commits.len(), 2);
        assert!(m.can_create() && m.create_label() == "Push and Create");
    }
}
