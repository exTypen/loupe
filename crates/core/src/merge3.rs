//! A three-way merge by lines, and the index stages of a conflicted path.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkKind { Same, Ours, Theirs, Both, Conflict }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    // line ranges in each version
    pub base: Range<usize>,
    pub ours: Range<usize>,
    pub theirs: Range<usize>,
}

/// A three-way merge by lines, as diff3 does it: base↔ours and base↔theirs are diffed, the lines all three agree on
/// split the files into chunks, and each chunk is either unchanged, changed on one side (taken as is), changed alike on
/// both, or a conflict. Conflicts are trimmed of the lines both sides added alike at their start and end, like git's
/// "zealous" merge, so they match the conflicts git writes into the file.
#[derive(Clone, Debug)]
pub struct Merge3 {
    pub base: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub chunks: Vec<Chunk>,
    /// Neither side ends in a newline: the merged file shouldn't either.
    pub no_final_newline: bool,
}

impl Merge3 {
    pub fn new(base: &str, ours: &str, theirs: &str) -> Self {
        let (b, o, t) = (lines(base), lines(ours), lines(theirs));
        let no_final_newline = !ours.is_empty() && !theirs.is_empty() && !ours.ends_with('\n') && !theirs.ends_with('\n');
        let (mo, mt) = (matches(&b, &o), matches(&b, &t));
        let mut out = vec![];
        let (mut i, mut oi, mut ti, mut stable) = (0, 0, 0, 0); // stable: lines all three agree on, not yet emitted
        let flush = |out: &mut Vec<Chunk>, stable: &mut usize, i: usize, oi: usize, ti: usize| {
            if *stable == 0 { return }
            out.push(Chunk { kind: ChunkKind::Same, base: i - *stable..i, ours: oi - *stable..oi, theirs: ti - *stable..ti });
            *stable = 0;
        };
        while i < b.len() || oi < o.len() || ti < t.len() {
            if i < b.len() && mo[i] == Some(oi) && mt[i] == Some(ti) {
                i += 1; oi += 1; ti += 1; stable += 1;
                continue;
            }
            flush(&mut out, &mut stable, i, oi, ti);
            // The next base line both sides kept; the matching is monotonic, so it lies at or after oi and ti.
            let mut j = i;
            while j < b.len() && (mo[j].is_none() || mt[j].is_none()) { j += 1 }
            let (oj, tj) = if j < b.len() { (mo[j].unwrap(), mt[j].unwrap()) } else { (o.len(), t.len()) };
            out.extend(classify(&b, &o, &t, i..j, oi..oj, ti..tj));
            (i, oi, ti) = (j, oj, tj);
        }
        flush(&mut out, &mut stable, i, oi, ti);
        Merge3 { base: b, ours: o, theirs: t, chunks: out, no_final_newline }
    }

    pub fn conflict_count(&self) -> usize { self.chunks.iter().filter(|c| c.kind == ChunkKind::Conflict).count() }

    /// The merged lines of a chunk that isn't a conflict (a conflict gives ours).
    pub fn merged(&self, c: &Chunk) -> &[String] {
        match c.kind {
            ChunkKind::Theirs => &self.theirs[c.theirs.clone()],
            _ => &self.ours[c.ours.clone()],
        }
    }
}

/// The merged file with each conflict's lines from `conflict` (all ours, all theirs, both...).
pub fn merged_with(m: &Merge3, conflict: impl Fn(&Chunk) -> Vec<String>) -> String {
    m.chunks.iter()
        .flat_map(|c| if c.kind == ChunkKind::Conflict { conflict(c) } else { m.merged(c).to_vec() })
        .map(|l| l + "\n")
        .collect()
}

fn classify(base: &[String], ours: &[String], theirs: &[String], b: Range<usize>, o: Range<usize>, t: Range<usize>) -> Vec<Chunk> {
    let (bl, ol, tl) = (&base[b.clone()], &ours[o.clone()], &theirs[t.clone()]);
    let chunk = |kind, base: Range<usize>, ours: Range<usize>, theirs: Range<usize>| Chunk { kind, base, ours, theirs };
    if ol == bl { return vec![chunk(if tl == bl { ChunkKind::Same } else { ChunkKind::Theirs }, b, o, t)] }
    if tl == bl { return vec![chunk(ChunkKind::Ours, b, o, t)] }
    if ol == tl { return vec![chunk(ChunkKind::Both, b, o, t)] }
    let mut pre = 0;
    while pre < o.len() && pre < t.len() && ours[o.start + pre] == theirs[t.start + pre] { pre += 1 }
    let mut suf = 0;
    while suf < o.len() - pre && suf < t.len() - pre && ours[o.end - 1 - suf] == theirs[t.end - 1 - suf] { suf += 1 }
    let mut r = vec![];
    if pre > 0 { r.push(chunk(ChunkKind::Both, b.start..b.start, o.start..o.start + pre, t.start..t.start + pre)) }
    r.push(chunk(ChunkKind::Conflict, b.clone(), o.start + pre..o.end - suf, t.start + pre..t.end - suf));
    if suf > 0 { r.push(chunk(ChunkKind::Both, b.end..b.end, o.end - suf..o.end, t.end - suf..t.end)) }
    r
}

