//! The merge editor's model, without gpui (MergeSession in MergeEditor.swift): loading a conflicted path's stages,
//! the result being edited and its regions (byte ranges that follow edits), taking a version of a conflict, picking
//! lines, navigation, tints, and the AI suggestion's prompt and cleanup.
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use loupe_core::conflicts::{self, ConflictText};
use loupe_core::merge3::{self, ChunkKind, ConflictKind, ConflictStages, Merge3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Take { Ours, Theirs, OursThenTheirs, TheirsThenOurs, Base, Markers }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side { Ours, Theirs }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role { Ours, Theirs, Base, Result }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pick { pub side: Side, pub line: usize }

/// A conflict's three versions. `base` is None when there is no common ancestor (both sides added the file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sides { pub ours: Vec<String>, pub base: Option<Vec<String>>, pub theirs: Vec<String> }

/// A stretch of the result that came from a change: an auto-merged one, or a conflict. Its byte range follows edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub range: Range<usize>,
    pub kind: ChunkKind,
    pub chunk: Option<usize>, // in merge.chunks; None for a conflict that couldn't be matched to one
    pub sides: Option<Sides>,
    pub picks: Vec<Pick>,
}

/// Git's own marker labels, so a write changes only what the user resolved. `base` is Some (git's label after
/// `|||||||`) only when git wrote the diff3 style: Loupe never adds a base section git didn't write (FLOWS §14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Markers { pub ours: String, pub base: Option<String>, pub theirs: String, pub cr: bool }

impl Default for Markers {
    fn default() -> Self { Markers { ours: "HEAD".into(), base: None, theirs: "theirs".into(), cr: false } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint { Ours, OursPicked, Theirs, TheirsPicked, Both, Changed, Conflict, ConflictCurrent, Resolved, ResolvedCurrent }

/// What `load` read off the main thread.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub stages: ConflictStages,
    pub binary: bool,
    pub symlink: bool,
    pub sizes: BTreeMap<u8, usize>,
    pub merge_tool: Option<(String, bool)>,
    pub disk: Option<String>,
    /// The text conflict, ready to edit; None for the special ones.
    pub session: Option<Session>,
    /// A delete/modify or one-sided conflict: the side that's left, shown read-only.
    pub survivor: Option<String>,
}

impl Loaded {
    pub fn kind(&self) -> ConflictKind { self.stages.kind() }
    pub fn is_text(&self) -> bool { !self.binary && matches!(self.kind(), ConflictKind::Text | ConflictKind::BothAdded) }
}

/// `git ls-files -u`, each stage's blob, what's on disk, the merge tool; then the three-way merge. Blocking.
pub fn load(dir: &Path, path: &str) -> Loaded {
    let stages = ConflictStages::read(dir, path);
    let data = conflicts::stage_blobs(dir, path, &stages);
    let symlink = stages.is_symlink();
    let binary = symlink || data.values().any(|d| merge3::looks_binary(d) || std::str::from_utf8(d).is_err());
    let disk = std::fs::read(dir.join(path)).ok().and_then(|d| String::from_utf8(d).ok());
    let mut l = Loaded {
        sizes: data.iter().map(|(k, v)| (*k, v.len())).collect(),
        merge_tool: conflicts::configured_merge_tool(dir),
        stages, binary, symlink, disk: disk.clone(), session: None, survivor: None,
    };
    let text = |n: u8| data.get(&n).map(|d| String::from_utf8_lossy(d).into_owned());
    if l.is_text() {
        l.session = Some(Session::build(text(1), &text(2).unwrap_or_default(), &text(3).unwrap_or_default(), disk.as_deref()));
    } else if !binary && let Some(t) = text(2).or_else(|| text(3)) {
        l.survivor = Some(t);
    }
    l
}

/// The result being edited and everything derived from it.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub merge: Option<Merge3>,
    pub regions: Vec<Region>,
    pub conflicts: Vec<usize>, // indices into regions
    pub text: String,
    pub edited_outside: bool,
    pub no_final_newline: bool,
    pub markers: Markers,
    pub current: usize,
    pub resolved: Vec<bool>,
    pub markers_left: bool,
}

