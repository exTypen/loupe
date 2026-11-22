//! Unified diff parsing (`git diff`, Forgejo `.diff`) and patches for `git apply`.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind { Context, Added, Removed, Hunk, Meta }

/// One row of a diff. Its text is a range of the file's `source`: no per-line allocation, so a 50 MB PR diff parses
/// into one string and a flat array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    start: u32,
    len: u32,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub hunk: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,     // "@@ -a,b +c,d @@ ..."
    pub lines: Range<usize>, // indices into FileDiff::lines (excluding the header row)
}

static NEXT_UID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct FileDiff {
    /// Cheap identity: equality never walks `lines`. A clone keeps it (same content).
    pub uid: u64,
    /// The text the lines point into, shared by every file of one parse.
    pub source: Arc<str>,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub header_lines: Vec<String>, // "diff --git", "index", "---", "+++" ... needed to rebuild patches
    pub lines: Vec<DiffLine>,      // flat, hunk header rows included: this is what the table renders
    pub hunks: Vec<Hunk>,
    pub is_binary: bool,
    pub copied_from: Option<String>, // `git diff -C`: a new file copied from this one; its lines are what differs
    pub similarity: Option<u32>,     // percent, for a copy or rename
    pub additions: usize,
    pub deletions: usize,
}

impl PartialEq for FileDiff {
    fn eq(&self, o: &Self) -> bool { self.uid == o.uid }
}

impl Default for FileDiff {
    fn default() -> Self { FileDiff::new(Arc::from("")) }
}

impl FileDiff {
    pub fn new(source: Arc<str>) -> Self {
        FileDiff {
            uid: NEXT_UID.fetch_add(1, Ordering::Relaxed),
            source,
            old_path: None,
            new_path: None,
            header_lines: vec![],
            lines: vec![],
            hunks: vec![],
            is_binary: false,
            copied_from: None,
            similarity: None,
            additions: 0,
            deletions: 0,
        }
    }

    pub fn path(&self) -> &str { self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("?") }

    /// A line's text, without the leading +/-/space (hunk and meta rows: the whole row).
    pub fn text(&self, l: &DiffLine) -> &str { &self.source[l.start as usize..(l.start + l.len) as usize] }

    pub fn line_text(&self, i: usize) -> &str { self.text(&self.lines[i]) }

    /// A fresh identity, for a diff whose lines were changed after the parse.
    pub fn renewed(mut self) -> Self {
        self.uid = NEXT_UID.fetch_add(1, Ordering::Relaxed);
        self
    }

    /// A standalone patch containing only hunk `i`, suitable for `git apply`.
    pub fn patch_for_hunk(&self, i: usize) -> String {
        let sel: HashSet<usize> = self.hunks[i].lines.clone().collect();
        self.patch_selecting(&sel, false).unwrap_or_default()
    }

    /// A patch with only the `selected` changed lines (indices into `lines`), the way `git add -p` edits lines.
    /// An unselected change becomes context on the side the patch is applied to and disappears from the other:
    /// forward (stage) applies to the old side, `reverse` (discard, unstage) to the new side, which must match disk.
    /// None when no selected line is a change.
    pub fn patch_selecting(&self, selected: &HashSet<usize>, reverse: bool) -> Option<String> {
        let mut out = self.header_lines.join("\n") + "\n";
        let mut any = false;
        let is_change = |i: usize| matches!(self.lines[i].kind, LineKind::Added | LineKind::Removed);
        for h in &self.hunks {
            if !h.lines.clone().any(|i| selected.contains(&i) && is_change(i)) { continue }
            let (mut body, mut old_count, mut new_count, mut kept_previous) = (String::new(), 0, 0, true);
            for i in h.lines.clone() {
                let l = &self.lines[i];
                let mark = match l.kind {
                    LineKind::Context => Some(' '),
                    LineKind::Added => if selected.contains(&i) { Some('+') } else if reverse { Some(' ') } else { None },
                    LineKind::Removed => if selected.contains(&i) { Some('-') } else if reverse { None } else { Some(' ') },
                    LineKind::Meta => {
                        // "\ No newline" belongs to the line before
                        if kept_previous { body += self.text(l); body.push('\n') }
                        continue;
                    }
                    LineKind::Hunk => continue,
                };
                kept_previous = mark.is_some();
                let Some(mark) = mark else { continue };
                body.push(mark);
                body += self.text(l);
                body.push('\n');
                if mark != '+' { old_count += 1 }
                if mark != '-' { new_count += 1 }
            }
            let (o, n) = parse_hunk_header(&h.header);
            // Keep the function context git prints after the second "@@".
            let tail = h.header.get(2..).and_then(|s| s.find("@@").map(|p| &s[p + 2..])).unwrap_or("");
            out += &format!("@@ -{o},{old_count} +{n},{new_count} @@{tail}\n");
            out += &body;
            any = true;
        }
        any.then_some(out)
    }
}

