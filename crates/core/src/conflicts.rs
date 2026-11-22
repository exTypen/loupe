//! Conflict markers in a file, and the git side of the merge editor.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use crate::git::{self, Result};
use crate::merge3::{self, ChunkKind, ConflictStages, Merge3};
use crate::model::GitOperation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub current: Vec<String>,
    pub base: Option<Vec<String>>,
    pub incoming: Vec<String>,
    pub current_label: String,
    pub incoming_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(Vec<String>),
    Conflict(Conflict),
}

/// A file with conflict markers, split into the text around the conflicts and the conflicts themselves.
/// Handles the diff3 style (`|||||||` base section) too.
#[derive(Clone, Debug, Default)]
pub struct ConflictText {
    pub segments: Vec<Segment>,
    pub conflict_count: usize,
    /// For each segment, the index of the conflict it is (counting conflicts only), for numbering and resolving.
    pub numbered: Vec<Option<usize>>,
}

/// A marker line: seven `c`, then the end or a space.
fn marker(l: &str, c: u8) -> bool {
    let b = l.as_bytes();
    b.len() >= 7 && b[..7].iter().all(|&x| x == c) && (b.len() == 7 || b[7] == b' ')
}

fn label(l: &str) -> String { l[7..].trim().to_string() }

impl ConflictText {
    pub fn new(text: &str) -> Self {
        let mut t = ConflictText::default();
        let mut plain: Vec<String> = vec![];
        let mut cur: Option<Conflict> = None;
        let mut section = 0; // 1 current, 2 base, 3 incoming
        for line in merge3::split(text) {
            match cur.as_mut() {
                None if marker(&line, b'<') => {
                    if !plain.is_empty() { t.segments.push(Segment::Text(std::mem::take(&mut plain))) }
                    cur = Some(Conflict { current: vec![], base: None, incoming: vec![], current_label: label(&line), incoming_label: String::new() });
                    section = 1;
                }
                None => plain.push(line),
                Some(c) if section == 1 && marker(&line, b'|') => { c.base = Some(vec![]); section = 2 }
                Some(_) if section < 3 && (line == "=======" || line == "=======\r") => section = 3,
                Some(c) if section == 3 && marker(&line, b'>') => {
                    c.incoming_label = label(&line);
                    t.segments.push(Segment::Conflict(cur.take().unwrap()));
                    section = 0;
                }
                Some(c) => match section {
                    1 => c.current.push(line),
                    2 => c.base.as_mut().unwrap().push(line),
                    _ => c.incoming.push(line),
                },
            }
        }
        if !plain.is_empty() { t.segments.push(Segment::Text(plain)) }
        let mut k = 0;
        t.numbered = t.segments.iter().map(|s| matches!(s, Segment::Conflict(_)).then(|| { k += 1; k - 1 })).collect();
        t.conflict_count = k;
        t
    }

    /// The file with conflict `index` (counting conflicts only) replaced by `lines`.
    pub fn resolving(&self, index: usize, lines: &[String]) -> String {
        let mut out: Vec<String> = vec![];
        let mut k = 0;
        for seg in &self.segments {
            match seg {
                Segment::Text(t) => out.extend(t.iter().cloned()),
                Segment::Conflict(c) => {
                    if k == index { out.extend(lines.iter().cloned()) } else {
                        out.push(format!("<<<<<<< {}", c.current_label));
                        out.extend(c.current.iter().cloned());
                        if let Some(b) = &c.base { out.push("|||||||".into()); out.extend(b.iter().cloned()) }
                        out.push("=======".into());
                        out.extend(c.incoming.iter().cloned());
                        out.push(format!(">>>>>>> {}", c.incoming_label));
                    }
                    k += 1;
                }
            }
        }
        out.join("\n")
    }

    /// Each `<<<<<<<` … `>>>>>>>` block in the file: its byte range (through the closing line's newline) and its sides.
    pub fn blocks(text: &str) -> Vec<(Range<usize>, Conflict)> {
        let mut out = vec![];
        let (mut loc, mut start, mut block) = (0usize, None::<usize>, Vec::<String>::new());
        for line in merge3::split(text) {
            let len = line.len() + 1;
            if start.is_none() && marker(&line, b'<') { start = Some(loc); block.clear() }
            if let Some(s) = start {
                let closing = marker(&line, b'>');
                block.push(line);
                if closing {
                    if let Some(Segment::Conflict(c)) = ConflictText::new(&block.join("\n")).segments.into_iter().next() {
                        out.push((s..(loc + len).min(text.len()), c));
                    }
                    start = None;
                }
            }
            loc += len;
        }
        out
    }
}