impl Session {
    /// `base` is None when stage 1 is missing (both added).
    pub fn build(base: Option<String>, ours: &str, theirs: &str, disk: Option<&str>) -> Session {
        let has_base = base.is_some();
        let zealous = Merge3::new(base.as_deref().unwrap_or(""), ours, theirs);
        let blocks = disk.map(ConflictText::blocks).unwrap_or_default();
        let markers = disk.map(markers_in).unwrap_or_default();
        let ranges: Vec<Range<usize>> = blocks.iter().map(|b| b.0.clone()).collect();
        let agrees = |m: &Merge3| {
            let (lines, regions) = from_chunks(m, &markers, has_base);
            let core: Vec<conflicts::Region> = regions.iter()
                .map(|r| conflicts::Region { range: r.range.clone(), kind: r.kind, chunk: r.chunk.unwrap_or(0), sides: None }).collect();
            let ok = disk.is_none_or(|d| conflicts::agree(d, &ranges, &lines, &core));
            (ok, lines, regions)
        };
        // Git trims conflicts of the lines both sides added alike ("zealous"), except in the diff3 style, which keeps
        // them inside the conflict. Use whichever matches the file.
        let mut m = zealous;
        let (mut ok, mut lines, mut chunk_regions) = agrees(&m);
        if !ok && markers.base.is_some() {
            let plain = untrimmed(&m);
            let (ok2, l2, r2) = agrees(&plain);
            if ok2 { (m, ok, lines, chunk_regions) = (plain, ok2, l2, r2) }
        }
        // Git's file and ours agree outside the conflicts: build the result from the chunks, which marks what merged on
        // its own. Otherwise it was edited elsewhere or merged differently (attributes, -X options, line endings, another
        // diff algorithm): keep it as it is, and track only its markers.
        let outside = !ok;
        let (text, regions) = match disk {
            Some(d) if outside => {
                let mut used = vec![false; m.chunks.len()];
                let regions = blocks.into_iter().map(|(range, c)| {
                    let j = (0..m.chunks.len()).find(|&j| {
                        let ch = &m.chunks[j];
                        ch.kind == ChunkKind::Conflict && !used[j] && m.ours[ch.ours.clone()] == c.current[..] && m.theirs[ch.theirs.clone()] == c.incoming[..]
                    });
                    if let Some(j) = j { used[j] = true }
                    let base = c.base.or_else(|| j.filter(|_| has_base).map(|j| m.base[m.chunks[j].base.clone()].to_vec()));
                    Region { range, kind: ChunkKind::Conflict, chunk: j, sides: Some(Sides { ours: c.current, base, theirs: c.incoming }), picks: vec![] }
                }).collect();
                (d.to_string(), regions)
            }
            _ => (lines.iter().map(|l| format!("{l}\n")).collect(), chunk_regions),
        };
        // A kept file keeps its last line as it is; a merged one ends like both sides do.
        let no_final_newline = if outside { disk.is_some_and(|d| !d.is_empty() && !d.ends_with('\n')) } else { m.no_final_newline };
        let mut s = Session { merge: Some(m), regions, text, edited_outside: outside, no_final_newline, markers, ..Default::default() };
        s.conflicts = (0..s.regions.len()).filter(|&i| s.regions[i].kind == ChunkKind::Conflict).collect();
        s.refresh_derived();
        s
    }

    pub fn conflict_count(&self) -> usize { self.conflicts.len() }
    pub fn unresolved_count(&self) -> usize { self.resolved.iter().filter(|r| !**r).count() }
    pub fn current_region(&self) -> Option<usize> { self.conflicts.get(self.current).copied() }

