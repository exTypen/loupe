//! End-to-end tests of the whole app shell, headless: the real Shell, views and models in a `HeadlessAppContext`
//! with GPUI's Metal offscreen renderer, against fixture repositories ([`fixture`]) and an in-process mock Forgejo
//! ([`forge`]). No window opens, no event reaches the desktop, and nothing reads or writes the real settings or the
//! Keychain: settings live in a plist file in the fixture folder (`settings::use_file`), the account is the
//! scenario's in-memory one, and HOME is the fixture's.
//!
//!     cargo test -p loupe --test e2e                 # all
//!     cargo test -p loupe --test e2e -- history      # names containing "history"
//!     UPDATE_BASELINE=1 cargo test -p loupe --test e2e   # rewrite app/tests/baselines/*.png
mod fixture;
mod forge;
mod shot;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, Window, App, px, size};
use loupe_app::shell::Shell;
use loupe_app::state::{AppModel, Tab, WorktreeModel, scenario::Scenario};

use fixture::Fixture;
use forge::Forge;

/// The window every test draws in, in points (the shots are at the test display's scale).
const WINDOW: (f32, f32) = (1180., 740.);

fn root() -> PathBuf { PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e") }

pub struct T {
    pub cx: HeadlessAppContext,
    pub fx: Fixture,
    pub app: Entity<AppModel>,
    pub shell: Entity<Shell>,
    pub window: AnyWindowHandle,
    pub forge: &'static Forge,
    shots: Vec<String>,
}

impl T {
    /// A fresh fixture and a fresh app on it, settled with `~/app` selected on Changes.
    fn new(forge: &'static Forge) -> T {
        let fx = Fixture::build(&root(), &forge.url);
        let base = fx.git(&fx.app, &["merge-base", "main", "feature"]);
        let diff = fx.git(&fx.app, &["diff", "--no-color", &format!("{base}...feature")]) + "\n";
        forge.reset(&fx.feature_sha, &base, diff);
        let s = Scenario {
            forge: forge.url.clone(), token: forge::TOKEN.into(), user: forge::USER.into(),
            repos: vec![fx.app.clone()], big: Some(fx.app.clone()), ..Default::default()
        };
        loupe_app::state::scenario::prepare(&s);
        {
            let mut p = loupe_forge::settings::shared();
            p.set_string("aiHarness", "off");
            for k in ["collapsed", "recent", "currentSpace", "commitOverview", "filePanel", "markdownPreview", "pullsMine", "diffLayout"] { p.set(k, None) }
        }
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(gpui_kit::assets::AllAssets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_kit::init(cx);
            loupe_app::ui::install_theme(cx);
            loupe_app::shell::keys::load_shortcuts(cx);
        });
        let app = cx.update(|cx| cx.new(|cx| AppModel::new(Some(s), cx)));
        cx.update(|cx| {
            loupe_app::state::adapt::apply_code_theme();
            loupe_app::shell::keys::install(cx);
        });
        let (window, shell) = cx.update(|cx| {
            let options = gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds { origin: Default::default(), size: size(px(WINDOW.0), px(WINDOW.1)) })),
                focus: true, show: false, ..Default::default()
            };
            let app = app.clone();
            gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(app, window, cx))).unwrap()
        });
        let mut t = T { cx, fx, app, shell, window, forge, shots: vec![] };
        let app_path = t.fx.app.clone();
        t.wait("~/app's status", |t| t.model(&app_path).is_some_and(|m| t.cx.read_entity(&m, |m, _| !m.state.files().is_empty())));
        t
    }

    /// Runs everything ready, advancing the fake clock, until `cond` holds (10 s of real time at most).
    pub fn wait(&mut self, what: &str, mut cond: impl FnMut(&mut T) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.cx.run_until_parked();
            self.frame();
            if cond(self) { return }
            assert!(Instant::now() < end, "timed out waiting for {what}");
            self.cx.advance_clock(Duration::from_millis(100));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Lets queued work and timers finish (the watcher's debounce, toasts' timers stay).
    pub fn settle(&mut self) {
        for _ in 0..5 { self.cx.run_until_parked(); self.cx.advance_clock(Duration::from_millis(50)); }
        self.frame();
    }

    pub fn frame(&mut self) { self.win(|w, cx| w.render_frame(cx)) }

    pub fn win<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        self.cx.update_window(self.window, |_, w, cx| f(w, cx)).unwrap()
    }

    pub fn press(&mut self, key: &str) { self.win(|w, cx| w.press(key, cx)); self.settle() }
    pub fn click(&mut self, id: impl Into<gpui_kit::ElementId>) { let id = id.into(); self.win(|w, cx| w.click(id, cx)); self.settle() }

    pub fn model(&self, path: &str) -> Option<Entity<WorktreeModel>> { self.cx.read_entity(&self.app, |a, _| a.model(path)) }
    pub fn read<R>(&self, f: impl FnOnce(&AppModel, &App) -> R) -> R { self.cx.read_entity(&self.app, f) }
    pub fn tab(&self) -> Tab { self.read(|a, _| a.tab) }

    /// Compares the window with baseline `name`; mismatches are collected and fail the test at its end.
    pub fn shot(&mut self, name: &str) {
        self.settle();
        let img = self.cx.capture_screenshot(self.window).expect("Metal offscreen rendering");
        if let Err(e) = shot::check(name, &img) { self.shots.push(format!("{name}: {e}")) }
    }
}