/// A whole conflict, or a `<<<<<<<` / `>>>>>>>` line left behind from one: not resolved yet.
pub fn has_markers(s: &str) -> bool {
    ConflictText::new(s).conflict_count > 0
        || merge3::split(s).iter().any(|l| {
            let b = l.as_bytes();
            b"<>".iter().any(|&c| b.len() >= 7 && b[..7].iter().all(|&x| x == c)
                && l[7..].chars().next().is_none_or(char::is_whitespace))
        })
}

// MARK: - What Ours and Theirs mean right now

/// Git's "ours" and "theirs" swap meaning between a merge and a rebase; the editor names them for what they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeLabels {
    pub ours: String,
    pub ours_detail: String,
    pub theirs: String,
    pub theirs_detail: String,
}

impl MergeLabels {
    pub fn new(operation: Option<GitOperation>, branch: Option<&str>, subject: &str) -> Self {
        let head = branch.unwrap_or("HEAD").to_string();
        let (o, od, t, td) = match operation {
            Some(GitOperation::Merge) => ("Your Branch", head, "Incoming", subject.to_string()),
            Some(GitOperation::Rebase) => ("Upstream", "what you're rebasing onto".into(), "Your Commit", subject.to_string()),
            Some(GitOperation::CherryPick) => ("Your Branch", head, "Cherry-picked", subject.to_string()),
            Some(GitOperation::Revert) => ("Your Branch", head, "Reverted", format!("the code before {subject}")),
            None => ("Ours", head, "Theirs", String::new()),
        };
        MergeLabels { ours: o.into(), ours_detail: od, theirs: t.into(), theirs_detail: td }
    }
}

// MARK: - The merge editor's result

/// A conflict's three sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sides {
    pub ours: Vec<String>,
    pub base: Option<Vec<String>>,
    pub theirs: Vec<String>,
}

/// A chunk of the result that isn't unchanged: its byte range in the result text, what it is, and for a conflict its sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub range: Range<usize>,
    pub kind: ChunkKind,
    pub chunk: usize,
    pub sides: Option<Sides>,
}

pub fn marker_block(s: &Sides, ours_label: &str, theirs_label: &str) -> Vec<String> {
    let mut l = vec![format!("<<<<<<< {ours_label}")];
    l.extend(s.ours.iter().cloned());
    if let Some(b) = &s.base { l.push("||||||| base".into()); l.extend(b.iter().cloned()) }
    l.push("=======".into());
    l.extend(s.theirs.iter().cloned());
    l.push(format!(">>>>>>> {theirs_label}"));
    l
}

/// The result built from the chunks: what merged on its own taken in, each conflict as a marker block.
pub fn from_chunks(m: &Merge3, ours_label: &str, theirs_label: &str) -> (Vec<String>, Vec<Region>) {
    let (mut regions, mut out, mut loc) = (vec![], vec![], 0);
    for (j, c) in m.chunks.iter().enumerate() {
        let mut sides = None;
        let lines = if c.kind == ChunkKind::Conflict {
            let s = Sides { ours: m.ours[c.ours.clone()].to_vec(), base: Some(m.base[c.base.clone()].to_vec()), theirs: m.theirs[c.theirs.clone()].to_vec() };
            let l = marker_block(&s, ours_label, theirs_label);
            sides = Some(s);
            l
        } else { m.merged(c).to_vec() };
        let len: usize = lines.iter().map(|l| l.len() + 1).sum();
        if c.kind != ChunkKind::Same { regions.push(Region { range: loc..loc + len, kind: c.kind, chunk: j, sides }) }
        out.extend(lines);
        loc += len;
    }
    (out, regions)
}

/// Whether the file on disk and the chunks' result have the same text around their conflicts (and as many).
pub fn agree(disk: &str, blocks: &[Range<usize>], result: &[String], regions: &[Region]) -> bool {
    let cr: Vec<Range<usize>> = regions.iter().filter(|r| r.kind == ChunkKind::Conflict).map(|r| r.range.clone()).collect();
    if cr.len() != blocks.len() { return false }
    fn around<'a>(s: &'a str, cut: &[Range<usize>]) -> Vec<&'a str> {
        let (mut out, mut at) = (vec![], 0);
        for r in cut {
            out.push(s.get(at..r.start.max(at)).unwrap_or(""));
            at = r.end;
        }
        out.push(s.get(at..).unwrap_or(""));
        out
    }
    let d = if disk.is_empty() || disk.ends_with('\n') { disk.to_string() } else { format!("{disk}\n") };
    let r: String = result.iter().map(|l| format!("{l}\n")).collect();
    around(&d, blocks) == around(&r, &cr)
}