    /// The text that goes to disk: without the trailing newline when neither side had one.
    pub fn file_text(&self) -> String {
        let mut s = self.text.clone();
        if self.no_final_newline && s.ends_with('\n') { s.pop(); }
        s
    }

    /// An edit of the result: bytes `lo..old_hi` became `lo..new_hi`; `text` is the result after it.
    pub fn edited(&mut self, lo: usize, old_hi: usize, new_hi: usize, text: String) {
        let delta = new_hi as isize - old_hi as isize;
        for r in &mut self.regions { r.range = moved(&r.range, lo, old_hi, delta) }
        self.text = text;
        self.refresh_derived();
    }

    pub fn refresh_derived(&mut self) {
        self.resolved = self.conflicts.iter().map(|&i| {
            let r = &self.regions[i].range;
            self.text.get(r.clone()).is_some_and(|t| ConflictText::new(t).conflict_count == 0)
        }).collect();
        self.markers_left = conflicts::has_markers(&self.text);
    }

    /// The replacement that puts version `take` into conflict `k`: the byte range and its new text.
    pub fn take(&mut self, take: Take, k: usize) -> Option<(Range<usize>, String)> {
        let i = *self.conflicts.get(k)?;
        let s = self.regions[i].sides.as_ref()?;
        let lines = match take {
            Take::Ours => s.ours.clone(),
            Take::Theirs => s.theirs.clone(),
            Take::OursThenTheirs => [&s.ours[..], &s.theirs[..]].concat(),
            Take::TheirsThenOurs => [&s.theirs[..], &s.ours[..]].concat(),
            Take::Base => s.base.clone()?,
            Take::Markers => marker_block(s, &self.markers),
        };
        self.regions[i].picks.clear();
        Some((self.regions[i].range.clone(), joined(&lines)))
    }

    pub fn can_use_base(&self, k: usize) -> bool {
        self.conflicts.get(k).is_some_and(|&i| self.regions[i].sides.as_ref().is_some_and(|s| s.base.is_some()))
    }

    /// A click on `line` of Ours or Theirs: adds it to its conflict's result, or takes it out again. Returns the
    /// replacement; the conflict becomes current.
    pub fn click(&mut self, side: Side, line: usize) -> Option<(Range<usize>, String)> {
        let m = self.merge.as_ref()?;
        let k = self.conflicts.iter().position(|&i| self.regions[i].chunk
            .is_some_and(|j| if side == Side::Ours { m.chunks[j].ours.contains(&line) } else { m.chunks[j].theirs.contains(&line) }))?;
        let i = self.conflicts[k];
        let p = Pick { side, line };
        let picks = &mut self.regions[i].picks;
        match picks.iter().position(|x| *x == p) { Some(at) => { picks.remove(at); } None => picks.push(p) }
        self.current = k;
        if picks.is_empty() { return self.take(Take::Markers, k) }
        // The side picked from first comes first; each side's lines keep their order.
        let order = if picks[0].side == Side::Ours { [Side::Ours, Side::Theirs] } else { [Side::Theirs, Side::Ours] };
        let lines: Vec<String> = order.iter().flat_map(|s| {
            let mut ls: Vec<usize> = picks.iter().filter(|p| p.side == *s).map(|p| p.line).collect();
            ls.sort();
            ls.into_iter().map(|l| if *s == Side::Ours { m.ours[l].clone() } else { m.theirs[l].clone() }).collect::<Vec<_>>()
        }).collect();
        Some((self.regions[i].range.clone(), joined(&lines)))
    }

    /// `go(±1)` wraps around.
    pub fn go(&self, delta: isize) -> Option<usize> {
        let n = self.conflict_count() as isize;
        (n > 0).then(|| ((self.current as isize + delta).rem_euclid(n)) as usize)
    }

    /// The next conflict that still has markers, after the current one.
    pub fn next_unresolved(&self) -> Option<usize> {
        let n = self.conflict_count().max(1);
        (1..=n).map(|d| (self.current + d) % n).find(|&k| self.resolved.get(k) == Some(&false))
    }

