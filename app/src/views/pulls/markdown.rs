//! Block-level Markdown for review comments and bot reports (Conversation.swift's MarkdownText): headings,
//! paragraphs, lists with checkboxes, quotes, fenced code (highlighted, at most 30 lines), tables and rules. HTML tags
//! are dropped and their text kept; a `<summary>` reads as a bold line. Inline: `code`, **bold**, *italic*,
//! ~~strike~~ and [links](…). Parsed blocks are cached per text, so a re-render doesn't parse again.
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, IntoElement, ParentElement as _, SharedString, StrikethroughStyle,
    Styled as _, StyledText, UnderlineStyle, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(String),
    Heading(usize, String),
    List(Vec<(usize, String, String)>), // indent, marker, text
    Quote(String),
    Code(String, String),               // language, text
    Table(Vec<Vec<String>>),
    Rule,
}

fn trim(s: &str) -> &str { s.trim_matches(|c: char| c == ' ' || c == '\t') }

/// A table's lines as cells, without the |---|:--:| line under the header. `\|` is a pipe inside a cell.
pub fn rows(lines: &[String]) -> Vec<Vec<String>> {
    lines.iter().map(|line| {
        let mut l = trim(line);
        if let Some(r) = l.strip_prefix('|') { l = r }
        if l.ends_with('|') && !l.ends_with("\\|") { l = &l[..l.len() - 1] }
        let (mut cells, mut cur, mut escaped) = (vec![], String::new(), false);
        for ch in l.chars() {
            if escaped { if ch != '|' { cur.push('\\') } cur.push(ch); escaped = false }
            else if ch == '\\' { escaped = true }
            else if ch == '|' { cells.push(trim(&cur).to_string()); cur.clear() }
            else { cur.push(ch) }
        }
        cells.push(trim(&cur).to_string());
        cells
    })
    .filter(|row| !row.iter().all(|c| { let c = c.trim_matches(':'); !c.is_empty() && c.chars().all(|x| x == '-') }))
    .collect()
}

fn strip_tags(s: &str) -> String {
    let (mut out, mut in_tag) = (String::new(), false);
    for c in s.chars() {
        match c { '<' => in_tag = true, '>' if in_tag => in_tag = false, _ if !in_tag => out.push(c), _ => {} }
    }
    out
}