type Test = fn(&mut T);

fn main() {
    // Before any thread starts: the app and git see the fixture's HOME, UTC, no system git config.
    let home = root().join("home");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("TZ", "UTC");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        for k in ["LOUPE_STRESS", "LOUPE_FLOWS_AI", "LOUPE_FLOWS_MERGE", "LOUPE_FLOWS_MERGE_DO", "LOUPE_FLOWS_CREATE_PULL", "GIT_DIR", "GIT_WORK_TREE"] { std::env::remove_var(k) }
    }
    std::fs::create_dir_all(root()).unwrap();
    assert!(loupe_forge::settings::use_file(root().join("settings.plist")), "settings were read before the test picked its file");
    loupe_app::views::pulls::time::freeze(fixture::NOW);
    let forge: &'static Forge = Box::leak(Box::new(Forge::start()));

    let filter: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    let tests: &[(&str, Test)] = &[
        ("changes_list_and_diff", changes_list_and_diff),
        ("stage_unstage_hunk_and_commit", stage_unstage_hunk_and_commit),
        ("history_list_and_commit_diff", history_list_and_commit_diff),
        ("worktree_and_branch_switch", worktree_and_branch_switch),
        ("merge_conflict_resolve", merge_conflict_resolve),
        ("pull_request_review", pull_request_review),
        ("settings_toggles", settings_toggles),
        ("sidebar_slide_toggle", sidebar_slide_toggle),
        ("quick_switcher", quick_switcher),
    ];
    let (mut passed, mut failed) = (0, vec![]);
    let start = Instant::now();
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) { continue }
        let t0 = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut t = T::new(forge);
            test(&mut t);
            std::mem::take(&mut t.shots)
        }));
        match r {
            Ok(shots) if shots.is_empty() => { passed += 1; println!("test {name} ... ok ({:.1}s)", t0.elapsed().as_secs_f64()) }
            Ok(shots) => { println!("test {name} ... FAILED (screenshots)\n    {}", shots.join("\n    ")); failed.push(*name) }
            Err(_) => { println!("test {name} ... FAILED"); failed.push(*name) }
        }
    }
    println!("\ne2e: {passed} passed, {} failed in {:.1}s", failed.len(), start.elapsed().as_secs_f64());
    if !failed.is_empty() { println!("failed: {}", failed.join(", ")); std::process::exit(1) }
}

// MARK: - Tests

impl T {
    fn files(&self, path: &str) -> Vec<String> {
        let m = self.model(path).unwrap();
        self.cx.read_entity(&m, |m, _| m.state.files().iter().map(|f| f.id()).collect())
    }
    fn shown_diff(&self, path: &str) -> Option<(String, usize)> {
        let m = self.model(path).unwrap();
        self.cx.read_entity(&m, |m, _| m.diff.as_ref().map(|d| (d.new_path.clone().or(d.old_path.clone()).unwrap_or_default(), d.hunks.len())))
    }
    fn history(&self) -> Entity<loupe_app::views::history::HistoryView> {
        self.cx.read_entity(&self.shell, |s, _| s.views[1].clone()).downcast().unwrap()
    }
    fn pulls(&self) -> Entity<loupe_app::views::pulls::PullsView> {
        self.cx.read_entity(&self.shell, |s, _| s.views[2].clone()).downcast().unwrap()
    }
    fn exists(&mut self, id: impl Into<gpui_kit::ElementId>) -> bool { let id = id.into(); self.win(|w, _| w.try_find(id).is_some()) }
}