    /// The conflict the caret is in, if any.
    pub fn conflict_at(&self, offset: usize) -> Option<usize> {
        self.conflicts.iter().position(|&i| self.regions[i].range.contains(&offset))
    }

    /// Line ranges of Ours, Theirs or Base to tint (MergeEditor.swift's tints).
    pub fn pane_tints(&self, role: Role) -> Vec<(Range<usize>, Tint)> {
        let Some(m) = &self.merge else { return vec![] };
        let lines = |c: &merge3::Chunk| match role { Role::Ours => c.ours.clone(), Role::Theirs => c.theirs.clone(), _ => c.base.clone() };
        let mut t = vec![];
        for c in &m.chunks {
            let tint = match (c.kind, role) {
                (ChunkKind::Same, _) => continue,
                (ChunkKind::Conflict, _) => Tint::Conflict,
                (_, Role::Base) => Tint::Changed,
                (ChunkKind::Ours, Role::Ours) => Tint::Ours,
                (ChunkKind::Theirs, Role::Theirs) => Tint::Theirs,
                (ChunkKind::Both, _) => Tint::Both,
                _ => continue,
            };
            t.push((lines(c), tint));
        }
        if let Some(j) = self.current_region().and_then(|i| self.regions[i].chunk) { t.push((lines(&m.chunks[j]), Tint::ConflictCurrent)) }
        if role != Role::Base {
            for &i in &self.conflicts {
                for p in &self.regions[i].picks {
                    if (p.side == Side::Ours) == (role == Role::Ours) {
                        t.push((p.line..p.line + 1, if p.side == Side::Ours { Tint::OursPicked } else { Tint::TheirsPicked }));
                    }
                }
            }
        }
        t
    }

    /// Byte ranges of the result to tint.
    pub fn result_tints(&self) -> Vec<(Range<usize>, Tint)> {
        let mut n = 0;
        self.regions.iter().map(|r| {
            let tint = match r.kind {
                ChunkKind::Ours => Tint::Ours,
                ChunkKind::Theirs => Tint::Theirs,
                ChunkKind::Both | ChunkKind::Same => Tint::Both,
                ChunkKind::Conflict => {
                    let k = n;
                    n += 1;
                    match (self.resolved.get(k) == Some(&true), k == self.current) {
                        (true, true) => Tint::ResolvedCurrent,
                        (true, false) => Tint::Resolved,
                        (false, true) => Tint::ConflictCurrent,
                        (false, false) => Tint::Conflict,
                    }
                }
            };
            (r.range.clone(), tint)
        }).collect()
    }

    /// The lines of chunk `j`'s conflict in a pane, to scroll to.
    pub fn chunk_lines(&self, role: Role) -> Option<Range<usize>> {
        let j = self.regions[self.current_region()?].chunk?;
        let c = &self.merge.as_ref()?.chunks[j];
        Some(match role { Role::Ours => c.ours.clone(), Role::Theirs => c.theirs.clone(), _ => c.base.clone() })
    }

