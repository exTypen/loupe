//! Interactive rebase: the plan, its todo, running it, and what a remote already has.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::git::{self, Error, Result};
use crate::undo::{OPERATION_FILES, Recipe, Reset, UndoStack};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RebaseAction { Pick, Reword, Edit, Squash, Fixup, Drop }

impl RebaseAction {
    pub const ALL: [RebaseAction; 6] = [Self::Pick, Self::Reword, Self::Edit, Self::Squash, Self::Fixup, Self::Drop];
    pub fn name(self) -> &'static str {
        match self { Self::Pick => "pick", Self::Reword => "reword", Self::Edit => "edit", Self::Squash => "squash", Self::Fixup => "fixup", Self::Drop => "drop" }
    }
    pub fn title(self) -> &'static str {
        match self { Self::Pick => "Pick", Self::Reword => "Reword", Self::Edit => "Edit", Self::Squash => "Squash", Self::Fixup => "Fixup", Self::Drop => "Drop" }
    }
}

/// One commit of an interactive rebase and what to do with it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RebaseItem {
    pub sha: String,
    pub parents: Vec<String>,
    pub message: String, // the commit's full message
    pub action: RebaseAction,
    pub new_message: Option<String>, // reword / squash: this commit's part of the result message, when edited
}

impl RebaseItem {
    pub fn new(sha: impl Into<String>, parents: Vec<String>, message: impl Into<String>) -> Self {
        RebaseItem { sha: sha.into(), parents, message: message.into(), action: RebaseAction::Pick, new_message: None }
    }
    pub fn subject(&self) -> &str { self.message.split('\n').next().unwrap_or("") }
    pub fn short(&self) -> &str { &self.sha[..self.sha.len().min(8)] }
    pub fn effective_message(&self) -> &str { self.new_message.as_deref().unwrap_or(&self.message) }
}

/// The commits `base..HEAD`, oldest first. `base` None means from the root commit.
#[derive(Clone, Debug, PartialEq)]
pub struct RebaseRange {
    pub base: Option<String>,
    pub items: Vec<RebaseItem>,
    pub pushed: HashSet<String>, // commits some remote branch already has
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub todo: String,
    pub messages: HashMap<String, String>, // file name in the message directory → the message it holds
}

pub fn shell_quoted(s: &str) -> String { format!("'{}'", s.replace('\'', "'\\''")) }

/// The todo for `git rebase -i`, oldest first. Git only ever sees pick/edit/fixup/drop/exec: a reword or squash
/// becomes a fixup followed by `exec git commit --amend -F <prepared message>`, so no editor ever opens.
/// Squash/fixup meld into the closest older commit that isn't dropped, as in git.
pub fn build(items: &[RebaseItem], message_dir: &str) -> Result<Built> {
    let (mut lines, mut messages) = (Vec::<String>::new(), HashMap::new());
    let mut group: Option<(String, Vec<String>)> = None; // (head message, parts)
    let flush = |group: &mut Option<(String, Vec<String>)>, lines: &mut Vec<String>, messages: &mut HashMap<String, String>| {
        let Some((head, parts)) = group.take() else { return };
        let message = parts.join("\n\n");
        if message == head { return }
        let name = format!("message-{}", messages.len() + 1);
        lines.push(format!("exec git commit --amend --allow-empty --quiet -F {}", shell_quoted(&format!("{message_dir}/{name}"))));
        messages.insert(name, message);
    };
    for item in items {
        match item.action {
            RebaseAction::Drop => lines.push(format!("drop {}", item.sha)),
            RebaseAction::Pick | RebaseAction::Reword | RebaseAction::Edit => {
                flush(&mut group, &mut lines, &mut messages);
                lines.push(format!("{} {}", if item.action == RebaseAction::Edit { "edit" } else { "pick" }, item.sha));
                let part = if item.action == RebaseAction::Reword { item.effective_message() } else { &item.message };
                group = Some((item.message.clone(), vec![part.to_string()]));
            }
            RebaseAction::Squash | RebaseAction::Fixup => {
                let Some(g) = group.as_mut() else {
                    return Err(Error::msg(format!("{} “{}” has no older commit left to {} into.", item.short(), item.subject(), item.action.name())));
                };
                lines.push(format!("fixup {}", item.sha));
                if item.action == RebaseAction::Squash { g.1.push(item.effective_message().to_string()) }
            }
        }
    }
    flush(&mut group, &mut lines, &mut messages);
    Ok(Built { todo: lines.join("\n") + "\n", messages })
}