/// Changes: the status list (unstaged src/lib.rs, untracked notes.txt) and the first file's two-hunk diff.
fn changes_list_and_diff(t: &mut T) {
    let app = t.fx.app.clone();
    assert_eq!(t.tab(), Tab::Changes);
    assert_eq!(t.files(&app), ["W:src/lib.rs", "W:notes.txt"]);
    t.wait("the diff", |t| t.shown_diff(&app).is_some());
    assert_eq!(t.shown_diff(&app), Some(("src/lib.rs".into(), 2)));
    assert!(t.exists(("hunk", 0usize)) && t.exists(("hunk", 2usize)), "Stage Hunk and Discard on the first hunk");
    t.shot("changes");
    let m = t.model(&app).unwrap();
    t.cx.update_entity(&m, |m, cx| m.select_file(Some("W:notes.txt".into()), cx));
    t.wait("notes.txt's diff", |t| t.shown_diff(&app).is_some_and(|d| d.0 == "notes.txt"));
}

/// Stage the first hunk, unstage it from the staged copy, stage it again, commit with a typed summary.
fn stage_unstage_hunk_and_commit(t: &mut T) {
    let app = t.fx.app.clone();
    t.wait("the diff", |t| t.shown_diff(&app).is_some());
    t.click(("hunk", 0usize)); // Stage Hunk on the first hunk
    t.wait("a staged copy", |t| t.files(&app).contains(&"S:src/lib.rs".to_string()));
    let cached = t.fx.git(&app, &["diff", "--cached"]);
    assert!(cached.contains("+pub const VERSION") && !cached.contains("pub fn last"), "only the first hunk is staged:\n{cached}");
    t.shot("staged-hunk");

    let m = t.model(&app).unwrap();
    t.cx.update_entity(&m, |m, cx| m.select_file(Some("S:src/lib.rs".into()), cx));
    t.wait("the staged diff", |t| t.cx.read_entity(&m, |m, _| m.state.selected_file().is_some_and(|f| f.staged)) && t.shown_diff(&app).is_some_and(|d| d.1 == 1));
    t.click(("hunk", 1usize)); // Unstage Hunk
    t.wait("nothing staged", |t| !t.files(&app).iter().any(|f| f.starts_with("S:")));
    assert_eq!(t.fx.git(&app, &["diff", "--cached"]), "");

    t.cx.update_entity(&m, |m, cx| m.select_file(Some("W:src/lib.rs".into()), cx));
    t.wait("the unstaged diff", |t| t.shown_diff(&app).is_some_and(|d| d.1 == 2));
    t.click(("hunk", 0usize));
    t.wait("staged again", |t| t.files(&app).contains(&"S:src/lib.rs".to_string()));
    t.click("commit-summary");
    t.win(|w, cx| w.input("Add a version constant", cx));
    t.settle();
    t.press("cmd-enter");
    t.wait("the commit", |t| !t.files(&app).iter().any(|f| f.starts_with("S:")));
    assert_eq!(t.fx.git(&app, &["log", "-1", "--format=%s"]), "Add a version constant");
    assert!(t.fx.git(&app, &["show", "HEAD:src/lib.rs"]).contains("VERSION"));
    assert_eq!(t.files(&app), ["W:src/lib.rs", "W:notes.txt"], "the second hunk and notes.txt stay uncommitted");
    t.shot("committed");
}

