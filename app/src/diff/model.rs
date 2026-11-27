//! What the diff renderer shows, and the pure logic behind it (rows, intra-line spans, change marks, search, word
//! ranges). No gpui here, so all of it is unit-tested. The integrator adapts loupe-core's parser output to these
//! types; `FileDiff::parse_patch` exists for the demo and the tests.
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind { Context, Added, Removed, Hunk, Meta }

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: Range<u32>,           // into `FileDiff::text`, without the leading +/-/space
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub hunk: u32,                  // index into `FileDiff::hunks`
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub header: String,             // "@@ -a,b +c,d @@ ..."
    pub lines: Range<usize>,        // indices into `FileDiff::lines`, the header row excluded
}

/// One file's diff. `uid` is the cheap identity: a reloaded file gets a new one, and the highlight cache is keyed on it.
#[derive(Clone, Debug, Default)]
pub struct FileDiff {
    pub uid: u64,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub text: String,               // every line's text, back to back: one allocation for a 150k-line file
    pub lines: Vec<DiffLine>,       // flat, hunk header rows included: this is what the table renders
    pub hunks: Vec<Hunk>,
    pub is_binary: bool,
    pub additions: u32,
    pub deletions: u32,
}

pub fn new_uid() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl FileDiff {
    pub fn new(old_path: Option<String>, new_path: Option<String>) -> Self {
        Self { uid: new_uid(), old_path, new_path, ..Default::default() }
    }

    pub fn path(&self) -> &str { self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("?") }

    pub fn line_text(&self, i: usize) -> &str {
        let r = &self.lines[i].text;
        &self.text[r.start as usize..r.end as usize]
    }

    /// Appends a line; a `Hunk` line opens a new hunk whose lines are the ones pushed after it.
    pub fn push(&mut self, kind: LineKind, text: &str, old_line: Option<u32>, new_line: Option<u32>) {
        let start = self.text.len() as u32;
        self.text.push_str(text);
        if kind == LineKind::Hunk {
            let at = self.lines.len() + 1;
            self.hunks.push(Hunk { header: text.to_string(), lines: at..at });
        }
        let hunk = self.hunks.len().saturating_sub(1) as u32;
        match kind {
            LineKind::Added => self.additions += 1,
            LineKind::Removed => self.deletions += 1,
            _ => {}
        }
        self.lines.push(DiffLine { kind, text: start..self.text.len() as u32, old_line, new_line, hunk });
        if kind != LineKind::Hunk && let Some(h) = self.hunks.last_mut() { h.lines.end = self.lines.len() }
    }

    /// The hunks of one file's unified diff (anything before the first "@@" is ignored, "Binary files" marks it
    /// binary). For the demo and tests; the app parses with loupe-core.
    pub fn parse_patch(old_path: Option<String>, new_path: Option<String>, patch: &str) -> Self {
        let mut f = Self::new(old_path, new_path);
        let (mut o, mut n) = (0u32, 0u32);
        let mut in_hunk = false;
        for l in patch.lines() {
            if l.starts_with("@@") {
                let (a, b) = parse_hunk_header(l);
                (o, n) = (a, b);
                in_hunk = true;
                f.push(LineKind::Hunk, l, None, None);
            } else if !in_hunk {
                if l.starts_with("Binary files ") || l.starts_with("GIT binary patch") { f.is_binary = true }
            } else if let Some(t) = l.strip_prefix('+') {
                f.push(LineKind::Added, t, None, Some(n)); n += 1;
            } else if let Some(t) = l.strip_prefix('-') {
                f.push(LineKind::Removed, t, Some(o), None); o += 1;
            } else if l.starts_with('\\') {
                f.push(LineKind::Meta, l, None, None);
            } else {
                f.push(LineKind::Context, l.strip_prefix(' ').unwrap_or(l), Some(o), Some(n)); o += 1; n += 1;
            }
        }
        f
    }
}