    /// The AI prompt for conflict `k`: the path, 20 lines on each side, and the three versions.
    pub fn prompt(&self, path: &str, k: usize) -> Option<String> {
        let i = *self.conflicts.get(k)?;
        let s = self.regions[i].sides.as_ref()?;
        let r = &self.regions[i].range;
        let pre = &self.text[..r.start.min(self.text.len())];
        let post = &self.text[r.end.min(self.text.len())..];
        let before: Vec<&str> = pre.split('\n').collect();
        let before = before[before.len().saturating_sub(20)..].join("\n");
        let after = post.split('\n').take(20).collect::<Vec<_>>().join("\n");
        let j = |l: &[String]| l.join("\n");
        Some(format!("You resolve one git merge conflict in {path}. Below are the lines before it, the base version (the common ancestor), \
ours and theirs, and the lines after it. Combine the intent of both changes. Reply with only the lines that replace the \
conflict: no code fences, no explanation, no conflict markers, and keep the file's indentation. Don't run tools or look at \
files; everything you need is here.

Before:
{before}
Base:
{}
Ours:
{}
Theirs:
{}
After:
{after}", s.base.as_deref().map_or("(not available)".to_string(), j), j(&s.ours), j(&s.theirs)))
    }
}

/// Where a region is after an edit of bytes `lo..old_hi` that changed the length by `delta`.
pub fn moved(r: &Range<usize>, lo: usize, old_hi: usize, delta: isize) -> Range<usize> {
    let add = |x: usize| (x as isize + delta).max(0) as usize;
    // Text put where a region resolved to nothing is that region's (typed into it, or its undo).
    let fills = r.is_empty() && lo == r.start;
    if !fills && old_hi <= r.start { return add(r.start)..add(r.end) }
    if !fills && lo >= r.end { return r.clone() }
    let start = r.start.min(lo);
    start..add(r.end.max(old_hi)).max(start)
}

fn joined(lines: &[String]) -> String { lines.iter().map(|l| format!("{l}\n")).collect() }

/// The labels git wrote on the first conflict, and its base label if it wrote the diff3 style.
pub fn markers_in(disk: &str) -> Markers {
    let blocks = ConflictText::blocks(disk);
    let Some((range, c)) = blocks.first() else { return Markers::default() };
    let base = disk[range.clone()].split('\n')
        .find(|l| l.len() >= 7 && l.as_bytes()[..7].iter().all(|&b| b == b'|') && (l.len() == 7 || l.as_bytes()[7] == b' '))
        .map(|l| l[7..].trim().to_string());
    // A CRLF file gets CRLF marker lines from git: keep them, so the file matches outside what the user resolved.
    let cr = disk[range.clone()].split('\n').next().is_some_and(|l| l.ends_with('\r'));
    Markers { ours: c.current_label.clone(), base, theirs: c.incoming_label.clone(), cr }
}

/// A conflict as git writes it, in git's style: a base section only when git wrote one.
pub fn marker_block(s: &Sides, mk: &Markers) -> Vec<String> {
    let cr = if mk.cr { "\r" } else { "" };
    let mut l = vec![format!("<<<<<<< {}{cr}", mk.ours)];
    l.extend(s.ours.iter().cloned());
    if let (Some(label), Some(b)) = (&mk.base, &s.base) {
        l.push(if label.is_empty() { format!("|||||||{cr}") } else { format!("||||||| {label}{cr}") });
        l.extend(b.iter().cloned());
    }
    l.push(format!("======={cr}"));
    l.extend(s.theirs.iter().cloned());
    l.push(format!(">>>>>>> {}{cr}", mk.theirs));
    l
}

/// The result built from the chunks: what merged on its own taken in, each conflict as a marker block.
pub fn from_chunks(m: &Merge3, mk: &Markers, has_base: bool) -> (Vec<String>, Vec<Region>) {
    let (mut regions, mut out, mut loc) = (vec![], vec![], 0);
    for (j, c) in m.chunks.iter().enumerate() {
        let mut sides = None;
        let lines = if c.kind == ChunkKind::Conflict {
            let s = Sides { ours: m.ours[c.ours.clone()].to_vec(), base: has_base.then(|| m.base[c.base.clone()].to_vec()), theirs: m.theirs[c.theirs.clone()].to_vec() };
            let l = marker_block(&s, mk);
            sides = Some(s);
            l
        } else { m.merged(c).to_vec() };
        let len: usize = lines.iter().map(|l| l.len() + 1).sum();
        if c.kind != ChunkKind::Same { regions.push(Region { range: loc..loc + len, kind: c.kind, chunk: Some(j), sides, picks: vec![] }) }
        out.extend(lines);
        loc += len;
    }
    (out, regions)
}

/// The merge with each conflict taking back the lines both sides added alike at its edges, as git's diff3 style
/// writes it.
fn untrimmed(m: &Merge3) -> Merge3 {
    let mut out: Vec<merge3::Chunk> = vec![];
    let trim = |c: &merge3::Chunk| c.kind == ChunkKind::Both && c.base.is_empty();
    let mut i = 0;
    while i < m.chunks.len() {
        let c = m.chunks[i].clone();
        if c.kind == ChunkKind::Conflict {
            let mut c = c;
            if let Some(p) = out.last().filter(|p| trim(p) && p.base.end == c.base.start) {
                c.ours.start = p.ours.start; c.theirs.start = p.theirs.start;
                out.pop();
            }
            if let Some(n) = m.chunks.get(i + 1).filter(|n| trim(n) && n.base.start == c.base.end) {
                c.ours.end = n.ours.end; c.theirs.end = n.theirs.end;
                i += 1;
            }
            out.push(c);
        } else { out.push(c) }
        i += 1;
    }
    Merge3 { chunks: out, ..m.clone() }
}

/// The byte offset each line starts at, plus the end.
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut st = vec![0];
    st.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    if st.last() != Some(&text.len()) { st.push(text.len()) }
    st
}

/// The line holding byte `offset`.
pub fn line_at(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1).min(starts.len().saturating_sub(2))
}

