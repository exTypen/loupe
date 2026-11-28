//! Loupe's performance regression benches: the real views and models in GPUI's `BenchAppContext` (Metal headless
//! renderer, production threading), against repositories generated once per run under target/tmp/bench.
//!
//! Run one at a time through scripts/bench.sh (`record` / `compare`); by hand:
//!
//!     BENCH_MEASUREMENT=foreground-instructions cargo bench --profile release-fast -p loupe-bench -- diff_scroll
//!
//! | bench | what one iteration is |
//! |---|---|
//! | diff_open_150k | parse a 150,000-line diff, build its DiffView, draw the first frame |
//! | diff_scroll_150k | scroll that diff by three lines and draw |
//! | tab_switch | Changes → History → Pull Requests → Changes, a frame after each |
//! | history_open_20k | a fresh window on a 20,000-commit repository's History, until the log is listed and drawn |
//! | changes_open_5k | a fresh window on a worktree with 5,000 changed files, until the list is drawn |
//! | merge_editor_large | open the merge editor on a 20,000-line file with 500 conflicts, until loaded and drawn |
//!
//! diff_open_150k also prints `LOUPE_BENCH peak_phys_footprint=<bytes>`: the process's lifetime peak
//! phys_footprint (proc_pid_rusage), what Activity Monitor calls Memory.
use std::cell::Cell;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{App, AppContext as _, BenchAppContext, Context, Entity, IntoElement, Render, Task, Window, div, px, size};
use gpui::prelude::*;
use loupe_app::diff::DiffView;
use loupe_app::shell::Shell;
use loupe_app::state::{AppEvent, AppModel, Tab, adapt, scenario::Scenario};

const DIFF_LINES: usize = 150_000;
const HISTORY: usize = 20_000;
const CHANGED: usize = 5_000;
const MERGE_LINES: usize = 20_000;
const CONFLICT_EVERY: usize = 40; // 500 conflicts

fn root() -> PathBuf { PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench") }

// MARK: - Fixtures

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "@1768208400 +0000").env("GIT_COMMITTER_DATE", "@1768208400 +0000").output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// A repository from a fast-import stream (fixed dates: the same SHAs every run).
fn import(dir: &Path, stream: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    let mut c = Command::new("git").args(["fast-import", "--quiet"]).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(stream).unwrap();
    assert!(c.wait().unwrap().success());
    git(dir, &["reset", "-q", "--hard", "main"]);
}

fn blob(s: &mut Vec<u8>, path: &str, data: &str) {
    s.extend(format!("M 100644 inline {path}\ndata {}\n", data.len()).as_bytes());
    s.extend(data.as_bytes());
    s.push(b'\n');
}

fn commit(s: &mut Vec<u8>, branch: &str, i: usize, msg: &str, from: Option<&str>, files: impl FnOnce(&mut Vec<u8>)) {
    let t = 1_768_208_400 + i * 60;
    s.extend(format!("commit refs/heads/{branch}\nmark :{}\ncommitter Alice Mock <alice@example.com> {t} +0000\ndata {}\n{msg}\n", i + 1, msg.len() + 1).as_bytes());
    if let Some(f) = from { s.extend(format!("from {f}\n").as_bytes()) }
    files(s);
}

fn code_line(i: usize) -> String { format!("    let value_{i} = compute({i}, \"item {i}\").map(|x| x * {});", i % 97) }

struct Fixtures { history: String, changes: String, merge: String, merge_file: String, diff: String }

