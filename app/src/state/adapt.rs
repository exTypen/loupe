//! loupe-core's parsed diff → the renderer's `diff::FileDiff`, and the syntax highlighter for it.
use std::sync::Arc;

use loupe_code::syntax::{self, Language, State};
use loupe_core::diff as core;

use crate::diff::{self, Highlighter, Span, Syntax};

/// Same rows in the same order (hunk headers included, Hunk.lines without them), so a line index the renderer hands
/// back (`on_hunk_action`) is an index into `d.lines` too. Always a fresh uid: it keys the highlight cache.
pub fn file_diff(d: &core::FileDiff) -> diff::FileDiff {
    let mut f = diff::FileDiff::new(d.old_path.clone(), d.new_path.clone());
    f.text.reserve(d.lines.iter().map(|l| d.text(l).len()).sum());
    f.lines.reserve(d.lines.len());
    for l in &d.lines {
        let t = d.text(l);
        let start = f.text.len() as u32;
        f.text.push_str(t);
        f.lines.push(diff::DiffLine { kind: kind(l.kind), text: start..f.text.len() as u32, old_line: l.old_line, new_line: l.new_line, hunk: l.hunk });
    }
    f.hunks = d.hunks.iter().map(|h| diff::Hunk { header: h.header.clone(), lines: h.lines.clone() }).collect();
    f.is_binary = d.is_binary;
    f.additions = d.additions as u32;
    f.deletions = d.deletions as u32;
    f
}

fn kind(k: core::LineKind) -> diff::LineKind {
    match k {
        core::LineKind::Context => diff::LineKind::Context,
        core::LineKind::Added => diff::LineKind::Added,
        core::LineKind::Removed => diff::LineKind::Removed,
        core::LineKind::Hunk => diff::LineKind::Hunk,
        core::LineKind::Meta => diff::LineKind::Meta,
    }
}

/// loupe-code's tokenizer over a diff, with byte ranges into each raw line (what the renderer expects). Old and new
/// sides keep separate state, so a removed line opening a block comment doesn't swallow added lines; a hunk header
/// resets both (Syntax.highlight in Swift).
pub fn highlighter() -> Highlighter {
    Arc::new(|f: &diff::FileDiff| -> Syntax {
        let Some(lang) = Language::for_path(f.path()) else { return vec![Vec::new(); f.lines.len()] };
        highlight(f, lang)
    })
}

fn highlight(f: &diff::FileDiff, lang: &Language) -> Syntax {
    let (mut old, mut new) = (State::default(), State::default());
    let spans = |text: &str, st: &mut State| -> Vec<Span> {
        // Only what's drawn: a minified line is cut at MAX_DRAWN_CHARS.
        let cut = text.char_indices().nth(syntax::MAX_DRAWN_CHARS).map_or(text, |(i, _)| &text[..i]);
        syntax::tokenize(cut, lang, st).into_iter().map(|s| Span { range: s.start..s.start + s.len, kind: s.kind as u8 }).collect()
    };
    (0..f.lines.len()).map(|i| {
        let t = f.line_text(i);
        match f.lines[i].kind {
            diff::LineKind::Hunk => { old = State::default(); new = State::default(); vec![] }
            diff::LineKind::Meta => vec![],
            diff::LineKind::Removed => spans(t, &mut old),
            diff::LineKind::Added => spans(t, &mut new),
            diff::LineKind::Context => { let s = spans(t, &mut old); new = old; s }
        }
    }).collect()
}

/// The code theme the diffs draw their syntax in: "editorDarkTheme" (the app is dark-only).
pub fn apply_code_theme() {
    use loupe_code::theme::EditorSettings as E;
    let p = loupe_forge::settings::shared();
    let id = p.string(E::KEY_DARK_THEME).unwrap_or_default();
    diff::set_token_colors(loupe_code::theme::CodeTheme::named(&id, true).tokens);
    // ST-08: the font, size and line height too (Settings › Editor calls this on every change).
    let d = E::default();
    let e = E { font_family: p.string(E::KEY_FONT).unwrap_or(d.font_family), font_size: p.f64(E::KEY_FONT_SIZE).unwrap_or(d.font_size),
                line_height_factor: p.f64(E::KEY_LINE_HEIGHT).unwrap_or(d.line_height_factor), ..d };
    diff::set_editor_font(&e.font_family, e.font_size as f32, e.line_height() as f32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_hunks_line_up_with_core() {
        let src = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    /* old\n+    let x = 1; // new\n }\n";
        let d = core::parse(src).remove(0);
        let f = file_diff(&d);
        assert_eq!(f.lines.len(), d.lines.len());
        for i in 0..d.lines.len() { assert_eq!(f.line_text(i), d.line_text(i)) }
        assert_eq!(f.hunks[0].lines, d.hunks[0].lines);
        assert_eq!((f.additions, f.deletions), (1, 1));
        let syn = (highlighter())(&f);
        assert_eq!(syn.len(), f.lines.len());
        // The removed line opens a block comment; the added line after it is still code (its own side's state).
        let added = f.lines.iter().position(|l| l.kind == diff::LineKind::Added).unwrap();
        assert!(syn[added].iter().any(|s| s.kind == 0 && &f.line_text(added)[s.range.start as usize..s.range.end as usize] == "let"));
        assert_ne!(file_diff(&d).uid, f.uid);
    }
}
