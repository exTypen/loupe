//! The diff renderer on its own: a synthetic 150k-line diff and a small realistic one with comments.
//!
//!   cargo run -p loupe --example diff_demo [--release]
//!   DEMO_FILE=small …          start on the small file
//!   DEMO_SCROLL=1 …            scroll the big file for 600 frames, print frame times, quit
//!
//! Prints how long the big file took from `set_file` to its first frame.
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, WindowBounds, WindowOptions, div, px, size,
};
use loupe_app::diff::*;
use loupe_app::theme;

fn big_file() -> FileDiff {
    let mut f = FileDiff::new(Some("src/generated/big.rs".into()), Some("src/generated/big.rs".into()));
    let (mut o, mut n) = (1u32, 1u32);
    let mut h = 0;
    while f.lines.len() < 150_000 {
        f.push(LineKind::Hunk, &format!("@@ -{o},40 +{n},41 @@ fn block_{h}()"), None, None);
        for k in 0..38 {
            let t = format!("    let value_{h}_{k} = compute(\"item {k}\", {}); // keep {}", h * 7 + k, k % 5);
            match k % 13 {
                5 => { f.push(LineKind::Removed, &t, Some(o), None); o += 1;
                       f.push(LineKind::Added, &t.replace("compute", "compute_fast"), None, Some(n)); n += 1 }
                9 => { f.push(LineKind::Added, &format!("    // added in block {h}\tline {k}"), None, Some(n)); n += 1 }
                _ => { f.push(LineKind::Context, &t, Some(o), Some(n)); o += 1; n += 1 }
            }
        }
        h += 1;
    }
    f
}