/// Index of the first item the plan changes (order or action), None when the plan does nothing. Commits before
/// it keep their SHAs: git fast-forwards over unchanged picks.
pub fn first_change(original: &[String], plan: &[RebaseItem]) -> Option<usize> {
    let mut first = (plan.len() < original.len()).then_some(plan.len());
    for (i, item) in plan.iter().enumerate() {
        if i >= original.len() || original[i] != item.sha
            || (item.action == RebaseAction::Reword && item.effective_message() != item.message)
            || (item.action != RebaseAction::Pick && item.action != RebaseAction::Reword) {
            first = Some(i);
            break;
        }
    }
    // A squash or fixup also rewrites the commit it melds into, which may sit before the first change.
    let mut head = None;
    for (i, item) in plan.iter().enumerate() {
        match item.action {
            RebaseAction::Pick | RebaseAction::Reword | RebaseAction::Edit => head = Some(i),
            RebaseAction::Squash | RebaseAction::Fixup => {
                if let Some(h) = head && h < first.unwrap_or(usize::MAX) { first = Some(h) }
            }
            RebaseAction::Drop => {}
        }
    }
    first
}

/// The commits the plan rewrites that a remote already has.
pub fn rewritten_pushed(range: &RebaseRange, plan: &[RebaseItem]) -> Vec<RebaseItem> {
    let original: Vec<String> = range.items.iter().map(|i| i.sha.clone()).collect();
    let Some(first) = first_change(&original, plan) else { return vec![] };
    plan[first..].iter().filter(|i| range.pushed.contains(&i.sha)).cloned().collect()
}

/// As seen in History: newest on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone { Above, Below, Onto }

/// Drag-and-drop in History: `sha` dropped above / below / onto `target`. Onto squashes (or fixes up) into it.
pub fn move_item(items: &[RebaseItem], sha: &str, zone: Zone, target: &str, fixup: bool) -> Vec<RebaseItem> {
    let Some(mut moved) = items.iter().find(|i| i.sha == sha).cloned().filter(|_| sha != target) else { return items.to_vec() };
    let mut out: Vec<RebaseItem> = items.iter().filter(|i| i.sha != sha).cloned().collect();
    let Some(t) = out.iter().position(|i| i.sha == target) else { return items.to_vec() };
    // Oldest first here, so "above" (newer) is after the target.
    if zone == Zone::Onto { moved.action = if fixup { RebaseAction::Fixup } else { RebaseAction::Squash } }
    out.insert(if zone == Zone::Below { t } else { t + 1 }, moved);
    out
}

/// The plan with `sha`'s action (and, for a reword, its new message) set.
pub fn with_action(items: &[RebaseItem], sha: &str, action: RebaseAction, new_message: Option<String>) -> Vec<RebaseItem> {
    items.iter().cloned().map(|mut i| {
        if i.sha == sha {
            i.action = action;
            if new_message.is_some() { i.new_message = new_message.clone() }
        }
        i
    }).collect()
}

pub fn parse_log(s: &str) -> Vec<RebaseItem> {
    s.split('\u{1e}').filter_map(|rec| {
        let f: Vec<&str> = rec.trim_start_matches('\n').splitn(3, '\u{1f}').collect();
        if f.len() != 3 || f[0].is_empty() { return None }
        Some(RebaseItem::new(f[0], f[1].split(' ').filter(|p| !p.is_empty()).map(String::from).collect(), f[2].trim()))
    }).collect()
}

