//! The decisions ChangeOverview.swift made while drawing the lenses' report, without the drawing: what each row says,
//! what folds away, how two versions of a definition line up. The app's overview view renders from these.
//! ponytail: no HighlightCache: `syntax::highlight_snippet` takes microseconds here, so snippets are highlighted per draw.

use crate::lens::{BranchStatus, ChangedPath, DBColumn, DBData, DBObject, DBTable, FactKind, LensFact, LensReport, ReportKind, plural};
use crate::{file_name, parent};

/// The tiles in the summary strip: the files lens, and the others only when there are two or more (alone, a lens's
/// tile would repeat its section's heading).
pub fn summary_tiles(reports: &[LensReport]) -> Vec<&LensReport> {
    let others: Vec<&LensReport> = reports.iter().filter(|r| r.kind != ReportKind::Files).collect();
    reports.iter().filter(|r| r.kind == ReportKind::Files).chain(if others.len() > 1 { others } else { Vec::new() }).collect()
}

/// The sections below the strip: every lens but files, which lives in its tile.
pub fn sections(reports: &[LensReport]) -> impl Iterator<Item = &LensReport> {
    reports.iter().filter(|r| r.kind != ReportKind::Files)
}

/// "Nothing structural beyond the files: no schema, dependency or configuration changes."
pub fn nothing_structural(reports: &[LensReport]) -> bool {
    !reports.iter().any(|r| r.kind != ReportKind::Files && r.kind != ReportKind::FollowUps)
}

pub const NOTHING_STRUCTURAL: &str = "Nothing structural beyond the files: no schema, dependency or configuration changes.";
pub const READING: &str = "Reading the change…";

/// "1 table ~1" says the same thing twice: then a tile shows the summary instead of the deltas.
pub fn tile_restates(r: &LensReport) -> bool {
    let m = &r.metric;
    r.kind != ReportKind::Files && [m.added, m.removed, m.changed].iter().filter(|&&n| n > 0).count() <= 1
}

/// "N to read" beside the files count, when some of the files are generated.
pub fn files_to_read(r: &LensReport) -> Option<usize> {
    let generated = r.kinds.get("Generated").map_or(0, Vec::len);
    (generated > 0).then(|| r.metric.value - generated)
}

/// "● 2 SQL", "● 1 test", "● 3 docs".
pub fn kind_chip_label(f: &LensFact) -> String {
    let n = f.count.unwrap_or(0);
    let word = if f.title == "SQL" {
        "SQL".to_string()
    } else if n == 1 {
        match f.title.as_str() {
            "Tests" => "test".into(),
            "Dependencies" => "dependency".into(),
            "Docs" => "doc".into(),
            "Assets" => "asset".into(),
            t => t.to_lowercase(),
        }
    } else {
        f.title.to_lowercase()
    };
    format!("{n} {word}")
}

/// What happened to a file, for the mark beside it.
pub fn change_kind(c: &ChangedPath) -> FactKind {
    match (&c.old, &c.new) {
        (None, _) => FactKind::Added,
        (_, None) => FactKind::Removed,
        (Some(o), Some(n)) if o != n => FactKind::Renamed,
        _ => FactKind::Changed,
    }
}

// MARK: Database

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableRow {
    /// An index into `DBTable::columns`.
    Column(usize),
    /// A run of this many untouched columns, folded.
    Gap(usize),
}

/// In a changed table, runs of three or more untouched columns fold into one row.
pub fn table_rows(t: &DBTable, show_all: bool) -> Vec<TableRow> {
    if show_all || t.kind != FactKind::Changed {
        return (0..t.columns.len()).map(TableRow::Column).collect();
    }
    let (mut out, mut run) = (Vec::new(), Vec::new());
    let flush = |out: &mut Vec<TableRow>, run: &mut Vec<usize>| {
        if run.len() >= 3 {
            out.push(TableRow::Gap(run.len()));
        } else {
            out.extend(run.iter().map(|&i| TableRow::Column(i)));
        }
        run.clear();
    };
    for (i, c) in t.columns.iter().enumerate() {
        if c.kind == FactKind::Info {
            run.push(i);
        } else {
            flush(&mut out, &mut run);
            out.push(TableRow::Column(i));
        }
    }
    flush(&mut out, &mut run);
    out
}

/// How many rows a table shows before "Show N More Columns": all of a changed one.
pub fn table_row_limit(t: &DBTable, show_all: bool) -> usize {
    if t.kind == FactKind::Changed || show_all { usize::MAX } else { 12 }
}