/// Each stage's blob of a conflicted path (none for a submodule).
pub fn stage_blobs(dir: &Path, path: &str, stages: &ConflictStages) -> BTreeMap<u8, Vec<u8>> {
    if stages.kind() == merge3::ConflictKind::Submodule { return BTreeMap::new() }
    stages.entries.keys()
        .filter_map(|&n| git::run(dir, ["show".to_string(), format!(":{n}:{path}")]).ok().map(|d| (n, d)))
        .collect()
}

/// The merge tool the user configured, and whether it's a GUI one. Loupe runs it without a terminal: vim or emacs
/// there would wait for one forever.
pub fn configured_merge_tool(dir: &Path) -> Option<(String, bool)> {
    let get = |k: &str| git::trimmed(dir, ["config", "--get", k]).ok().filter(|v| !v.is_empty());
    if let Some(g) = get("merge.guitool") { return Some((g, true)) }
    get("merge.tool").filter(|t| !TERMINAL_TOOLS.contains(&t.as_str())).map(|t| (t, false))
}

pub const TERMINAL_TOOLS: &[&str] = &["vimdiff", "vimdiff1", "vimdiff2", "vimdiff3", "nvimdiff", "nvimdiff1", "nvimdiff2", "nvimdiff3", "emerge"];

/// Puts the conflict markers back (`checkout -m`): the editor's Reset.
pub fn reset_conflict(dir: &Path, path: &str) -> Result<()> { git::run(dir, ["checkout", "-m", "--", path]).map(drop) }

pub fn run_merge_tool(dir: &Path, path: &str, gui: bool) -> Result<()> {
    let mut a = vec!["mergetool", "--no-prompt"];
    if gui { a.push("--gui") }
    a.extend(["--", path]);
    git::run(dir, a).map(drop)
}

/// A submodule conflict resolved to one stage's commit.
pub fn resolve_submodule(dir: &Path, path: &str, sha: &str) -> Result<()> {
    git::run(dir, ["update-index", "--cacheinfo", &format!("160000,{sha},{path}")]).map(drop)
}