// MARK: - Running it

pub const LIMIT: usize = 1000;

/// `oldest^..HEAD` (or all of HEAD from the root), refusing merges: a hand-built todo can't keep them.
pub fn rebase_range(oldest: &str, dir: &Path) -> Result<RebaseRange> {
    let base = git::trimmed(dir, ["rev-parse", "--verify", "-q", &format!("{oldest}^")]).ok().filter(|s| !s.is_empty());
    let rev = base.as_ref().map(|b| format!("{b}..HEAD")).unwrap_or_else(|| "HEAD".into());
    let n = (LIMIT + 1).to_string();
    let (log, local) = std::thread::scope(|s| {
        let log = s.spawn(|| git::string(dir, ["log", "--no-show-signature", "--reverse", "-n", &n, "--format=%H%x1f%P%x1f%B%x1e", &rev, "--"]));
        let local = git::string(dir, ["rev-list", "-n", &n, &rev, "--not", "--remotes", "--"]);
        (log.join().unwrap(), local)
    });
    let items = parse_log(&log?);
    if items.len() > LIMIT { return Err(Error::msg(format!("More than {LIMIT} commits to rewrite. Use a terminal for a rebase this big."))) }
    if !items.iter().any(|i| i.sha == oldest) {
        return Err(Error::msg(format!("{} isn't on the current branch.", &oldest[..oldest.len().min(8)])));
    }
    if let Some(m) = items.iter().rev().find(|i| i.parents.len() > 1) {
        return Err(Error::msg(format!("The commits to rewrite include the merge {} “{}”. Rewriting it would flatten the merge, so Loupe leaves this to a terminal (git rebase -i --rebase-merges).", m.short(), m.subject())));
    }
    let not_pushed: HashSet<String> = local?.lines().map(String::from).collect();
    let pushed = items.iter().map(|i| i.sha.clone()).filter(|s| !not_pushed.contains(s)).collect();
    Ok(RebaseRange { base, items, pushed })
}

fn temp_dir() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("loupe-rebase-{}-{nanos}", std::process::id()))
}

/// Runs the plan with `git rebase -i --autostash`. The prepared todo is copied in by GIT_SEQUENCE_EDITOR and
/// GIT_EDITOR is `true`, so no editor opens. Ok(true) when the rebase stopped (an edit, a conflict, a failed
/// hook) and waits in the git dir for Continue or Abort.
pub fn run_rebase(range: &RebaseRange, plan: &[RebaseItem], dir: &Path) -> Result<bool> {
    let git_dir = PathBuf::from(git::trimmed(dir, ["rev-parse", "--path-format=absolute", "--git-dir"])?);
    let has = |n: &str| git_dir.join(n).exists();
    // Otherwise git's "already a rebase in progress" would look like our own stop below.
    if let Some(busy) = OPERATION_FILES.iter().find(|f| has(f)) {
        return Err(Error::msg(if busy.starts_with("rebase") { "A rebase is already in progress. Continue or abort it first.".to_string() } else {
            format!("A {} is in progress. Finish or abort it first.", match *busy { "MERGE_HEAD" => "merge", "REVERT_HEAD" => "revert", _ => "cherry-pick" })
        }));
    }
    let tmp = temp_dir();
    std::fs::create_dir_all(&tmp)?;
    let built = build(plan, &tmp.to_string_lossy())?;
    let todo = tmp.join("todo");
    std::fs::write(&todo, &built.todo)?;
    for (name, m) in &built.messages { std::fs::write(tmp.join(name), m)? }
    let env = [("GIT_SEQUENCE_EDITOR", format!("cp {}", shell_quoted(&todo.to_string_lossy()))), ("GIT_EDITOR", "true".into())];
    let failure = git::git(dir, ["rebase", "-i", "--autostash", "--no-autosquash", range.base.as_deref().unwrap_or("--root")]).env(env).run().err();
    let stopped = has("rebase-merge");
    // The exec lines still to run read their messages from here; keep it until the rebase is over.
    if !stopped { let _ = std::fs::remove_dir_all(&tmp); }
    match failure {
        Some(f) if !stopped => Err(f),
        _ => Ok(stopped),
    }
}

