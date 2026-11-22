//! The structural story of a change, without AI (ChangeLens.swift and Lenses.swift): each lens recognises one kind of
//! file and reports exact facts. Everything here blocks on git and parsing: run it on the background executor.

use crate::sql::{self, Schema, Statement};
use crate::{extension, file_name, parent, yaml};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

// MARK: - States A and B

/// One side of a change: a commit, the working tree, or nothing (before a root commit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snapshot {
    Rev(String),
    Worktree,
    Empty,
}

impl Snapshot {
    /// Every file path in this state.
    pub fn list(&self, dir: &str) -> Vec<String> {
        let split = |d: Vec<u8>| d.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>();
        match self {
            Snapshot::Rev(r) => split(crate::git::run(&["ls-tree", "-r", "-z", "--name-only", r], dir, None, false).unwrap_or_default()),
            Snapshot::Worktree => split(crate::git::run(&["ls-files", "-z", "--cached", "--others", "--exclude-standard"], dir, None, false).unwrap_or_default())
                .into_iter()
                .filter(|p| std::path::Path::new(dir).join(p).exists())
                .collect(),
            Snapshot::Empty => Vec::new(),
        }
    }

    /// File contents (text files only; binary and missing ones are left out). One git process for any number of files.
    pub fn read(&self, paths: &[String], dir: &str, max_size: usize) -> HashMap<String, String> {
        if paths.is_empty() {
            return HashMap::new();
        }
        match self {
            Snapshot::Empty => HashMap::new(),
            Snapshot::Worktree => paths
                .iter()
                .filter_map(|p| {
                    let d = std::fs::read(std::path::Path::new(dir).join(p)).ok()?;
                    (d.len() <= max_size && !d[..d.len().min(8000)].contains(&0)).then(|| (p.clone(), String::from_utf8_lossy(&d).into_owned()))
                })
                .collect(),
            Snapshot::Rev(r) => {
                let mut input: String = paths.iter().map(|p| format!("{r}:{p}")).collect::<Vec<_>>().join("\n");
                input.push('\n');
                match crate::git::run(&["cat-file", "--batch"], dir, Some(input.as_bytes()), false) {
                    Ok(data) => parse_batch(&data, paths, max_size),
                    Err(_) => HashMap::new(),
                }
            }
        }
    }
}

/// `git cat-file --batch` output: "<sha> <type> <size>\n<content>\n" per request, "<req> missing\n" when absent.
pub fn parse_batch(data: &[u8], paths: &[String], max_size: usize) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let (mut i, mut k) = (0, 0);
    while i < data.len() && k < paths.len() {
        let Some(nl) = data[i..].iter().position(|&b| b == b'\n').map(|p| p + i) else { break };
        let header = String::from_utf8_lossy(&data[i..nl]).into_owned();
        let header: Vec<&str> = header.split(' ').filter(|s| !s.is_empty()).collect();
        i = nl + 1;
        k += 1;
        let Some(size) = (header.len() == 3).then(|| header[2].parse::<usize>().ok()).flatten() else { continue }; // "missing"
        let end = (i + size).min(data.len());
        let body = &data[i..end];
        if header[1] == "blob" && size <= max_size && !body[..body.len().min(8000)].contains(&0) {
            out.insert(paths[k - 1].clone(), String::from_utf8_lossy(body).into_owned());
        }
        i = end + 1; // the trailing newline
    }
    out
}

/// A file the change touches; None on the side where it doesn't exist.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangedPath {
    pub old: Option<String>,
    pub new: Option<String>,
}

impl ChangedPath {
    pub fn path(&self) -> &str {
        self.new.as_deref().or(self.old.as_deref()).unwrap_or("")
    }
}

// MARK: - Facts

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FactKind {
    Added,
    Removed,
    Changed,
    Renamed,
    Info,
    Warning,
}