/// "@@ -a,b +c,d @@" → (a, c).
pub fn parse_hunk_header(h: &str) -> (u32, u32) {
    let mut it = h.split(' ').skip(1);
    let num = |s: Option<&str>| s.and_then(|s| s[1..].split(',').next()?.parse().ok()).unwrap_or(0);
    (num(it.next()), num(it.next()))
}

/// A syntax token: a byte range in a line's text and a kind index (0 keyword, 1 string, 2 comment, 3 number,
/// 4 type, 5 function, 6 macro, 7 attribute, 8 property: Swift's `TokenKind`).
#[derive(Clone, Debug, PartialEq)]
pub struct Span { pub range: Range<u32>, pub kind: u8 }
pub type Syntax = Vec<Vec<Span>>;   // per line of `FileDiff::lines`

/// Anything rendered as an inline comment under a diff line.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineComment {
    pub author: String,
    pub body: String,
    pub new_line: Option<u32>,
    pub old_line: Option<u32>,
    pub draft_id: Option<u64>,      // Some → a pending draft, deletable
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkAction { Stage, Unstage, Discard }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeAction { Definition, References }

/// Both sides with the changes marked, or one side alone (the file before, or after).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DiffSide { #[default] Diff, Old, New }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Layout { #[default] Unified, Split }

/// Blame beside each new-side line. `shas[n - 1]` is the commit of new line `n`; all zeroes → not committed.
#[derive(Clone, Debug, Default)]
pub struct Blame {
    pub shas: Vec<String>,
    pub commits: HashMap<String, BlameCommit>,
}
#[derive(Clone, Debug)]
pub struct BlameCommit { pub author: String, pub age: String }   // age already formatted ("3 d")

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Line(usize),                          // index into file.lines
    Hunk(usize, usize),                   // (line index, hunk index)
    Comment(usize),                       // index into the comments
    Composer(usize),                      // the comment being written under this line
    Split(Option<usize>, Option<usize>),  // old side, new side
}

/// The rows a file shows. Comments sit under their line; those whose line isn't in the diff any more (outdated) go
/// to the end rather than vanish. One side alone leaves out the other side's lines and their comments, which aren't
/// outdated, and the hunk headers.
pub fn build_rows(f: &FileDiff, side: DiffSide, layout: Layout, comments: &[InlineComment], composer: Option<usize>) -> Vec<Row> {
    let mut by_new: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut by_old: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, c) in comments.iter().enumerate() {
        match (c.new_line, c.old_line) {
            (Some(n), _) if n > 0 => by_new.entry(n).or_default().push(k),
            (_, Some(o)) => by_old.entry(o).or_default().push(k),
            _ => {}
        }
    }
    let mut comments_for = |i: usize, out: &mut Vec<Row>| {
        let l = &f.lines[i];
        let mut take = |m: &mut HashMap<u32, Vec<usize>>, k: Option<u32>| {
            if let Some(v) = k.and_then(|k| m.remove(&k)) { out.extend(v.into_iter().map(Row::Comment)) }
        };
        match l.kind {
            LineKind::Added => take(&mut by_new, l.new_line),
            LineKind::Removed => take(&mut by_old, l.old_line),
            LineKind::Context => { take(&mut by_new, l.new_line); take(&mut by_old, l.old_line) }
            _ => {}
        }
        if composer == Some(i) { out.push(Row::Composer(i)) }
    };

    let split = layout == Layout::Split && side == DiffSide::Diff;
    let mut out = Vec::with_capacity(f.lines.len() + comments.len());
    let mut i = 0;
    while i < f.lines.len() {
        let l = &f.lines[i];
        if l.kind == LineKind::Hunk {
            if side == DiffSide::Diff { out.push(Row::Hunk(i, l.hunk as usize)) }
            i += 1;
            continue;
        }
        if side == DiffSide::Old && l.kind == LineKind::Added || side == DiffSide::New && l.kind == LineKind::Removed {
            i += 1;
            continue;
        }
        if split && matches!(l.kind, LineKind::Removed | LineKind::Added) {
            // A run of removals then additions pairs up line by line; the shorter side pads with blanks.
            let dels: Vec<usize> = (i..f.lines.len()).take_while(|&j| f.lines[j].kind == LineKind::Removed).collect();
            let j = i + dels.len();
            let adds: Vec<usize> = (j..f.lines.len()).take_while(|&k| f.lines[k].kind == LineKind::Added).collect();
            for k in 0..dels.len().max(adds.len()) {
                let (a, b) = (dels.get(k).copied(), adds.get(k).copied());
                out.push(Row::Split(a, b));
                if let Some(a) = a { comments_for(a, &mut out) }
                if let Some(b) = b { comments_for(b, &mut out) }
            }
            i = j + adds.len();
            continue;
        }
        out.push(if split && l.kind == LineKind::Context { Row::Split(Some(i), Some(i)) } else { Row::Line(i) });
        comments_for(i, &mut out);
        i += 1;
    }
    if side == DiffSide::Diff {
        let mut rest: Vec<usize> = by_new.into_values().chain(by_old.into_values()).flatten().collect();
        rest.sort_unstable();
        out.extend(rest.into_iter().map(Row::Comment));
    }
    out
}