/// HEAD's first-parent commits back to the first merge (a rebase can't cross it) and short of the root: these can
/// be dragged. Pushed ones too: a rewrite asks before rewriting what a remote already has.
pub fn movable_commits(dir: &Path) -> HashSet<String> {
    let out = git::string(dir, ["rev-list", "--first-parent", "--parents", "-n", &LIMIT.to_string(), "HEAD", "--"]).unwrap_or_default();
    let mut set = HashSet::new();
    for line in out.lines() {
        let ids: Vec<&str> = line.split(' ').collect(); // the commit, then its parents
        if ids.len() != 2 { break } // a merge (or the root) ends what a rebase can reach
        set.insert(ids[0].to_string());
    }
    set
}

/// What submitting a plan needs.
#[derive(Clone, Debug, PartialEq)]
pub enum Submit {
    /// The plan changes nothing.
    Nothing,
    /// Ask first: it rewrites what a remote has, or drops a commit.
    Confirm { pushed: Vec<RebaseItem>, dropping: Vec<RebaseItem> },
    Run,
}

/// Whether the plan can run as it is. Err: it can't be built (a squash with nothing older to meld into).
pub fn submit_check(range: &RebaseRange, plan: &[RebaseItem], confirmed: bool) -> Result<Submit> {
    let original: Vec<String> = range.items.iter().map(|i| i.sha.clone()).collect();
    if first_change(&original, plan).is_none() { return Ok(Submit::Nothing) }
    build(plan, "/")?;
    let pushed = rewritten_pushed(range, plan);
    let dropping: Vec<RebaseItem> = plan.iter().filter(|i| i.action == RebaseAction::Drop).cloned().collect();
    if !confirmed && (!pushed.is_empty() || !dropping.is_empty()) { return Ok(Submit::Confirm { pushed, dropping }) }
    Ok(Submit::Run)
}

/// Runs a confirmed plan as `label`: recorded for Undo when it finishes here (`reset --keep` to HEAD from before);
/// one that stops (Ok(true)) waits in Changes, and Continue there records it from the rebase's orig-head.
pub fn execute(label: &str, range: &RebaseRange, plan: &[RebaseItem], dir: &Path) -> Result<bool> {
    let head = git::trimmed(dir, ["rev-parse", "HEAD"])?;
    record_rewrite(label, &head, dir);
    let mut stopped = false;
    UndoStack::for_path(dir).run(Some(Recipe::Reset(Reset { from: Some(head), ..Reset::new(label) })), || {
        stopped = run_rebase(range, plan, dir)?;
        Ok(())
    })?;
    Ok(stopped)
}

/// The last history rewrite Loupe started per worktree: `git reset --keep <sha>` puts the branch back where it was.
static REWRITES: LazyLock<Mutex<HashMap<PathBuf, (String, String)>>> = LazyLock::new(Default::default);

pub fn record_rewrite(label: &str, sha: &str, worktree: &Path) {
    REWRITES.lock().unwrap().insert(worktree.to_path_buf(), (label.into(), sha.into()));
}
pub fn last_rewrite(worktree: &Path) -> Option<(String, String)> { REWRITES.lock().unwrap().get(worktree).cloned() }
pub fn forget_rewrite(worktree: &Path) { REWRITES.lock().unwrap().remove(worktree); }

/// A commit's whole message, for the Reword sheet.
pub fn commit_message(dir: &Path, sha: &str) -> Result<String> { git::trimmed(dir, ["show", "-s", "--format=%B", sha]) }

