//! Hand tokenizer vs tree-sitter-highlight on the same 150k-line Rust diff (the Swift app's benchmark: 0.44 s).
//! `cargo test -p loupe-code --release --test highlight_bench -- --ignored --nocapture`

use loupe_code::syntax::{self, LineKind};
use std::time::Instant;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[test]
#[ignore]
fn hand_tokenizer_vs_tree_sitter_highlight() {
    let lines: Vec<String> = (0..150_000).map(|i| format!("    let value_{i} = compute({i}, \"x\"); // note")).collect();

    let t = Instant::now();
    let spans = syntax::highlight(lines.iter().map(|l| (LineKind::Added, l.as_str())), &syntax::RUST);
    let hand = t.elapsed();
    assert_eq!(spans.len(), lines.len());

    let names = ["keyword", "string", "comment", "number", "type", "function", "macro", "attribute", "property"];
    let mut config = HighlightConfiguration::new(tree_sitter_rust::LANGUAGE.into(), "rust", tree_sitter_rust::HIGHLIGHTS_QUERY, "", "").unwrap();
    config.configure(&names);
    let mut h = Highlighter::new();
    // Best case for tree-sitter: the side as one text, parsed once (a diff's lines, joined per side).
    let text = lines.join("\n");
    let t = Instant::now();
    let mut n = 0;
    for e in h.highlight(&config, text.as_bytes(), None, |_| None).unwrap() {
        if let HighlightEvent::HighlightStart(_) = e.unwrap() {
            n += 1;
        }
    }
    let ts = t.elapsed();
    println!("hand tokenizer: {:?} ({} spans); tree-sitter-highlight: {ts:?} ({n} spans)", hand, spans.iter().map(Vec::len).sum::<usize>());
}