/// `soft_breaks`: a Markdown file (a line break inside a paragraph is a space); comments keep their line breaks.
pub fn parse(text: &str, soft_breaks: bool) -> Vec<Block> {
    let mut out = vec![];
    let (mut para, mut list, mut quote, mut table): (Vec<String>, Vec<(usize, String, String)>, Vec<String>, Vec<String>) = Default::default();
    let flush = |out: &mut Vec<Block>, para: &mut Vec<String>, list: &mut Vec<_>, quote: &mut Vec<String>, table: &mut Vec<String>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(if soft_breaks { para.iter().map(|l| trim(l)).collect::<Vec<_>>().join(" ") } else { para.join("\n") }));
            para.clear();
        }
        if !list.is_empty() { out.push(Block::List(std::mem::take(list))) }
        if !quote.is_empty() { out.push(Block::Quote(quote.join("\n"))); quote.clear() }
        if !table.is_empty() { out.push(Block::Table(rows(table))); table.clear() }
    };
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        i += 1;
        let t = trim(raw);
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut out, &mut para, &mut list, &mut quote, &mut table);
            let (fence, lang) = (&t[..3], trim(&t[3..]).to_string());
            let mut code = vec![];
            while i < lines.len() && !trim(lines[i]).starts_with(fence) { code.push(lines[i]); i += 1 }
            i += 1;
            out.push(Block::Code(lang, code.join("\n")));
            continue;
        }
        let mut line = raw.to_string();
        if t.starts_with('<') {
            let stripped = trim(&strip_tags(t)).to_string();
            if stripped.is_empty() { continue }
            if t.to_lowercase().starts_with("<summary") {
                flush(&mut out, &mut para, &mut list, &mut quote, &mut table);
                out.push(Block::Heading(4, stripped));
                continue;
            }
            line = stripped;
        }
        let s = trim(&line).to_string();
        if s.is_empty() { flush(&mut out, &mut para, &mut list, &mut quote, &mut table); continue }
        if s.starts_with('|') {
            if table.is_empty() { flush(&mut out, &mut para, &mut list, &mut quote, &mut table) }
            table.push(s);
            continue;
        }
        if s == "---" || s == "***" || s == "___" { flush(&mut out, &mut para, &mut list, &mut quote, &mut table); out.push(Block::Rule); continue }
        let hashes = s.chars().take_while(|&c| c == '#').count();
        if hashes > 0 && hashes <= 6 && s[hashes..].starts_with(' ') {
            flush(&mut out, &mut para, &mut list, &mut quote, &mut table);
            out.push(Block::Heading(hashes, trim(&s[hashes..]).to_string()));
            continue;
        }
        if let Some(q) = s.strip_prefix('>') {
            if quote.is_empty() { flush(&mut out, &mut para, &mut list, &mut quote, &mut table) }
            quote.push(trim(q).to_string());
            continue;
        }
        let indent = line.chars().take_while(|&c| c == ' ').count() / 2;
        if let Some((marker, body)) = list_item(&s) {
            if list.is_empty() { flush(&mut out, &mut para, &mut list, &mut quote, &mut table) }
            let body = if let Some(b) = body.strip_prefix("[ ] ") { format!("☐ {b}") }
                       else if body.len() >= 4 && body[..4].eq_ignore_ascii_case("[x] ") { format!("☑ {}", &body[4..]) }
                       else { body.to_string() };
            let marker = if ["-", "*", "+"].contains(&marker) { "•".to_string() } else { marker.to_string() };
            list.push((indent, marker, body));
            continue;
        }
        if !list.is_empty() && raw.starts_with("  ") { let last = list.len() - 1; list[last].2 += &format!(" {s}"); continue }
        if !list.is_empty() || !quote.is_empty() || !table.is_empty() { flush(&mut out, &mut para, &mut list, &mut quote, &mut table) }
        para.push(line);
    }
    flush(&mut out, &mut para, &mut list, &mut quote, &mut table);
    out
}

/// "- x", "* x", "+ x", "1. x", "2) x".
fn list_item(s: &str) -> Option<(&str, &str)> {
    let end = if s.starts_with(['-', '*', '+']) { 1 } else {
        let d = s.chars().take_while(char::is_ascii_digit).count();
        if d == 0 || !s[d..].starts_with(['.', ')']) { return None }
        d + 1
    };
    let rest = &s[end..];
    let body = rest.trim_start();
    (rest.len() > body.len()).then(|| (&s[..end], body))
}