#[cfg(test)]
mod tests {
    use super::*;
    use RebaseAction::*;

    fn item(sha: &str, message: &str, action: RebaseAction, new: Option<&str>) -> RebaseItem {
        RebaseItem { sha: sha.into(), parents: vec!["p".into()], message: message.into(), action, new_message: new.map(String::from) }
    }
    fn pick(sha: &str, m: &str) -> RebaseItem { item(sha, m, Pick, None) }
    fn shas(items: &[RebaseItem]) -> Vec<&str> { items.iter().map(|i| i.sha.as_str()).collect() }

    #[test]
    fn todo_keeps_order_and_plain_actions() {
        let b = build(&[pick("a", "A"), item("b", "B", Edit, None), item("c", "C", Drop, None), pick("d", "D")], "/m").unwrap();
        assert_eq!(b.todo, "pick a\nedit b\ndrop c\npick d\n");
        assert!(b.messages.is_empty());
    }

    #[test]
    fn todo_turns_reword_and_squash_into_amend_execs() {
        let b = build(&[
            item("a", "A", Reword, Some("A2")),
            pick("b", "B"), item("c", "C", Squash, None), item("d", "D", Fixup, None), item("e", "E", Squash, Some("E2")),
            pick("f", "F"), item("g", "G", Fixup, None), // fixup alone keeps F's message: no exec
            item("h", "H", Reword, None),               // reword without a change: no exec
        ], "/tmp/it's").unwrap();
        assert_eq!(b.todo, "pick a\nexec git commit --amend --allow-empty --quiet -F '/tmp/it'\\''s/message-1'\npick b\nfixup c\nfixup d\nfixup e\nexec git commit --amend --allow-empty --quiet -F '/tmp/it'\\''s/message-2'\npick f\nfixup g\npick h\n");
        assert_eq!(b.messages, HashMap::from([("message-1".into(), "A2".into()), ("message-2".into(), "B\n\nC\n\nE2".into())]));
    }

    #[test]
    fn squash_skips_dropped_commits_and_needs_an_older_one() {
        let b = build(&[pick("a", "A"), item("b", "B", Drop, None), item("c", "C", Fixup, None)], "/m").unwrap();
        assert_eq!(b.todo, "pick a\ndrop b\nfixup c\n");
        assert!(build(&[item("a", "A", Drop, None), item("b", "B", Squash, None)], "/m").is_err());
    }

    #[test]
    fn drag_moves_and_melds() {
        let items: Vec<_> = ["a", "b", "c", "d"].iter().map(|s| pick(s, &s.to_uppercase())).collect(); // oldest first
        assert_eq!(shas(&move_item(&items, "a", Zone::Above, "c", false)), ["b", "c", "a", "d"]);
        assert_eq!(shas(&move_item(&items, "d", Zone::Below, "b", false)), ["a", "d", "b", "c"]);
        let melded = move_item(&items, "d", Zone::Onto, "a", true);
        assert_eq!(shas(&melded), ["a", "d", "b", "c"]);
        assert_eq!(melded[1].action, Fixup);
        assert_eq!(move_item(&items, "a", Zone::Onto, "c", false)[2].action, Squash);
    }

    #[test]
    fn first_change_and_pushed_commits() {
        let items: Vec<_> = ["a", "b", "c"].iter().map(|s| pick(s, &s.to_uppercase())).collect();
        let range = RebaseRange { base: Some("x".into()), items: items.clone(), pushed: HashSet::from(["a".into(), "b".into()]) };
        let orig: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(first_change(&orig, &items), None);
        let mut plan = items.clone();
        plan[2].action = Drop;
        assert_eq!(first_change(&orig, &plan), Some(2));
        assert!(rewritten_pushed(&range, &plan).is_empty()); // a and b keep their SHAs
        plan = items.clone();
        plan[1].action = Reword;
        plan[1].new_message = Some("B2".into());
        assert_eq!(shas(&rewritten_pushed(&range, &plan)), ["b"]);
    }