/// History: the log of main (newest first) and a selected commit's files.
fn history_list_and_commit_diff(t: &mut T) {
    t.press("cmd-2");
    assert_eq!(t.tab(), Tab::History);
    let h = t.history();
    t.wait("the log", |t| t.cx.read_entity(&h, |h, cx| h.commit_shas(cx).len() == 5));
    let shas = t.cx.read_entity(&h, |h, cx| h.commit_shas(cx));
    assert_eq!(shas[0], t.fx.main_sha);
    let parser = t.fx.git(&t.fx.app, &["rev-parse", "main~3"]); // "Add a parser"
    let app = t.fx.app.clone();
    t.cx.update_entity(&t.app.clone(), |a, cx| a.emit(loupe_app::state::AppEvent::ShowCommit { worktree: app, sha: parser }, cx));
    t.wait("the commit's files", |t| t.cx.read_entity(&h, |h, cx| h.file_paths(cx) == ["src/parser.rs"]));
    t.shot("history");
}

/// The sidebar picks another worktree (⌥⌘2 and a click); the branch picker switches the worktree's branch.
fn worktree_and_branch_switch(t: &mut T) {
    let (app, feature) = (t.fx.app.clone(), t.fx.feature.clone());
    t.press("alt-cmd-2");
    assert_eq!(t.read(|a, _| a.selection().map(str::to_string)), Some(feature.clone()));
    t.press("alt-cmd-1");
    assert_eq!(t.read(|a, _| a.selection().map(str::to_string)), Some(app.clone()));
    t.press("alt-cmd-2");
    t.wait("feature's status", |t| t.model(&feature).is_some_and(|m| t.cx.read_entity(&m, |m, _| m.state.branch.as_deref() == Some("feature"))));
    t.click("branch");
    t.wait("the branch list", |t| t.exists("br-0-topic"));
    t.shot("branches");
    t.click("br-0-topic");
    t.wait("the switch", |t| t.model(&feature).is_some_and(|m| t.cx.read_entity(&m, |m, _| m.state.branch.as_deref() == Some("topic"))));
    assert_eq!(t.fx.git(&feature, &["branch", "--show-current"]), "topic");
}

/// The merge worktree's conflicted file opens the merge editor; "Use topic" then Mark Resolved stages the result.
fn merge_conflict_resolve(t: &mut T) {
    let merge = t.fx.merge.clone();
    t.press("alt-cmd-3");
    t.wait("the conflict", |t| t.model(&merge).is_some_and(|m| t.cx.read_entity(&m, |m, _| m.state.conflicted().len() == 1)));
    let m = t.model(&merge).unwrap();
    let id = t.cx.read_entity(&m, |m, _| m.state.conflicted()[0].id());
    t.cx.update_entity(&m, |m, cx| m.select_file(Some(id), cx));
    t.wait("the merge editor", |t| t.exists("mark-resolved"));
    t.shot("merge-editor");
    t.press("alt-cmd-right"); // Use Theirs for Conflict
    t.click("mark-resolved");
    t.wait("no conflicts", |t| t.cx.read_entity(&m, |m, _| m.state.conflicted().is_empty()));
    assert_eq!(t.fx.git(&merge, &["diff", "--name-only", "--diff-filter=U"]), "");
    let staged = std::fs::read_to_string(std::path::Path::new(&merge).join("src/conflict.rs")).unwrap();
    assert!(staged.contains("\"topic\"") && !staged.contains("<<<<<<<"), "{staged}");
    assert!(t.read(|a, _| a.overlay.is_none()), "the editor closes once the file is resolved");
}