/// Drops ANSI escapes and code fences, and blank lines around the code; keeps its indentation (MergeAI.clean).
pub fn clean_suggestion(s: &str) -> String {
    let mut t = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1B}' && it.peek() == Some(&'[') {
            let mut probe = it.clone();
            probe.next();
            while probe.peek().is_some_and(|c| c.is_ascii_digit() || *c == ';' || *c == '?') { probe.next(); }
            if probe.peek().is_some_and(|c| c.is_ascii_alphabetic()) { probe.next(); it = probe; continue }
        }
        t.push(c);
    }
    let mut lines: Vec<&str> = t.split('\n').collect();
    let junk = |l: &&str| l.trim().is_empty() || l.starts_with("```");
    while lines.first().is_some_and(junk) { lines.remove(0); }
    while lines.last().is_some_and(junk) { lines.pop(); }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ours changes b (conflict) and e (clean), theirs b, g (clean) and i (conflict).
    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
    const OURS: &str = "a\nb ours\nc\nd\nE\nf\ng\nh\ni ours\nj\nk\n";
    const THEIRS: &str = "a\nb theirs\nc\nd\ne\nf\nG\nh\ni theirs\nj\nk\n";

    fn git_style(diff3: bool) -> String {
        let base = |b: &str| if diff3 { format!("||||||| merged common ancestors\n{b}\n") } else { String::new() };
        format!("a\n<<<<<<< HEAD\nb ours\n{}=======\nb theirs\n>>>>>>> topic\nc\nd\nE\nf\nG\nh\n<<<<<<< HEAD\ni ours\n{}=======\ni theirs\n>>>>>>> topic\nj\nk\n", base("b"), base("i"))
    }

    fn apply(s: &mut Session, r: Range<usize>, new: &str) {
        let mut t = s.text.clone();
        t.replace_range(r.clone(), new);
        s.edited(r.start, r.end, r.start + new.len(), t);
    }

    #[test]
    fn builds_the_result_with_gits_markers_and_no_base_section_it_did_not_write() {
        let s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(false)));
        assert!(!s.edited_outside);
        assert_eq!(s.text, git_style(false)); // the first autosave changes nothing (FLOWS §14)
        assert_eq!(s.conflict_count(), 2);
        assert!(s.can_use_base(0)); // the base is still there for Use Base
        let d3 = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(true)));
        assert_eq!(d3.text, git_style(true));
        assert_eq!(d3.markers.base.as_deref(), Some("merged common ancestors"));
    }

    #[test]
    fn both_added_has_no_base_to_use() {
        let s = Session::build(None, "one\ntwo\n", "one\nthree\n", Some("one\n<<<<<<< HEAD\ntwo\n=======\nthree\n>>>>>>> topic\n"));
        assert!(!s.edited_outside && s.conflict_count() == 1);
        assert!(!s.can_use_base(0));
    }

    #[test]
    fn takes_versions_and_tracks_resolution() {
        let mut s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(false)));
        assert!(s.markers_left && s.unresolved_count() == 2);
        let (r, t) = s.take(Take::Theirs, 0).unwrap();
        apply(&mut s, r, &t);
        assert_eq!(s.resolved, [true, false]);
        let (r, t) = s.take(Take::OursThenTheirs, 1).unwrap();
        apply(&mut s, r, &t);
        assert!(!s.markers_left);
        assert_eq!(s.text, "a\nb theirs\nc\nd\nE\nf\nG\nh\ni ours\ni theirs\nj\nk\n");
        let (r, t) = s.take(Take::Markers, 0).unwrap(); // Restore
        apply(&mut s, r, &t);
        assert_eq!(s.resolved, [false, true]);
        let (r, t) = s.take(Take::Base, 0).unwrap();
        apply(&mut s, r, &t);
        assert!(s.text.starts_with("a\nb\nc\n"));
    }

    #[test]
    fn regions_follow_edits_and_an_empty_region_takes_text_typed_into_it() {
        assert_eq!(moved(&(10..20), 0, 2, 3), 13..23);  // before: shifts
        assert_eq!(moved(&(10..20), 25, 30, 4), 10..20); // after: stays
        assert_eq!(moved(&(10..20), 12, 15, -3), 10..17); // inside: shrinks
        assert_eq!(moved(&(10..10), 10, 10, 4), 10..14); // resolved to nothing, typed into
        let mut s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(false)));
        let r = s.regions[s.conflicts[0]].range.clone();
        apply(&mut s, r.clone(), ""); // resolved to nothing
        assert!(s.regions[s.conflicts[0]].range.is_empty() && s.resolved[0]);
        apply(&mut s, r.start..r.start, "typed\n");
        assert_eq!(&s.text[s.regions[s.conflicts[0]].range.clone()], "typed\n");
    }

    #[test]
    fn picking_lines_builds_the_conflict_from_them() {
        let mut s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(false)));
        let (r, t) = s.click(Side::Theirs, 8).unwrap(); // "i theirs" (conflict 1)
        assert_eq!(t, "i theirs\n");
        apply(&mut s, r, &t);
        assert_eq!(s.current, 1);
        let (r, t) = s.click(Side::Ours, 8).unwrap();
        assert_eq!(t, "i theirs\ni ours\n"); // the side picked first comes first
        apply(&mut s, r, &t);
        assert!(s.pane_tints(Role::Ours).contains(&(8..9, Tint::OursPicked)));
        let (r, t) = s.click(Side::Theirs, 8).unwrap();
        apply(&mut s, r, &t);
        let (r, t) = s.click(Side::Ours, 8).unwrap(); // unpicking everything restores the markers
        assert!(t.starts_with("<<<<<<< HEAD\n"));
        apply(&mut s, r, &t);
        assert!(s.click(Side::Ours, 2).is_none()); // outside a conflict
    }

    #[test]
    fn navigation_wraps_and_finds_unresolved() {
        let mut s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(false)));
        assert_eq!(s.go(-1), Some(1));
        assert_eq!(s.go(1), Some(1));
        let (r, t) = s.take(Take::Ours, 1).unwrap();
        apply(&mut s, r, &t);
        assert_eq!(s.next_unresolved(), Some(0));
        s.current = 1;
        assert_eq!(s.next_unresolved(), Some(0));
    }

    #[test]
    fn a_file_edited_outside_keeps_its_text_and_tracks_its_markers() {
        let disk = git_style(false).replace("c\n", "c edited in vim\n");
        let s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&disk));
        assert!(s.edited_outside && s.text == disk && s.conflict_count() == 2);
        assert!(s.regions[s.conflicts[0]].chunk.is_some()); // matched to chunks: still pickable
    }

    #[test]
    fn crlf_and_no_final_newline() {
        let disk = "a\r\n<<<<<<< HEAD\r\nB ours\r\n=======\r\nB theirs\r\n>>>>>>> topic\r\nc\r\n";
        let s = Session::build(Some("a\r\nb\r\nc\r\n".into()), "a\r\nB ours\r\nc\r\n", "a\r\nB theirs\r\nc\r\n", Some(disk));
        assert!(!s.edited_outside);
        assert_eq!(s.text, disk);
        let s = Session::build(Some("x\ny".into()), "x\ny ours", "x\ny theirs", Some("x\n<<<<<<< HEAD\ny ours\n=======\ny theirs\n>>>>>>> topic\n"));
        assert!(s.no_final_newline);
        let mut s = s;
        let (r, t) = s.take(Take::Ours, 0).unwrap();
        apply(&mut s, r, &t);
        assert_eq!(s.file_text(), "x\ny ours");
    }

    #[test]
    fn cleans_ai_replies_but_keeps_indentation() {
        assert_eq!(clean_suggestion("\u{1B}[1m```rust\n    let x = 1;\n```\n\n"), "    let x = 1;");
        assert_eq!(clean_suggestion("\n\n"), "");
        let s = Session::build(Some(BASE.into()), OURS, THEIRS, Some(&git_style(false)));
        let p = s.prompt("f.txt", 0).unwrap();
        assert!(p.contains("Base:\nb\nOurs:\nb ours\nTheirs:\nb theirs\nAfter:\nc\n"));
    }

    /// The fixture's merges, by real git: merge style, diff3 and CRLF files must agree with the chunks, so the
    /// first write changes only what the user resolved.
    #[test]
    fn agrees_with_what_git_writes() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let g = |args: &[&str]| { std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap() };
        let w = |n: &str, s: &str| std::fs::write(dir.join(n), s).unwrap();
        g(&["init", "-q", "-b", "main"]);
        w("n.txt", "header\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\nfooter\n");
        w("c.txt", "a\r\nb\r\nc\r\n");
        g(&["add", "."]); g(&["commit", "-qm", "base"]); g(&["switch", "-qc", "topic"]);
        w("n.txt", "header\none\ntwo theirs\nthree\nFOUR theirs\nfive\nsix\nSEVEN both\neight\nshared top\nten theirs\nshared bottom\neleven\ntwelve\nfooter\n");
        w("c.txt", "a\r\nB theirs\r\nc\r\n");
        g(&["commit", "-qam", "topic"]); g(&["switch", "-q", "main"]);
        w("n.txt", "header\nONE ours\ntwo ours\nthree\nfour\nfive\nsix\nSEVEN both\neight\nshared top\nten ours\nshared bottom\neleven\ntwelve\nfooter\n");
        w("c.txt", "a\r\nB ours\r\nc\r\n");
        g(&["commit", "-qam", "main"]);
        for style in ["merge", "diff3", "zdiff3"] {
            g(&["-c", &format!("merge.conflictStyle={style}"), "merge", "topic"]);
            for f in ["n.txt", "c.txt"] {
                let l = load(dir, f);
                let s = l.session.unwrap_or_else(|| panic!("{f} is a text conflict"));
                assert!(!s.edited_outside, "{style} {f}: {:?}", l.disk);
                assert_eq!(s.text, l.disk.unwrap(), "{style} {f}");
            }
            g(&["merge", "--abort"]);
        }
    }

    #[test]
    fn line_lookup() {
        let st = line_starts("ab\ncd\n");
        assert_eq!(st, [0, 3, 6]);
        assert_eq!((line_at(&st, 0), line_at(&st, 4), line_at(&st, 6)), (0, 1, 1));
    }
}