/// Built once per process: HOME, settings file and the repositories.
fn fixtures() -> &'static Fixtures {
    static F: OnceLock<Fixtures> = OnceLock::new();
    F.get_or_init(|| {
        let root = root();
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".gitconfig"), "[user]\n\tname = Alice Mock\n\temail = alice@example.com\n[gc]\n\tauto = 0\n[core]\n\tfsmonitor = false\n").unwrap();
        // Benches are single-threaded until here (criterion's own threads don't read the environment).
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::remove_var("LOUPE_STRESS");
        }
        assert!(loupe_forge::settings::use_file(root.join("settings.plist")), "settings were read before the bench picked its file");
        let home = home.canonicalize().unwrap();

        // History: 20,000 commits, each touching one of 200 files.
        let history = home.join("history");
        let mut s = Vec::new();
        for i in 0..HISTORY {
            commit(&mut s, "main", i, &format!("Change {i}: adjust module {}", i % 200), None, |s| blob(s, &format!("src/m{}.rs", i % 200), &format!("pub const V: u32 = {i};\n")));
        }
        import(&history, &s);

        // Changes: 5,000 tracked files, every one modified in the worktree.
        let changes = home.join("changes");
        let mut s = Vec::new();
        commit(&mut s, "main", 0, "Add files", None, |s| for i in 0..CHANGED { blob(s, &format!("d{}/f{i}.txt", i / 100), "one\n") });
        import(&changes, &s);
        for i in 0..CHANGED { std::fs::write(changes.join(format!("d{}/f{i}.txt", i / 100)), "two\n").unwrap() }

        // Merge: one 20,000-line file both sides change on every 40th line.
        let merge = home.join("merge");
        let lines = |tag: &str| (0..MERGE_LINES).map(|i| if !tag.is_empty() && i % CONFLICT_EVERY == 0 { format!("{} // {tag}\n", code_line(i)) } else { code_line(i) + "\n" }).collect::<String>();
        let mut s = Vec::new();
        commit(&mut s, "main", 0, "Base", None, |s| blob(s, "big.rs", &lines("")));
        commit(&mut s, "main", 1, "Ours", None, |s| blob(s, "big.rs", &lines("ours")));
        commit(&mut s, "theirs", 2, "Theirs", Some(":1"), |s| blob(s, "big.rs", &lines("theirs")));
        import(&merge, &s);
        let o = Command::new("git").args(["merge", "--no-edit", "theirs"]).current_dir(&merge).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
        assert!(!o.status.success(), "the merge must conflict");

        // A 150,000-line added file, as `git diff` prints it.
        let mut diff = format!("diff --git a/big.rs b/big.rs\nnew file mode 100644\n--- /dev/null\n+++ b/big.rs\n@@ -0,0 +1,{DIFF_LINES} @@\n");
        for i in 0..DIFF_LINES { diff += "+"; diff += &code_line(i); diff += "\n" }

        let p = |d: &Path| d.to_string_lossy().into_owned();
        Fixtures { history: p(&history), changes: p(&changes), merge: p(&merge), merge_file: "big.rs".into(), diff }
    })
}

// MARK: - App helpers

fn init(cx: &mut BenchAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        loupe_app::ui::install_theme(cx);
        loupe_app::shell::keys::load_shortcuts(cx);
        adapt::apply_code_theme();
        loupe_app::shell::keys::install(cx);
    });
}

fn window_options() -> gpui::WindowOptions {
    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds { origin: Default::default(), size: size(px(1440.), px(900.)) })),
        ..Default::default()
    }
}

/// A new AppModel and main window on `repo`, showing `tab`.
fn open_app(cx: &mut App, repo: &str, tab: Tab) -> (gpui::AnyWindowHandle, Entity<AppModel>) {
    let s = Scenario { repos: vec![repo.into()], big: Some(repo.into()), ..Default::default() };
    loupe_app::state::scenario::prepare(&s);
    {
        let mut p = loupe_forge::settings::shared();
        p.set_string("tab", tab.key());
        p.set_string("aiHarness", "off");
    }
    let app = cx.new(|cx| AppModel::new(Some(s), cx));
    let a = app.clone();
    let (w, _) = gpui_kit::open_window(window_options(), cx, |window, cx| cx.new(|cx| Shell::new(a, window, cx))).unwrap();
    w.update(cx, |_, w, _| w.activate_window()).ok();
    (w, app)
}

/// A task that ends once `ready` holds and a frame has been drawn after it.
fn until(cx: &mut App, w: gpui::AnyWindowHandle, ready: impl Fn(&App) -> bool + 'static) -> Task<()> {
    cx.spawn(async move |cx| {
        while !cx.update(|cx| ready(cx)) { cx.background_executor().timer(Duration::from_micros(200)).await }
        let drawn = Rc::new(Cell::new(false));
        let d = drawn.clone();
        w.update(cx, |_, w, _| w.on_next_frame(move |_, _| d.set(true))).ok();
        while !drawn.get() { cx.background_executor().timer(Duration::from_micros(200)).await }
    })
}