/// The diff line a row shows: in split, the new side when there is one (`right`), else the old.
pub fn row_line(row: Row, right: bool) -> Option<usize> {
    match row {
        Row::Line(i) => Some(i),
        Row::Split(a, b) => if right { b.or(a) } else { a.or(b) },
        _ => None,
    }
}

pub type Highlights = HashMap<usize, Range<usize>>;   // line index → changed byte range

/// For each removed/added run pair, the span between the common prefix and suffix. Only 1:1 edits of ≤ 50 lines,
/// lines under 1000 bytes; a whole-line change gets none, it would only add noise.
pub fn intra_line_highlights(f: &FileDiff) -> Highlights {
    let mut out = HashMap::new();
    let mut i = 0;
    let n = f.lines.len();
    while i < n {
        if f.lines[i].kind != LineKind::Removed { i += 1; continue }
        let d0 = i;
        while i < n && f.lines[i].kind == LineKind::Removed { i += 1 }
        let a0 = i;
        while i < n && f.lines[i].kind == LineKind::Added { i += 1 }
        let (dels, adds) = (a0 - d0, i - a0);
        if dels != adds || dels > 50 { continue }
        for k in 0..dels {
            let (d, a) = (d0 + k, a0 + k);
            let (x, y) = (f.line_text(d), f.line_text(a));
            if x.len() >= 1000 || y.len() >= 1000 { continue }
            let (xb, yb) = (x.as_bytes(), y.as_bytes());
            let mut p = xb.iter().zip(yb).take_while(|(a, b)| a == b).count();
            while !x.is_char_boundary(p) || !y.is_char_boundary(p) { p -= 1 }
            let max_s = (x.len() - p).min(y.len() - p);
            let mut s = xb.iter().rev().zip(yb.iter().rev()).take(max_s).take_while(|(a, b)| a == b).count();
            while !x.is_char_boundary(x.len() - s) || !y.is_char_boundary(y.len() - s) { s -= 1 }
            if p == 0 && s == 0 { continue }
            if x.len() - p - s > 0 { out.insert(d, p..x.len() - s); }
            if y.len() - p - s > 0 { out.insert(a, p..y.len() - s); }
        }
    }
    out
}

/// Lines longer than this are cut on screen (minified files); copy still gets the full text.
pub const MAX_DRAWN_CHARS: usize = 1500;