/// Writes the editor's result, keeping the file's mode (an executable script stays executable).
pub fn write_result(text: &str, path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok().map(|m| m.permissions().mode());
    std::fs::write(path, text)?;
    if let Some(m) = mode { std::fs::set_permissions(path, std::fs::Permissions::from_mode(m))? }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge3::ConflictKind;

    fn v(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn parses_and_resolves_conflict_markers() {
        let file = "a\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> abc123 (fix)\nb\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> abc123 (fix)\n";
        let t = ConflictText::new(file);
        assert_eq!(t.conflict_count, 2);
        let Segment::Conflict(c) = &t.segments[1] else { panic!("no conflict") };
        assert!(c.current == ["mine"] && c.base == Some(v(&["old"])) && c.incoming == ["theirs"]);
        assert!(c.current_label == "HEAD" && c.incoming_label == "abc123 (fix)");
        let once = t.resolving(0, &c.incoming);
        assert_eq!(once, "a\ntheirs\nb\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> abc123 (fix)\n"); // the other conflict survives verbatim
        assert_eq!(ConflictText::new(&once).resolving(0, &v(&["x", "y"])), "a\ntheirs\nb\nx\ny\n");
        assert_eq!(ConflictText::new("no markers\n").conflict_count, 0);
    }

    #[test]
    fn numbers_conflicts_from_zero() {
        let t = ConflictText::new("a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\nc\n<<<<<<< HEAD\np\n=======\nq\n>>>>>>> b\n");
        assert_eq!(t.numbered.iter().flatten().copied().collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn finds_marker_blocks_with_their_ranges() {
        let text = "keep\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\nyours\n>>>>>>> topic\ntail\n";
        let b = ConflictText::blocks(text);
        assert_eq!(b.len(), 1);
        let block = &text[b[0].0.clone()];
        assert!(block.starts_with("<<<<<<< HEAD\n") && block.ends_with(">>>>>>> topic\n"));
        assert!(b[0].1.current == ["mine"] && b[0].1.base == Some(v(&["old"])) && b[0].1.incoming == ["yours"]);
    }

    #[test]
    fn names_sides_for_the_operation() {
        let rebase = MergeLabels::new(Some(GitOperation::Rebase), None, "abc Fix it");
        assert!(rebase.ours == "Upstream" && rebase.theirs == "Your Commit");
        let merge = MergeLabels::new(Some(GitOperation::Merge), Some("main"), "def Feature");
        assert!(merge.ours == "Your Branch" && merge.ours_detail == "main" && merge.theirs == "Incoming");
    }

    #[test]
    fn marker_blocks_round_trip() {
        let block = marker_block(&Sides { ours: v(&["o"]), base: Some(v(&["b"])), theirs: v(&["t"]) }, "HEAD", "topic");
        assert_eq!(ConflictText::new(&(block.join("\n") + "\n")).conflict_count, 1);
    }

    #[test]
    fn crlf_conflict_blocks() {
        let text = "x\r\n<<<<<<< HEAD\r\nmine\r\n=======\r\nyours\r\n>>>>>>> topic\r\ny\r\n";
        assert_eq!(ConflictText::new(text).conflict_count, 1);
        let b = ConflictText::blocks(text);
        assert!(b.len() == 1 && b[0].1.incoming_label == "topic" && b[0].1.current == ["mine\r"]);
        assert!(text[b[0].0.clone()].ends_with(">>>>>>> topic\r\n"));
    }

    #[test]
    fn edits_outside_the_conflicts_keep_the_file_as_it_is() {
        let m = Merge3::new("a\nx\nb\nc\nd\n", "A\nx\nours\nc\nd\n", "a\nx\ntheirs\nc\nd\n");
        let (lines, regions) = from_chunks(&m, "HEAD", "topic");
        let git = "A\nx\n<<<<<<< HEAD\nours\n||||||| base\nb\n=======\ntheirs\n>>>>>>> topic\nc\nd\n";
        let ok = |disk: &str| {
            let blocks: Vec<_> = ConflictText::blocks(disk).into_iter().map(|b| b.0).collect();
            agree(disk, &blocks, &lines, &regions)
        };
        assert!(ok(git));
        assert!(!ok(&git.replace("c\n", "c edited in vim\n"))); // a write would undo that edit
        assert!(!ok(&git.replace('\n', "\r\n")));               // autocrlf checkout: keep its line endings
        assert!(!ok("A\nx\nours\nc\nd\n"));                     // resolved in an editor, or by rerere
    }

    #[test]
    fn stray_marker_lines_still_count_as_unresolved() {
        assert!(has_markers("a\n<<<<<<< HEAD\nb\n"));
        assert!(has_markers(">>>>>>> topic\n"));
        assert!(!has_markers("Title\n=======\n<<<<<<<< not a marker\n"));
    }

    #[test]
    fn writing_the_result_keeps_the_file_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.sh");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_result("#!/bin/sh\necho hi\n", &p).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "#!/bin/sh\necho hi\n");
    }

    #[test]
    fn merges_a_real_conflict_from_the_index_stages() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let g = |args: &[&str]| { git::git(dir, ["-c", "user.name=t", "-c", "user.email=t@t"].iter().chain(args)).run().unwrap(); };
        let w = |n: &str, s: &str| std::fs::write(dir.join(n), s).unwrap();
        g(&["init", "-q", "-b", "main"]);
        w("f.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\n");
        w("gone.txt", "base\n");
        g(&["add", "."]);
        g(&["commit", "-qm", "base"]);
        g(&["switch", "-qc", "topic"]);
        w("f.txt", "one\ntwo\nTHREE topic\nfour\nfive\nsix\nseven\neight\n");
        w("gone.txt", "changed on topic\n");
        g(&["commit", "-qam", "topic"]);
        g(&["switch", "-q", "main"]);
        w("f.txt", "ONE\ntwo\nthree main\nfour\nfive\nsix\nseven\n");
        g(&["rm", "-q", "gone.txt"]);
        g(&["commit", "-qam", "main"]);
        let _ = git::git(dir, ["-c", "user.name=t", "-c", "user.email=t@t", "merge", "--no-edit", "topic"]).allow_failure().run();

        let stages = ConflictStages::read(dir, "f.txt");
        assert_eq!(stages.kind(), ConflictKind::Text);
        let blobs = stage_blobs(dir, "f.txt", &stages);
        let s = |n: u8| String::from_utf8(blobs[&n].clone()).unwrap();
        let m = Merge3::new(&s(1), &s(2), &s(3));
        assert_eq!(m.conflict_count(), 1);
        let disk = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        let blocks = ConflictText::blocks(&disk);
        assert_eq!(blocks.len(), m.conflict_count()); // ours lines up with git's own conflicts
        let both = crate::merge3::merged_with(&m,|c| [&m.ours[c.ours.clone()], &m.theirs[c.theirs.clone()]].concat());
        assert_eq!(both, "ONE\ntwo\nthree main\nTHREE topic\nfour\nfive\nsix\nseven\neight\n");
        // Built from the chunks with git's labels, the result is git's file: what merged on its own is the same text.
        let (lines, regions) = from_chunks(&m, &blocks[0].1.current_label, &blocks[0].1.incoming_label);
        let ranges: Vec<_> = blocks.iter().map(|b| b.0.clone()).collect();
        assert!(agree(&disk, &ranges, &lines, &regions));
        assert_eq!(ConflictStages::read(dir, "gone.txt").kind(), ConflictKind::DeletedByUs);
    }
}