/// Only a key, index or trigger changed: the columns say nothing and aren't shown.
pub fn table_columns_hidden(t: &DBTable, show_all: bool) -> bool {
    t.kind == FactKind::Changed && !show_all && !t.extras.is_empty() && t.columns.iter().all(|c| c.kind == FactKind::Info)
}

pub fn table_status_label(t: &DBTable) -> &'static str {
    if t.kind == FactKind::Removed { "Dropped" } else { t.kind.label() }
}

pub fn table_sql_title(t: &DBTable) -> &'static str {
    if t.kind == FactKind::Removed { "Definition before this change" } else { "Definition" }
}

/// In a new or dropped table every column shares the table's fate; only columns that changed on their own get a
/// mark. Returns (own mark, drawn quiet).
pub fn column_marks(c: &DBColumn, table: FactKind) -> (bool, bool) {
    let own = table == FactKind::Changed && c.kind != FactKind::Info;
    (own, table == FactKind::Removed || (table == FactKind::Changed && !own))
}

/// The line under a column: what a new one is, or what changed (the type is shown beside the name already).
pub fn column_notes(c: &DBColumn, table: FactKind) -> Vec<String> {
    let (own, _) = column_marks(c, table);
    if own && c.kind == FactKind::Added {
        [c.not_null.then(|| "not null".to_string()), c.default_value.as_ref().map(|d| format!("default {d}")), c.references.as_ref().map(|r| format!("references {r}"))]
            .into_iter()
            .flatten()
            .collect()
    } else {
        c.notes.iter().filter(|n| !n.starts_with("type ")).cloned().collect()
    }
}

/// "old → new" or the type alone.
pub fn column_type(c: &DBColumn) -> String {
    c.old_type.as_ref().map_or(c.ty.clone(), |o| format!("{o} → {}", c.ty))
}

/// How a key, constraint, index or trigger's definition shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtraBody {
    /// A long one (a CHECK, say) as the SQL it was: before above after.
    Diff { before: String, after: String },
    Code { text: String, kind: FactKind },
    /// A short one on one line.
    Inline(String),
    None,
}

pub fn extra_body(f: &LensFact) -> ExtraBody {
    if f.tag.as_deref() == Some("trigger") {
        return ExtraBody::None;
    }
    let (b, a) = (f.before.as_deref(), f.after.as_deref());
    let long = b.unwrap_or("").chars().count() + a.unwrap_or("").chars().count() > 90 || b.or(a).unwrap_or("").contains('\n');
    match (b, a) {
        (Some(b), Some(a)) if long => ExtraBody::Diff { before: b.into(), after: a.into() },
        (Some(b), None) if long => ExtraBody::Code { text: b.into(), kind: FactKind::Removed },
        (None, Some(a)) if long => ExtraBody::Code { text: a.into(), kind: FactKind::Added },
        (Some(b), Some(a)) => ExtraBody::Inline(format!("{b} → {a}")),
        (Some(v), None) | (None, Some(v)) => ExtraBody::Inline(v.into()),
        (None, None) => ExtraBody::None,
    }
}

/// The icon an extra row uses (SF Symbol names, as hints).
pub fn extra_icon(f: &LensFact) -> &'static str {
    match f.tag.as_deref() {
        Some("index") => "list.bullet",
        Some("trigger") => "bolt",
        Some("key") => "key",
        _ => "checkmark.shield",
    }
}

/// Functions, views and triggers are code: their definitions show as code, behind a disclosure.
pub fn object_is_code(o: &DBObject) -> bool {
    matches!(o.ty.as_str(), "function" | "view" | "trigger")
}

/// A short definition is the change itself: shown, not behind a click.
pub fn object_open_by_default(o: &DBObject) -> bool {
    o.after.as_deref().or(o.before.as_deref()).unwrap_or("").split('\n').filter(|l| !l.is_empty()).count() <= 25
}

pub fn object_status_label(o: &DBObject) -> &'static str {
    match o.kind {
        FactKind::Removed => "Dropped",
        FactKind::Changed if object_is_code(o) => "Redefined",
        k => k.label(),
    }
}

/// The one-line value of a type or index object: "a, b  →  a, b, c".
pub fn object_inline(o: &DBObject) -> Option<String> {
    if object_is_code(o) {
        return None;
    }
    let a = o.after.as_ref().or(o.before.as_ref())?;
    Some(match (&o.before, &o.after) {
        (Some(b), Some(_)) => format!("{b}  →  {a}"),
        _ => a.clone(),
    })
}