    #[test]
    fn melding_into_a_pushed_commit_counts_as_rewriting_it() {
        let items = vec![pick("p", "P"), pick("c", "C")];
        let range = RebaseRange { base: Some("x".into()), items: items.clone(), pushed: HashSet::from(["p".into()]) };
        let mut plan = items.clone();
        plan[1].action = Fixup;
        assert_eq!(first_change(&["p".into(), "c".into()], &plan), Some(0));
        assert_eq!(shas(&rewritten_pushed(&range, &plan)), ["p"]);
        // Past a dropped commit too: c melds into a.
        let mut plan: Vec<_> = ["a", "b", "c"].iter().map(|s| pick(s, &s.to_uppercase())).collect();
        plan[1].action = Drop;
        plan[2].action = Squash;
        assert_eq!(first_change(&["a".into(), "b".into(), "c".into()], &plan), Some(0));
    }

    #[test]
    fn parses_log_with_multiline_messages() {
        let items = parse_log("aaa\u{1f}p1\u{1f}Subject\n\nBody line\n\u{1e}\nbbb\u{1f}p1 p2\u{1f}Merge x\n\u{1e}\n");
        assert_eq!(shas(&items), ["aaa", "bbb"]);
        assert_eq!(items[0].message, "Subject\n\nBody line");
        assert_eq!(items[0].subject(), "Subject");
        assert_eq!(items[1].parents, ["p1", "p2"]);
    }

    // MARK: - End to end on a temp repo