/// Inline Markdown → the plain text and its styled ranges (byte ranges into the text).
pub fn inline(s: &str) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let (mut out, mut hl) = (String::new(), vec![]);
    let b = s.as_bytes();
    let mut i = 0;
    let style = |f: &dyn Fn(&mut HighlightStyle)| { let mut h = HighlightStyle::default(); f(&mut h); h };
    while i < b.len() {
        let rest = &s[i..];
        // `code`
        if rest.starts_with('`') && let Some(end) = rest[1..].find('`') {
            let start = out.len();
            out.push_str(&rest[1..1 + end]);
            hl.push((start..out.len(), style(&|h| { h.color = Some(theme::accent()); h.background_color = Some(theme::raised()) })));
            i += end + 2;
            continue;
        }
        // **bold** / __bold__ / ~~strike~~
        let pair = [("**", false), ("__", false), ("~~", true)].into_iter()
            .find_map(|(mark, strike)| rest.strip_prefix(mark).and_then(|r| r.find(mark)).filter(|&e| e > 0).map(|e| (e, strike)));
        if let Some((end, strike)) = pair {
            // ponytail: no nesting inside bold (gpui highlight runs mustn't overlap); merge styles if it's missed
            let (inner, _) = inline(&rest[2..2 + end]);
            let start = out.len();
            out.push_str(&inner);
            hl.push((start..out.len(), style(&|h| if strike { h.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None }) }
                                                  else { h.font_weight = Some(FontWeight::BOLD) })));
            i += end + 4;
            continue;
        }
        // *italic* / _italic_ (not inside a word for _)
        if (rest.starts_with('*') || (rest.starts_with('_') && (i == 0 || !b[i - 1].is_ascii_alphanumeric())))
            && let Some(end) = rest[1..].find(&rest[..1]).filter(|&e| e > 0 && !rest[1..].starts_with(' ')) {
            let start = out.len();
            out.push_str(&rest[1..1 + end]);
            hl.push((start..out.len(), style(&|h| h.font_style = Some(FontStyle::Italic))));
            i += end + 2;
            continue;
        }
        // ![alt](url): the alt text
        if rest.starts_with("![") { i += 1; continue }
        // [text](url)
        if rest.starts_with('[') && let Some(close) = rest.find("](") && let Some(paren) = rest[close..].find(')') {
            let start = out.len();
            out.push_str(&rest[1..close]);
            hl.push((start..out.len(), style(&|h| { h.color = Some(theme::blue()); h.underline = Some(UnderlineStyle { thickness: px(1.), color: None, wavy: false }) })));
            i += close + paren + 1;
            continue;
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    (out, hl)
}

thread_local! { static CACHE: RefCell<HashMap<(bool, String), Rc<Vec<Block>>>> = RefCell::new(HashMap::new()); }

fn blocks(text: &str, document: bool) -> Rc<Vec<Block>> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 500 { c.clear() } // ponytail: wholesale clear, an LRU if a long session thrashes it
        c.entry((document, text.to_string())).or_insert_with(|| Rc::new(parse(text, document))).clone()
    })
}

fn styled(s: &str) -> StyledText {
    let (t, hl) = inline(s);
    StyledText::new(SharedString::from(t)).with_highlights(hl)
}

/// A code block's lines with the code theme's token colours.
fn code_block(lang: &str, text: &str) -> AnyElement {
    use loupe_code::syntax::{Language, State, tokenize};
    let colors = loupe_code::theme::CodeTheme::named(
        &loupe_forge::settings::shared().string(loupe_code::theme::EditorSettings::KEY_DARK_THEME).unwrap_or_default(), true).tokens;
    let lang = Language::for_path(&format!("x.{}", if lang.is_empty() { "txt" } else { lang }));
    let mut st = State::default();
    let lines: Vec<&str> = text.split('\n').collect();
    let shown = lines.len().min(30);
    div().flex().flex_col().w_full().px_3().py_2().rounded(px(6.)).bg(theme::card()).border_1().border_color(theme::border())
        .font_family(theme::CODE_FONT).text_size(px(12.)).overflow_hidden()
        .children(lines[..shown].iter().map(|l| {
            let hl: Vec<(Range<usize>, HighlightStyle)> = lang.map(|g| tokenize(l, g, &mut st)).unwrap_or_default().into_iter()
                .filter(|s| (s.start + s.len) as usize <= l.len())
                .map(|s| (s.start as usize..(s.start + s.len) as usize, HighlightStyle { color: Some(rgb(colors[s.kind as usize % 9]).into()), ..Default::default() }))
                .collect();
            div().whitespace_nowrap().min_h(px(17.)).child(StyledText::new(SharedString::from(l.to_string())).with_highlights(hl))
        }))
        .when(lines.len() > shown, |d| d.child(div().text_color(theme::text_faint()).child(format!("… {} more lines", lines.len() - shown))))
        .into_any_element()
}