/// "Insert into ", "Delete from ", "Update ": before the table's name in a data row.
pub fn data_verb(d: &DBData) -> String {
    let mut v = d.verb.to_lowercase();
    if let Some(f) = v.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    v + match d.verb.as_str() {
        "INSERT" => " into ",
        "DELETE" => " from ",
        _ => " ",
    }
}

/// "2 statements changed".
pub fn data_summary(d: &DBData) -> String {
    let what = match d.kind {
        FactKind::Added => "added",
        FactKind::Removed => "removed",
        _ => "changed",
    };
    format!("{} {what}", plural(d.count(), "statement"))
}

/// The statements a data row discloses: (before, after) pairs for a changed one, else one side.
pub fn data_statements(d: &DBData) -> Vec<(Option<&str>, Option<&str>)> {
    match d.kind {
        FactKind::Changed => d.before.iter().zip(&d.after).map(|(b, a)| (Some(b.as_str()), Some(a.as_str()))).collect(),
        FactKind::Added => d.after.iter().map(|a| (None, Some(a.as_str()))).collect(),
        _ => d.before.iter().map(|b| (Some(b.as_str()), None)).collect(),
    }
}

/// The file a warning names at its right, None when its text already does. A migration's directory names it better
/// than its up.sql.
pub fn warning_file_label(f: &LensFact) -> Option<String> {
    let p = f.path.as_deref()?;
    let name = file_name(p);
    let file = if ["up.sql", "down.sql"].contains(&name.to_lowercase().as_str()) { file_name(parent(p)) } else { name };
    (!f.notes.iter().any(|n| n.contains(file))).then(|| file.to_string())
}

/// "a.rs:12" for a follow-up row.
pub fn location_label(f: &LensFact) -> String {
    format!("{}:{}", file_name(f.path.as_deref().unwrap_or("")), f.line.unwrap_or(1))
}

// MARK: Branch

/// "3 commits behind main"
pub fn branch_headline(s: &BranchStatus) -> String {
    format!("{} behind {}", plural(s.behind, "commit"), s.target)
}

/// "None of them change these files" / "All of them…" / "2 of them…"
pub fn branch_touching(s: &BranchStatus) -> String {
    if s.touching.is_empty() {
        "None of them change these files".into()
    } else if s.touching.len() == s.behind {
        "All of them change these files".into()
    } else {
        format!("{} of them change these files", s.touching.len())
    }
}

/// "Merges cleanly" / "2 conflicting files"; None when git couldn't tell.
pub fn branch_merge_label(s: &BranchStatus) -> Option<String> {
    s.conflicts.as_ref().map(|c| if c.is_empty() { "Merges cleanly".into() } else { plural(c.len(), "conflicting file") })
}

/// Who changed a conflicting file on the other side: whom to ask about the conflict.
pub fn conflict_authors(s: &BranchStatus, file: &str) -> Option<String> {
    let mut by: Vec<&str> = s.touching.iter().filter(|c| c.files.iter().any(|f| f == file)).map(|c| c.author.as_str()).collect();
    by.sort();
    by.dedup();
    (!by.is_empty()).then(|| format!("changed on {} by {}", s.target, by.join(", ")))
}

// MARK: Code snippets

/// A snippet's lines as a code block shows them: blank lines trimmed off both ends, and the indentation every line
/// shares removed.
pub fn code_block_lines(text: &str) -> Vec<String> {
    let blank = |l: &&str| l.trim_matches([' ', '\t']).is_empty();
    let mut l: Vec<&str> = text.split('\n').collect();
    while l.last().is_some_and(blank) {
        l.pop();
    }
    let start = l.iter().position(|x| !blank(x)).unwrap_or(l.len());
    let l = &l[start..];
    let lead = |s: &str| s.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let indent = l.iter().filter(|x| !blank(x)).map(|x| lead(x)).min().unwrap_or(0);
    l.iter().map(|x| x.chars().skip(indent.min(lead(x))).collect()).collect()
}

/// A code block shows this many lines before "Show All N Lines".
pub fn code_block_shown(lines: usize, max: usize, expanded: bool) -> usize {
    if expanded { lines } else { lines.min(max) }
}