impl FactKind {
    pub fn label(self) -> &'static str {
        match self {
            FactKind::Added => "New",
            FactKind::Removed => "Removed",
            FactKind::Changed => "Modified",
            FactKind::Renamed => "Renamed",
            FactKind::Info => "Unchanged",
            FactKind::Warning => "Warning",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LensFact {
    pub kind: FactKind,
    /// the thing: a table, column, package, function, key
    pub title: String,
    /// what happened to it
    pub detail: Option<String>,
    /// where to look
    pub path: Option<String>,
    pub line: Option<usize>,
    /// what sort of thing it is ("function", "index") or how big ("major")
    pub tag: Option<String>,
    /// the old value or code, when it's worth showing
    pub before: Option<String>,
    pub after: Option<String>,
    pub count: Option<usize>,
    /// more about it, a line each: what a trigger runs, what a function raises
    pub notes: Vec<String>,
}

impl LensFact {
    pub fn new(kind: FactKind, title: impl Into<String>) -> Self {
        Self { kind, title: title.into(), detail: None, path: None, line: None, tag: None, before: None, after: None, count: None, notes: Vec::new() }
    }
    fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    fn at(mut self, path: Option<&str>, line: Option<usize>) -> Self {
        self.path = path.map(str::to_string);
        self.line = line;
        self
    }
    fn tag(mut self, t: impl Into<String>) -> Self {
        self.tag = Some(t.into());
        self
    }
    fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LensSection {
    pub title: String,
    pub facts: Vec<LensFact>,
}

/// The headline number of a report, for the strip at the top of the overview.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LensMetric {
    pub value: usize,
    pub unit: String,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Files,
    Database,
    Code,
    Dependencies,
    Configuration,
    FollowUps,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LensReport {
    pub kind: ReportKind,
    pub title: String,
    /// The SF Symbol the Swift app showed; a hint for the UI's own icon.
    pub icon: &'static str,
    /// one line for the collapsed card
    pub summary: String,
    pub sections: Vec<LensSection>,
    pub metric: LensMetric,
    pub database: Vec<DBGroup>,
    /// the files lens: the files of each kind
    pub kinds: BTreeMap<String, Vec<ChangedPath>>,
}

impl LensReport {
    fn new(kind: ReportKind, title: &str, icon: &'static str, summary: String, sections: Vec<LensSection>, metric: LensMetric) -> Self {
        Self { kind, title: title.into(), icon, summary, sections, metric, database: Vec::new(), kinds: BTreeMap::new() }
    }
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {}", if n == 1 { word.to_string() } else { format!("{word}s") })
}

fn counts(facts: &[&LensFact]) -> (usize, usize, usize) {
    let c = |k| facts.iter().filter(|f| f.kind == k).count();
    (c(FactKind::Added), c(FactKind::Removed), c(FactKind::Changed))
}

/// `upstream`: the tip of the branch the change goes into, when that's further on than A (a pull request's base
/// branch): migrations are checked against what it already has.
pub fn analyze(changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, upstream: Option<&str>, dir: &str) -> Vec<LensReport> {
    let (sql, deps, config, lines, generated) = std::thread::scope(|s| {
        let sql = s.spawn(|| sql_report(changes, a, b, upstream, dir));
        let deps = s.spawn(|| dependency_report(changes, a, b, dir));
        let config = s.spawn(|| config_report(changes, a, b, dir));
        let lines = s.spawn(|| changed_lines(changes, a, b, dir));
        let generated = generated(changes, b, dir);
        (sql.join().unwrap(), deps.join().unwrap(), config.join().unwrap(), lines.join().unwrap(), generated)
    });
    let files = files_report(changes, &generated);
    let code = code_report(&lines, changes, a, b, dir);
    let notes = follow_up_report(&lines.added, |p| generated.contains(p) || ["Generated", "Docs"].contains(&category(p, false)));
    std::iter::once(files).chain([sql, code, deps, config, notes].into_iter().flatten()).collect()
}

// MARK: - Files

pub const FILE_KINDS: [&str; 9] = ["Code", "Tests", "SQL", "Dependencies", "Config", "Build & CI", "Docs", "Assets", "Generated"];

fn words(s: &str, sep: &[char]) -> Vec<String> {
    s.split(sep).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// What sort of file a path is. `generated`: the file says a tool wrote it (see `generated`).
pub fn category(p: &str, generated: bool) -> &'static str {
    let l = p.to_lowercase();
    let (name, ext) = (file_name(&l), extension(&l));
    let mut dirs = words(&l, &['/']);
    dirs.pop();
    let in_dir = |names: &[&str]| dirs.iter().any(|d| names.contains(&d.as_str()));
    if generated || generated_name(&l) {
        return "Generated";
    }
    if in_dir(&["test", "tests", "__tests__", "__mocks__", "spec", "specs", "e2e", "acceptance", "testdata", "testing", "fixtures"]) || ext == "feature" {
        return "Tests";
    }
    if ["md", "mdx", "rst", "txt", "adoc"].contains(&ext) || in_dir(&["docs"]) {
        return "Docs";
    }
    // foo_test.go, foo.test.ts, test_foo.py, foo_e2e.rs, FooTests.swift; not latest.rs or Inspector.swift.
    let stem = file_name(p).split('.').find(|s| !s.is_empty()).unwrap_or("");
    if words(name, &['.', '_', '-']).iter().any(|w| ["test", "tests", "spec", "e2e"].contains(&w.as_str()))
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.ends_with("Spec")
    {
        return "Tests";
    }
    if ext == "sql" {
        return "SQL";
    }
    if ["cargo.toml", "package.json", "go.mod", "package.swift", "pyproject.toml", "requirements.txt", "flake.nix", "gemfile", "composer.json"].contains(&name) {
        return "Dependencies";
    }
    if l.starts_with(".github/") || l.starts_with(".forgejo/") || l.starts_with(".gitea/") || name == "dockerfile" || name.starts_with("dockerfile.") || name.ends_with(".nix") {
        return "Build & CI";
    }
    // An animation's JSON under assets/ is content the app ships, not configuration.
    if ["json", "xml"].contains(&ext) && in_dir(&["assets", "public", "static", "res", "resources"]) {
        return "Assets";
    }
    if ["json", "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "plist", "xml", "properties"].contains(&ext) || name.starts_with(".env") {
        return "Config";
    }
    if ["png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "pdf", "ttf", "otf", "woff", "woff2", "mp4", "mov", "zip"].contains(&ext) {
        return "Assets";
    }
    "Code"
}

/// Lock files and the usual names of generated code (a lowercased path).
pub fn generated_name(l: &str) -> bool {
    let (name, ext) = (file_name(l), extension(l));
    if ["lock", "lockb", "lockfile"].contains(&ext)
        || name.ends_with("-lock.json")
        || name.ends_with("-lock.yaml")
        || ["go.sum", "package.resolved", "cartfile.resolved", "packages.lock.json", "npm-shrinkwrap.json"].contains(&name)
    {
        return true;
    }
    if name.contains(".min.") || name.contains(".generated.") || [".pb.go", "_pb2.py", ".pb.swift", ".g.dart", ".freezed.dart"].iter().any(|s| name.ends_with(s)) {
        return true;
    }
    let mut dirs = words(l, &['/']);
    dirs.pop();
    dirs.iter().any(|d| d == "generated" || d == "__generated__")
}

static GENERATED_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)@generated|do not edit|code generated|auto-?generated|automatically generated").unwrap());

/// Changed files that say in their first lines that a tool wrote them.
pub fn generated(changes: &[ChangedPath], b: &Snapshot, dir: &str) -> HashSet<String> {
    let paths: Vec<String> = changes.iter().filter_map(|c| c.new.clone()).filter(|p| !generated_name(&p.to_lowercase()) && category(p, false) != "Assets").take(1000).collect();
    b.read(&paths, dir, 1_000_000).into_iter().filter(|(_, t)| t.split('\n').take(5).any(|l| GENERATED_MARK.is_match(l))).map(|(p, _)| p).collect()
}

pub fn files_report(changes: &[ChangedPath], generated: &HashSet<String>) -> LensReport {
    let added = changes.iter().filter(|c| c.old.is_none()).count();
    let removed = changes.iter().filter(|c| c.new.is_none()).count();
    let renamed = changes.iter().filter(|c| c.old.is_some() && c.new.is_some() && c.old != c.new).count();
    let modified = changes.len() - added - removed - renamed;
    let mut groups: BTreeMap<String, Vec<ChangedPath>> = BTreeMap::new();
    for c in changes {
        groups.entry(category(c.path(), generated.contains(c.path())).into()).or_default().push(c.clone());
    }
    let facts = FILE_KINDS
        .iter()
        .filter_map(|&k| {
            let g = groups.get(k)?;
            let (a, r) = (g.iter().filter(|c| c.old.is_none()).count(), g.iter().filter(|c| c.new.is_none()).count());
            let mut parts = vec![plural(g.len(), "file")];
            if a > 0 {
                parts.push(format!("{a} new"));
            }
            if r > 0 {
                parts.push(format!("{r} deleted"));
            }
            Some(LensFact::new(FactKind::Info, k).detail(parts.join(" · ")).count(g.len()))
        })
        .collect();
    let mut summary = vec![plural(changes.len(), "file")];
    for (n, w) in [(added, "added"), (removed, "deleted"), (renamed, "renamed"), (modified, "modified")] {
        if n > 0 {
            summary.push(format!("{n} {w}"));
        }
    }
    let metric = LensMetric { value: changes.len(), unit: if changes.len() == 1 { "file" } else { "files" }.into(), added, removed, changed: modified + renamed };
    let mut r = LensReport::new(ReportKind::Files, "Files", "doc.on.doc", summary.join(" · "), vec![LensSection { title: "By kind".into(), facts }], metric);
    r.kinds = groups;
    r
}

// MARK: - SQL

#[derive(Clone, Debug, PartialEq)]
pub struct DBColumn {
    pub name: String,
    pub ty: String,
    /// when the type changed
    pub old_type: Option<String>,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub references: Option<String>,
    pub primary: bool,
    /// Info: unchanged
    pub kind: FactKind,
    /// what changed, in words
    pub notes: Vec<String>,
    pub path: Option<String>,
    pub line: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DBTable {
    pub name: String,
    pub old_name: Option<String>,
    pub kind: FactKind,
    pub columns: Vec<DBColumn>,
    /// keys, constraints, indexes, triggers
    pub extras: Vec<LensFact>,
    /// the CREATE TABLE as written; the old one for a dropped table
    pub sql: Option<String>,
    pub path: Option<String>,
    pub line: Option<usize>,
}

/// Enum types, views, functions, and indexes or triggers on tables the schema doesn't define.
#[derive(Clone, Debug, PartialEq)]
pub struct DBObject {
    pub ty: String,
    pub name: String,
    pub kind: FactKind,
    pub detail: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub notes: Vec<String>,
    pub path: Option<String>,
    pub line: Option<usize>,
}

/// INSERT / UPDATE / DELETE statements on one table that appear, disappear or change in one file.
#[derive(Clone, Debug, PartialEq)]
pub struct DBData {
    pub verb: String,
    pub table: String,
    pub kind: FactKind,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub path: String,
    pub line: Option<usize>,
}

impl DBData {
    pub fn title(&self) -> String {
        let mid = match self.verb.as_str() {
            "INSERT" => " into ",
            "DELETE" => " from ",
            _ => " ",
        };
        format!("{}{mid}{}", self.verb, self.table)
    }
    pub fn count(&self) -> usize {
        self.before.len().max(self.after.len())
    }
}

/// One schema (a migrations directory), A → B.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DBGroup {
    pub root: String,
    pub tables: Vec<DBTable>,
    pub objects: Vec<DBObject>,
    pub data: Vec<DBData>,
    pub warnings: Vec<LensFact>,
}

impl DBGroup {
    /// The same, as flat facts: for the command line and tests.
    pub fn facts(&self) -> Vec<LensFact> {
        let mut out = Vec::new();
        for t in &self.tables {
            let at = |f: LensFact| f.at(t.path.as_deref(), t.line);
            match t.kind {
                FactKind::Added => out.push(at(LensFact::new(FactKind::Added, &t.name).detail(format!(
                    "table added · {}: {}",
                    plural(t.columns.len(), "column"),
                    t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                )))),
                FactKind::Removed => out.push(at(LensFact::new(FactKind::Removed, &t.name).detail(format!("table dropped · had {}", plural(t.columns.len(), "column"))))),
                FactKind::Renamed => out.push(at(LensFact::new(FactKind::Renamed, format!("{} → {}", t.old_name.as_deref().unwrap_or("?"), t.name)).detail("table renamed"))),
                _ => {
                    for c in &t.columns {
                        let title = format!("{}.{}", t.name, c.name);
                        let detail = match c.kind {
                            FactKind::Added => {
                                let mut d = format!("column added · {}", c.ty);
                                if c.not_null {
                                    d += " NOT NULL";
                                }
                                if let Some(v) = &c.default_value {
                                    d += &format!(" DEFAULT {v}");
                                }
                                if let Some(r) = &c.references {
                                    d += &format!(" → {r}");
                                }
                                d
                            }
                            FactKind::Removed => format!("column dropped · was {}", c.ty),
                            FactKind::Changed => format!("column changed · {}", c.notes.join(", ")),
                            _ => continue,
                        };
                        out.push(LensFact::new(c.kind, title).detail(detail).at(c.path.as_deref(), c.line));
                    }
                }
            }
            out.extend(t.extras.iter().map(|f| {
                let mut f = f.clone();
                f.title = format!("{}: {}", t.name, f.title);
                f.detail = Some(f.tag.iter().chain(f.detail.iter()).chain(f.notes.iter()).cloned().collect::<Vec<_>>().join(" · "));
                f
            }));
        }
        out.extend(self.objects.iter().map(|o| LensFact::new(o.kind, &o.name).detail(&o.detail).at(o.path.as_deref(), o.line)));
        out.extend(self.data.iter().map(|d| {
            let what = match d.kind {
                FactKind::Added => "added",
                FactKind::Removed => "removed",
                _ => "changed",
            };
            LensFact::new(d.kind, d.title()).detail(format!("{} {what}", plural(d.count(), "data statement"))).at(Some(&d.path), d.line)
        }));
        out.extend(self.warnings.iter().cloned());
        out
    }
}

pub fn is_down(p: &str) -> bool {
    let n = file_name(p).to_lowercase();
    n == "down.sql" || n.ends_with(".down.sql") || n.ends_with("_down.sql") || (n.starts_with('u') && n[1..].chars().next().is_some_and(|c| c.is_numeric())) // flyway undo
}

/// A migration's version, from the leading number of its name: "2026-11-03-100000_x" → [2026, 11, 3, 100000],
/// "V2_1__x.sql" → [2, 1], "0042_x.up.sql" → [42]; None when the name doesn't start with one.
pub fn version(name: &str) -> Option<Vec<u64>> {
    let mut s = name;
    let mut c = s.chars();
    if let (Some(f), Some(n)) = (c.next(), c.next()) {
        if "VvUuRr".contains(f) && n.is_numeric() {
            s = &s[1..];
        }
    }
    let head: String = s.chars().take_while(|c| c.is_ascii_digit() || "-_.".contains(*c)).collect();
    let v: Vec<u64> = head.split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).collect();
    (!v.is_empty()).then_some(v)
}

pub fn sql_report(changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, upstream: Option<&str>, dir: &str) -> Option<LensReport> {
    let sql_changes: Vec<&ChangedPath> = changes.iter().filter(|c| c.path().to_lowercase().ends_with(".sql")).collect();
    if sql_changes.is_empty() {
        return None;
    }
    // Group: Diesel-style "<root>/<migration>/up.sql" replays every "<root>/*/up.sql"; otherwise a directory's .sql files.
    let mut roots: BTreeMap<String, bool> = BTreeMap::new(); // root → diesel layout
    for c in &sql_changes {
        for p in c.old.iter().chain(c.new.iter()) {
            let name = file_name(p).to_lowercase();
            if name == "up.sql" || name == "down.sql" {
                roots.insert(parent(parent(p)).to_string(), true);
            } else {
                roots.entry(parent(p).to_string()).or_insert(false);
            }
        }
    }
    let (list_a, list_b) = (a.list(dir), b.list(dir));
    let list_u = upstream.map(|u| Snapshot::Rev(u.into()).list(dir)).unwrap_or_default();
    let changed_here: HashSet<String> = sql_changes.iter().flat_map(|c| c.old.iter().chain(c.new.iter()).cloned()).collect();
    let mut groups: Vec<DBGroup> = Vec::new();
    let mut renames: BTreeMap<String, String> = BTreeMap::new(); // old → new table name, so seed files elsewhere pair statements across a rename
    // Migrations before seed directories, so their renames are known when the seeds are read.
    let mut ordered: Vec<(String, bool)> = roots.into_iter().collect();
    ordered.sort_by_key(|(k, diesel)| (if *diesel { 0 } else if k.to_lowercase().contains("migrat") { 1 } else { 2 }, k.clone()));
    for (root, diesel) in ordered {
        let members = |list: &[String]| {
            let mut m: Vec<String> = list
                .iter()
                .filter(|p| {
                    if diesel {
                        parent(parent(p)) == root && file_name(p).to_lowercase() == "up.sql"
                    } else {
                        parent(p) == root && p.to_lowercase().ends_with(".sql") && !is_down(p)
                    }
                })
                .cloned()
                .collect();
            m.sort();
            m
        };
        let (ma, mb, mu) = (members(&list_a), members(&list_b), members(&list_u));
        let (text_a, text_b) = std::thread::scope(|s| {
            let ta = s.spawn(|| a.read(&ma, dir, 2_000_000));
            (ta.join().unwrap(), b.read(&mb, dir, 2_000_000))
        });
        let files = |m: &[String], t: &HashMap<String, String>| m.iter().filter_map(|p| t.get(p).map(|x| (p.clone(), x.clone()))).collect::<Vec<_>>();
        let sa = sql::replay(&files(&ma, &text_a));
        let sb = sql::replay(&files(&mb, &text_b));
        let mut g = DBGroup { root: if root.is_empty() { "/".into() } else { root.clone() }, ..Default::default() };
        (g.tables, g.objects) = model(&sa, &sb, &text_a, &text_b);
        let mut all_members = list_a.clone();
        all_members.extend(list_b.iter().cloned());
        let all: HashSet<String> = members(&all_members).into_iter().collect();
        let mut known = renames.clone();
        for t in &g.tables {
            if let Some(o) = &t.old_name {
                known.entry(o.clone()).or_insert(t.name.clone());
            }
        }
        let mut here: Vec<String> = changed_here.iter().filter(|p| all.contains(*p) || parent(p) == root).cloned().collect();
        here.sort();
        g.data = data_changes(&here, &text_a, &text_b, &known);
        // New migrations against the ones already there: the same version, or older than some of them.
        let id = |p: &str| file_name(if diesel { parent(p) } else { p }).to_string();
        let in_a: HashSet<&String> = ma.iter().collect();
        let new: Vec<&String> = mb.iter().filter(|p| !in_a.contains(p) && changed_here.contains(*p)).collect();
        let new_set: HashSet<&String> = new.iter().copied().collect();
        let there: Vec<(String, Vec<u64>)> =
            ma.iter().chain(mu.iter()).collect::<BTreeSet<_>>().into_iter().filter(|p| !new_set.contains(p)).filter_map(|p| version(&id(p)).map(|v| (id(p), v))).collect();
        for p in &new {
            let pid = id(p);
            let Some(v) = version(&pid) else { continue };
            let others: Vec<&(String, Vec<u64>)> = there.iter().filter(|o| o.0 != pid).collect();
            if let Some(same) = others.iter().find(|o| o.1 == v) {
                let mut f = LensFact::new(FactKind::Warning, "Migration version already used").at(Some(p), Some(1));
                f.notes = vec![format!("{pid} has the same version as {}. Tools that track migrations by version take one of them as already run.", same.0)];
                g.warnings.push(f);
            }
            let mut later: Vec<&&(String, Vec<u64>)> = others.iter().filter(|o| v < o.1).collect();
            later.sort_by(|x, y| x.1.cmp(&y.1));
            if let Some(last) = later.last() {
                let mut f = LensFact::new(FactKind::Warning, format!("Migration older than {}", plural(later.len(), "existing one"))).at(Some(p), Some(1));
                let more = if later.len() > 1 { format!(" and {} more", later.len() - 1) } else { String::new() };
                f.notes = vec![format!("{pid} sorts before {}{more}. Databases that already ran those run this one out of order.", last.0)];
                g.warnings.push(f);
            }
        }
        // What the changed migrations cost to run: locks, rewrites, lost data.
        let existing: HashSet<String> = sa.tables.keys().cloned().collect();
        let migration = diesel || root.to_lowercase().contains("migrat");
        for p in mb.iter().filter(|p| changed_here.contains(*p)) {
            let Some(text) = text_b.get(p) else { continue };
            let lines = sql::lines(text);
            let old: HashSet<String> = text_a.get(p).map(|t| sql::statements(t).into_iter().map(|s| s.text).collect()).unwrap_or_default();
            for st in sql::statements(text).iter().filter(|s| !old.contains(&s.text)) {
                for r in sql::risks(st, &existing, migration) {
                    let mut f = LensFact::new(FactKind::Warning, r.title).at(Some(p), Some(st.line));
                    f.after = Some(st.source(&lines));
                    f.notes = vec![r.detail];
                    g.warnings.push(f);
                }
            }
        }
        let mut skipped: BTreeMap<&str, Vec<&sql::Location>> = BTreeMap::new();
        for (kind, at) in sb.skipped.iter().filter(|(_, at)| changed_here.contains(&at.file)) {
            skipped.entry(kind).or_default().push(at);
        }
        for (kind, ws) in skipped {
            let at = ws[0];
            let src = text_b.get(&at.file).and_then(|t| sql::statements(t).into_iter().find(|s| s.line == at.line).map(|s| s.source(&sql::lines(t))));
            let mut f = LensFact::new(FactKind::Warning, format!("{kind} not replayed")).detail(format!("{} not replayed", plural(ws.len(), "statement"))).at(Some(&at.file), Some(at.line)).count(ws.len());
            f.after = src;
            f.notes = vec![format!("Loupe doesn't run procedural SQL, so the schema shown doesn't include {} effect.", if ws.len() == 1 { "its" } else { "their" })];
            g.warnings.push(f);
        }
        for t in &g.tables {
            if let Some(o) = &t.old_name {
                renames.insert(o.clone(), t.name.clone());
            }
        }
        if !(g.tables.is_empty() && g.objects.is_empty() && g.data.is_empty() && g.warnings.is_empty()) {
            groups.push(g);
        }
    }
    if groups.is_empty() {
        return None;
    }
    let tables: Vec<&DBTable> = groups.iter().flat_map(|g| &g.tables).collect();
    let objects: Vec<&DBObject> = groups.iter().flat_map(|g| &g.objects).collect();
    let data: Vec<&DBData> = groups.iter().flat_map(|g| &g.data).collect();
    let tn = |k| tables.iter().filter(|t| t.kind == k).count();
    let (added, removed, renamed, changed) = (tn(FactKind::Added), tn(FactKind::Removed), tn(FactKind::Renamed), tn(FactKind::Changed));
    let mut summary = Vec::new();
    if added > 0 {
        summary.push(plural(added, "new table"));
    }
    if removed > 0 {
        summary.push(plural(removed, "table") + " dropped");
    }
    if renamed > 0 {
        summary.push(plural(renamed, "table") + " renamed");
    }
    if changed > 0 {
        summary.push(plural(changed, "table") + " changed");
    }
    let mut by_type: BTreeMap<&str, Vec<&DBObject>> = BTreeMap::new();
    for o in &objects {
        by_type.entry(&o.ty).or_default().push(o);
    }
    for (ty, list) in by_type {
        let verbs: HashSet<FactKind> = list.iter().map(|o| o.kind).collect();
        let verb = if verbs == [FactKind::Added].into() {
            " added"
        } else if verbs == [FactKind::Removed].into() {
            " dropped"
        } else {
            " changed"
        };
        summary.push(plural(list.len(), ty) + verb);
    }
    if !data.is_empty() {
        summary.push(plural(data.iter().map(|d| d.count()).sum(), "data statement"));
    }
    let metric = if tables.is_empty() {
        let k = |kind| objects.iter().filter(|o| o.kind == kind).count() + data.iter().filter(|d| d.kind == kind).count();
        LensMetric { value: objects.len() + data.len(), unit: "schema changes".into(), added: k(FactKind::Added), removed: k(FactKind::Removed), changed: k(FactKind::Changed) }
    } else {
        LensMetric { value: tables.len(), unit: if tables.len() == 1 { "table" } else { "tables" }.into(), added, removed, changed: changed + renamed }
    };
    let summary = if summary.is_empty() { "SQL changed, schema the same".into() } else { summary.join(" · ") };
    let sections = groups.iter().map(|g| LensSection { title: g.root.clone(), facts: g.facts() }).collect();
    let mut r = LensReport::new(ReportKind::Database, "Database", "cylinder.split.1x2", summary, sections, metric);
    r.database = groups;
    Some(r)
}

/// Schema A → schema B, as flat facts.
pub fn schema_diff(a: &Schema, b: &Schema) -> Vec<LensFact> {
    let mut g = DBGroup::default();
    (g.tables, g.objects) = model(a, b, &HashMap::new(), &HashMap::new());
    g.facts()
}

static CONSTRAINT_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(\s*)(ALTER\s+TABLE\s+(ONLY\s+)?\S+\s+ADD\s+)?(CONSTRAINT\s+\S+\s+)?").unwrap());

fn spaces(l: &str) -> usize {
    l.bytes().take_while(|&b| b == b' ').count()
}

/// Schema A → schema B: every table that changed with all its columns (unchanged ones too, for context), and the
/// other objects. `text_a` / `text_b` are the files, for showing definitions as written.
pub fn model(a: &Schema, b: &Schema, text_a: &HashMap<String, String>, text_b: &HashMap<String, String>) -> (Vec<DBTable>, Vec<DBObject>) {
    let mut parsed: HashMap<(bool, String), Vec<Statement>> = HashMap::new();
    let mut source = |at: Option<&sql::Location>, old: bool| -> Option<String> {
        let at = at?;
        let t = (if old { text_a } else { text_b }).get(&at.file)?;
        let sts = parsed.entry((old, at.file.clone())).or_insert_with(|| sql::statements(t));
        sts.iter().find(|s| s.line == at.line).map(|s| s.source(&sql::lines(t)))
    };
    // A constraint as written: its lines from the migration, without the indentation they share.
    let written = |c: &sql::Constraint, old: bool| -> String {
        let Some(t) = (if old { text_a } else { text_b }).get(&c.at.file) else { return c.definition.clone() };
        let lines = sql::lines(t);
        if c.at.line < 1 || c.end_line > lines.len() || c.at.line > c.end_line {
            return c.definition.clone();
        }
        let mut ls: Vec<String> = lines[c.at.line - 1..c.end_line].iter().map(|s| s.to_string()).collect();
        // Just the constraint: not the ALTER TABLE … ADD or CONSTRAINT <name> it was written after.
        ls[0] = CONSTRAINT_HEAD.replace(&ls[0], "$1").into_owned();
        let indent = ls.iter().filter(|l| !l.trim_matches([' ', '\t']).is_empty()).map(|l| spaces(l)).min().unwrap_or(0);
        ls.iter().map(|l| &l[indent.min(spaces(l))..]).collect::<Vec<_>>().join("\n").trim_matches(|c: char| c == ',' || c == ';' || c.is_whitespace()).to_string()
    };
    let col = |c: &sql::Column, t: &sql::Table, kind: FactKind| DBColumn {
        name: c.name.clone(),
        ty: c.ty.clone(),
        old_type: None,
        not_null: c.not_null,
        default_value: c.default_value.clone(),
        references: c.references.clone(),
        primary: t.primary_key.contains(&c.name),
        kind,
        notes: Vec::new(),
        path: c.at.as_ref().map(|l| l.file.clone()),
        line: c.at.as_ref().map(|l| l.line),
    };
    let table = |name: &str, kind, t: &sql::Table, columns| DBTable {
        name: name.into(),
        old_name: None,
        kind,
        columns,
        extras: Vec::new(),
        sql: None,
        path: t.at.as_ref().map(|l| l.file.clone()),
        line: t.at.as_ref().map(|l| l.line),
    };
    let mut tables: BTreeMap<String, DBTable> = BTreeMap::new();
    let mut context: BTreeMap<String, DBTable> = BTreeMap::new(); // tables that exist on both sides unchanged, in case an index or trigger on them changed
    // A dropped and an added table with the same columns is a rename.
    let mut dropped: BTreeSet<&String> = a.tables.keys().filter(|k| !b.tables.contains_key(*k)).collect();
    let mut added: BTreeSet<&String> = b.tables.keys().filter(|k| !a.tables.contains_key(*k)).collect();
    for old in dropped.clone() {
        let oc = &a.tables[old].columns;
        let Some(new) = added.iter().find(|n| b.tables[**n].columns == *oc && !oc.is_empty()).copied() else { continue };
        let t = &b.tables[new];
        let mut d = table(new, FactKind::Renamed, t, t.columns.iter().map(|c| col(c, t, FactKind::Info)).collect());
        d.old_name = Some(old.clone());
        tables.insert(new.clone(), d);
        dropped.remove(old);
        added.remove(new);
    }
    for n in added {
        let t = &b.tables[n];
        let mut d = table(n, FactKind::Added, t, t.columns.iter().map(|c| col(c, t, FactKind::Added)).collect());
        d.sql = source(t.at.as_ref(), false);
        tables.insert(n.clone(), d);
    }
    for n in dropped {
        let t = &a.tables[n];
        let mut d = table(n, FactKind::Removed, t, t.columns.iter().map(|c| col(c, t, FactKind::Removed)).collect());
        d.sql = source(t.at.as_ref(), true);
        tables.insert(n.clone(), d);
    }
    let mut extras: BTreeMap<String, Vec<LensFact>> = BTreeMap::new();
    for n in a.tables.keys().filter(|k| b.tables.contains_key(*k)) {
        let (ta, tb) = (&a.tables[n], &b.tables[n]);
        fn first(t: &sql::Table) -> HashMap<&str, &sql::Column> {
            let mut m: HashMap<&str, &sql::Column> = HashMap::new();
            for c in &t.columns {
                m.entry(&c.name).or_insert(c);
            }
            m
        }
        let (ca, cb) = (first(ta), first(tb));
        let mut cols: Vec<DBColumn> = Vec::new();
        let mut changed = false;
        for c in &tb.columns {
            let Some(o) = ca.get(c.name.as_str()) else {
                cols.push(col(c, tb, FactKind::Added));
                changed = true;
                continue;
            };
            let mut d = col(c, tb, FactKind::Info);
            if *o != c {
                if o.ty != c.ty {
                    d.old_type = Some(o.ty.clone());
                    d.notes.push(format!("type {} → {}", o.ty, c.ty));
                }
                if o.not_null != c.not_null {
                    d.notes.push(if c.not_null { "now NOT NULL" } else { "now nullable" }.into());
                }
                let none = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".into());
                if o.default_value != c.default_value {
                    d.notes.push(format!("default {} → {}", none(&o.default_value), none(&c.default_value)));
                }
                if o.references != c.references {
                    d.notes.push(format!("reference {} → {}", none(&o.references), none(&c.references)));
                }
                d.kind = FactKind::Changed;
                changed = true;
            }
            cols.push(d);
        }
        // Dropped columns go back where they were: after the column that preceded them.
        for (i, c) in ta.columns.iter().enumerate().filter(|(_, c)| !cb.contains_key(c.name.as_str())) {
            let prev = ta.columns[..i].iter().rev().find(|x| cols.iter().any(|y| y.name == x.name));
            let at = prev.and_then(|p| cols.iter().position(|y| y.name == p.name)).map_or(0, |i| i + 1);
            cols.insert(at, col(c, ta, FactKind::Removed));
            changed = true;
        }
        let ex = extras.entry(n.clone()).or_default();
        let tb_at = tb.at.as_ref();
        if ta.primary_key != tb.primary_key {
            let mut f = LensFact::new(FactKind::Changed, "primary key").at(tb_at.map(|l| l.file.as_str()), tb_at.map(|l| l.line)).tag("key");
            f.before = Some(format!("({})", ta.primary_key.join(", ")));
            f.after = Some(format!("({})", tb.primary_key.join(", ")));
            ex.push(f);
        }
        // Constraints by name: added, dropped, or re-created with another definition (DROP + ADD of one name).
        for (c, con) in &tb.constraints {
            let at = |f: LensFact| f.at(Some(&con.at.file), Some(con.at.line)).tag("constraint");
            if let Some(o) = ta.constraints.get(c) {
                if o.definition == con.definition {
                    continue;
                }
                let mut f = at(LensFact::new(FactKind::Changed, c).detail("constraint changed"));
                f.before = Some(written(o, true));
                f.after = Some(written(con, false));
                ex.push(f);
            } else {
                let mut f = at(LensFact::new(FactKind::Added, c).detail("constraint added"));
                f.after = Some(written(con, false));
                ex.push(f);
            }
        }
        for (c, con) in ta.constraints.iter().filter(|(c, _)| !tb.constraints.contains_key(*c)) {
            let mut f = LensFact::new(FactKind::Removed, c).detail("constraint dropped").tag("constraint");
            f.before = Some(written(con, true));
            ex.push(f);
        }
        let t = table(n, FactKind::Changed, tb, cols);
        if changed {
            tables.insert(n.clone(), t);
        } else {
            context.insert(n.clone(), t);
        }
    }
    let unique = |i: &sql::Index| format!("{}ON {} {}", if i.unique { "UNIQUE " } else { "" }, i.table, i.definition);
    for (n, i) in &b.indexes {
        let at = |f: LensFact| f.at(i.at.as_ref().map(|l| l.file.as_str()), i.at.as_ref().map(|l| l.line)).tag("index");
        if let Some(o) = a.indexes.get(n) {
            if o != i {
                let mut f = at(LensFact::new(FactKind::Changed, n).detail("index changed"));
                f.before = Some(unique(o));
                f.after = Some(unique(i));
                extras.entry(i.table.clone()).or_default().push(f);
            }
        } else {
            let mut f = at(LensFact::new(FactKind::Added, n).detail(format!("{}index added", if i.unique { "unique " } else { "" })));
            f.after = Some(unique(i));
            extras.entry(i.table.clone()).or_default().push(f);
        }
    }
    for (n, i) in a.indexes.iter().filter(|(n, i)| !b.indexes.contains_key(*n) && b.tables.contains_key(&i.table)) {
        let mut f = LensFact::new(FactKind::Removed, n).detail("index dropped").tag("index");
        f.before = Some(unique(i));
        extras.entry(i.table.clone()).or_default().push(f);
    }
    // When a trigger fires, what it runs, and what that raises.
    let about = |t: &sql::Trigger, s: &Schema| -> Vec<String> {
        let mut out = vec![t.detail.clone()];
        if let Some(f) = t.function.as_ref().and_then(|f| s.functions.get(f)) {
            out.extend(sql::raises(&f.body).into_iter().map(|r| format!("Raises {r}")));
        }
        out
    };
    for (n, t) in &b.triggers {
        let at = |f: LensFact| f.at(Some(&t.at.file), Some(t.at.line)).tag("trigger");
        if let Some(o) = a.triggers.get(n) {
            if o.detail == t.detail && o.table == t.table {
                continue;
            }
            let mut f = at(LensFact::new(FactKind::Changed, n).detail("trigger changed"));
            f.before = source(Some(&o.at), true);
            f.after = source(Some(&t.at), false);
            f.notes = std::iter::once(format!("was {}", o.detail)).chain(about(t, b)).collect();
            extras.entry(t.table.clone()).or_default().push(f);
        } else {
            let mut f = at(LensFact::new(FactKind::Added, n).detail("trigger added"));
            f.after = source(Some(&t.at), false);
            f.notes = about(t, b);
            extras.entry(t.table.clone()).or_default().push(f);
        }
    }
    for (n, t) in a.triggers.iter().filter(|(n, t)| !b.triggers.contains_key(*n) && b.tables.contains_key(&t.table)) {
        let mut f = LensFact::new(FactKind::Removed, n).detail("trigger dropped").tag("trigger");
        f.before = source(Some(&t.at), true);
        f.notes = about(t, a);
        extras.entry(t.table.clone()).or_default().push(f);
    }
    let mut objects: Vec<DBObject> = Vec::new();
    for (table, mut facts) in extras {
        if facts.is_empty() {
            continue;
        }
        facts.sort_by(|x, y| (x.tag.as_deref().unwrap_or(""), &x.title).cmp(&(y.tag.as_deref().unwrap_or(""), &y.title)));
        if let Some(t) = tables.get_mut(&table) {
            t.extras = facts;
        } else if let Some(mut t) = context.remove(&table) {
            t.extras = facts;
            tables.insert(table, t);
        } else {
            objects.extend(facts.into_iter().map(|f| DBObject {
                ty: f.tag.clone().unwrap_or_else(|| "index".into()),
                name: f.title,
                kind: f.kind,
                detail: format!("{} · ON {table}", f.detail.unwrap_or_default()),
                before: f.before,
                after: f.after,
                notes: f.notes,
                path: f.path,
                line: f.line,
            }));
        }
    }
    let object = |ty: &str, name: &str, kind, detail: String| DBObject { ty: ty.into(), name: name.into(), kind, detail, before: None, after: None, notes: Vec::new(), path: None, line: None };
    for (n, vals) in &b.enums {
        let at = b.enum_at.get(n);
        let mut o = if let Some(old) = a.enums.get(n) {
            let add: Vec<&str> = vals.iter().filter(|v| !old.contains(v)).map(String::as_str).collect();
            let rem: Vec<&str> = old.iter().filter(|v| !vals.contains(v)).map(String::as_str).collect();
            if add.is_empty() && rem.is_empty() {
                continue;
            }
            let mut d = Vec::new();
            if !add.is_empty() {
                d.push(format!("+ {}", add.join(", ")));
            }
            if !rem.is_empty() {
                d.push(format!("− {}", rem.join(", ")));
            }
            let mut o = object("type", n, FactKind::Changed, format!("enum values {}", d.join("  ")));
            o.before = Some(old.join(", "));
            o.after = Some(vals.join(", "));
            o
        } else {
            let detail = if vals.is_empty() { "type added".into() } else { format!("enum added · {}", vals.join(", ")) };
            let mut o = object("type", n, FactKind::Added, detail);
            o.after = (!vals.is_empty()).then(|| vals.join(", "));
            o
        };
        o.path = at.map(|l| l.file.clone());
        o.line = at.map(|l| l.line);
        objects.push(o);
    }
    for (n, vals) in a.enums.iter().filter(|(n, _)| !b.enums.contains_key(*n)) {
        let mut o = object("type", n, FactKind::Removed, "type dropped".into());
        o.before = Some(vals.join(", "));
        objects.push(o);
    }
    let raises = |body: &str| sql::raises(body).into_iter().map(|r| format!("Raises {r}")).collect::<Vec<_>>();
    for (x, y, what) in [(&a.views, &b.views, "view"), (&a.functions, &b.functions, "function")] {
        for (n, v) in y.iter().filter(|(n, _)| !x.contains_key(*n)) {
            let mut o = object(what, n, FactKind::Added, format!("{what} added"));
            o.after = source(Some(&v.at), false);
            o.notes = raises(&v.body);
            (o.path, o.line) = (Some(v.at.file.clone()), Some(v.at.line));
            objects.push(o);
        }
        for (n, v) in x.iter().filter(|(n, _)| !y.contains_key(*n)) {
            let mut o = object(what, n, FactKind::Removed, format!("{what} dropped"));
            o.before = source(Some(&v.at), true);
            objects.push(o);
        }
        for (n, v) in x.iter() {
            let Some(w) = y.get(n).filter(|w| w.body != v.body) else { continue };
            let mut o = object(what, n, FactKind::Changed, format!("{what} redefined"));
            o.before = source(Some(&v.at), true);
            o.after = source(Some(&w.at), false);
            o.notes = raises(&w.body);
            (o.path, o.line) = (Some(w.at.file.clone()), Some(w.at.line));
            objects.push(o);
        }
    }
    let rank = |k: FactKind| match k {
        FactKind::Removed => 0,
        FactKind::Added => 1,
        FactKind::Renamed => 2,
        FactKind::Changed => 3,
        _ => 9,
    };
    let mut tables: Vec<DBTable> = tables.into_values().collect();
    tables.sort_by(|x, y| (rank(x.kind), &x.name).cmp(&(rank(y.kind), &y.name)));
    (tables, objects)
}

/// Data statements (INSERT / UPDATE / DELETE) that appear, disappear or change in a changed file, per table.
/// `renames` (old → new table name) pairs a statement on a renamed table with its counterpart.
pub fn data_changes(paths: &[String], a: &HashMap<String, String>, b: &HashMap<String, String>, renames: &BTreeMap<String, String>) -> Vec<DBData> {
    struct D {
        verb: String,
        table: String,
        st: Statement,
    }
    fn data(s: Vec<Statement>) -> Vec<D> {
        s.into_iter()
            .filter_map(|st| {
                let f = st.tokens.first().filter(|f| f.is_word())?;
                let mut i = 1;
                match f.text.as_str() {
                    "insert" if st.tokens.get(i).is_some_and(|t| t.is("into")) => i += 1,
                    "delete" if st.tokens.get(i).is_some_and(|t| t.is("from")) => i += 1,
                    "insert" | "delete" | "update" => {}
                    _ => return None,
                }
                let mut t = st.tokens.get(i)?.ident()?.to_string();
                if t == "public" && i + 2 < st.tokens.len() && st.tokens[i + 1].text == "." {
                    if let Some(n) = st.tokens[i + 2].ident() {
                        t = n.to_string();
                    }
                }
                Some(D { verb: f.text.to_uppercase(), table: t, st })
            })
            .collect()
    }
    let mut out = Vec::new();
    let empty = String::new();
    for p in paths {
        let (ta, tb) = (a.get(p).unwrap_or(&empty), b.get(p).unwrap_or(&empty));
        let (la, lb) = (sql::lines(ta), sql::lines(tb));
        let da: Vec<D> = data(sql::statements(ta)).into_iter().map(|d| D { table: renames.get(&d.table).cloned().unwrap_or(d.table), ..d }).collect();
        let db = data(sql::statements(tb));
        let texts_a: HashSet<&str> = da.iter().map(|d| d.st.text.as_str()).collect();
        let texts_b: HashSet<&str> = db.iter().map(|d| d.st.text.as_str()).collect();
        // Pair by table: a statement on the same table that exists on both sides but differs is "changed".
        let mut new_by_key: BTreeMap<String, std::collections::VecDeque<&D>> = BTreeMap::new();
        for d in db.iter().filter(|d| !texts_a.contains(d.st.text.as_str())) {
            new_by_key.entry(format!("{} {}", d.verb, d.table)).or_default().push_back(d);
        }
        let mut groups: BTreeMap<String, DBData> = BTreeMap::new();
        let new = |d: &D, kind, line| DBData { verb: d.verb.clone(), table: d.table.clone(), kind, before: Vec::new(), after: Vec::new(), path: p.clone(), line };
        for g in da.iter().filter(|d| !texts_b.contains(d.st.text.as_str())) {
            let k = format!("{} {}", g.verb, g.table);
            if let Some(n) = new_by_key.get_mut(&k).and_then(|l| l.pop_front()) {
                let e = groups.entry(format!("c {k}")).or_insert_with(|| new(g, FactKind::Changed, Some(n.st.line)));
                e.before.push(g.st.source(&la));
                e.after.push(n.st.source(&lb));
            } else {
                groups.entry(format!("r {k}")).or_insert_with(|| new(g, FactKind::Removed, None)).before.push(g.st.source(&la));
            }
        }
        for (k, list) in &new_by_key {
            for n in list {
                groups.entry(format!("a {k}")).or_insert_with(|| new(n, FactKind::Added, Some(list[0].st.line))).after.push(n.st.source(&lb));
            }
        }
        let rank = |k: FactKind| match k {
            FactKind::Removed => 0,
            FactKind::Added => 1,
            FactKind::Changed => 2,
            _ => 9,
        };
        let mut g: Vec<DBData> = groups.into_values().collect();
        g.sort_by(|x, y| (rank(x.kind), &x.table, &x.verb).cmp(&(rank(y.kind), &y.table, &y.verb)));
        out.extend(g);
    }
    out
}

// MARK: - Dependencies

fn unquote(s: &str) -> String {
    s.trim_matches(['"', ' ']).to_string()
}

/// "<>=!~;[ ": where a requirement's name ends.
fn split_requirement(l: &str) -> (String, String) {
    let idx = l.find(|c| "<>=!~;[ ".contains(c)).unwrap_or(l.len());
    (l[..idx].to_lowercase(), l[idx..].trim_matches([' ', '\t']).to_string())
}

fn json(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok()
}

/// name → version (or requirement), per file kind.
pub fn parse_dependencies(path: &str, text: &str) -> Option<BTreeMap<String, String>> {
    let name = file_name(path);
    let mut out = BTreeMap::new();
    match name {
        "Cargo.lock" => {
            let mut cur: Option<String> = None;
            for l in text.split('\n') {
                if let Some(n) = l.strip_prefix("name = ") {
                    cur = Some(unquote(n));
                } else if let (Some(v), Some(c)) = (l.strip_prefix("version = "), cur.take()) {
                    let v = unquote(v);
                    let key = if out.contains_key(&c) { format!("{c} ({v})") } else { c };
                    out.insert(key, v);
                }
            }
        }
        "Cargo.toml" | "pyproject.toml" => return Some(toml_deps(text, name == "Cargo.toml")),
        "package.json" => {
            let j = json(text)?;
            j.as_object()?;
            for (sec, label) in [("dependencies", ""), ("devDependencies", " (dev)"), ("peerDependencies", " (peer)"), ("optionalDependencies", " (optional)")] {
                // A section with a non-string value isn't [String: String] to Swift, and was left out whole.
                let Some(d) = j[sec].as_object().filter(|d| d.values().all(Value::is_string)) else { continue };
                for (k, v) in d {
                    out.insert(format!("{k}{label}"), v.as_str().unwrap().to_string());
                }
            }
        }
        "package-lock.json" => {
            let j = json(text)?;
            for (k, v) in j.get("packages")?.as_object()? {
                let (Some(rest), Some(ver)) = (k.strip_prefix("node_modules/"), v["version"].as_str()) else { continue };
                out.insert(rest.replace("/node_modules/", " › "), ver.to_string());
            }
        }
        "go.mod" => {
            let mut in_block = false;
            for raw in text.split('\n') {
                let l = raw.split("//").next().unwrap_or("").trim_matches([' ', '\t']);
                if l.starts_with("require (") {
                    in_block = true;
                    continue;
                }
                if in_block && l == ")" {
                    in_block = false;
                    continue;
                }
                let body = if in_block { l } else { l.strip_prefix("require ").unwrap_or("") };
                let f: Vec<&str> = body.split(' ').filter(|s| !s.is_empty()).collect();
                if f.len() >= 2 {
                    out.insert(f[0].to_string(), f[1].to_string());
                }
            }
        }
        "Package.resolved" => {
            let j = json(text)?;
            j.as_object()?;
            let pins = j["pins"].as_array().or_else(|| j["object"]["pins"].as_array()).cloned().unwrap_or_default();
            for p in pins {
                let n = p["identity"].as_str().or(p["package"].as_str()).unwrap_or("?");
                let st = &p["state"];
                let v = st["version"].as_str().map(str::to_string).or_else(|| st["revision"].as_str().map(|r| r.chars().take(8).collect())).unwrap_or("?".into());
                out.insert(n.to_string(), v);
            }
        }
        "flake.lock" => {
            let j = json(text)?;
            for (k, v) in j.get("nodes")?.as_object()? {
                let Some(locked) = v["locked"].as_object().filter(|_| k != "root") else { continue };
                let rev = locked
                    .get("rev")
                    .and_then(Value::as_str)
                    .map(|r| r.chars().take(8).collect())
                    .or_else(|| locked.get("narHash").and_then(Value::as_str).map(|h| h.chars().take(12).collect()))
                    .unwrap_or_else(|| "?".to_string());
                let date = locked.get("lastModified").and_then(Value::as_f64).map(abbreviated_date);
                out.insert(k.clone(), date.map_or(rev.clone(), |d| format!("{rev} ({d})")));
            }
        }
        _ if name.starts_with("requirements") && name.ends_with(".txt") => {
            for raw in text.split('\n') {
                let l = raw.split('#').next().unwrap_or("").trim_matches([' ', '\t']);
                if l.is_empty() || l.starts_with('-') {
                    continue;
                }
                let (k, v) = split_requirement(l);
                out.insert(k, v);
            }
        }
        _ => return None,
    }
    Some(out)
}

/// "Nov 22, 2026", as `Date.formatted(date: .abbreviated, time: .omitted)` printed it in English (UTC here).
fn abbreviated_date(secs: f64) -> String {
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = (secs / 86_400.0).floor() as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
}

static TOML_VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"version\s*=\s*"([^"]+)""#).unwrap());
static TOML_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"path\s*=\s*"([^"]+)""#).unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).unwrap());

/// [dependencies]-style tables in Cargo.toml, and PEP 621 / Poetry dependencies in pyproject.toml.
fn toml_deps(text: &str, cargo: bool) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    let mut in_array = false;
    for raw in text.split('\n') {
        let l = raw.trim_matches([' ', '\t']);
        if l.starts_with('[') {
            section = l.trim_matches(['[', ']', ' ']).to_string();
            in_array = false;
            continue;
        }
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if cargo {
            if !section.ends_with("dependencies") {
                continue;
            }
            let label = if section == "dependencies" || section == "workspace.dependencies" {
                String::new()
            } else {
                format!(" ({})", section.replace("dependencies", "").trim_matches(['.', '-']))
            };
            let Some(eq) = l.find('=') else { continue };
            let (k, v) = (unquote(&l[..eq]), l[eq + 1..].trim_matches([' ', '\t']));
            let value = if v.starts_with('{') {
                let ver = TOML_VERSION.captures(v).map(|c| c[1].to_string());
                let ws = v.contains("workspace = true").then(|| "workspace".to_string());
                let path = TOML_PATH.captures(v).map(|c| format!("path {}", &c[1]));
                ver.or(ws).or(path).unwrap_or_else(|| v.to_string())
            } else {
                unquote(v)
            };
            out.insert(k + &label, value);
        } else {
            if section == "project" && l.starts_with("dependencies") && l.contains('[') {
                in_array = true;
            }
            if in_array {
                for m in QUOTED.captures_iter(l) {
                    let (k, v) = split_requirement(&m[1]);
                    out.insert(k, v);
                }
                if l.contains(']') {
                    in_array = false;
                }
            } else if section.starts_with("tool.poetry") && section.ends_with("dependencies") {
                if let Some(eq) = l.find('=') {
                    out.insert(unquote(&l[..eq]).to_lowercase(), unquote(&l[eq + 1..]));
                }
            }
        }
    }
    out
}

/// "1.4.2" → "1.10.0": major / minor / patch, or a downgrade.
pub fn bump(a: &str, b: &str) -> Option<&'static str> {
    fn nums(s: &str) -> Option<Vec<u64>> {
        let start = s.find(|c: char| c.is_ascii_digit())?;
        let core: String = s[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let mut n: Vec<u64> = core.split('.').filter_map(|x| x.parse().ok()).collect();
        if n.is_empty() {
            return None;
        }
        while n.len() < 3 {
            n.push(0);
        }
        Some(n)
    }
    let (x, y) = (nums(a)?, nums(b)?);
    if y < x {
        Some("downgrade")
    } else if y[0] != x[0] {
        Some("major")
    } else if y[1] != x[1] {
        Some("minor")
    } else if y[2] != x[2] {
        Some("patch")
    } else {
        None
    }
}

pub fn dependency_report(changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, dir: &str) -> Option<LensReport> {
    // By name: an empty text doesn't parse as JSON, so asking `parse` would drop package.json and the other JSON files.
    const NAMES: [&str; 8] = ["Cargo.lock", "Cargo.toml", "pyproject.toml", "package.json", "package-lock.json", "go.mod", "Package.resolved", "flake.lock"];
    let deps: Vec<&ChangedPath> = changes
        .iter()
        .filter(|c| {
            let n = file_name(c.path());
            NAMES.contains(&n) || (n.starts_with("requirements") && n.ends_with(".txt"))
        })
        .collect();
    if deps.is_empty() {
        return None;
    }
    let olds: Vec<String> = deps.iter().filter_map(|c| c.old.clone()).collect();
    let news: Vec<String> = deps.iter().filter_map(|c| c.new.clone()).collect();
    let (text_a, text_b) = (a.read(&olds, dir, 20_000_000), b.read(&news, dir, 20_000_000));
    let mut sections = Vec::new();
    let (mut added, mut removed, mut changed, mut major) = (0, 0, 0, 0);
    for c in deps {
        let parse = |p: &Option<String>, t: &HashMap<String, String>| p.as_ref().and_then(|p| t.get(p).and_then(|x| parse_dependencies(p, x))).unwrap_or_default();
        let (pa, pb) = (parse(&c.old, &text_a), parse(&c.new, &text_b));
        let mut facts = Vec::new();
        for (k, v) in pb.iter().filter(|(k, _)| !pa.contains_key(*k)) {
            let mut f = LensFact::new(FactKind::Added, k).detail(v).at(c.new.as_deref(), None);
            f.after = Some(v.clone());
            facts.push(f);
            added += 1;
        }
        for (k, v) in pa.iter().filter(|(k, _)| !pb.contains_key(*k)) {
            let mut f = LensFact::new(FactKind::Removed, k).detail(v);
            f.before = Some(v.clone());
            facts.push(f);
            removed += 1;
        }
        for (k, va) in &pa {
            let Some(vb) = pb.get(k).filter(|vb| *vb != va) else { continue };
            let kind = bump(va, vb);
            if kind == Some("major") {
                major += 1;
            }
            let mut f = LensFact::new(FactKind::Changed, k).detail(format!("{va} → {vb}{}", kind.map(|k| format!(" · {k}")).unwrap_or_default())).at(c.new.as_deref(), None);
            f.tag = kind.map(str::to_string);
            f.before = Some(va.clone());
            f.after = Some(vb.clone());
            facts.push(f);
            changed += 1;
        }
        if !facts.is_empty() {
            sections.push(LensSection { title: c.path().into(), facts });
        }
    }
    if sections.is_empty() {
        return None;
    }
    let mut s = Vec::new();
    if added > 0 {
        s.push(format!("{added} added"));
    }
    if removed > 0 {
        s.push(format!("{removed} removed"));
    }
    if changed > 0 {
        s.push(format!("{changed} updated{}", if major > 0 { format!(" ({major} major)") } else { String::new() }));
    }
    let total = added + removed + changed;
    let metric = LensMetric { value: total, unit: if total == 1 { "package" } else { "packages" }.into(), added, removed, changed };
    Some(LensReport::new(ReportKind::Dependencies, "Dependencies", "shippingbox", s.join(" · "), sections, metric))
}

// MARK: - Config: key paths in JSON / TOML / YAML / .env

pub const HANDLED_ELSEWHERE: [&str; 11] =
    ["package.json", "package-lock.json", "Cargo.toml", "Cargo.lock", "pyproject.toml", "flake.lock", "Package.resolved", "composer.lock", "yarn.lock", "pnpm-lock.yaml", "tsconfig.json"];

pub fn is_config(p: &str) -> bool {
    let (name, ext) = (file_name(p), extension(p).to_lowercase());
    if HANDLED_ELSEWHERE.contains(&name) || ["Assets", "Tests", "Generated"].contains(&category(p, false)) {
        return false;
    }
    ["json", "toml", "yaml", "yml", "properties"].contains(&ext.as_str()) || name.starts_with(".env") || ext == "env"
}

static INDEX_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d+\]").unwrap());

/// Judged on the last key component: "design-tokens-parity.steps[3].run" isn't a secret, "with.token" is.
pub fn secret(key: &str) -> bool {
    let parts = components(key);
    let last = parts.last().map_or(key, String::as_str);
    let k = INDEX_SUFFIX.replace_all(last, "").to_lowercase();
    ["secret", "token", "password", "passwd", "apikey", "api_key", "private", "credential", "auth"].iter().any(|w| k.contains(w))
}

/// A key path's parts: "rules[apps: deployments].verbs" → ["rules[apps: deployments]", "verbs"]. Dots inside brackets
/// (an item named "fleetdriver.com: projects") don't split.
pub fn components(key: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0);
    for c in key.chars() {
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
        }
        if c == '.' && depth == 0 {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// A value as Swift's "\(v)" printed it, for scalars. Booleans print as true/false (Swift's NSNumber gave 1/0).
fn describe(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Array(a) => format!("[{}]", a.iter().map(describe).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

/// Flattens a file into "a.b[web].c" → value.
pub fn flatten(path: &str, text: &str) -> BTreeMap<String, String> {
    let (name, ext) = (file_name(path), extension(path).to_lowercase());
    let mut out = BTreeMap::new();
    if ext == "json" {
        let Some(j) = json(text) else { return out };
        walk(&j, "", &mut out);
    } else if ext == "toml" {
        let mut section = String::new();
        for raw in text.split('\n') {
            let l = raw.trim_matches([' ', '\t']);
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if l.starts_with('[') {
                section = l.trim_matches(['[', ']', ' ']).to_string();
                continue;
            }
            let Some(eq) = l.find('=') else { continue };
            let k = l[..eq].trim_matches([' ', '\t']);
            out.insert(if section.is_empty() { k.to_string() } else { format!("{section}.{k}") }, l[eq + 1..].trim_matches([' ', '\t']).to_string());
        }
    } else if name.starts_with(".env") || ext == "env" || ext == "properties" {
        for raw in text.split('\n') {
            let mut l = raw.trim_matches([' ', '\t']);
            if l.is_empty() || l.starts_with('#') || l.starts_with('!') {
                continue;
            }
            l = l.strip_prefix("export ").unwrap_or(l);
            let Some(eq) = l.find('=') else { continue };
            out.insert(l[..eq].to_string(), l[eq + 1..].to_string());
        }
    } else {
        let docs = yaml::documents(text);
        for (i, d) in docs.iter().enumerate() {
            // Several manifests in one file are told apart by what they are, not where they sit.
            let id = [d["kind"].as_str(), d["metadata"]["name"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(" ");
            let k = if docs.len() > 1 { format!("[{}]", if id.is_empty() { i.to_string() } else { id }) } else { String::new() };
            walk(d, &k, &mut out);
        }
    }
    out
}

/// Lists of plain values stay one value ("[get, create]"). The items of a list of mappings are named by what
/// identifies them (`containers[web]`, `rules[configmaps]`) when every item has a distinct one, so inserting an item
/// doesn't make every later one look changed.
pub fn walk(v: &Value, k: &str, out: &mut BTreeMap<String, String>) {
    match v {
        Value::Object(d) => {
            for (kk, vv) in d {
                walk(vv, &if k.is_empty() { kk.clone() } else { format!("{k}.{kk}") }, out);
            }
        }
        Value::Array(a) => {
            if a.iter().all(|x| !x.is_object() && !x.is_array()) {
                out.insert(k.to_string(), describe(v));
                return;
            }
            // A list of [name, options] pairs and bare names (Expo's plugins, Babel's): each under its name.
            let named: Vec<Option<(&str, Option<&Value>)>> = a
                .iter()
                .map(|x| match x {
                    Value::String(s) => Some((s.as_str(), None)),
                    Value::Array(p) => match (p.first().and_then(Value::as_str), p.len()) {
                        (Some(n), 1) => Some((n, None)),
                        (Some(n), 2) if p[1].is_object() => Some((n, Some(&p[1]))),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            if a.iter().any(Value::is_array) && named.iter().all(Option::is_some) && named.iter().map(|n| n.unwrap().0).collect::<HashSet<_>>().len() == a.len() {
                for (n, x) in named.into_iter().flatten() {
                    match x {
                        Some(x) => walk(x, &format!("{k}[{n}]"), out),
                        None => {
                            out.insert(format!("{k}[{n}]"), "on".into());
                        }
                    }
                }
                return;
            }
            let ids = identities(a);
            for (i, x) in a.iter().enumerate() {
                let id = ids.as_ref().map_or_else(|| i.to_string(), |ids| ids[i].clone());
                walk(x, &format!("{k}[{id}]"), out);
            }
        }
        _ => {
            out.insert(k.to_string(), describe(v));
        }
    }
}

pub fn identities(a: &[Value]) -> Option<Vec<String>> {
    let ds: Vec<&serde_json::Map<String, Value>> = a.iter().filter_map(Value::as_object).collect();
    if ds.len() != a.len() {
        return None;
    }
    fn text(v: Option<&Value>) -> Option<String> {
        match v? {
            Value::Array(a) => Some(a.iter().map(describe).collect::<Vec<_>>().join(", ")),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }
    let string = |d: &serde_json::Map<String, Value>, k: &str| d.get(k).and_then(Value::as_str).map(str::to_string);
    for namer in 0..5 {
        let ids: Vec<String> = ds
            .iter()
            .filter_map(|d| match namer {
                0 => string(d, "name"),
                1 => string(d, "id"),
                2 => string(d, "key"),
                3 => string(d, "type"),
                // RBAC rules, by what they grant on: "configmaps", "fleetdriver.com: projects, objectstorages".
                _ => text(d.get("resources")).map(|r| {
                    let g = text(d.get("apiGroups")).unwrap_or_default();
                    if g.is_empty() { r } else { format!("{g}: {r}") }
                }),
            })
            .collect();
        if ids.len() == ds.len() && ids.iter().collect::<HashSet<_>>().len() == ids.len() && !ids.iter().any(String::is_empty) {
            return Some(ids);
        }
    }
    None
}

/// Keys under a parent that exists only on one side collapse into the parent: a new CI job is one fact.
pub fn collapse(keys: &[String], absent_from: &BTreeMap<String, String>) -> Vec<(String, usize)> {
    let under = |k: &str, pre: &str| k.len() > pre.len() && k.starts_with(pre) && matches!(k.as_bytes()[pre.len()], b'.' | b'[');
    let mut groups: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for k in keys {
        let parts = components(k);
        let mut prefixes = Vec::new();
        let mut cur = String::new();
        for part in &parts {
            cur = if cur.is_empty() { part.clone() } else { format!("{cur}.{part}") };
            prefixes.push(cur.clone());
        }
        prefixes.pop();
        // The shortest prefix that the other side lacks entirely and that holds more than this key.
        let p = prefixes
            .into_iter()
            .find(|pre| !absent_from.keys().any(|o| o == pre || under(o, pre)) && keys.iter().filter(|x| under(x, pre)).count() > 1)
            .unwrap_or_else(|| k.clone());
        if !groups.contains_key(&p) {
            order.push(p.clone());
        }
        *groups.entry(p).or_default() += 1;
    }
    order.into_iter().map(|p| {
        let n = groups[&p];
        (p, n)
    }).collect()
}

/// Key-level facts between two flattened versions of a file.
pub fn key_facts(fa: &BTreeMap<String, String>, fb: &BTreeMap<String, String>, path: Option<&str>) -> Vec<LensFact> {
    fn one(v: &str) -> String {
        let lines: Vec<&str> = v.split('\n').filter(|l| !l.is_empty()).collect();
        let first: String = lines.first().unwrap_or(&"").chars().take(80).collect();
        if lines.len() > 1 { format!("{first} … ({} lines)", lines.len()) } else { first }
    }
    let show = |k: &str, v: &str| (!secret(k)).then(|| one(v));
    // A small new or removed group reads better as its values than as "3 keys".
    let inline = |k: &str, f: &BTreeMap<String, String>| -> Option<String> {
        // Values the name already says (an RBAC rule's resources) or that say nothing (the core API group's [""]) are left out.
        let pre = format!("{k}.");
        let kids: Vec<&String> =
            f.keys().filter(|x| x.starts_with(&pre) && !["", "[]"].contains(&f[*x].as_str()) && !k.contains(&format!("[{}]", f[*x].trim_matches(['[', ']'])))).collect();
        if kids.len() > 4 || kids.iter().any(|x| secret(x) || f[*x].contains('\n')) {
            return None;
        }
        Some(kids.iter().map(|x| format!("{}: {}", &x[pre.len()..], f[*x])).collect::<Vec<_>>().join(" · "))
    };
    fn list(v: &str) -> Option<Vec<String>> {
        let inner = v.strip_prefix('[')?.strip_suffix(']')?;
        Some(inner.split(',').filter(|s| !s.is_empty()).map(|s| s.trim_matches([' ', '\t']).to_string()).collect())
    }
    let mut facts = Vec::new();
    let only = |x: &BTreeMap<String, String>, y: &BTreeMap<String, String>| x.keys().filter(|k| !y.contains_key(*k)).cloned().collect::<Vec<_>>();
    for (k, n) in collapse(&only(fb, fa), fa) {
        let small = if n > 1 { inline(&k, fb) } else { None };
        let value = show(&k, fb.get(&k).map_or("", String::as_str));
        let detail = if n > 1 { small.clone().unwrap_or_else(|| format!("added · {n} keys")) } else { value.clone().unwrap_or_else(|| "(value hidden)".into()) };
        let mut f = LensFact::new(FactKind::Added, &k).detail(detail).at(path, None).count(n);
        f.tag = (n > 1 && small.is_none()).then(|| format!("{n} keys"));
        f.after = if n > 1 { small } else { value };
        facts.push(f);
    }
    for (k, n) in collapse(&only(fa, fb), fb) {
        let small = if n > 1 { inline(&k, fa) } else { None };
        let value = show(&k, fa.get(&k).map_or("", String::as_str));
        let detail = if n > 1 { small.clone().unwrap_or_else(|| format!("removed · {n} keys")) } else { value.clone().unwrap_or_else(|| "(value hidden)".into()) };
        let mut f = LensFact::new(FactKind::Removed, &k).detail(detail).count(n);
        f.tag = (n > 1 && small.is_none()).then(|| format!("{n} keys"));
        f.before = if n > 1 { small } else { value };
        facts.push(f);
    }
    for (k, va) in fa {
        let Some(vb) = fb.get(k).filter(|vb| *vb != va) else { continue };
        if !secret(k) {
            if let (Some(la), Some(lb)) = (list(va), list(vb)) {
                // A list of values: what joined and what left, not the whole list twice.
                let d: Vec<String> = lb.iter().filter(|x| !la.contains(x)).map(|x| format!("+{x}")).chain(la.iter().filter(|x| !lb.contains(x)).map(|x| format!("−{x}"))).collect();
                let d = d.join("  ");
                facts.push(LensFact::new(FactKind::Changed, k).detail(if d.is_empty() { "reordered".into() } else { d }).at(path, None));
                continue;
            }
        }
        let multi = va.contains('\n') || vb.contains('\n');
        let d = if secret(k) {
            "(value hidden)".into()
        } else if multi {
            format!("changed ({} lines)", vb.split('\n').filter(|l| !l.is_empty()).count())
        } else {
            format!("{} → {}", one(va), one(vb))
        };
        let plain = !secret(k) && !multi;
        let mut f = LensFact::new(FactKind::Changed, k).detail(d).at(path, None);
        f.before = plain.then(|| one(va));
        f.after = plain.then(|| one(vb));
        facts.push(f);
    }
    facts
}

// MARK: Custom resource definitions

/// A CRD as its resource sees it: each schema field with its type ("status.phase" → "Pending | Bound | Lost ·
/// required") and the columns `kubectl get` prints.
#[derive(Clone, Debug, PartialEq)]
pub struct Crd {
    pub kind: String,
    pub fields: BTreeMap<String, String>,
    pub columns: Vec<String>,
}

pub fn crd(text: &str) -> Option<Crd> {
    // A file of one manifest: beside other manifests, the CRD's name would head changes that aren't its own.
    let docs = yaml::documents(text);
    if docs.len() != 1 || docs[0]["kind"].as_str() != Some("CustomResourceDefinition") {
        return None;
    }
    let spec = docs[0]["spec"].as_object()?;
    let versions: Vec<&serde_json::Map<String, Value>> = spec.get("versions").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_object).collect()).unwrap_or_default();
    let (mut fields, mut columns) = (BTreeMap::new(), Vec::new());
    for v in &versions {
        let pre = if versions.len() > 1 { format!("{} ", v.get("name").and_then(Value::as_str).unwrap_or("?")) } else { String::new() };
        if let Some(s) = v.get("schema").and_then(|s| s["openAPIV3Schema"].as_object()) {
            schema(s, &pre, false, &mut fields);
        }
        if let Some(cols) = v.get("additionalPrinterColumns").and_then(Value::as_array) {
            columns.extend(cols.iter().filter_map(|c| c["name"].as_str()).map(|n| format!("{pre}{n}")));
        }
    }
    let kind = spec.get("names").and_then(|n| n["kind"].as_str()).unwrap_or("Resource").to_string();
    Some(Crd { kind, fields, columns })
}

fn schema(s: &serde_json::Map<String, Value>, path: &str, required: bool, out: &mut BTreeMap<String, String>) {
    fn ty(s: &serde_json::Map<String, Value>) -> String {
        if let Some(e) = s.get("enum").and_then(Value::as_array) {
            return e.iter().map(describe).collect::<Vec<_>>().join(" | ");
        }
        if s.get("x-kubernetes-int-or-string").and_then(Value::as_str) == Some("true") {
            return "int or string".into();
        }
        let t = s.get("type").and_then(Value::as_str).unwrap_or("object");
        s.get("format").map_or(t.to_string(), |f| format!("{t} ({})", describe(f)))
    }
    let (mut node, mut t) = (s, ty(s));
    if t == "array" {
        if let Some(item) = s.get("items").and_then(Value::as_object) {
            t = format!("list of {}", ty(item));
            node = item;
        }
    }
    let root = path.is_empty() || path.ends_with(' ');
    if !root {
        out.insert(path.to_string(), t + if required { " · required" } else { "" });
    }
    let req: HashSet<String> = node.get("required").and_then(Value::as_array).map(|r| r.iter().map(describe).collect()).unwrap_or_default();
    if let Some(props) = node.get("properties").and_then(Value::as_object) {
        for (k, v) in props {
            if let Some(v) = v.as_object() {
                schema(v, &if root { format!("{path}{k}") } else { format!("{path}.{k}") }, req.contains(k), out);
            }
        }
    }
}

pub fn crd_facts(a: &Crd, b: &Crd, path: Option<&str>) -> Vec<LensFact> {
    // A new or removed object's own fields, by name: "type, status, reason, message".
    let fields = |k: &str, f: &BTreeMap<String, String>| {
        let pre = format!("{k}.");
        f.keys().filter_map(|x| x.strip_prefix(&pre)).filter(|x| !x.contains('.')).collect::<Vec<_>>().join(", ")
    };
    let plain = |t: &str| t.replace(" · required", "");
    // New (or gone) fields whose parent isn't: a new object is one fact, its fields listed in it.
    let tops = |x: &BTreeMap<String, String>, y: &BTreeMap<String, String>| {
        let only: Vec<&String> = x.keys().filter(|k| !y.contains_key(*k)).collect();
        only.iter().filter(|k| !only.iter().any(|o| k.starts_with(&format!("{o}.")))).map(|k| k.to_string()).collect::<Vec<_>>()
    };
    let mut facts = Vec::new();
    let describe_new = |k: &str, f: &BTreeMap<String, String>| {
        let kids = fields(k, f);
        format!("{}{}", f.get(k).map_or("", String::as_str), if kids.is_empty() { String::new() } else { format!(" · {kids}") })
    };
    for k in tops(&b.fields, &a.fields) {
        let t = describe_new(&k, &b.fields);
        let mut f = LensFact::new(FactKind::Added, &k).detail(&t).at(path, None);
        f.after = Some(t);
        facts.push(f);
    }
    for k in tops(&a.fields, &b.fields) {
        let t = describe_new(&k, &a.fields);
        let mut f = LensFact::new(FactKind::Removed, &k).detail(&t).tag("breaking");
        f.before = Some(t);
        facts.push(f);
    }
    for (k, x) in &a.fields {
        let Some(y) = b.fields.get(k).filter(|y| *y != x) else { continue };
        // Readers break when a type changes or a field becomes required; a field becoming optional breaks no one.
        let breaking = plain(x) != plain(y) || (y.ends_with("required") && !x.ends_with("required"));
        let mut f = LensFact::new(FactKind::Changed, k).detail(format!("{x} → {y}")).at(path, None);
        f.tag = breaking.then(|| "breaking".into());
        f.before = Some(x.clone());
        f.after = Some(y.clone());
        facts.push(f);
    }
    if a.columns != b.columns {
        let row = |c: &[String]| if c.is_empty() { "NAME · AGE".to_string() } else { std::iter::once("NAME").chain(c.iter().map(String::as_str)).collect::<Vec<_>>().join(" · ") };
        let mut f = LensFact::new(FactKind::Changed, "kubectl get").detail(format!("{} → {}", row(&a.columns), row(&b.columns))).at(path, None).tag("columns");
        f.before = Some(row(&a.columns));
        f.after = Some(row(&b.columns));
        facts.push(f);
    }
    facts
}

pub fn config_report(changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, dir: &str) -> Option<LensReport> {
    let cfg: Vec<&ChangedPath> = changes.iter().filter(|c| is_config(c.path())).collect();
    if cfg.is_empty() {
        return None;
    }
    let olds: Vec<String> = cfg.iter().filter_map(|c| c.old.clone()).collect();
    let news: Vec<String> = cfg.iter().filter_map(|c| c.new.clone()).collect();
    let (text_a, text_b) = (a.read(&olds, dir, 1_000_000), b.read(&news, dir, 1_000_000));
    let mut sections = Vec::new();
    let mut crds: Vec<(String, String, Vec<LensFact>)> = Vec::new();
    for c in &cfg {
        let raw_a = c.old.as_ref().and_then(|p| text_a.get(p));
        let raw_b = c.new.as_ref().and_then(|p| text_b.get(p));
        let mut fa = c.old.as_ref().zip(raw_a).map(|(p, t)| flatten(p, t)).unwrap_or_default();
        let mut fb = c.new.as_ref().zip(raw_b).map(|(p, t)| flatten(p, t)).unwrap_or_default();
        // Whole new or deleted files: one line, not every key.
        if c.old.is_none() {
            let f = LensFact::new(FactKind::Added, c.path()).detail(format!("new file · {} keys", fb.len())).at(c.new.as_deref(), None).tag("new file").count(fb.len());
            sections.push(LensSection { title: c.path().into(), facts: vec![f] });
            continue;
        }
        if c.new.is_none() {
            let f = LensFact::new(FactKind::Removed, c.path()).detail(format!("deleted · had {} keys", fa.len())).tag("deleted").count(fa.len());
            sections.push(LensSection { title: c.path().into(), facts: vec![f] });
            continue;
        }
        if let (Some(x), Some(y)) = (raw_a.and_then(|t| crd(t)), raw_b.and_then(|t| crd(t))) {
            // The schema and columns are told as fields; the rest of the file as keys.
            let rest = |f: &mut BTreeMap<String, String>| f.retain(|k, _| !k.contains(".schema.openAPIV3Schema") && !k.contains(".additionalPrinterColumns"));
            rest(&mut fa);
            rest(&mut fb);
            let mut facts = crd_facts(&x, &y, c.new.as_deref());
            facts.extend(key_facts(&fa, &fb, c.new.as_deref()));
            if !facts.is_empty() {
                crds.push((y.kind, c.path().into(), facts));
            }
            continue;
        }
        let facts = key_facts(&fa, &fb, c.new.as_deref());
        if !facts.is_empty() {
            sections.push(LensSection { title: c.path().into(), facts });
        }
    }
    let mut all_sections = crd_sections(&crds);
    all_sections.extend(sections);
    if all_sections.is_empty() {
        return None;
    }
    let all: Vec<&LensFact> = all_sections.iter().flat_map(|s| &s.facts).collect();
    let (added, removed, changed) = counts(&all);
    let total = all.len();
    let summary = format!("{} in {}", plural(total, "change"), plural(cfg.len(), "file"));
    let metric = LensMetric { value: total, unit: if total == 1 { "config change" } else { "config changes" }.into(), added, removed, changed };
    Some(LensReport::new(ReportKind::Configuration, "Configuration", "slider.horizontal.3", summary, all_sections, metric))
}

/// One section for what every changed CRD shares (the same field added to three resources is one fact), then one per
/// resource for the rest.
pub fn crd_sections(crds: &[(String, String, Vec<LensFact>)]) -> Vec<LensSection> {
    let sig = |f: &LensFact| {
        let o = |s: &Option<String>| s.clone().unwrap_or_default();
        format!("{:?}|{}|{}|{}|{}|{}", f.kind, f.title, o(&f.detail), o(&f.before), o(&f.after), o(&f.tag))
    };
    let shared: HashSet<String> = if crds.len() > 1 {
        crds[0].2.iter().map(sig).filter(|s| crds[1..].iter().all(|c| c.2.iter().any(|f| sig(f) == *s))).collect()
    } else {
        HashSet::new()
    };
    let mut out = Vec::new();
    if !shared.is_empty() {
        let mut kinds: Vec<&str> = crds.iter().map(|c| c.0.as_str()).collect();
        kinds.sort();
        out.push(LensSection { title: format!("{} · the same in each", kinds.join(", ")), facts: crds[0].2.iter().filter(|f| shared.contains(&sig(f))).cloned().collect() });
    }
    let mut sorted: Vec<&(String, String, Vec<LensFact>)> = crds.iter().collect();
    sorted.sort_by(|x, y| x.0.cmp(&y.0));
    for (kind, path, facts) in sorted {
        let own: Vec<LensFact> = facts.iter().filter(|f| !shared.contains(&sig(f))).cloned().collect();
        if !own.is_empty() {
            out.push(LensSection { title: format!("{kind} · {}", file_name(path)), facts: own });
        }
    }
    out
}

// MARK: - Lines a change adds and removes

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub from: String,
    pub to: String,
    pub similarity: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangedLines {
    pub added: Vec<Line>,
    pub removed: Vec<Line>,
    pub copies: Vec<Copy>,
}

/// What `changed_lines` needs of a `git diff -U0` file. ponytail: a local reader of Diff.swift's header rules, not
/// loupe-core's full DiffParser (hunks and patches aren't needed here).
#[derive(Debug, Default)]
struct DiffFile {
    old_path: Option<String>,
    new_path: Option<String>,
    copied_from: Option<String>,
    similarity: Option<u32>,
    added: Vec<(usize, String)>,
    removed: Vec<(usize, String)>,
}

fn strip_prefix(s: &str) -> Option<String> {
    let s = s.split('\t').next().unwrap_or(s); // "--- a/file\t2024-..." from --no-index
    if s == "/dev/null" {
        return None;
    }
    Some(s.strip_prefix("a/").or_else(|| s.strip_prefix("b/")).unwrap_or(s).to_string())
}

fn parse_diff(text: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let (mut old_no, mut new_no, mut in_hunk) = (0, 0, false);
    for line in text.split('\n') {
        if line.starts_with("diff --git ") || line.starts_with("diff --cc ") {
            in_hunk = false;
            let mut f = DiffFile::default();
            // Fallback paths from "diff --git a/x b/x": overwritten by ---/+++ when present.
            if let Some(r) = line.rfind(" b/") {
                f.new_path = Some(line[r + 3..].to_string());
                f.old_path = f.new_path.clone();
            }
            files.push(f);
            continue;
        }
        if files.is_empty() {
            files.push(DiffFile::default());
        }
        let cur = files.last_mut().unwrap();
        if in_hunk && matches!(line.as_bytes().first(), Some(b' ' | b'+' | b'-' | b'\\')) {
            match line.as_bytes()[0] {
                b'+' => {
                    cur.added.push((new_no, line[1..].to_string()));
                    new_no += 1;
                }
                b'-' => {
                    cur.removed.push((old_no, line[1..].to_string()));
                    old_no += 1;
                }
                b' ' => {
                    old_no += 1;
                    new_no += 1;
                }
                _ => {}
            }
            continue;
        }
        if line.starts_with("@@") {
            for tok in line.split(' ').filter(|t| !t.is_empty()).take(3) {
                let n = |t: &str| t[1..].split(',').next().and_then(|x| x.parse().ok()).unwrap_or(0);
                if tok.starts_with('-') {
                    old_no = n(tok);
                } else if tok.starts_with('+') {
                    new_no = n(tok);
                }
            }
            in_hunk = true;
            continue;
        }
        in_hunk = false;
        if let Some(p) = line.strip_prefix("--- ") {
            cur.old_path = strip_prefix(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            cur.new_path = strip_prefix(p);
        } else if let Some(p) = line.strip_prefix("rename from ") {
            cur.old_path = Some(p.into());
        } else if let Some(p) = line.strip_prefix("rename to ") {
            cur.new_path = Some(p.into());
        } else if let Some(p) = line.strip_prefix("copy from ") {
            cur.copied_from = Some(p.into());
            cur.old_path = Some(p.into());
        } else if let Some(p) = line.strip_prefix("copy to ") {
            cur.new_path = Some(p.into());
        } else if let Some(p) = line.strip_prefix("similarity index ") {
            cur.similarity = p.strip_suffix('%').and_then(|x| x.parse().ok());
        }
    }
    files
}

/// The diff from A to B without context, copies found (`-C -C`) so a copied file's lines are only what differs from
/// its source; and the whole of each new file the diff doesn't cover (untracked, or all of them when A is empty).
pub fn changed_lines(changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, dir: &str) -> ChangedLines {
    let revs: Option<Vec<&str>> = match (a, b) {
        (Snapshot::Rev(x), Snapshot::Rev(y)) => Some(vec![x, y]),
        (Snapshot::Rev(x), Snapshot::Worktree) => Some(vec![x]),
        _ => None,
    };
    let mut r = ChangedLines::default();
    let mut seen = HashSet::new();
    if let Some(revs) = &revs {
        let mut args = vec!["diff", "-U0", "-M", "-C", "-C", "--no-color", "--no-ext-diff"];
        args.extend(revs);
        if let Ok(data) = crate::git::run(&args, dir, None, false) {
            for f in parse_diff(&String::from_utf8_lossy(&data)) {
                let path = f.new_path.clone().or(f.old_path.clone()).unwrap_or_else(|| "?".into());
                seen.insert(path.clone());
                if let Some(from) = f.copied_from {
                    r.copies.push(Copy { from, to: path.clone(), similarity: f.similarity.unwrap_or(100) });
                }
                if generated_name(&path.to_lowercase()) {
                    continue;
                }
                r.added.extend(f.added.into_iter().map(|(n, text)| Line { path: path.clone(), line: n, text }));
                r.removed.extend(f.removed.into_iter().map(|(n, text)| Line { path: path.clone(), line: n, text }));
            }
        }
    }
    let rest: Vec<String> = changes
        .iter()
        .filter(|c| c.old.is_none() || revs.is_none())
        .filter_map(|c| c.new.clone())
        .filter(|p| !seen.contains(p) && !generated_name(&p.to_lowercase()))
        .take(300)
        .collect();
    let texts = b.read(&rest, dir, 2_000_000);
    for p in &rest {
        let Some(t) = texts.get(p) else { continue };
        r.added.extend(t.split('\n').enumerate().map(|(i, l)| Line { path: p.clone(), line: i + 1, text: l.to_string() }));
    }
    r
}

// MARK: - Follow-ups: what a change leaves for later or switches off

pub const FOLLOW_UP_KINDS: [(&str, &str, &str); 3] = [
    ("To Do", "to-do", r"\b(TODO|FIXME|XXX|HACK)\b"),
    (
        "Skipped and Focused Tests",
        "skipped test",
        r"#\[ignore\b|\b(it|test|describe|context)\.(skip|only)\(|\b(xit|xdescribe|xtest)\(|@(Disabled|Ignore)\b|\bt\.Skip(Now|f)?\(|\b(pytest\.mark|unittest)\.skip|\bXCTSkip",
    ),
    (
        "Silenced Checks",
        "silenced check",
        r"@ts-(ignore|expect-error|nocheck)|eslint-disable|biome-ignore|#!?\[allow\(|\bnoqa\b|type:\s*ignore|\bnolint\b|NOLINT|swiftlint:disable|rubocop:disable|pylint:\s*disable|#pragma warning disable|@SuppressWarnings",
    ),
];

static FOLLOW_UP_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| FOLLOW_UP_KINDS.iter().map(|k| Regex::new(k.2).unwrap()).collect());

/// The added lines that match, per kind; `skip` leaves out files not worth it (generated code, docs).
pub fn follow_up_report(lines: &[Line], skip: impl Fn(&str) -> bool) -> Option<LensReport> {
    let mut skipped: HashMap<&str, bool> = HashMap::new();
    let (mut sections, mut counts) = (Vec::new(), Vec::new());
    for (k, re) in FOLLOW_UP_KINDS.iter().zip(FOLLOW_UP_RES.iter()) {
        let mut facts = Vec::new();
        for l in lines {
            let Some(m) = re.find(&l.text) else { continue };
            if in_string(&l.text[..m.start()]) {
                continue;
            }
            if *skipped.entry(&l.path).or_insert_with(|| skip(&l.path)) {
                continue;
            }
            // The line from a little before the match, so a long one still shows it.
            let at = l.text[..m.start()].chars().count();
            let from = if at > 60 { at - 20 } else { 0 };
            let tail: String = l.text.chars().skip(from).collect();
            let text = format!("{}{}", if from > 0 { "…" } else { "" }, tail.trim_matches([' ', '\t']));
            facts.push(LensFact::new(FactKind::Warning, text).at(Some(&l.path), Some(l.line)).tag(m.as_str()));
            if facts.len() == 200 {
                break;
            }
        }
        if !facts.is_empty() {
            counts.push(plural(facts.len(), k.1));
            sections.push(LensSection { title: k.0.into(), facts });
        }
    }
    if sections.is_empty() {
        return None;
    }
    let total: usize = sections.iter().map(|s| s.facts.len()).sum();
    let metric = LensMetric { value: total, unit: if total == 1 { "follow-up" } else { "follow-ups" }.into(), ..Default::default() };
    Some(LensReport::new(ReportKind::FollowUps, "Follow-ups", "checklist", counts.join(" · "), sections, metric))
}

/// Whether the end of `before` (the text up to a match) sits inside a string literal: a pattern the code looks for,
/// or test data, isn't a to-do of its own. An apostrophe between letters (a comment's "don't") opens no string.
pub fn in_string(before: &str) -> bool {
    let s: Vec<char> = before.chars().collect();
    let mut open: Option<char> = None;
    for (i, &c) in s.iter().enumerate() {
        if !matches!(c, '"' | '\'' | '`') {
            continue;
        }
        if i > 0 && s[i - 1] == '\\' {
            continue;
        }
        if c == '\'' && i > 0 && i + 1 < s.len() && s[i - 1].is_alphabetic() && s[i + 1].is_alphabetic() {
            continue;
        }
        if open.is_none() {
            open = Some(c);
        } else if open == Some(c) {
            open = None;
        }
    }
    open.is_some()
}

// MARK: - Code: what reading the diff file by file hides

/// Not every definition (the diff shows those): only what a reader going file by file would miss. Files copied from
/// elsewhere, an import swapped for another across files (a refactor's story), and native code in an Expo app whose
/// runtimeVersion stayed put.
pub fn code_report(d: &ChangedLines, changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, dir: &str) -> Option<LensReport> {
    let mut sections = Vec::new();
    let native = native_warning(changes, a, b, dir);
    if let Some(w) = native.clone() {
        sections.push(LensSection { title: "App build".into(), facts: vec![w] });
    }
    let copies = copy_groups(&d.copies);
    if !copies.is_empty() {
        sections.push(LensSection { title: "Copied".into(), facts: copies.clone() });
    }
    let modified: HashSet<String> = changes.iter().filter(|c| c.old.is_some() && c.new.is_some()).filter_map(|c| c.new.clone()).collect();
    let added: HashSet<String> = changes.iter().filter(|c| c.old.is_none()).filter_map(|c| c.new.clone()).collect();
    let shifts = import_shifts(&d.added, &d.removed, &modified, &added);
    if !shifts.is_empty() {
        sections.push(LensSection { title: "Imports across files".into(), facts: shifts.clone() });
    }
    if sections.is_empty() {
        return None;
    }
    let mut summary = Vec::new();
    if native.is_some() {
        summary.push("native code, same runtime".to_string());
    }
    if !copies.is_empty() {
        summary.push(plural(d.copies.len(), "file") + " copied");
    }
    if !shifts.is_empty() {
        summary.push(plural(shifts.len(), "import change") + " across files");
    }
    let all: Vec<&LensFact> = sections.iter().flat_map(|s| &s.facts).collect();
    let (added, removed, changed) = counts(&all);
    let metric = LensMetric { value: all.len(), unit: if all.len() == 1 { "finding" } else { "findings" }.into(), added, removed, changed };
    Some(LensReport::new(ReportKind::Code, "Code", "chevron.left.forwardslash.chevron.right", summary.join(" · "), sections, metric))
}

/// Copies by where they came from and went: "client/src/ → client/src/classic/ · 26 files".
pub fn copy_groups(copies: &[Copy]) -> Vec<LensFact> {
    let mut groups: HashMap<String, Vec<&Copy>> = HashMap::new();
    let mut order = Vec::new();
    for c in copies {
        // The common tail is the file's own path; what differs before it is where it went.
        let f: Vec<&str> = c.from.split('/').filter(|s| !s.is_empty()).collect();
        let t: Vec<&str> = c.to.split('/').filter(|s| !s.is_empty()).collect();
        let mut n = 0;
        while n < f.len().min(t.len()) && f[f.len() - 1 - n] == t[t.len() - 1 - n] {
            n += 1;
        }
        let dir = |p: &[&str]| if p.len() <= n { "./".to_string() } else { p[..p.len() - n].join("/") + "/" };
        let key = if n == 0 { format!("{} → {}", c.from, c.to) } else { format!("{} → {}", dir(&f), dir(&t)) };
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(c);
    }
    order
        .into_iter()
        .map(|k| {
            let g = &groups[&k];
            let (lo, hi) = (g.iter().map(|c| c.similarity).min().unwrap(), g.iter().map(|c| c.similarity).max().unwrap());
            let same = if lo == hi { format!("{lo}%") } else { format!("{lo}–{hi}%") };
            let detail = if g.len() == 1 { format!("{same} the same") } else { format!("{} files · {same} the same", g.len()) };
            LensFact::new(FactKind::Info, k).detail(detail).at(Some(&g[0].to), None).count(g.len())
        })
        .collect()
}

static IMPORT: LazyLock<[Regex; 6]> = LazyLock::new(|| {
    [
        r#"^\s*import\s+(?:type\s+)?(.+?)\s+from\s+['"]([^'"]+)['"]"#,
        r#"^\s*\}\s*from\s+['"]([^'"]+)['"]"#,
        r#"^\s*import\s+['"]([^'"]+)['"]"#,
        r"^\s*from\s+(\S+)\s+import\s+(.+)",
        r"^\s*import\s+([\w.]+)",
        r"^\s*(?:pub\s+)?use\s+([\w:]+?)(?:::\{(.*)\})?\s*;",
    ]
    .map(|p| Regex::new(p).unwrap())
});
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// An import line's module and what it brings in, for JavaScript/TypeScript, Python and Rust.
pub fn import_of(text: &str, path: &str) -> Option<(String, String)> {
    // Most lines aren't imports: a look at how the line starts before any regex.
    let head = text.trim_start_matches([' ', '\t']);
    if !["import", "from", "use ", "pub use", "}"].iter().any(|p| head.starts_with(p)) {
        return None;
    }
    let first = |i: usize| IMPORT[i].captures(text).map(|c| (1..c.len()).map(|g| c.get(g).map_or("", |m| m.as_str()).to_string()).collect::<Vec<_>>());
    let clean = |s: &str| SPACES.replace_all(s.trim_matches(['{', '}', ' ']), " ").into_owned();
    match extension(path).to_lowercase().as_str() {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "vue" | "svelte" => {
            if let Some(g) = first(0) {
                return Some((g[1].clone(), clean(&g[0])));
            }
            if let Some(g) = first(1).or_else(|| first(2)) {
                return Some((g[0].clone(), String::new()));
            }
        }
        "py" => {
            if let Some(g) = first(3) {
                return Some((g[0].clone(), clean(&g[1])));
            }
            if let Some(g) = first(4) {
                return Some((g[0].clone(), String::new()));
            }
        }
        "rs" => {
            if let Some(g) = first(5) {
                return Some((g[0].clone(), clean(&g[1])));
            }
        }
        _ => {}
    }
    None
}

/// Imports that came or went in several files the change edits: one swapped for another ("Button → PillButton ·
/// 5 files"), or a module now used by several. A module whose import only lists other names isn't a change, and new
/// files are left out: each imports its usual libraries.
pub fn import_shifts(added: &[Line], removed: &[Line], modified: &HashSet<String>, new_files: &HashSet<String>) -> Vec<LensFact> {
    struct Use {
        files: BTreeSet<String>,
        names: String,
    }
    let collect = |lines: &[Line]| {
        let mut out: BTreeMap<String, Use> = BTreeMap::new();
        for l in lines.iter().filter(|l| modified.contains(&l.path)) {
            let Some((module, names)) = import_of(&l.text, &l.path) else { continue };
            let u = out.entry(module).or_insert_with(|| Use { files: BTreeSet::new(), names: names.clone() });
            u.files.insert(l.path.clone());
            if u.names.is_empty() {
                u.names = names;
            }
        }
        out
    };
    let (mut plus, mut minus) = (collect(added), collect(removed));
    for (m, p) in plus.iter_mut() {
        if let Some(r) = minus.get_mut(m) {
            let both: BTreeSet<String> = p.files.intersection(&r.files).cloned().collect();
            p.files.retain(|f| !both.contains(f));
            r.files.retain(|f| !both.contains(f));
        }
    }
    let label = |m: &str, names: &str| {
        if names.is_empty() || names.chars().count() > 40 {
            let last = m.rsplit("::").next().unwrap_or(m);
            last.split('/').filter(|s| !s.is_empty()).last().unwrap_or(m).to_string()
        } else {
            names.to_string()
        }
    };
    // A module that's a file this change adds counts from two importers; any other from three.
    let is_new = |m: &str| {
        let stem = m.split('/').filter(|s| !s.is_empty() && !matches!(*s, "." | ".." | "~" | "@")).collect::<Vec<_>>().join("/");
        !stem.is_empty() && new_files.iter().any(|f| crate::path::without_extension(f).ends_with(&stem) || f.ends_with(&stem))
    };
    let mut facts = Vec::new();
    let mut used: HashSet<&str> = HashSet::new();
    // Swaps: a removed and an added module in the same files, best overlaps first.
    let mut pairs: Vec<(&str, &str, Vec<&String>)> = Vec::new();
    for (r, rv) in minus.iter().filter(|(_, v)| v.files.len() >= 2) {
        for (a, av) in plus.iter().filter(|(_, v)| v.files.len() >= 2) {
            let shared: Vec<&String> = rv.files.intersection(&av.files).collect();
            if shared.len() >= 2 {
                pairs.push((r, a, shared));
            }
        }
    }
    pairs.sort_by(|x, y| y.2.len().cmp(&x.2.len()));
    for (r, a, files) in pairs {
        if used.contains(r) || used.contains(a) {
            continue;
        }
        used.insert(r);
        used.insert(a);
        let title = format!("{} → {}", label(r, &minus[r].names), label(a, &plus[a].names));
        facts.push(LensFact::new(FactKind::Changed, title).detail(format!("{} · {a}", plural(files.len(), "file"))).at(files.first().map(|f| f.as_str()), None).count(files.len()));
    }
    fn by_count(m: &BTreeMap<String, Use>) -> Vec<(&String, &Use)> {
        let mut v: Vec<(&String, &Use)> = m.iter().collect();
        v.sort_by(|x, y| y.1.files.len().cmp(&x.1.files.len()));
        v
    }
    for (m, v) in by_count(&plus) {
        if used.contains(m.as_str()) || v.files.len() < if is_new(m) { 2 } else { 3 } {
            continue;
        }
        let mut f = LensFact::new(FactKind::Added, label(m, &v.names))
            .detail(format!("now imported in {} · {m}", plural(v.files.len(), "file")))
            .at(v.files.iter().next().map(String::as_str), None)
            .count(v.files.len());
        f.tag = is_new(m).then(|| "new".into());
        facts.push(f);
    }
    for (m, v) in by_count(&minus) {
        if used.contains(m.as_str()) || v.files.len() < 3 {
            continue;
        }
        facts.push(LensFact::new(FactKind::Removed, label(m, &v.names)).detail(format!("no longer imported in {} · {m}", plural(v.files.len(), "file"))).count(v.files.len()));
    }
    facts
}

/// An Expo app with OTA updates: native code that changed while runtimeVersion stayed the same reaches, through an
/// update, installs built without it. A runtimeVersion policy (appVersion, fingerprint) moves by itself and isn't
/// checked. ponytail: reads the root app.json only, not app.config.js or apps in subfolders.
pub fn native_warning(changes: &[ChangedPath], a: &Snapshot, b: &Snapshot, dir: &str) -> Option<LensFact> {
    let native: Vec<&String> = changes
        .iter()
        .filter_map(|c| c.new.as_ref())
        .filter(|p| ["swift", "m", "mm", "h", "kt", "java", "cpp", "cc", "c"].contains(&extension(p).to_lowercase().as_str()) || file_name(p) == "Podfile.lock")
        .collect();
    if native.is_empty() {
        return None;
    }
    let nb = b.read(&["package.json".into(), "app.json".into()], dir, 2_000_000);
    if !nb.get("package.json").is_some_and(|p| p.contains("\"expo-updates\"")) {
        return None;
    }
    let na = a.read(&["app.json".into()], dir, 2_000_000);
    let runtime = |t: Option<&String>| t.and_then(|t| json(t)).and_then(|j| j["expo"]["runtimeVersion"].as_str().map(str::to_string));
    let rb = runtime(nb.get("app.json"))?;
    if Some(&rb) != runtime(na.get("app.json")).as_ref() {
        return None;
    }
    let names = native.iter().take(3).map(|p| file_name(p)).collect::<Vec<_>>().join(", ") + if native.len() > 3 { "…" } else { "" };
    Some(
        LensFact::new(FactKind::Warning, format!("Native code changed; runtimeVersion is still {rb}"))
            .detail(format!("An OTA update with this JavaScript would reach installs built without it. {}: {names}", plural(native.len(), "native file")))
            .at(Some(native[0]), None)
            .count(native.len()),
    )
}

// MARK: - Branch: the change's base against the branch it goes into

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchCommit {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchStatus {
    pub target: String,
    pub behind: usize,
    /// commits on the target since the change branched off that change the same files
    pub touching: Vec<BranchCommit>,
    /// None when git can't tell (before 2.38); empty: it merges cleanly
    pub conflicts: Option<Vec<String>>,
}

/// None when `head` already has everything on `upstream`.
pub fn branch_status(head: &str, upstream: &str, target: &str, paths: &[String], dir: &str) -> Option<BranchStatus> {
    let range = format!("{head}..{upstream}");
    let n: usize = crate::git::string(&["rev-list", "--count", &range], dir).ok()?.trim().parse().ok()?;
    if n == 0 {
        return None;
    }
    let (log, merge) = std::thread::scope(|s| {
        let log = s.spawn(|| {
            let mut args = vec!["--literal-pathspecs", "log", "--format=%x1e%h%x1f%an%x1f%ar%x1f%s", "--name-only", &range, "--"];
            args.extend(paths.iter().take(500).map(String::as_str));
            crate::git::string(&args, dir).unwrap_or_default()
        });
        let merge = crate::git::run(&["merge-tree", "--write-tree", "--name-only", "--no-messages", upstream, head], dir, None, true).unwrap_or_default();
        (log.join().unwrap(), merge)
    });
    let touching = log
        .split('\u{1e}')
        .filter(|r| !r.is_empty())
        .filter_map(|rec| {
            let mut lines = rec.split('\n').filter(|l| !l.is_empty());
            let f: Vec<&str> = lines.next()?.split('\u{1f}').collect();
            (f.len() == 4).then(|| BranchCommit { id: f[0].into(), subject: f[3].into(), author: f[1].into(), date: f[2].into(), files: lines.map(str::to_string).collect() })
        })
        .collect();
    // merge-tree prints the merged tree, then the conflicted paths; an older git prints its usage instead.
    let out = String::from_utf8_lossy(&merge);
    let lines: Vec<&str> = out.split('\n').filter(|l| !l.is_empty()).collect();
    let conflicts = lines.first().filter(|l| l.len() >= 40 && l.chars().all(|c| c.is_ascii_hexdigit())).map(|_| lines[1..].iter().map(|s| s.to_string()).collect());
    Some(BranchStatus { target: target.into(), behind: n, touching, conflicts })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn replays_migrations_into_a_schema_diff() {
        let m1 = "CREATE TABLE public.users (id uuid PRIMARY KEY, email text NOT NULL, name text);
CREATE TABLE \"lanes\" (id bigint, state text DEFAULT 'open', CONSTRAINT lanes_pkey PRIMARY KEY (id));
CREATE UNIQUE INDEX users_email_idx ON users (lower(email));
CREATE TYPE role AS ENUM ('admin', 'member');
-- a comment; with a semicolon
CREATE FUNCTION touch() RETURNS trigger AS $$ BEGIN NEW.updated_at = now(); RETURN NEW; END; $$ LANGUAGE plpgsql;";
        let m2 = "ALTER TABLE ONLY users ADD COLUMN IF NOT EXISTS role role NOT NULL DEFAULT 'member', DROP COLUMN name;
ALTER TABLE users ALTER COLUMN email TYPE varchar(320);
ALTER TYPE role ADD VALUE 'owner';
DROP TABLE IF EXISTS lanes CASCADE;
CREATE TABLE audit (id bigint, user_id uuid REFERENCES users(id));
ALTER TABLE audit RENAME TO audit_events;
DO $$ BEGIN RAISE NOTICE 'x'; END $$;";
        let a = sql::replay(&[("m/001/up.sql", m1)]);
        let b = sql::replay(&[("m/001/up.sql", m1), ("m/002/up.sql", m2)]);
        assert_eq!(a.tables.keys().collect::<Vec<_>>(), ["lanes", "users"]);
        assert!(a.functions.contains_key("touch"));
        assert_eq!(a.tables["lanes"].primary_key, ["id"]);
        assert!(a.tables["users"].columns[0].not_null);
        let facts = schema_diff(&a, &b);
        let has = |kind, title: &str, detail: &str| facts.iter().any(|f| f.kind == kind && f.title == title && f.detail.as_deref().unwrap_or("").contains(detail));
        assert!(has(FactKind::Added, "audit_events", "table added"));
        assert!(has(FactKind::Removed, "lanes", "table dropped"));
        assert!(has(FactKind::Added, "users.role", "role NOT NULL DEFAULT 'member'"));
        assert!(has(FactKind::Removed, "users.name", "column dropped"));
        assert!(has(FactKind::Changed, "users.email", "type text → varchar(320)"));
        assert!(has(FactKind::Changed, "role", "+ owner"));
        assert_eq!(b.skipped.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["DO block"]);
        assert_eq!(facts.iter().find(|f| f.title == "users.role").and_then(|f| f.line), Some(1)); // points at the ALTER in m/002
    }

    #[test]
    fn reports_seed_data_changes_per_table() {
        let a: HashMap<String, String> = [("seed.sql".into(), "INSERT INTO lanes (id) VALUES (1);\nINSERT INTO public.users (id) VALUES (1);\nUPDATE users SET x = 1;".into())].into();
        let b: HashMap<String, String> = [("seed.sql".into(), "INSERT INTO public.users (id) VALUES (2);\nUPDATE users SET x = 1;\nDELETE FROM jobs;".into())].into();
        let f = data_changes(&["seed.sql".into()], &a, &b, &BTreeMap::new());
        assert!(f.iter().any(|d| d.kind == FactKind::Removed && d.title() == "INSERT into lanes"));
        assert!(f.iter().any(|d| d.kind == FactKind::Changed && d.title() == "INSERT into users"));
        assert!(f.iter().any(|d| d.kind == FactKind::Added && d.title() == "DELETE from jobs"));
        assert!(!f.iter().any(|d| d.title().starts_with("UPDATE"))); // unchanged
    }

    #[test]
    fn reads_dependency_files_and_bumps() {
        let lock = "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n\n[[package]]\nname = \"tokio\"\nversion = \"1.40.0\"\n";
        assert_eq!(parse_dependencies("Cargo.lock", lock), Some(m(&[("serde", "1.0.200"), ("tokio", "1.40.0")])));
        let toml = "[package]\nname = \"x\"\n[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\nanyhow = \"1\"\n[dev-dependencies]\ninsta = \"1.3\"\n";
        assert_eq!(parse_dependencies("Cargo.toml", toml), Some(m(&[("serde", "1.0"), ("anyhow", "1"), ("insta (dev)", "1.3")])));
        assert_eq!(
            parse_dependencies("package.json", r#"{"dependencies":{"react":"^19.1.0"},"devDependencies":{"vite":"^6.0.3"}}"#),
            Some(m(&[("react", "^19.1.0"), ("vite (dev)", "^6.0.3")]))
        );
        assert_eq!(
            parse_dependencies("go.mod", "module m\nrequire (\n\tgolang.org/x/net v0.30.0 // indirect\n)\nrequire github.com/a/b v1.2.3\n"),
            Some(m(&[("golang.org/x/net", "v0.30.0"), ("github.com/a/b", "v1.2.3")]))
        );
        assert!(bump("4.17.0", "4.18.1") == Some("minor") && bump("^1.2.3", "^2.0.0") == Some("major") && bump("1.2.3", "1.2.0") == Some("downgrade"));
        assert_eq!(parse_dependencies("requirements.txt", "# pinned\nrequests>=2.31 # http\n-r other.txt\n"), Some(m(&[("requests", ">=2.31")])));
        assert_eq!(abbreviated_date(1_700_000_000.0), "Nov 14, 2023");
    }

    #[test]
    fn flattens_yaml_and_hides_secrets() {
        let y = "name: ci
on:
  push:
    paths:
      - \"src/**\"
jobs:
  build:
    runs-on: linux
    steps:
      - uses: actions/checkout@v4
      - name: test
        run: |
          cargo test
          cargo clippy
";
        let f = flatten("ci.yml", y);
        assert!(f["name"] == "ci" && f["on.push.paths"] == "[src/**]" && f["jobs.build.runs-on"] == "linux");
        assert!(f["jobs.build.steps[0].uses"] == "actions/checkout@v4" && f["jobs.build.steps[1].name"] == "test"); // not every step is named
        assert_eq!(f["jobs.build.steps[1].run"], "cargo test\ncargo clippy");
        assert!(secret("jobs.x.with.token") && !secret("jobs.design-tokens.steps[3].run"));
    }

    #[test]
    fn reads_kubernetes_yaml_by_what_items_are() {
        // RBAC rules match by what they grant on: a rule inserted in the middle is one new rule, not every later one changed.
        let before = "rules:
  - apiGroups: [\"\"]
    resources: [\"namespaces\"]
    verbs: [\"get\", \"create\", \"patch\"]
  # Create only.
  - apiGroups: [\"\"]
    resources: [\"persistentvolumeclaims\"]
    verbs: [\"get\", \"create\"]";
        let after = "rules:
  - apiGroups: [\"\"]
    resources: [\"namespaces\"]
    verbs: [\"get\", \"create\"]
  - apiGroups: [\"\"]
    resources: [\"configmaps\"]
    verbs: [\"get\"]
  - apiGroups: [\"\"]
    resources: [\"persistentvolumeclaims\"]
    verbs: [\"get\", \"create\"]";
        let facts = key_facts(&flatten("role.yaml", before), &flatten("role.yaml", after), None);
        let mut d: Vec<String> = facts.iter().map(|f| format!("{:?} {} {}", f.kind, f.title, f.detail.as_deref().unwrap_or(""))).collect();
        d.sort();
        assert_eq!(d, ["Added rules[configmaps] verbs: [get]", "Changed rules[namespaces].verbs −patch"]);
        assert_eq!(components("rules[fleetdriver.com: projects].verbs"), ["rules[fleetdriver.com: projects]", "verbs"]);

        // A sequence at its key's own indent, a mapping opened by "- key:", a sequence item that is itself a sequence.
        let y = flatten("x.yaml", "versions:\n- name: v1\n  additionalPrinterColumns:\n  - jsonPath: .spec.size\n    name: Size\n  served: true\nmatrix:\n  - - a\n    - b\n");
        assert!(y["versions[v1].additionalPrinterColumns[Size].jsonPath"] == ".spec.size" && y["versions[v1].served"] == "true" && y["matrix[0]"] == "[a, b]");

        let crd_text = |status: &str| {
            format!(
                "kind: CustomResourceDefinition
spec:
  names:
    kind: Volume
  versions:
  - name: v1
    schema:
      openAPIV3Schema:
        properties:
          status:
            properties:
{status}"
            )
        };
        let a = crd(&crd_text("              phase:\n                enum:\n                - Pending\n                - Bound\n                type: string\n              ready:\n                type: boolean\n            required:\n            - ready")).unwrap();
        let b = crd(&crd_text("              conditions:\n                items:\n                  properties:\n                    type:\n                      type: string\n                  type: object\n                type: array\n              ready:\n                type: string")).unwrap();
        assert!(a.kind == "Volume" && a.fields["status.phase"] == "Pending | Bound" && a.fields["status.ready"] == "boolean · required");
        let mut d: Vec<String> = crd_facts(&a, &b, None).iter().map(|f| format!("{:?} {} {} {}", f.kind, f.title, f.detail.as_deref().unwrap_or(""), f.tag.as_deref().unwrap_or(""))).collect();
        d.sort();
        assert_eq!(d, ["Added status.conditions list of object · type ", "Changed status.ready boolean · required → string breaking", "Removed status.phase Pending | Bound breaking"]);
    }

    #[test]
    fn classifies_files_by_kind() {
        let c = |p| category(p, false);
        assert_eq!(c("src/latest.rs"), "Code");
        assert_eq!(c("Sources/Loupe/FileInspector.swift"), "Code");
        assert_eq!(c("src/block_storage.rs"), "Code");
        assert_eq!(c("ecp/src/infra_e2e.rs"), "Tests");
        assert_eq!(c("pkg/net/conn_test.go"), "Tests");
        assert_eq!(c("web/src/a.test.ts"), "Tests");
        assert_eq!(c("features/network.feature"), "Tests");
        assert_eq!(c("integration/src/acceptance/steps/x.rs"), "Tests");
        assert_eq!(c("Tests/LoupeTests/ParserTests.swift"), "Tests");
        assert_eq!(c("docs/spec.md"), "Docs");
        assert_eq!(c("Cargo.lock"), "Generated");
        assert_eq!(c("web/pnpm-lock.yaml"), "Generated");
        assert_eq!(c("Cargo.toml"), "Dependencies");
        assert_eq!(category("src/x.rs", true), "Generated");
    }

    #[test]
    fn reads_migration_versions_and_follow_ups() {
        assert_eq!(version("2026-11-03-100000_lock"), Some(vec![2026, 11, 3, 100000]));
        assert_eq!(version("V2_1__init.sql"), Some(vec![2, 1]));
        assert_eq!(version("0042_add.up.sql"), Some(vec![42]));
        assert_eq!(version("init.sql"), None);
        let lines: Vec<Line> = ["// TODO: retry", "model.fit(x)", "it.skip('flaky', () => {})", "#[allow(dead_code)]", "let latest = 1"]
            .iter()
            .enumerate()
            .map(|(i, t)| Line { path: "a.ts".into(), line: i + 1, text: t.to_string() })
            .collect();
        let r = follow_up_report(&lines, |_| false).unwrap();
        assert_eq!(r.sections.iter().map(|s| s.facts.iter().map(|f| f.line.unwrap()).collect::<Vec<_>>()).collect::<Vec<_>>(), [[1], [3], [4]]);
        // A pattern the code looks for, or test data, is inside a string; a comment's apostrophe opens none.
        assert!(in_string(&r#"let p = "TODO|FIXME""#[..9]) && in_string(&"x = ['it.skip(']"[..7]));
        assert!(!in_string(&"// don't ship: TODO"[..15]));
    }

    #[test]
    fn code_lens_tells_copies_and_import_swaps() {
        let c = |f: &str, t: &str, s| Copy { from: f.into(), to: t.into(), similarity: s };
        let copies = copy_groups(&[c("src/a/x.tsx", "src/classic/a/x.tsx", 97), c("src/b.tsx", "src/classic/b.tsx", 80), c("lib/old.ts", "lib/new.ts", 60)]);
        assert_eq!(
            copies.iter().map(|f| format!("{} {}", f.title, f.detail.as_deref().unwrap_or(""))).collect::<Vec<_>>(),
            ["src/ → src/classic/ 2 files · 80–97% the same", "lib/old.ts → lib/new.ts 60% the same"]
        );
        let lines = |l: Vec<(&str, &str)>| l.into_iter().map(|(p, t)| Line { path: p.into(), line: 1, text: t.into() }).collect::<Vec<_>>();
        let files = ["a.tsx", "b.tsx", "c.tsx"];
        let mut removed: Vec<(&str, &str)> = files.iter().map(|f| (*f, "import Button from '~/components/Button'")).collect();
        removed.push(("a.tsx", "import { useEffect, useState } from 'react'"));
        let mut added: Vec<(&str, &str)> = files.iter().map(|f| (*f, "import { PillButton } from '~/components/pill-button'")).collect();
        added.push(("a.tsx", "import { useState } from 'react'"));
        added.push(("new.tsx", "import { PillButton } from '~/components/pill-button'"));
        let modified: HashSet<String> = files.iter().map(|s| s.to_string()).collect();
        let facts = import_shifts(&lines(added), &lines(removed), &modified, &["new.tsx".to_string()].into());
        // The react import only lists other names: not a change. The new file's import doesn't count.
        assert_eq!(facts.iter().map(|f| format!("{:?} {} {}", f.kind, f.title, f.count.unwrap_or(0))).collect::<Vec<_>>(), ["Changed Button → PillButton 3"]);
        assert_eq!(import_of("use crate::controllers::{status, Status};", "x.rs").map(|i| i.0), Some("crate::controllers".into()));
        assert_eq!(import_of("} from \"./x\"", "x.ts").map(|i| i.0), Some("./x".into()));

        // Expo's [name, options] plugins by name; gradle.properties read as keys.
        let app = flatten("app.json", r#"{"expo": {"plugins": ["expo-router", ["expo-build-properties", {"android": {"minSdkVersion": 26}}]]}}"#);
        assert!(app["expo.plugins[expo-build-properties].android.minSdkVersion"] == "26" && app["expo.plugins[expo-router]"] == "on");
        assert_eq!(flatten("gradle.properties", "! comment\nandroid.minSdkVersion=26\n"), m(&[("android.minSdkVersion", "26")]));
    }

    #[test]
    fn parses_cat_file_batch_and_diffs() {
        let data = b"abc blob 3\nhi\n\nx:gone missing\nabd blob 2\n\0x\n";
        let paths: Vec<String> = ["a", "gone", "bin"].iter().map(|s| s.to_string()).collect();
        let out = parse_batch(data, &paths, 100);
        assert_eq!(out.get("a").map(String::as_str), Some("hi\n"));
        assert!(!out.contains_key("gone") && !out.contains_key("bin"));
        let d = parse_diff("diff --git a/x b/y\nsimilarity index 90%\ncopy from x\ncopy to y\n--- a/x\n+++ b/y\n@@ -3 +3,2 @@\n-old\n+new\n+more\n");
        assert_eq!((d[0].copied_from.as_deref(), d[0].new_path.as_deref(), d[0].similarity), (Some("x"), Some("y"), Some(90)));
        assert_eq!(d[0].added, [(3, "new".to_string()), (4, "more".to_string())]);
        assert_eq!(d[0].removed, [(3, "old".to_string())]);
    }

    #[test]
    fn analyzes_a_real_repository() {
        let dir = std::env::temp_dir().join(format!("loupe-lens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("migrations")).unwrap();
        let d = dir.to_str().unwrap();
        let git = |args: &[&str]| crate::git::run(args, d, None, false).unwrap();
        std::fs::write(dir.join("migrations/001_init.sql"), "CREATE TABLE users (id int PRIMARY KEY);\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[dependencies]\nserde = \"1.0\"\n").unwrap();
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qm", "a"]);
        std::fs::write(dir.join("migrations/002_more.sql"), "ALTER TABLE users ADD COLUMN email text NOT NULL;\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[dependencies]\nserde = \"2.0\"\n").unwrap();
        std::fs::write(dir.join("lib.rs"), "// TODO: later\n").unwrap();
        let changes = vec![
            ChangedPath { old: None, new: Some("migrations/002_more.sql".into()) },
            ChangedPath { old: Some("Cargo.toml".into()), new: Some("Cargo.toml".into()) },
            ChangedPath { old: None, new: Some("lib.rs".into()) },
        ];
        let head = crate::git::string(&["rev-parse", "HEAD"], d).unwrap().trim().to_string();
        let reports = analyze(&changes, &Snapshot::Rev(head), &Snapshot::Worktree, None, d);
        let kinds: Vec<ReportKind> = reports.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [ReportKind::Files, ReportKind::Database, ReportKind::Dependencies, ReportKind::FollowUps]);
        assert_eq!(reports[1].summary, "1 table changed");
        assert!(reports[1].database[0].warnings.iter().any(|w| w.title == "NOT NULL column without a default: users.email"));
        assert_eq!(reports[2].sections[0].facts[0].detail.as_deref(), Some("1.0 → 2.0 · major"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