/// A line as drawn: tabs as four spaces, cut at `MAX_DRAWN_CHARS`. Borrowed when there's nothing to change.
pub fn display_text(t: &str) -> std::borrow::Cow<'_, str> {
    let cut = t.char_indices().nth(MAX_DRAWN_CHARS).map_or(t, |(b, _)| &t[..b]);
    if cut.contains('\t') { cut.replace('\t', "    ").into() } else { cut.into() }
}

/// A byte offset in a line's text, as an offset in its `display_text`.
pub fn display_offset(raw: &str, display_len: usize, byte: usize) -> usize {
    let tabs = raw.as_bytes()[..byte.min(raw.len())].iter().filter(|&&b| b == b'\t').count();
    (byte + tabs * 3).min(display_len)
}

/// Char column → byte offset in `s` (clamped).
pub fn col_to_byte(s: &str, col: usize) -> usize { s.char_indices().nth(col).map_or(s.len(), |(b, _)| b) }
pub fn byte_to_col(s: &str, byte: usize) -> usize { s[..byte.min(s.len())].chars().count() }

/// The identifier around a column, as a column range (empty when there's none). Left of the text the column is
/// negative: no word there.
pub fn word_range(chars: &[char], col: isize) -> Range<usize> {
    if col < 0 { return 0..0 }
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii();
    let mut a = (col as usize).min(chars.len());
    let mut b = a;
    if a < chars.len() && (is_word(chars[a]) || (a > 0 && is_word(chars[a - 1]))) {
        if a == chars.len() || !is_word(chars[a]) { a -= 1; b = a }
        while a > 0 && is_word(chars[a - 1]) { a -= 1 }
        while b < chars.len() && is_word(chars[b]) { b += 1 }
    }
    a..b
}

/// A run of changed rows, for the change ruler. Comments inside a run don't break it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark { pub rows: Range<usize>, pub added: u32, pub removed: u32, pub line: u32 }

pub fn change_marks(f: &FileDiff, rows: &[Row], side: DiffSide) -> Vec<Mark> {
    if side != DiffSide::Diff { return vec![] }   // Before / After are plain code
    let mut out: Vec<Mark> = vec![];
    for (r, row) in rows.iter().enumerate() {
        let lines: [Option<usize>; 2] = match *row { Row::Line(i) => [Some(i), None], Row::Split(a, b) => [a, b], _ => continue };
        let (mut added, mut removed) = (0, 0);
        for i in lines.into_iter().flatten() {
            match f.lines[i].kind { LineKind::Added => added += 1, LineKind::Removed => removed += 1, _ => {} }
        }
        if added + removed == 0 { continue }
        if let Some(last) = out.last_mut() && rows[last.rows.end..r].iter().all(|x| matches!(x, Row::Comment(_) | Row::Composer(_))) {
            last.rows.end = r + 1;
            last.added += added;
            last.removed += removed;
        } else {
            let l = &f.lines[lines.into_iter().flatten().next().unwrap()];
            out.push(Mark { rows: r..r + 1, added, removed, line: l.new_line.or(l.old_line).unwrap_or(0) });
        }
    }
    out
}

fn is_change(f: &FileDiff, row: Row) -> bool {
    match row {
        Row::Hunk(..) => true,
        Row::Line(i) => matches!(f.lines[i].kind, LineKind::Added | LineKind::Removed),
        Row::Split(a, b) => [a, b].into_iter().flatten().any(|i| matches!(f.lines[i].kind, LineKind::Added | LineKind::Removed)),
        _ => false,
    }
}

/// n / p: the next or previous change from `top` (the first row in view): a hunk header, or in a whole-file view the
/// first row of a run of changes. Returns the row to put at the top: a hunk header itself, or a run `margin` rows up
/// so the context above it shows.
pub fn jump_target(f: &FileDiff, rows: &[Row], top: usize, forward: bool) -> Option<usize> {
    const MARGIN: usize = 3;
    let starts = |r: usize| {
        is_change(f, rows[r]) && (matches!(rows[r], Row::Hunk(..)) || r == 0 || !is_change(f, rows[r - 1]))
    };
    let start = top + MARGIN;
    let found = if forward { (start + 1..rows.len()).find(|&r| starts(r)) } else { (0..start.min(rows.len())).rev().find(|&r| starts(r)) };
    found.map(|r| if matches!(rows[r], Row::Hunk(..)) { r } else { r.saturating_sub(MARGIN) })
}