/// The rendered text: comment mode keeps single line breaks; `document` is a Markdown file.
pub fn render(text: &str, document: bool) -> AnyElement {
    let bs = blocks(text, document);
    div().flex().flex_col().gap_2().w_full().min_w_0().text_size(px(13.)).text_color(theme::text())
        .children(bs.iter().map(|b| match b {
            Block::Paragraph(s) => div().w_full().child(styled(s)).into_any_element(),
            Block::Heading(l, s) => div().pt_0p5().font_weight(FontWeight::SEMIBOLD)
                .text_size(px(match (document, l) { (true, 1) => 22., (true, 2) => 18., (_, 1) => 16., (_, 2) => 14.5, _ => 13.5 }))
                .child(styled(s)).into_any_element(),
            Block::List(items) => div().flex().flex_col().gap_0p5().children(items.iter().map(|(indent, m, t)| {
                div().flex().gap_1p5().pl(px(*indent as f32 * 14.))
                    .child(div().min_w(px(12.)).text_right().text_color(theme::text_muted()).child(m.clone()))
                    .child(div().flex_1().min_w_0().child(styled(t)))
            })).into_any_element(),
            Block::Quote(s) => div().pl_2p5().border_l_2().border_color(theme::border_strong()).text_color(theme::text_muted())
                .child(styled(s)).into_any_element(),
            Block::Code(lang, s) => code_block(lang, s),
            Block::Table(rows) => {
                let width = rows.iter().map(Vec::len).max().unwrap_or(0);
                div().flex().flex_col().w_full().rounded(px(4.)).border_1().border_color(theme::border()).overflow_hidden()
                    .children(rows.iter().enumerate().map(|(i, r)| div().flex().w_full().when(i > 0, |d| d.border_t_1().border_color(theme::border()))
                        .children((0..width).map(|k| div().flex_1().min_w_0().px_2().py_1()
                            .when(i == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(styled(r.get(k).map_or("", String::as_str)))))))
                    .into_any_element()
            }
            Block::Rule => crate::ui::divider().into_any_element(),
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_like_the_swift_app() {
        let md = "# Title\n\nline one\nline two\n\n- [ ] todo\n- [x] done\n1. first\n\n> quoted\n\n```rust\nfn a() {}\n```\n\n| a | b \\| c |\n|---|:-:|\n| 1 | 2 |\n\n---\n<details>\n<summary>More</summary>\n\n<b>bold</b>";
        let b = parse(md, false);
        assert_eq!(b[0], Block::Heading(1, "Title".into()));
        assert_eq!(b[1], Block::Paragraph("line one\nline two".into()));
        assert_eq!(b[2], Block::List(vec![(0, "•".into(), "☐ todo".into()), (0, "•".into(), "☑ done".into()), (0, "1.".into(), "first".into())]));
        assert_eq!(b[3], Block::Quote("quoted".into()));
        assert_eq!(b[4], Block::Code("rust".into(), "fn a() {}".into()));
        assert_eq!(b[5], Block::Table(vec![vec!["a".into(), "b | c".into()], vec!["1".into(), "2".into()]]));
        assert_eq!(b[6], Block::Rule);
        assert_eq!(b[7], Block::Heading(4, "More".into()));
        assert_eq!(b[8], Block::Paragraph("bold".into()));
        assert_eq!(parse("a\nb", true), vec![Block::Paragraph("a b".into())]);
    }

    #[test]
    fn inline_styles() {
        let (t, h) = inline("use `x` and **bold** or [link](http://a)");
        assert_eq!(t, "use x and bold or link");
        assert_eq!(h.len(), 3);
        assert_eq!(&t[h[0].0.clone()], "x");
        assert_eq!(&t[h[1].0.clone()], "bold");
        assert_eq!(inline("snake_case_name").0, "snake_case_name");
        assert_eq!(inline("a *b* c").0, "a b c");
    }
}