/// Pull Requests against the mock Forgejo: the list, PR #7's files, its 60 checks (two status pages), a comment.
fn pull_request_review(t: &mut T) {
    t.press("cmd-3");
    let p = t.pulls();
    t.wait("the PR list", |t| t.cx.read_entity(&p, |p, _| p.list_count() == 1));
    t.shot("pulls");
    let app = t.fx.app.clone();
    t.cx.update_entity(&t.app.clone(), |a, cx| a.emit(loupe_app::state::AppEvent::OpenPull { worktree: app.clone(), number: 7 }, cx));
    t.wait("PR #7's diff", |t| t.forge.requests().iter().any(|r| r.contains("/pulls/7.diff")) && t.read(|a, _| a.controls().view.is_some()));
    let set_mode = |t: &mut T, mode| {
        let (_, _, set) = t.read(|a, _| a.controls().view.clone()).unwrap();
        t.win(|w, cx| set(mode, w, cx));
        t.settle();
    };
    set_mode(t, loupe_app::state::ViewMode::Files);
    t.settle();
    t.shot("pr-files");

    // Checks: 60 statuses, read past Forgejo's first page of 50.
    let sha = t.fx.feature_sha.clone();
    t.cx.update_entity(&t.app.clone(), |a, cx| a.emit(loupe_app::state::AppEvent::ShowChecks { worktree: app.clone(), sha, pull: Some(7) }, cx));
    let overlay = |t: &T| t.read(|a, _| a.overlay.clone().and_then(|o| o.downcast::<loupe_app::views::ci::ChecksOverlay>().ok()));
    t.wait("the checks", |t| overlay(t).is_some_and(|o| t.cx.read_entity(&o, |o, cx| o.check_count(cx) > 0)));
    let o = overlay(t).unwrap();
    assert_eq!(t.cx.read_entity(&o, |o, cx| o.check_count(cx)), forge::STATUSES);
    assert!(t.forge.requests().iter().any(|r| r.contains("/status?") && r.contains("page=2")), "status page 2 was read");
    t.shot("checks");
    t.cx.update_entity(&t.app.clone(), |a, cx| a.dismiss(cx));
    t.settle();

    set_mode(t, loupe_app::state::ViewMode::Conversation);
    t.wait("the composer", |t| t.exists("comment-box"));
    t.click("comment-box");
    t.win(|w, cx| w.input("Ship it", cx));
    t.settle();
    t.click("comment-send");
    t.wait("the posted comment", |t| t.forge.state.lock().unwrap().comments.len() == 2);
    assert_eq!(t.forge.state.lock().unwrap().comments[1]["body"], "Ship it");
    assert!(t.forge.requests().iter().any(|r| r.starts_with("POST ") && r.contains("/issues/7/comments")));
    t.shot("pr-conversation");
}

/// ⌘, opens Settings (a second window): its tab and Appearance persist; the toolbar's Full mode persists.
fn settings_toggles(t: &mut T) {
    t.click("mode-1"); // Full
    assert!(t.read(|a, _| a.full_mode));
    t.settle();
    assert!(loupe_forge::settings::shared().bool("prFullMode"));
    t.click("mode-0");
    assert!(!t.read(|a, _| a.full_mode));

    t.press("cmd-,");
    let w = t.cx.update(|cx| cx.windows().into_iter().find(|w| *w != t.window)).expect("the settings window");
    let main = std::mem::replace(&mut t.window, w);
    t.settle();
    t.click("settings-tabs-0"); // General
    t.click("appearance-2");    // Dark
    {
        let p = loupe_forge::settings::shared();
        assert_eq!((p.string("settingsTab").as_deref(), p.string("appearance").as_deref()), (Some("general"), Some("dark")));
    }
    t.click("settings-tabs-3"); // Shortcuts: the same on every machine (General lists the AI CLIs installed)
    assert_eq!(loupe_forge::settings::shared().string("settingsTab").as_deref(), Some("shortcuts"));
    t.shot("settings-shortcuts");
    t.window = main;
}

/// ⇧⌘S slides the sidebar shut and open again.
fn sidebar_slide_toggle(t: &mut T) {
    let open = |t: &T| t.cx.read_entity(&t.shell, |s, _| s.sidebar_open());
    let sliding = |t: &T| t.cx.read_entity(&t.shell, |s, _| s.sidebar_sliding());
    assert!(open(t) && !sliding(t));
    t.win(|w, cx| w.press("cmd-shift-s", cx));
    t.cx.run_until_parked();
    assert!(!open(t) && sliding(t), "it slides");
    t.wait("the slide to end", |t| !sliding(t)); // 260 ms of the test clock, then nothing stays animated
    t.shot("sidebar-closed");
    t.click("toggle-sidebar");
    assert!(open(t));
    t.wait("the slide to end", |t| !sliding(t));
    t.shot("changes");
}

/// ⌘K: the quick switcher filters worktrees as you type; Return selects.
fn quick_switcher(t: &mut T) {
    t.press("cmd-k");
    t.win(|w, cx| w.input("merge", cx));
    t.settle();
    t.shot("switcher");
    t.press("enter");
    assert_eq!(t.read(|a, _| a.selection().map(str::to_string)), Some(t.fx.merge.clone()));
}