/// A file's lines; a final newline doesn't start another line.
pub fn lines(s: &str) -> Vec<String> {
    let mut l = split(s);
    if l.last().is_some_and(|x| x.is_empty()) { l.pop(); }
    l
}

/// Splits at every "\n", keeping a CRLF line's "\r" (so a CRLF file is many lines, not one).
pub fn split(s: &str) -> Vec<String> { s.split('\n').map(String::from).collect() }

/// For each line of `a`, the line of `b` it's kept as, or None when the change removed it.
pub fn matches<'a>(a: &'a [String], b: &'a [String]) -> Vec<Option<usize>> {
    let mut m = vec![None; a.len()];
    let mut p = 0;
    while p < a.len() && p < b.len() && a[p] == b[p] { m[p] = Some(p); p += 1 }
    let mut s = 0;
    while s < a.len() - p && s < b.len() - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        m[a.len() - 1 - s] = Some(b.len() - 1 - s);
        s += 1;
    }
    // Lines as ints: the diff compares them many times.
    let mut ids: HashMap<&'a str, usize> = HashMap::new();
    let mut id = |l: &'a String| { let n = ids.len(); *ids.entry(l.as_str()).or_insert(n) };
    let x: Vec<usize> = a[p..a.len() - s].iter().map(&mut id).collect();
    let y: Vec<usize> = b[p..b.len() - s].iter().map(&mut id).collect();
    if x.is_empty() || y.is_empty() { return m }
    // ponytail: Myers, O((N+M)·D); a whole-file rewrite of a huge file is slow. Patience/histogram diff if that bites.
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &x, &y) {
        if let similar::DiffOp::Equal { old_index, new_index, len } = op {
            for k in 0..len { m[p + old_index + k] = Some(p + new_index + k) }
        }
    }
    m
}

// MARK: - The index stages of a conflicted path

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind { Text, BothAdded, DeletedByUs, DeletedByThem, AddedByUs, AddedByThem, Submodule, None }

/// What `git ls-files -u` says about one path: which of base (1), ours (2) and theirs (3) exist, their modes and blobs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictStages {
    pub entries: BTreeMap<u8, (String, String)>, // stage → (mode, sha)
}

impl ConflictStages {
    /// Parses `git ls-files -u -z -- path`: `<mode> <sha> <stage>\t<path>\0` per stage.
    pub fn parse(out: &str) -> Self {
        let mut entries = BTreeMap::new();
        for rec in out.split('\0').filter(|r| !r.is_empty()) {
            let meta = rec.split('\t').next().unwrap_or("");
            let f: Vec<&str> = meta.split(' ').filter(|x| !x.is_empty()).collect();
            if f.len() == 3
                && let Ok(st) = f[2].parse() { entries.insert(st, (f[0].to_string(), f[1].to_string())); }
        }
        ConflictStages { entries }
    }

    pub fn read(dir: &std::path::Path, path: &str) -> Self {
        Self::parse(&crate::git::string(dir, ["ls-files", "-u", "-z", "--", path]).unwrap_or_default())
    }

    pub fn kind(&self) -> ConflictKind {
        if self.entries.values().any(|e| e.0 == "160000") { return ConflictKind::Submodule }
        let has = |n| self.entries.contains_key(&n);
        match (has(1), has(2), has(3)) {
            (true, true, true) => ConflictKind::Text,
            (false, true, true) => ConflictKind::BothAdded,
            (true, false, true) => ConflictKind::DeletedByUs,
            (true, true, false) => ConflictKind::DeletedByThem,
            (false, true, false) => ConflictKind::AddedByUs,
            (false, false, true) => ConflictKind::AddedByThem,
            _ => ConflictKind::None,
        }
    }

    /// A symlink's blob is its target: merging it as text would write a regular file over the link.
    pub fn is_symlink(&self) -> bool { self.entries.values().any(|e| e.0 == "120000") }
}

/// Git's own test: a NUL in the first 8000 bytes.
pub fn looks_binary(data: &[u8]) -> bool { data[..data.len().min(8000)].contains(&0) }

#[cfg(test)]
mod tests {
    use super::*;
    use ChunkKind::*;

    fn kinds(m: &Merge3) -> Vec<ChunkKind> { m.chunks.iter().map(|c| c.kind).collect() }
    fn merged(m: &Merge3, conflict: impl Fn(&Chunk) -> Vec<String>) -> String { merged_with(m, conflict) }