    struct Repo(tempfile::TempDir);
    impl Repo {
        fn new() -> Self {
            let r = Repo(tempfile::tempdir().unwrap());
            r.git(&["init", "-q", "-b", "main"]);
            for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null")] {
                r.git(&["config", k, v]);
            }
            r
        }
        fn dir(&self) -> &Path { self.0.path() }
        fn git(&self, args: &[&str]) -> String { git::string(self.dir(), args).unwrap() }
        fn commit(&self, name: &str, file: Option<&str>) {
            std::fs::write(self.dir().join(file.unwrap_or(name)), format!("{name}\n")).unwrap();
            self.git(&["add", "."]);
            self.git(&["commit", "-qm", name]);
        }
        fn subjects(&self) -> Vec<String> { self.git(&["log", "--format=%s"]).lines().map(String::from).collect() }
        fn sha(&self, rev: &str) -> String { self.git(&["rev-parse", rev]).trim().to_string() }
        fn plan(&self, from: &str, change: impl Fn(&[RebaseItem]) -> Vec<RebaseItem>) -> Result<bool> {
            let range = rebase_range(&self.sha(from), self.dir())?;
            run_rebase(&range, &change(&range.items), self.dir())
        }
    }

    #[test]
    fn rebase_end_to_end() {
        let r = Repo::new();
        for n in ["a", "b", "c", "d"] { r.commit(n, None) }

        // Reorder: d below b.
        assert!(!r.plan("HEAD~2", |i| move_item(i, &i[2].sha, Zone::Below, &i[0].sha, false)).unwrap());
        assert_eq!(r.subjects(), ["c", "b", "d", "a"]);

        // Squash c into b, with b's and c's messages; fixup d into a, keeping a's.
        assert!(!r.plan("HEAD~3", |items| {
            let p = move_item(items, &items[3].sha, Zone::Onto, &items[2].sha, false); // c onto b
            move_item(&p, &p[1].sha.clone(), Zone::Onto, &p[0].sha.clone(), true)        // d onto a
        }).unwrap());
        assert_eq!(r.subjects(), ["b", "a"]);
        assert_eq!(r.git(&["log", "-1", "--format=%B"]).trim(), "b\n\nc");
        let mut names: Vec<_> = r.git(&["show", "--name-only", "--format=", "HEAD~1"]).lines().map(String::from).collect();
        names.sort();
        assert_eq!(names, ["a", "d"]);

        // Reword the root commit, with a quote and a second paragraph; the worktree is dirty, so --autostash carries it.
        std::fs::write(r.dir().join("a"), "dirty\n").unwrap();
        assert!(!r.plan("HEAD~1", |i| with_action(i, &i[0].sha, Reword, Some("a's new name\n\nwith a body".into()))).unwrap());
        assert_eq!(r.subjects(), ["b", "a's new name"]);
        assert_eq!(r.git(&["log", "-1", "--format=%B", "HEAD~1"]).trim(), "a's new name\n\nwith a body");
        assert_eq!(std::fs::read_to_string(r.dir().join("a")).unwrap(), "dirty\n");
        r.git(&["checkout", "--", "a"]);

        // Drop.
        r.commit("e", None);
        assert!(!r.plan("HEAD~1", |i| with_action(i, &i[0].sha, Drop, None)).unwrap());
        assert_eq!(r.subjects(), ["e", "a's new name"]);

        // Edit stops with the rebase waiting; Continue finishes it and runs the exec lines still queued.
        r.commit("f", None);
        let git_dir = r.dir().join(".git");
        assert!(r.plan("HEAD~1", |i| {
            let p = with_action(i, &i[0].sha, Edit, None);
            with_action(&p, &i[1].sha, Reword, Some("f2".into()))
        }).unwrap());
        assert!(git_dir.join("rebase-merge/amend").exists());
        git::git(r.dir(), ["-c", "core.editor=true", "rebase", "--continue"]).env([("GIT_EDITOR", "true")]).run().unwrap();
        assert!(!git_dir.join("rebase-merge").exists());
        assert_eq!(r.subjects(), ["f2", "e", "a's new name"]);

        // A conflict stops too, and Abort puts everything back.
        r.commit("g1", Some("same"));
        r.commit("g2", Some("same"));
        let before = r.sha("HEAD");
        assert!(r.plan("HEAD~1", |i| move_item(i, &i[1].sha, Zone::Below, &i[0].sha, false)).unwrap());
        assert!(!git_dir.join("rebase-merge/amend").exists());
        r.git(&["rebase", "--abort"]);
        assert_eq!(r.sha("HEAD"), before);
    }

    #[test]
    fn rebase_range_refuses_merges_and_marks_pushed() {
        let r = Repo::new();
        r.commit("a", None);
        r.commit("b", None);
        r.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]); // "pushed" up to b
        r.commit("c", None);
        let range = rebase_range(&r.sha("HEAD~1"), r.dir()).unwrap();
        assert_eq!(range.items.iter().map(|i| i.subject()).collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(range.pushed, HashSet::from([r.sha("HEAD~1")]));
        assert_eq!(range.base, Some(r.sha("HEAD~2")));
        let range = rebase_range(&r.sha("HEAD~2"), r.dir()).unwrap();
        assert!(range.base.is_none() && range.items.len() == 3); // from the root

        r.git(&["switch", "-qc", "side", "HEAD~1"]);
        r.commit("s", None);
        r.git(&["switch", "-q", "main"]);
        r.git(&["merge", "-q", "--no-ff", "--no-edit", "side"]);
        r.commit("d", None);
        assert!(rebase_range(&r.sha("HEAD~3"), r.dir()).is_err());
    }

    #[test]
    fn run_rebase_refuses_while_another_rebase_waits() {
        let r = Repo::new();
        for n in ["a", "b", "c"] { r.commit(n, None) }
        assert!(r.plan("HEAD~1", |i| with_action(i, &i[0].sha, Edit, None)).unwrap());
        let stopped_at = r.sha("HEAD");
        assert!(r.plan("HEAD", |i| with_action(i, &i[0].sha, Drop, None)).is_err());
        assert_eq!(r.sha("HEAD"), stopped_at);
        r.git(&["rebase", "--abort"]);
    }
}