/// Two versions of a short text (a constraint, a function) as one diff: lines matched ignoring their indentation,
/// so re-nesting doesn't paint everything; the new version's lines shown, the old one's where they're gone.
/// Marks: ' ' same, '+' added, '-' removed.
pub fn line_diff(before: &str, after: &str) -> Vec<(char, String)> {
    let (a, b): (Vec<&str>, Vec<&str>) = (before.split('\n').collect(), after.split('\n').collect());
    let ka: Vec<&str> = a.iter().map(|l| l.trim_matches([' ', '\t'])).collect();
    let kb: Vec<&str> = b.iter().map(|l| l.trim_matches([' ', '\t'])).collect();
    // Longest common subsequence over the trimmed lines; these texts are a few dozen lines.
    let mut l = vec![vec![0usize; kb.len() + 1]; ka.len() + 1];
    for i in (0..ka.len()).rev() {
        for j in (0..kb.len()).rev() {
            l[i][j] = if ka[i] == kb[j] { l[i + 1][j + 1] + 1 } else { l[i + 1][j].max(l[i][j + 1]) };
        }
    }
    let (mut out, mut i, mut j) = (Vec::new(), 0, 0);
    while i < ka.len() || j < kb.len() {
        if i < ka.len() && j < kb.len() && ka[i] == kb[j] {
            out.push((' ', b[j].to_string()));
            i += 1;
            j += 1;
        } else if j < kb.len() && (i == ka.len() || l[i][j + 1] >= l[i + 1][j]) {
            out.push(('+', b[j].to_string()));
            j += 1;
        } else {
            out.push(('-', a[i].to_string()));
            i += 1;
        }
    }
    out
}

// MARK: Layout

/// Cards in as many columns as fit (at least `min_width` each, at most `max_width`, so a few cards on a wide window
/// stay readable): the column count and width.
pub fn masonry_columns(width: f32, cards: usize, min_width: f32, max_width: f32, spacing: f32) -> (usize, f32) {
    let n = ((width + spacing) / (min_width + spacing)).floor().max(0.0) as usize;
    let n = n.min(cards).max(1);
    (n, max_width.min((width - (n - 1) as f32 * spacing) / n as f32))
}

/// Each card under the currently shortest column: (x, y) per card from their heights at `masonry_columns`' width.
pub fn masonry_place(columns: usize, width: f32, spacing: f32, heights: &[f32]) -> Vec<(f32, f32)> {
    let mut col = vec![0f32; columns.max(1)];
    heights
        .iter()
        .map(|h| {
            let c = (0..col.len()).fold(0, |best, i| if col[i] < col[best] { i } else { best });
            let at = (c as f32 * (width + spacing), col[c]);
            col[c] += h + spacing;
            at
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_diff_ignores_reindentation() {
        let d = line_diff("CHECK (\n    a\n    AND b\n)", "CHECK (\n    (\n        a\n        AND b\n    )\n    OR c\n)");
        assert!(!d.iter().any(|r| r.0 == '-'));
        let mut added: Vec<&str> = d.iter().filter(|r| r.0 == '+').map(|r| r.1.trim()).collect();
        added.sort();
        assert_eq!(added, ["(", ")", "OR c"]);
    }

    #[test]
    fn folds_untouched_columns_and_dedents_code() {
        let col = |n: &str, kind| DBColumn {
            name: n.into(),
            ty: "int".into(),
            old_type: None,
            not_null: false,
            default_value: None,
            references: None,
            primary: false,
            kind,
            notes: vec![],
            path: None,
            line: None,
        };
        let t = DBTable {
            name: "t".into(),
            old_name: None,
            kind: FactKind::Changed,
            columns: vec![col("a", FactKind::Info), col("b", FactKind::Info), col("c", FactKind::Info), col("d", FactKind::Added), col("e", FactKind::Info)],
            extras: vec![],
            sql: None,
            path: None,
            line: None,
        };
        assert_eq!(table_rows(&t, false), [TableRow::Gap(3), TableRow::Column(3), TableRow::Column(4)]);
        assert_eq!(table_rows(&t, true).len(), 5);
        assert_eq!(code_block_lines("\n\n    a\n      b\n\n"), ["a", "  b"]);
        assert_eq!(masonry_columns(1000.0, 5, 300.0, 640.0, 12.0), (3, (1000.0 - 24.0) / 3.0));
        assert_eq!(masonry_place(2, 100.0, 10.0, &[50.0, 20.0, 5.0]), [(0.0, 0.0), (110.0, 0.0), (110.0, 30.0)]);
    }
}