    #[test]
    fn merges_changes_on_different_lines_without_conflicts() {
        let m = Merge3::new("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "a\nb\nc\nD\ne\nf\n");
        assert_eq!(m.conflict_count(), 0);
        assert_eq!(kinds(&m), [Same, Ours, Same, Theirs, Same, Theirs]);
        assert_eq!(merged(&m, |_| vec![]), "a\nB\nc\nD\ne\nf\n");
    }

    #[test]
    fn finds_conflicts_and_keeps_their_base() {
        let m = Merge3::new("a\nb\nc\n", "a\nours\nc\n", "a\ntheirs\nc\n");
        assert_eq!(kinds(&m), [Same, Conflict, Same]);
        let c = &m.chunks[1];
        assert!(m.base[c.base.clone()] == ["b"] && m.ours[c.ours.clone()] == ["ours"] && m.theirs[c.theirs.clone()] == ["theirs"]);
        assert_eq!(merged(&m, |c| m.theirs[c.theirs.clone()].to_vec()), "a\ntheirs\nc\n");
    }

    #[test]
    fn same_change_on_both_sides_is_no_conflict() {
        assert_eq!(kinds(&Merge3::new("a\nb\n", "a\nx\n", "a\nx\n")), [Same, Both]);
    }

    #[test]
    fn conflicts_are_trimmed_of_lines_both_sides_added() {
        let m = Merge3::new("a\nold\nz\n", "a\nshared\nmine\nend\nz\n", "a\nshared\nyours\nend\nz\n");
        assert_eq!(kinds(&m), [Same, Both, Conflict, Both, Same]);
        let c = &m.chunks[2];
        assert!(m.ours[c.ours.clone()] == ["mine"] && m.theirs[c.theirs.clone()] == ["yours"] && m.base[c.base.clone()] == ["old"]);
    }

    #[test]
    fn insertions_at_the_same_place_conflict() {
        let m = Merge3::new("a\nb\n", "a\nx\nb\n", "a\ny\nb\n");
        assert_eq!(kinds(&m), [Same, Conflict, Same]);
        assert!(m.chunks[1].base.is_empty());
    }

    #[test]
    fn both_added_merges_against_an_empty_base() {
        let m = Merge3::new("", "one\ntwo\n", "one\nthree\n");
        assert_eq!(kinds(&m), [Both, Conflict]);
        assert!(!m.no_final_newline);
        assert!(Merge3::new("a", "b", "c").no_final_newline);
    }

    #[test]
    fn matches_lines_kept_by_a_diff() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(matches(&v(&["a", "b", "c", "d"]), &v(&["a", "x", "c", "d", "e"])), [Some(0), None, Some(2), Some(3)]);
        assert_eq!(matches(&v(&["a", "b"]), &[]), [None, None]);
        assert_eq!(matches(&v(&["x", "a", "y"]), &v(&["a"])), [None, Some(0), None]);
    }

    #[test]
    fn parses_index_stages() {
        let s = ConflictStages::parse("100644 aaa 1\tf.txt\u{0}100644 bbb 2\tf.txt\u{0}100644 ccc 3\tf.txt\u{0}");
        assert!(s.kind() == ConflictKind::Text && s.entries[&2].1 == "bbb");
        assert_eq!(ConflictStages::parse("100644 aaa 1\tf\u{0}100644 ccc 3\tf\u{0}").kind(), ConflictKind::DeletedByUs);
        assert_eq!(ConflictStages::parse("100644 aaa 1\tf\u{0}100644 bbb 2\tf\u{0}").kind(), ConflictKind::DeletedByThem);
        assert_eq!(ConflictStages::parse("100644 bbb 2\tf\u{0}100644 ccc 3\tf\u{0}").kind(), ConflictKind::BothAdded);
        assert_eq!(ConflictStages::parse("100644 ccc 3\tf\u{0}").kind(), ConflictKind::AddedByThem);
        assert_eq!(ConflictStages::parse("160000 aaa 1\tsub\u{0}160000 bbb 2\tsub\u{0}160000 ccc 3\tsub\u{0}").kind(), ConflictKind::Submodule);
        assert_eq!(ConflictStages::parse("").kind(), ConflictKind::None);
    }

    #[test]
    fn crlf_files_split_into_lines() {
        let m = Merge3::new("a\r\nb\r\nc\r\nd\r\n", "a\r\nB\r\nc\r\nd\r\n", "a\r\nb\r\nc\r\nD\r\n");
        assert!(m.conflict_count() == 0 && m.base.len() == 4);
        assert_eq!(merged(&m, |_| vec![]), "a\r\nB\r\nc\r\nD\r\n");
    }
}