/// A find match: a row, the side (split only) and a char column range of the drawn text.
#[derive(Clone, Debug, PartialEq)]
pub struct Match { pub row: usize, pub right: bool, pub cols: Range<usize> }

/// Case-insensitive matches in the drawn text of every line row.
pub fn search(f: &FileDiff, rows: &[Row], query: &str) -> Vec<Match> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() { return vec![] }
    let mut out = vec![];
    for (r, row) in rows.iter().enumerate() {
        let sides: [(Option<usize>, bool); 2] = match *row {
            Row::Line(i) => [(Some(i), false), (None, false)],
            Row::Split(a, b) => [(if a == b { None } else { a }, false), (b, true)],
            _ => continue,
        };
        for (i, right) in sides {
            let Some(i) = i else { continue };
            // Lowercasing can change a char's count (rare); matching per char keeps columns exact otherwise.
            let t: Vec<char> = display_text(f.line_text(i)).chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect();
            let mut c = 0;
            while c + q.len() <= t.len() {
                if t[c..c + q.len()] == q[..] { out.push(Match { row: r, right, cols: c..c + q.len() }); c += q.len() } else { c += 1 }
            }
        }
    }
    out
}

/// Blame labels per row showing a new-side line: the commit, its author and age on the first line of each run of
/// lines from one commit, and a shade that alternates per run.
pub fn blame_labels(f: &FileDiff, rows: &[Row], b: &Blame) -> HashMap<usize, (Option<String>, bool)> {
    let mut out = HashMap::new();
    let (mut last, mut odd): (Option<&str>, bool) = (None, false);
    for (r, row) in rows.iter().enumerate() {
        let Some(i) = row_line(*row, true) else { continue };
        let l = &f.lines[i];
        let Some(n) = l.new_line.filter(|_| l.kind != LineKind::Removed) else { continue };
        let Some(sha) = b.shas.get(n as usize - 1) else { continue };
        if last == Some(sha.as_str()) { out.insert(r, (None, odd)); continue }
        if last.is_some() { odd = !odd }
        last = Some(sha);
        if sha.bytes().all(|c| c == b'0') { out.insert(r, (Some("Not committed".into()), odd)); continue }
        let c = b.commits.get(sha);
        let label = format!("{}  {}  · {}", &sha[..sha.len().min(7)], c.map_or("", |c| &c.author), c.map_or("", |c| &c.age));
        out.insert(r, (Some(label), odd));
    }
    out
}

/// A Git LFS pointer (spec v1: `version`, then sorted keys, `oid` and `size` among them).
#[derive(Clone, Debug, PartialEq)]
pub struct LfsPointer { pub oid: String, pub size: u64 }

impl LfsPointer {
    pub fn parse<'a>(lines: &[&'a str]) -> Option<Self> {
        let v = lines.first()?;
        if !(v.starts_with("version https://git-lfs.github.com/spec/") || v.starts_with("version https://hawser.github.com/spec/")) { return None }
        let (mut oid, mut size) = (None, None);
        for l in &lines[1..] {
            if let Some(o) = l.strip_prefix("oid sha256:") { oid = Some(o.to_string()) }
            else if let Some(s) = l.strip_prefix("size ") { size = s.parse().ok() }
        }
        let oid = oid.filter(|o| o.len() == 64)?;
        Some(Self { oid, size: size? })
    }
}

/// What shows instead of lines.
#[derive(Clone, Debug, PartialEq)]
pub enum Placeholder {
    Binary,
    NoContent,                                          // a mode change or rename only
    Lfs { old: Option<LfsPointer>, new: Option<LfsPointer> },
}