/// Parses `git diff` / Forgejo `.diff` output. Not UTF-8 → replacement characters.
pub fn parse_bytes(data: &[u8]) -> Vec<FileDiff> {
    match std::str::from_utf8(data) {
        Ok(s) => parse(s),
        Err(_) => parse(&String::from_utf8_lossy(data)),
    }
}

/// Single pass; the lines are ranges of one shared copy of `text`.
pub fn parse(text: &str) -> Vec<FileDiff> {
    let source: Arc<str> = Arc::from(text);
    let src: &str = &source;
    let base = src.as_ptr() as usize;
    let mut files: Vec<FileDiff> = vec![];
    let mut cur: Option<FileDiff> = None;
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_hunk = false;
    let mut hunk_start = 0usize;

    fn close_hunk(cur: &mut Option<FileDiff>, in_hunk: &mut bool, hunk_start: usize) {
        if !*in_hunk { return }
        if let Some(c) = cur.as_mut() {
            let n = c.lines.len();
            if let Some(h) = c.hunks.last_mut() { h.lines = hunk_start..n }
        }
        *in_hunk = false;
    }

    for line in src.split_terminator('\n') {
        let off = (line.as_ptr() as usize - base) as u32;
        let row = |kind, skip: u32, old, new, hunk| DiffLine { kind, start: off + skip, len: line.len() as u32 - skip, old_line: old, new_line: new, hunk };
        if line.starts_with("diff --git ") || line.starts_with("diff --cc ") {
            close_hunk(&mut cur, &mut in_hunk, hunk_start);
            files.extend(cur.take());
            let mut f = FileDiff::new(source.clone());
            f.header_lines.push(line.to_string());
            // Fallback paths from "diff --git a/x b/x": overwritten by ---/+++ when present.
            if let Some(p) = line.rfind(" b/") {
                f.new_path = Some(line[p + 3..].to_string());
                f.old_path = f.new_path.clone();
            }
            cur = Some(f);
            continue;
        }
        // A diff without "diff --git" (plain unified).
        let c = cur.get_or_insert_with(|| FileDiff::new(source.clone()));
        let first = line.as_bytes().first().copied();
        if in_hunk && matches!(first, Some(b' ' | b'+' | b'-' | b'\\')) {
            let h = c.hunks.len() as u32 - 1;
            match first {
                Some(b'+') => { c.lines.push(row(LineKind::Added, 1, None, Some(new_no), h)); new_no += 1; c.additions += 1 }
                Some(b'-') => { c.lines.push(row(LineKind::Removed, 1, Some(old_no), None, h)); old_no += 1; c.deletions += 1 }
                Some(b' ') => { c.lines.push(row(LineKind::Context, 1, Some(old_no), Some(new_no), h)); old_no += 1; new_no += 1 }
                _ => c.lines.push(row(LineKind::Meta, 0, None, None, h)),
            }
            continue;
        }
        if line.starts_with("@@") {
            close_hunk(&mut cur, &mut in_hunk, hunk_start);
            let c = cur.as_mut().unwrap();
            (old_no, new_no) = parse_hunk_header(line);
            c.hunks.push(Hunk { header: line.to_string(), lines: 0..0 });
            c.lines.push(row(LineKind::Hunk, 0, None, None, c.hunks.len() as u32 - 1));
            hunk_start = c.lines.len();
            in_hunk = true;
            continue;
        }
        close_hunk(&mut cur, &mut in_hunk, hunk_start);
        let c = cur.as_mut().unwrap();
        c.header_lines.push(line.to_string());
        if let Some(p) = line.strip_prefix("--- ") { c.old_path = strip_prefix(p) }
        else if let Some(p) = line.strip_prefix("+++ ") { c.new_path = strip_prefix(p) }
        else if let Some(p) = line.strip_prefix("rename from ") { c.old_path = Some(p.into()) }
        else if let Some(p) = line.strip_prefix("rename to ") { c.new_path = Some(p.into()) }
        else if let Some(p) = line.strip_prefix("copy from ") { c.copied_from = Some(p.into()); c.old_path = Some(p.into()) }
        else if let Some(p) = line.strip_prefix("copy to ") { c.new_path = Some(p.into()) }
        else if let Some(p) = line.strip_prefix("similarity index ") { c.similarity = p.strip_suffix('%').and_then(|x| x.parse().ok()) }
        else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") { c.is_binary = true }
    }
    close_hunk(&mut cur, &mut in_hunk, hunk_start);
    files.extend(cur);
    files.retain(|f| !f.header_lines.is_empty() || !f.lines.is_empty());
    files
}

fn strip_prefix(s: &str) -> Option<String> {
    let s = s.split('\t').next().unwrap_or(s); // "--- a/file\t2024-..." from --no-index
    if s == "/dev/null" { return None }
    Some(s.strip_prefix("a/").or_else(|| s.strip_prefix("b/")).unwrap_or(s).to_string())
}