/// Runs the app until `t` finishes (setup, outside any measurement).
fn block_on(cx: &mut BenchAppContext, t: Task<()>) {
    let done = Rc::new(Cell::new(false));
    let d = done.clone();
    cx.update(|cx| cx.spawn(async move |_| { t.await; d.set(true) }).detach());
    cx.run_until(|| done.get().then_some(()));
}

fn close_windows(cx: &mut BenchAppContext) {
    cx.update(|cx| for w in cx.windows() { w.update(cx, |_, w, _| w.remove_window()).ok(); });
}

/// The process's lifetime peak phys_footprint in bytes.
fn peak_phys_footprint() -> u64 {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut libc::rusage_info_t) };
    if r == 0 { info.ri_lifetime_max_phys_footprint } else { 0 }
}

/// An active window (an inactive one's frames are throttled) showing a [`Host`].
fn open_host(cx: &mut App, host: impl FnOnce(&mut Context<Host>) -> Host) -> Entity<Host> {
    let (w, h) = gpui_kit::open_window(window_options(), cx, |_, cx| cx.new(host)).unwrap();
    w.update(cx, |_, w, _| w.activate_window()).ok();
    h
}

/// Holds whatever the bench shows.
struct Host { view: Option<Entity<DiffView>> }
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children(self.view.clone())
    }
}

fn big_file_view(text: &str, cx: &mut Context<Host>) -> Entity<DiffView> {
    let file = Arc::new(adapt::file_diff(&loupe_core::diff::parse(text).remove(0)));
    cx.new(|cx| { let mut d = DiffView::new(file, cx); d.set_highlighter(Some(adapt::highlighter()), cx); d })
}

// MARK: - Benches

#[gpui::bench]
fn diff_open_150k(cx: &mut BenchAppContext) {
    let f = fixtures();
    init(cx);
    let host = cx.update(|cx| open_host(cx, |_| Host { view: None }));
    cx.run_until_idle();
    let h2 = host.clone();
    let open = move |cx: &mut BenchAppContext| cx.update(|cx| {
        let painted = Rc::new(Cell::new(false));
        let p = painted.clone();
        h2.update(cx, |h, cx| {
            let v = big_file_view(&f.diff, cx);
            v.update(cx, |d, _| d.on_first_paint(move || p.set(true)));
            h.view = Some(v);
            cx.notify();
        });
        cx.spawn(async move |cx| while !painted.get() { cx.background_executor().timer(Duration::from_micros(200)).await })
    });
    // Memory, once per process: one diff open and drawn, nothing else of this bench run yet.
    static MEASURED: std::sync::Once = std::sync::Once::new();
    MEASURED.call_once(|| {
        let t = open(cx);
        block_on(cx, t);
        cx.settle();
        println!("LOUPE_BENCH peak_phys_footprint={}", peak_phys_footprint());
    });
    cx.bench_batched_task(move |cx| cx.update(|cx| host.update(cx, |h, cx| { h.view = None; cx.notify() })), move |_, cx| open(cx));
    close_windows(cx);
}

#[gpui::bench]
fn diff_scroll_150k(cx: &mut BenchAppContext) {
    let f = fixtures();
    init(cx);
    let host = cx.update(|cx| open_host(cx, |cx| Host { view: Some(big_file_view(&f.diff, cx)) }));
    cx.run_until_idle();
    let view = cx.read(|cx| host.read(cx).view.clone().unwrap());
    let w = cx.read(|cx| cx.windows()[0]);
    let n = Rc::new(Cell::new(0usize));
    cx.bench_batched_task(|_| (), move |_, cx| cx.update(|cx| {
        n.set(n.get() + 1);
        // 60 px a step, back to the top every 2,000 steps (~5,000 lines): always new rows to lay out.
        let k = n.get();
        if k % 2000 == 0 {
            view.update(cx, |v, cx| { v.list_state().scroll_to(gpui::ListOffset { item_ix: 0, offset_in_item: px(0.) }); cx.notify() });
        } else {
            // A trackpad wheel event in the middle of the diff, through the window's own dispatch.
            let e = gpui::ScrollEvent { position: gpui::point(px(720.), px(450.)), delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-60.))),
                modifiers: Default::default(), touch_phase: gpui::TouchPhase::Moved, source: Default::default() };
            w.update(cx, |_, w, cx| { w.dispatch_event(gpui::PlatformInput::Scroll(e), cx); }).ok();
        }
        until(cx, w, |_| true)
    }));
    close_windows(cx);
}