const SMALL: &str = "@@ -12,9 +12,11 @@ impl Repository {
     pub fn status(&self) -> Result<Status> {
-        let out = self.git(&[\"status\", \"--porcelain\"])?;
+        // v2 has rename scores and submodule state.
+        let out = self.git(&[\"status\", \"--porcelain=v2\", \"-z\"])?;
         let mut status = Status::default();
-        for line in out.lines() {
+        for line in out.split('\\0') {
             status.push(parse_entry(line)?);
         }
+        status.sort();
         Ok(status)
     }
@@ -40,6 +43,6 @@ impl Repository {
     fn git(&self, args: &[&str]) -> Result<String> {
         let mut cmd = Command::new(\"git\");
-        cmd.env(\"GIT_OPTIONAL_LOCKS\", \"1\");
+        cmd.env(\"GIT_OPTIONAL_LOCKS\", \"0\");
         cmd.args(args).current_dir(&self.dir);
         run(cmd, Duration::from_secs(30))
     }
";

/// A toy Rust tokenizer standing in for loupe-code's: runs on the background executor like the real one.
fn highlight(f: &FileDiff) -> Syntax {
    const KW: &[&str] = &["fn", "let", "mut", "pub", "impl", "for", "in", "self", "return", "use", "struct", "if", "else"];
    (0..f.lines.len()).map(|i| {
        let t = f.line_text(i).as_bytes();
        let mut out = vec![];
        let mut j = 0;
        while j < t.len() {
            let c = t[j];
            let start = j;
            let kind = if c == b'/' && t.get(j + 1) == Some(&b'/') { j = t.len(); 2 }
            else if c == b'"' { j += 1; while j < t.len() && t[j] != b'"' { j += 1 } j = (j + 1).min(t.len()); 1 }
            else if c.is_ascii_digit() { while j < t.len() && t[j].is_ascii_digit() { j += 1 } 3 }
            else if c.is_ascii_alphabetic() || c == b'_' {
                while j < t.len() && (t[j].is_ascii_alphanumeric() || t[j] == b'_') { j += 1 }
                let w = &t[start..j];
                if KW.iter().any(|k| k.as_bytes() == w) { 0 } else if w[0].is_ascii_uppercase() { 4 } else if t.get(j) == Some(&b'(') { 5 } else { continue }
            } else { j += 1; continue };
            out.push(Span { range: start as u32..j as u32, kind });
        }
        out
    }).collect()
}

struct Demo {
    diff: Entity<DiffView>,
    files: [Arc<FileDiff>; 2],
    comments: Vec<InlineComment>,
    selected: usize,
    bench: Option<(usize, Instant, Vec<Duration>)>,
}

impl Demo {
    fn select(&mut self, k: usize, cx: &mut Context<Self>) {
        self.selected = k;
        let (file, comments) = (self.files[k].clone(), if k == 1 { self.comments.clone() } else { vec![] });
        let t0 = Instant::now();
        let n = file.lines.len();
        self.diff.update(cx, |d, cx| {
            d.on_first_paint(move || eprintln!("opened {n} lines: first frame after {:.1} ms", t0.elapsed().as_secs_f64() * 1e3));
            d.set_file(file, cx);
            d.set_comments(comments, cx);
        });
        cx.notify();
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((frames, last, times)) = &mut self.bench {
            let now = Instant::now();
            if *frames > 0 { times.push(now - *last) }
            *last = now;
            *frames += 1;
            self.diff.read(cx).list_state().scroll_by(px(37.));
            if *frames > 600 {
                times.sort();
                let avg = times.iter().sum::<Duration>() / times.len() as u32;
                eprintln!("scroll: {} frames, avg {:.2} ms, p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms", times.len(),
                    avg.as_secs_f64() * 1e3, times[times.len() / 2].as_secs_f64() * 1e3,
                    times[times.len() * 99 / 100].as_secs_f64() * 1e3, times.last().unwrap().as_secs_f64() * 1e3);
                cx.quit();
            } else {
                window.request_animation_frame();
            }
        }
        let names = ["big.rs  · 150k lines", "repository.rs"];
        let side = div().w(px(240.)).h_full().flex_none().bg(theme::sidebar()).border_r_1().border_color(theme::border())
            .px(px(10.)).pt(px(44.)).flex().flex_col().gap(px(2.)).font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .child(div().px(px(10.)).pb(px(6.)).flex().justify_between().text_color(theme::text_muted())
                .child("Changes").child(div().text_color(theme::text_faint()).child("2")))
            .children(names.iter().enumerate().map(|(k, name)| {
                div().id(("file", k)).h(theme::ROW_HEIGHT).px(px(10.)).flex().items_center().justify_between().rounded(theme::RADIUS_ROW)
                    .text_color(if k == self.selected { theme::text() } else { theme::text_muted() })
                    .when_selected(k == self.selected).hover(|s| s.bg(theme::hover())).cursor_pointer()
                    .child(name.to_string())
                    .child(div().text_color(theme::text_faint()).text_size(px(11.)).child(format!("⌘{}", k + 1)))
                    .on_click(cx.listener(move |d, _, _, cx| d.select(k, cx)))
            }));
        div().size_full().flex().bg(theme::canvas()).text_color(theme::text())
            .child(side)
            .child(div().flex_1().min_w_0().h_full().pt(px(40.)).px(px(24.)).pb(px(24.)).child(self.diff.clone()))
    }
}

trait WhenSelected { fn when_selected(self, on: bool) -> Self; }
impl WhenSelected for gpui_kit::Stateful<gpui_kit::Div> {
    fn when_selected(self, on: bool) -> Self { if on { self.bg(theme::selected()) } else { self } }
}

fn main() {
    let t = Instant::now();
    let big = Arc::new(big_file());
    eprintln!("generated {} lines in {:.0} ms", big.lines.len(), t.elapsed().as_secs_f64() * 1e3);
    let small = Arc::new(FileDiff::parse_patch(Some("Sources/repository.rs".into()), Some("Sources/repository.rs".into()), SMALL));
    let comments = vec![
        InlineComment { author: "mira".into(), body: "Does v2 need git ≥ 2.11? We still claim 2.9 in the README.".into(), new_line: Some(15), old_line: None, draft_id: None },
        InlineComment { author: "You".into(), body: "Sorting here hides the order git reports; the tree sorts anyway.".into(), new_line: Some(20), old_line: None, draft_id: Some(1) },
    ];
    let start_small = std::env::var("DEMO_FILE").is_ok_and(|v| v == "small");
    let bench = std::env::var("DEMO_SCROLL").is_ok();

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        // The app is dark; the inputs inside the diff (find, comment composer) follow gpui-component's theme.
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui_kit::TitlebarOptions { title: Some("diff_demo".into()), appears_transparent: true, ..Default::default() }),
            ..Default::default()
        };
        gpui_kit::open_window(opts, cx, |_, cx| {
            let first = if start_small { small.clone() } else { big.clone() };
            let diff = cx.new(|cx| {
                let mut d = DiffView::new(Arc::new(FileDiff::default()), cx);
                d.set_highlighter(Some(Arc::new(highlight)), cx);
                d.set_hunk_actions(vec![HunkAction::Discard, HunkAction::Stage], cx);
                d.set_change_ruler(true, cx);
                d.on_hunk_action(|a, lines, _, _| eprintln!("hunk action {a:?} on {} lines", lines.len()));
                d.on_navigate(|w, a, _, _| eprintln!("navigate {a:?} {w}"));
                d.on_mark_viewed(|_, _, _| eprintln!("mark viewed"));
                d.header_actions(|_, _| div().h(px(22.)).px(px(10.)).flex().items_center().rounded(theme::RADIUS_PILL)
                    .bg(theme::text()).text_color(theme::canvas()).text_size(px(11.5)).font_family(theme::UI_FONT).child("Stage File").into_any_element());
                d
            });
            cx.new(|cx: &mut Context<Demo>| {
                let weak = cx.weak_entity();
                diff.update(cx, |d, _| {
                    let w2 = weak.clone();
                    d.on_add_comment(move |new, old, body, _, cx| {
                        w2.update(cx, |demo, cx| {
                            demo.comments.push(InlineComment { author: "You".into(), body, new_line: new, old_line: old, draft_id: Some(demo.comments.len() as u64 + 1) });
                            let c = demo.comments.clone();
                            demo.diff.update(cx, |d, cx| d.set_comments(c, cx));
                        }).ok();
                    });
                    d.on_delete_draft(move |id, _, cx| {
                        weak.update(cx, |demo, cx| {
                            demo.comments.retain(|c| c.draft_id != Some(id));
                            let c = demo.comments.clone();
                            demo.diff.update(cx, |d, cx| d.set_comments(c, cx));
                        }).ok();
                    });
                });
                let mut demo = Demo { diff, files: [big.clone(), small.clone()], comments, selected: 0, bench: bench.then(|| (0, Instant::now(), vec![])) };
                demo.select(if first.uid == small.uid { 1 } else { 0 }, cx);
                demo
            })
        }).expect("window");
        cx.activate(true);
    });
}