/// "@@ -12,7 +12,9 @@" → (12, 12)
pub fn parse_hunk_header(line: &str) -> (u32, u32) {
    let (mut old, mut new) = (0, 0);
    for tok in line.split(' ').filter(|t| !t.is_empty()).take(3) {
        let num = |t: &str| t.split(',').next().and_then(|x| x.parse().ok()).unwrap_or(0);
        if let Some(t) = tok.strip_prefix('-') { old = num(t) }
        if let Some(t) = tok.strip_prefix('+') { new = num(t) }
    }
    (old, new)
}

/// The diff as one whole file with its changes inline: no hunk rows, one hunk over every line.
/// Fed a diff made with a huge context (`-U1000000`).
pub fn whole_file(d: Option<FileDiff>) -> Option<FileDiff> {
    let mut d = d?;
    d.lines.retain(|l| l.kind != LineKind::Hunk);
    for l in &mut d.lines { l.hunk = 0 }
    let header = d.hunks.first().map(|h| h.header.clone()).unwrap_or_default();
    d.hunks = vec![Hunk { header, lines: 0..d.lines.len() }];
    Some(d.renewed())
}

/// An unchanged file, as context lines.
pub fn plain_file(path: &str, data: &[u8]) -> FileDiff {
    let mut f = FileDiff::default();
    f.old_path = Some(path.into());
    f.new_path = Some(path.into());
    if crate::merge3::looks_binary(data) {
        f.is_binary = true;
        return f;
    }
    let text = String::from_utf8_lossy(data);
    f.source = Arc::from(text.as_ref());
    let base = f.source.as_ptr() as usize;
    let mut lines = vec![];
    for (n, line) in f.source.split_terminator('\n').enumerate() {
        let n = n as u32 + 1;
        lines.push(DiffLine { kind: LineKind::Context, start: (line.as_ptr() as usize - base) as u32, len: line.len() as u32,
                              old_line: Some(n), new_line: Some(n), hunk: 0 });
    }
    f.hunks = vec![Hunk { header: String::new(), lines: 0..lines.len() }];
    f.lines = lines;
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/a.txt b/a.txt
index 1..2 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,3 @@
 one
-two
+twO
 three
@@ -10,2 +10,3 @@ ctx
 ten
+new
 eleven
diff --git a/old.txt b/new.txt
similarity index 90%
rename from old.txt
rename to new.txt
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ";

    #[test]
    fn parses_hunks_and_line_numbers() {
        let d = parse(SAMPLE);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].hunks.len(), 2);
        assert!(d[0].additions == 2 && d[0].deletions == 1);
        let added = d[0].lines.iter().find(|l| d[0].text(l) == "new").unwrap();
        assert!(added.new_line == Some(11) && added.old_line.is_none());
        assert!(d[0].patch_for_hunk(1).ends_with("@@ -10,2 +10,3 @@ ctx\n ten\n+new\n eleven\n"));
        assert!(d[1].old_path.as_deref() == Some("old.txt") && d[1].new_path.as_deref() == Some("new.txt"));
        assert_eq!(d[1].similarity, Some(90));
        assert!(d[2].is_binary);
        assert_ne!(d[0], d[1]);
        assert_eq!(d[0], d[0].clone());
    }

    #[test]
    fn whole_file_drops_hunk_rows() {
        let w = whole_file(parse(SAMPLE).into_iter().next()).unwrap();
        assert_eq!(w.hunks.len(), 1);
        assert!(w.lines.iter().all(|l| l.kind != LineKind::Hunk && l.hunk == 0));
        assert_eq!(w.hunks[0].lines, 0..w.lines.len());
        let p = plain_file("a", b"x\ny\n");
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.line_text(1), "y");
    }

    fn big_diff(files: usize, pairs: usize) -> String {
        let mut s = String::new();
        for f in 0..files {
            s += &format!("diff --git a/f{f}.swift b/f{f}.swift\nindex 1..2 100644\n--- a/f{f}.swift\n+++ b/f{f}.swift\n@@ -1,{0} +1,{0} @@\n", pairs * 2);
            for i in 0..pairs {
                s += &format!("-let value{i} = compute({i}, \"old\")\n+let value{i} = compute({i}, \"new\")\n");
            }
        }
        s
    }

    #[test]
    fn parses_large_diff_quickly() {
        // 500 files × 400 changed lines ≈ 200k diff lines, roughly a 10 MB PR.
        let s = big_diff(500, 200);
        let t = std::time::Instant::now();
        let files = parse(&s);
        let elapsed = t.elapsed();
        assert!(files.len() == 500 && files[0].lines.len() == 401);
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    /// `cargo test -p loupe-core --release -- --ignored --nocapture bench`: the Swift parser takes ~40 ms here.
    #[test]
    #[ignore]
    fn bench_parse_200k_lines() {
        let s = big_diff(500, 200);
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let files = parse(&s);
            let n: usize = files.iter().map(|f| f.lines.len()).sum();
            println!("parsed {n} lines ({:.1} MB) in {:?}", s.len() as f64 / 1e6, t.elapsed());
        }
    }
}