/// Binary, an LFS pointer change (a handful of lines on each side), or nothing to show.
pub fn placeholder(f: &FileDiff) -> Option<Placeholder> {
    if f.is_binary { return Some(Placeholder::Binary) }
    if f.lines.is_empty() { return Some(Placeholder::NoContent) }
    if f.lines.len() >= 24 { return None }
    let (mut a, mut b) = (vec![], vec![]);
    for (i, l) in f.lines.iter().enumerate() {
        match l.kind {
            LineKind::Context => { a.push(f.line_text(i)); b.push(f.line_text(i)) }
            LineKind::Removed => a.push(f.line_text(i)),
            LineKind::Added => b.push(f.line_text(i)),
            _ => {}
        }
    }
    let (o, n) = (LfsPointer::parse(&a), LfsPointer::parse(&b));
    if (o.is_none() && n.is_none()) || (o.is_none() && !a.is_empty()) || (n.is_none() && !b.is_empty()) { return None }
    Some(Placeholder::Lfs { old: o, new: n })
}

pub const IMAGE_TYPES: &[&str] = &["png", "jpg", "jpeg", "gif", "tif", "tiff", "bmp", "heic", "webp", "psd", "ico", "icns"];
pub fn is_image(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| IMAGE_TYPES.contains(&e.to_ascii_lowercase().as_str()))
}

/// "1.2 MB", as Finder counts (powers of 1000).
pub fn format_bytes(n: u64) -> String {
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000. && u < units.len() - 1 { v /= 1000.; u += 1 }
    // ByteCountFormatter's .file style: whole KB, one decimal for MB, two above.
    match u {
        0 if n == 0 => "Zero KB".into(),
        0 if n == 1 => "1 byte".into(),
        0 => format!("{n} bytes"),
        1 => format!("{v:.0} KB"),
        2 => format!("{v:.1} MB"),
        _ => format!("{v:.2} {}", units[u]),
    }
}

/// Last-three-files cache of the derived passes (intra-line, syntax), by file uid. Switching Changes / History /
/// Pull Requests builds a new view for the same file: without this every switch redid both passes (commit 1d10383).
pub mod cache {
    use super::{Highlights, Syntax};
    use std::cell::RefCell;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    pub struct Derived { pub highlights: Option<Arc<Highlights>>, pub syntax: Option<Arc<Syntax>> }

    thread_local! { static DERIVED: RefCell<Vec<(u64, Derived)>> = const { RefCell::new(Vec::new()) }; }

    pub fn get(uid: u64) -> Derived {
        DERIVED.with_borrow(|d| d.iter().find(|e| e.0 == uid).map(|e| e.1.clone()).unwrap_or_default())
    }

