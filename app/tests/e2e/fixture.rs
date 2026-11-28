//! The repositories every e2e test starts from, built fresh with git under `<root>/home` (HOME for the app and for
//! git). Every commit has a fixed author and date, so the SHAs, the history and the screenshots are the same on
//! every run.
//!
//! - `~/app` (main, remote `origin` = the mock forge's acme/app): five commits, two uncommitted hunks in
//!   src/lib.rs and an untracked notes.txt.
//! - `~/app-feature`: linked worktree on `feature` (PR #7 on the mock forge), one commit ahead of main's third.
//! - `~/app-merge`: linked worktree on `merge-test`, stopped in `git merge topic` with src/conflict.rs conflicted.
use std::path::{Path, PathBuf};
use std::process::Command;

/// 2026-01-12 09:00 UTC, the first commit; one day per commit after it.
pub const START: i64 = 1_768_208_400;
/// The app's frozen "now": a week after the first commit.
pub const NOW: i64 = START + 7 * 86_400;

pub struct Fixture {
    pub home: PathBuf,
    pub app: String,
    pub feature: String,
    pub merge: String,
    /// The bare repository behind `origin` (the mock forge reads the PR's diff from the real refs).
    pub bare: PathBuf,
    pub feature_sha: String,
    pub main_sha: String,
    clock: std::cell::Cell<i64>,
}

pub fn lib_rs(extra_top: &str, extra_bottom: &str) -> String {
    let mut s = String::from("//! A tiny library the e2e fixture commits to.\n\n");
    s += extra_top;
    for i in 1..=12 { s += &format!("pub fn step_{i}(x: u32) -> u32 {{\n    x + {i}\n}}\n\n") }
    s += extra_bottom;
    s
}

pub const CONFLICT_BASE: &str = "pub const GREETING: &str = \"hello\";\npub const TARGET: &str = \"world\";\n";

impl Fixture {
    /// Wipes `root` and builds the repositories; `forge` is the mock forge's base URL (origin's URL).
    pub fn build(root: &Path, forge: &str) -> Fixture {
        let _ = std::fs::remove_dir_all(root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".gitconfig"), "[user]\n\tname = Alice Mock\n\temail = alice@example.com\n[init]\n\tdefaultBranch = main\n\
            [advice]\n\tdetachedHead = false\n[gc]\n\tauto = 0\n[maintenance]\n\tauto = false\n[core]\n\tfsmonitor = false\n").unwrap();
        let home = home.canonicalize().unwrap();
        let p = |n: &str| home.join(n).to_string_lossy().into_owned();
        let (app, feature, merge, bare) = (p("app"), p("app-feature"), p("app-merge"), root.canonicalize().unwrap().join("forge/app.git"));
        let mut f = Fixture { home: home.clone(), app: app.clone(), feature: feature.clone(), merge: merge.clone(), bare: bare.clone(),
            feature_sha: String::new(), main_sha: String::new(), clock: std::cell::Cell::new(START) };
        std::fs::create_dir_all(&app).unwrap();
        f.git(&app, &["init", "-q", "-b", "main"]);
        f.write(&app, "README.md", "# app\n\nThe e2e fixture.\n");
        f.write(&app, "src/lib.rs", &lib_rs("", ""));
        f.write(&app, "src/conflict.rs", CONFLICT_BASE);
        f.commit(&app, "Initial commit");
        f.write(&app, "src/parser.rs", "pub fn parse(s: &str) -> Vec<&str> {\n    s.split(' ').collect()\n}\n");
        f.commit(&app, "Add a parser");
        f.write(&app, "README.md", "# app\n\nThe e2e fixture.\n\n## Parser\n\n`parse` splits on spaces.\n");
        f.commit(&app, "Document the parser");
        // feature: from here, one commit (PR #7).
        f.git(&app, &["branch", "feature"]);
        f.git(&app, &["branch", "topic"]);
        f.write(&app, "src/parser.rs", "pub fn parse(s: &str) -> Vec<&str> {\n    s.split_whitespace().collect()\n}\n");
        f.commit(&app, "Split on any whitespace");
        f.write(&app, "src/conflict.rs", "pub const GREETING: &str = \"hello\";\npub const TARGET: &str = \"main\";\n");
        f.commit(&app, "Greet main");

        f.git(&app, &["worktree", "add", "-q", &feature, "feature"]);
        f.write(&feature, "src/colors.rs", "pub const RED: u32 = 0xff0000;\npub const GREEN: u32 = 0x00ff00;\npub const BLUE: u32 = 0x0000ff;\n");
        f.write(&feature, "src/lib.rs", &lib_rs("pub mod colors;\n\n", ""));
        f.commit(&feature, "Add colors");

        f.git(&app, &["checkout", "-q", "topic"]);
        f.write(&app, "src/conflict.rs", "pub const GREETING: &str = \"hello\";\npub const TARGET: &str = \"topic\";\n");
        f.commit(&app, "Greet topic");
        f.git(&app, &["checkout", "-q", "main"]);

        // origin: a bare copy for the remote-tracking refs, then the mock forge's URL.
        f.git(&app, &["clone", "-q", "--bare", &app, &bare.to_string_lossy()]);
        f.git(&app, &["remote", "add", "origin", &bare.to_string_lossy()]);
        f.git(&app, &["fetch", "-q", "origin"]);
        f.git(&app, &["branch", "-q", "-u", "origin/main", "main"]);
        f.git(&app, &["branch", "-q", "-u", "origin/feature", "feature"]);
        f.git(&app, &["remote", "set-url", "origin", &format!("{forge}/acme/app.git")]);

        f.git(&app, &["worktree", "add", "-q", "-b", "merge-test", &merge, "main"]);
        let out = f.git_status(&merge, &["merge", "--no-edit", "topic"]);
        assert!(!out, "the merge must stop on a conflict");

        // Uncommitted in ~/app: two hunks in src/lib.rs, an untracked file.
        f.write(&app, "src/lib.rs", &lib_rs("/// Added at the top.\npub const VERSION: &str = \"0.2\";\n\n", "/// Added at the bottom.\npub fn last() -> u32 { 99 }\n"));
        f.write(&app, "notes.txt", "remember the milk\n");
        f.main_sha = f.git(&app, &["rev-parse", "main"]);
        f.feature_sha = f.git(&app, &["rev-parse", "feature"]);
        f
    }

    pub fn write(&self, dir: &str, rel: &str, text: &str) {
        let p = Path::new(dir).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn commit(&self, dir: &str, msg: &str) {
        self.git(dir, &["add", "-A"]);
        self.git(dir, &["commit", "-q", "-m", msg]);
        self.clock.set(self.clock.get() + 86_400);
    }

    fn command(&self, dir: &str, args: &[&str]) -> Command {
        let date = format!("@{} +0000", self.clock.get());
        let mut c = Command::new("git");
        c.args(args).current_dir(dir).env("HOME", &self.home).env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", &date).env("GIT_COMMITTER_DATE", &date).env("GIT_EDITOR", "true");
        c
    }

    /// Runs git in `dir` and returns its trimmed stdout; panics when it fails.
    pub fn git(&self, dir: &str, args: &[&str]) -> String {
        let o = self.command(dir, args).output().unwrap();
        assert!(o.status.success(), "git {args:?} in {dir}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn git_status(&self, dir: &str, args: &[&str]) -> bool {
        self.command(dir, args).output().unwrap().status.success()
    }
}