#[gpui::bench]
fn tab_switch(cx: &mut BenchAppContext) {
    let f = fixtures();
    init(cx);
    let (w, app) = cx.update(|cx| open_app(cx, &f.history, Tab::Changes));
    // Every tab loaded once first (History's log, the Pull Requests placeholder): the switch is what's measured.
    let t = cx.update(|cx| {
        for t in [Tab::History, Tab::Pulls, Tab::Changes] { app.update(cx, |a, cx| a.set_tab(t, cx)) }
        let h = app.clone();
        until(cx, w, move |cx| h.read(cx).selected_model().is_some_and(|m| !m.read(cx).state.branch.is_none()))
    });
    block_on(cx, t);
    let mut i = 0;
    cx.bench_iter(|cx| {
        i += 1;
        let t = [Tab::History, Tab::Pulls, Tab::Changes][i % 3];
        cx.update(|cx| app.update(cx, |a, cx| a.set_tab(t, cx)));
    });
    close_windows(cx);
}

#[gpui::bench]
fn history_open_20k(cx: &mut BenchAppContext) {
    let f = fixtures();
    init(cx);
    cx.bench_batched_task(close_windows, |_, cx| cx.update(|cx| {
        let (w, _) = open_app(cx, &f.history, Tab::History);
        let shell = w.downcast::<gpui_kit::base::Root>().and_then(|r| r.read(cx).ok().map(|r| r.view().clone()))
            .and_then(|v| v.downcast::<Shell>().ok()).unwrap();
        let h = shell.read(cx).views[1].clone().downcast::<loupe_app::views::history::HistoryView>().unwrap();
        until(cx, w, move |cx| h.read(cx).commit_shas(cx).len() >= 100)
    }));
    close_windows(cx);
}

#[gpui::bench]
fn changes_open_5k(cx: &mut BenchAppContext) {
    let f = fixtures();
    init(cx);
    let path = f.changes.clone();
    cx.bench_batched_task(close_windows, move |_, cx| {
        let path = path.clone();
        cx.update(|cx| {
            let (w, app) = open_app(cx, &path, Tab::Changes);
            until(cx, w, move |cx| app.read(cx).model(&path).is_some_and(|m| m.read(cx).state.files().len() == CHANGED))
        })
    });
    close_windows(cx);
}

#[gpui::bench]
fn merge_editor_large(cx: &mut BenchAppContext) {
    let f = fixtures();
    init(cx);
    let (w, app) = cx.update(|cx| open_app(cx, &f.merge, Tab::Changes));
    let a = app.clone();
    let m = f.merge.clone();
    let t = cx.update(|cx| until(cx, w, move |cx| a.read(cx).model(&m).is_some_and(|m| m.read(cx).state.conflicted().len() == 1)));
    block_on(cx, t);
    let (worktree, path) = (f.merge.clone(), f.merge_file.clone());
    let a = app.clone();
    cx.bench_batched_task(
        move |cx| cx.update(|cx| a.update(cx, |a, cx| a.dismiss(cx))),
        move |_, cx| cx.update(|cx| {
            app.update(cx, |a, cx| a.emit(AppEvent::OpenMergeEditor { worktree: worktree.clone(), path: path.clone() }, cx));
            let a = app.clone();
            until(cx, w, move |cx| a.read(cx).overlay.clone().and_then(|o| o.downcast::<loupe_app::views::merge::MergeEditor>().ok())
                .is_some_and(|e| e.read(cx).is_loaded()))
        }),
    );
    close_windows(cx);
}

gpui::bench_group! {
    name = benches;
    config = criterion::Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(5))
        .without_plots();
    targets = diff_open_150k, diff_scroll_150k, tab_switch, history_open_20k, changes_open_5k, merge_editor_large
}
gpui::bench_main!(benches);