    pub fn remember(uid: u64, highlights: Option<Arc<Highlights>>, syntax: Option<Arc<Syntax>>) {
        DERIVED.with_borrow_mut(|d| {
            let mut e = d.iter().position(|e| e.0 == uid).map(|p| d.remove(p).1).unwrap_or_default();
            if highlights.is_some() { e.highlights = highlights }
            if syntax.is_some() { e.syntax = syntax }
            d.push((uid, e));
            if d.len() > 3 { d.remove(0); }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "@@ -1,4 +1,4 @@ fn main\n a\n-let x = 1;\n+let x = 2;\n c\n-gone\n@@ -10,2 +10,3 @@\n k\n+n1\n+n2\n\\ No newline at end of file\n";

    fn file() -> FileDiff { FileDiff::parse_patch(Some("f.rs".into()), Some("f.rs".into()), PATCH) }

    #[test]
    fn parses_numbers_and_hunks() {
        let f = file();
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(f.hunks[0].lines, 1..6);
        assert_eq!(f.hunks[1].lines, 7..11);
        assert_eq!((f.additions, f.deletions), (3, 2));
        assert_eq!(f.lines[3].new_line, Some(2));
        assert_eq!(f.lines[5].old_line, Some(4));
        assert_eq!(f.lines[9].new_line, Some(12));
        assert_eq!(f.line_text(2), "let x = 1;");
        assert_eq!(parse_hunk_header("@@ -10,2 +11,3 @@ x"), (10, 11));
    }

    #[test]
    fn intra_line_marks_the_middle() {
        let h = intra_line_highlights(&file());
        assert_eq!(h.get(&2), Some(&(8..9)));
        assert_eq!(h.get(&3), Some(&(8..9)));
        assert!(!h.contains_key(&5));   // "gone": no partner
    }

    #[test]
    fn intra_line_respects_char_boundaries_and_skips_whole_line() {
        let mut f = FileDiff::new(None, Some("x".into()));
        f.push(LineKind::Hunk, "@@ -1 +1 @@", None, None);
        f.push(LineKind::Removed, "aé", Some(1), None);
        f.push(LineKind::Added, "aè", None, Some(1));
        f.push(LineKind::Removed, "abc", Some(2), None);
        f.push(LineKind::Removed, "xyz", Some(3), None);
        f.push(LineKind::Added, "def", None, Some(2));
        f.push(LineKind::Added, "uvw", None, Some(3));
        let h = intra_line_highlights(&f);
        assert_eq!(h.get(&1), Some(&(1..3)));
        assert_eq!(h.get(&2), Some(&(1..3)));
        assert!(!h.contains_key(&3) && !h.contains_key(&5));
    }

    #[test]
    fn rows_place_comments_and_outdated_at_end() {
        let f = file();
        let c = |n: Option<u32>, o: Option<u32>| InlineComment { author: "a".into(), body: "b".into(), new_line: n, old_line: o, draft_id: None };
        let comments = vec![c(Some(2), None), c(None, Some(4)), c(Some(99), None), c(Some(1), None)];
        let rows = build_rows(&f, DiffSide::Diff, Layout::Unified, &comments, Some(1));
        assert_eq!(rows[0], Row::Hunk(0, 0));
        assert_eq!(&rows[1..4], &[Row::Line(1), Row::Comment(3), Row::Composer(1)]);
        assert_eq!(&rows[5..7], &[Row::Line(3), Row::Comment(0)]);
        assert_eq!(rows[9], Row::Comment(1));
        assert_eq!(*rows.last().unwrap(), Row::Comment(2));
        // Before: no added lines, no hunk headers, no outdated tail.
        let old = build_rows(&f, DiffSide::Old, Layout::Unified, &comments, None);
        assert!(old.iter().all(|r| !matches!(r, Row::Hunk(..)) && *r != Row::Line(3)));
        assert!(!old.contains(&Row::Comment(2)));
    }

    #[test]
    fn split_pairs_runs() {
        let f = file();
        let rows = build_rows(&f, DiffSide::Diff, Layout::Split, &[], None);
        assert_eq!(&rows[..5], &[Row::Hunk(0, 0), Row::Split(Some(1), Some(1)), Row::Split(Some(2), Some(3)), Row::Split(Some(4), Some(4)), Row::Split(Some(5), None)]);
        assert_eq!(&rows[7..9], &[Row::Split(None, Some(8)), Row::Split(None, Some(9))]);
    }

    #[test]
    fn marks_join_runs_across_comments() {
        let f = file();
        let rows = vec![Row::Line(2), Row::Comment(0), Row::Line(3), Row::Line(4), Row::Line(5)];
        let m = change_marks(&f, &rows, DiffSide::Diff);
        assert_eq!(m, vec![Mark { rows: 0..3, added: 1, removed: 1, line: 2 }, Mark { rows: 4..5, added: 0, removed: 1, line: 4 }]);
        assert!(change_marks(&f, &rows, DiffSide::New).is_empty());
    }

    #[test]
    fn jumps_to_hunks_with_margin() {
        let f = file();
        let rows = build_rows(&f, DiffSide::Diff, Layout::Unified, &[], None);
        // rows: H0 L1 L2 L3 L4 L5 H6 L7 L8 L9 L10. Stops are 3 rows past the top, a run lands 3 rows above itself.
        assert_eq!(jump_target(&f, &rows, 0, true), Some(2));   // the run at L5
        assert_eq!(jump_target(&f, &rows, 2, true), Some(6));   // the second hunk header
        assert_eq!(jump_target(&f, &rows, 6, false), Some(5));  // the run at L8
        assert_eq!(jump_target(&f, &rows, 1, false), Some(0));
        let new = build_rows(&f, DiffSide::New, Layout::Unified, &[], None);   // L1 L3 L4 L7 L8 L9 L10: no headers
        assert_eq!(jump_target(&f, &new, 0, true), Some(1));
    }

    #[test]
    fn words_and_search() {
        let t: Vec<char> = "  foo_bar(x)".chars().collect();
        assert_eq!(word_range(&t, 4), 2..9);
        assert_eq!(word_range(&t, 9), 2..9);   // right after the word
        assert_eq!(word_range(&t, 0), 0..0);
        assert_eq!(word_range(&t, -1), 0..0);
        let f = file();
        let rows = build_rows(&f, DiffSide::Diff, Layout::Unified, &[], None);
        let m = search(&f, &rows, "LET");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].cols, 0..3);
    }

    #[test]
    fn display_expands_tabs() {
        assert_eq!(display_text("\tx"), "    x");
        assert_eq!(display_offset("\tx", 5, 1), 4);
        assert_eq!(col_to_byte("aéb", 2), 3);
        assert_eq!(byte_to_col("aéb", 3), 2);
    }

    #[test]
    fn blame_runs() {
        let mut f = FileDiff::new(None, Some("x".into()));
        for n in 1..=4 { f.push(LineKind::Context, "x", Some(n), Some(n)) }
        let b = Blame {
            shas: vec!["a".repeat(40), "a".repeat(40), "0".repeat(40), "b".repeat(40)],
            commits: [("a".repeat(40), BlameCommit { author: "Ann".into(), age: "2 d".into() })].into(),
        };
        let rows = build_rows(&f, DiffSide::Diff, Layout::Unified, &[], None);
        let l = blame_labels(&f, &rows, &b);
        assert_eq!(l[&0], (Some("aaaaaaa  Ann  · 2 d".into()), false));
        assert_eq!(l[&1], (None, false));
        assert_eq!(l[&2], (Some("Not committed".into()), true));
        assert_eq!(l[&3].1, false);
    }

    #[test]
    fn lfs_and_placeholders() {
        let oid = "a".repeat(64);
        let p = format!("@@ -0,0 +1,3 @@\n+version https://git-lfs.github.com/spec/v1\n+oid sha256:{oid}\n+size 1234\n");
        let f = FileDiff::parse_patch(None, Some("a.png".into()), &p);
        assert_eq!(placeholder(&f), Some(Placeholder::Lfs { old: None, new: Some(LfsPointer { oid, size: 1234 }) }));
        assert_eq!(placeholder(&file()), None);
        assert_eq!(placeholder(&FileDiff::parse_patch(None, Some("b".into()), "Binary files a and b differ")), Some(Placeholder::Binary));
        assert!(is_image("x/Y.PNG") && !is_image("x.rs"));
        assert_eq!(format_bytes(1234), "1 KB");
        assert_eq!(format_bytes(65_536), "66 KB");
        assert_eq!(format_bytes(1_234_567), "1.2 MB");
    }

    #[test]
    fn cache_keeps_three() {
        for uid in 1000..1005 { cache::remember(uid, Some(Default::default()), None) }
        assert!(cache::get(1001).highlights.is_none());
        assert!(cache::get(1004).highlights.is_some());
    }
}
