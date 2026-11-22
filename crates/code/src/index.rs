//! Tree-sitter go-to-definition and outline for the languages Loupe has grammars for. Parsers aren't thread-safe, so
//! each call makes its own; compiled queries are immutable and shared.
//!
//! Cancellation is a flag the caller owns (`&AtomicBool`): set it when the view that asked goes away and the parse
//! stops early. gpui drops a task to cancel it, so the app keeps an `Arc<AtomicBool>` beside the task and sets it on drop.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tree_sitter::{Language, ParseOptions, Parser, Query, QueryCursor, StreamingIterator};

/// A named definition or reference, as the grammars' tags queries report them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub is_definition: bool,
    pub path: String,
    /// 1-based
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Module,
    Type,
    Constant,
    Macro,
    Field,
    Other,
}

fn kind(s: &str) -> SymbolKind {
    match s {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" | "struct" | "enum" | "union" => SymbolKind::Class,
        "interface" | "trait" | "protocol" => SymbolKind::Interface,
        "module" | "namespace" => SymbolKind::Module,
        "type" => SymbolKind::Type,
        "constant" => SymbolKind::Constant,
        "macro" => SymbolKind::Macro,
        "field" | "property" => SymbolKind::Field,
        _ => SymbolKind::Other,
    }
}

pub struct Grammar {
    pub language: Language,
    pub tags: Option<Query>,
}

fn grammar_id(path: &str) -> Option<&'static str> {
    Some(match crate::extension(path).to_lowercase().as_str() {
        "rs" => "rust",
        "swift" => "swift",
        "go" => "go",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" => "python",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "mm" => "cpp",
        "java" => "java",
        _ => return None,
    })
}

/// The grammars' tags.scm at the pinned versions; TypeScript's extends JavaScript's and C++'s extends C's.
fn tags_source(id: &str) -> &'static str {
    match id {
        "rust" => include_str!("../queries/rust.scm"),
        "swift" => include_str!("../queries/swift.scm"),
        "go" => include_str!("../queries/go.scm"),
        "typescript" | "tsx" => include_str!("../queries/typescript.scm"),
        "javascript" => include_str!("../queries/javascript.scm"),
        "python" => include_str!("../queries/python.scm"),
        "c" => include_str!("../queries/c.scm"),
        "cpp" => include_str!("../queries/cpp.scm"),
        _ => include_str!("../queries/java.scm"),
    }
}

static GRAMMARS: LazyLock<Mutex<HashMap<&'static str, Arc<Grammar>>>> = LazyLock::new(Default::default);

/// The grammar for a path's extension, if Loupe has one. Built once per language.
pub fn grammar(path: &str) -> Option<Arc<Grammar>> {
    let id = grammar_id(path)?;
    let mut cache = GRAMMARS.lock().unwrap();
    if let Some(g) = cache.get(id) {
        return Some(g.clone());
    }
    let language: Language = match id {
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" => tree_sitter_cpp::LANGUAGE.into(),
        _ => tree_sitter_java::LANGUAGE.into(),
    };
    let tags = Query::new(&language, tags_source(id)).ok();
    let g = Arc::new(Grammar { language, tags });
    cache.insert(id, g.clone());
    Some(g)
}

pub fn can_navigate(path: &str) -> bool {
    grammar(path).is_some_and(|g| g.tags.is_some())
}

// Tree-sitter on a huge or minified file takes minutes and gigabytes (a 30 MB bundle never finished); such files
// aren't worth an outline. Past these, nothing is parsed.
pub const MAX_PARSE_BYTES: usize = 2 << 20;
pub const MAX_PARSE_LINES: usize = 50_000;
/// Minified: a few very long lines.
pub const MAX_AVERAGE_LINE_LENGTH: usize = 1_000;
/// A backstop for what slips under the caps.
pub const PARSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Too big (or minified) to parse: outline and symbol lookups skip it.
pub fn is_too_large(text: &str) -> bool {
    let bytes = text.len();
    if bytes > MAX_PARSE_BYTES {
        return true;
    }
    if bytes <= 64 << 10 {
        return false;
    }
    let mut lines = 1;
    for b in text.bytes() {
        if b == b'\n' {
            lines += 1;
            if lines > MAX_PARSE_LINES {
                return true;
            }
        }
    }
    bytes / lines > MAX_AVERAGE_LINE_LENGTH
}

/// Every definition and reference the language's tags query finds in `text`. Empty for a file over the caps, and
/// when `cancel` is set (the parse stops early).
pub fn symbols(text: &str, path: &str, cancel: &AtomicBool) -> Vec<CodeSymbol> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let Some(g) = grammar(path) else { return Vec::new() };
    let Some(query) = g.tags.as_ref() else { return Vec::new() };
    if is_too_large(text) || cancelled() {
        return Vec::new();
    }
    let mut parser = Parser::new();
    if parser.set_language(&g.language).is_err() {
        return Vec::new();
    }
    let deadline = Instant::now() + PARSE_TIMEOUT;
    let bytes = text.as_bytes();
    let mut progress = |_: &tree_sitter::ParseState| cancelled() || Instant::now() > deadline;
    let tree = parser.parse_with_options(
        &mut |i, _| if i < bytes.len() { &bytes[i..] } else { &[] },
        None,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    );
    let Some(tree) = tree else { return Vec::new() };
    if cancelled() {
        return Vec::new();
    }
    let names = query.capture_names();
    let Some(name_index) = query.capture_index_for_name("name") else { return Vec::new() };
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        if cancelled() {
            return Vec::new();
        }
        let Some(name) = m.captures.iter().find(|c| c.index == name_index) else { continue };
        let tag = m.captures.iter().map(|c| names[c.index as usize]).find(|n| n.starts_with("definition.") || n.starts_with("reference."));
        let Some((role, k)) = tag.and_then(|t| t.split_once('.')) else { continue };
        let Ok(s) = name.node.utf8_text(bytes) else { continue };
        if s.is_empty() || s.chars().count() >= 200 {
            continue;
        }
        out.push(CodeSymbol {
            name: s.to_string(),
            kind: kind(k),
            is_definition: role == "definition",
            path: path.to_string(),
            line: name.node.start_position().row + 1,
        });
    }
    out
}

/// The definitions in a file, in order, one per line: the file's outline.
pub fn outline(text: &str, path: &str, cancel: &AtomicBool) -> Vec<CodeSymbol> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<_> = symbols(text, path, cancel).into_iter().filter(|s| s.is_definition && seen.insert(s.line)).collect();
    out.sort_by_key(|s| s.line);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outline {
    Symbols(Arc<Vec<CodeSymbol>>),
    TooLarge,
}

/// The outline for a view: one parse per file content at a time however many views ask, cached. Call it off the UI
/// thread; it blocks until the parse ends or `cancel` is set.
pub fn outline_of(text: &Arc<str>, path: &str, cancel: &AtomicBool) -> Outline {
    if is_too_large(text) {
        return Outline::TooLarge;
    }
    Outline::Symbols(OutlineCache::shared().outline(text, path, cancel))
}

/// Where a parse leaves its outline, and the bell it rings.
type Slot = Arc<(Mutex<Option<Arc<Vec<CodeSymbol>>>>, Condvar)>;

struct Flight {
    result: Slot,
    cancel: Arc<AtomicBool>,
    waiters: usize,
}

/// Outlines by path and content, bounded; and the parses in flight, shared by every caller of the same key. A parse
/// is cancelled once all its callers are.
pub struct OutlineCache {
    // ponytail: evicts everything at the cap instead of least-recently-used; outlines are cheap to redo.
    cache: Mutex<HashMap<String, Arc<Vec<CodeSymbol>>>>,
    flights: Mutex<HashMap<String, Flight>>,
    /// Parses started (tests).
    pub parses: AtomicUsize,
}

const CACHE_LIMIT: usize = 64;

impl OutlineCache {
    pub fn new() -> Self {
        Self { cache: Default::default(), flights: Default::default(), parses: AtomicUsize::new(0) }
    }

    pub fn shared() -> &'static OutlineCache {
        static SHARED: LazyLock<OutlineCache> = LazyLock::new(OutlineCache::new);
        &SHARED
    }

    fn key(text: &str, path: &str) -> String {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        format!("{path}\0{}\0{}", text.len(), h.finish())
    }

    pub fn in_flight(&self) -> usize {
        self.flights.lock().unwrap().len()
    }

    /// Empty when `cancel` is set before the outline arrives.
    pub fn outline(&self, text: &Arc<str>, path: &str, cancel: &AtomicBool) -> Arc<Vec<CodeSymbol>> {
        let key = Self::key(text, path);
        if let Some(hit) = self.cache.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let (result, parse_cancel) = {
            let mut flights = self.flights.lock().unwrap();
            let f = flights.entry(key.clone()).or_insert_with(|| {
                self.parses.fetch_add(1, Ordering::Relaxed);
                let result: Slot = Default::default();
                let flag = Arc::new(AtomicBool::new(false));
                let (r, c, t, p) = (result.clone(), flag.clone(), text.clone(), path.to_string());
                std::thread::spawn(move || {
                    let s = Arc::new(outline(&t, &p, &c));
                    *r.0.lock().unwrap() = Some(s);
                    r.1.notify_all();
                });
                Flight { result, cancel: flag, waiters: 0 }
            });
            f.waiters += 1;
            (f.result.clone(), f.cancel.clone())
        };
        let mut done = result.0.lock().unwrap();
        while done.is_none() && !cancel.load(Ordering::Relaxed) {
            done = result.1.wait_timeout(done, Duration::from_millis(20)).unwrap().0;
        }
        let symbols = done.clone();
        drop(done);
        let cancelled = cancel.load(Ordering::Relaxed);
        self.leave(&key, cancelled && symbols.is_none());
        match symbols {
            Some(s) if !cancelled => {
                if !parse_cancel.load(Ordering::Relaxed) {
                    let mut cache = self.cache.lock().unwrap();
                    if cache.len() >= CACHE_LIMIT {
                        cache.clear();
                    }
                    cache.insert(key, s.clone());
                }
                s
            }
            _ => Arc::new(Vec::new()),
        }
    }

    /// A caller is done: the last one out of a cancelled wait cancels the parse.
    fn leave(&self, key: &str, cancel: bool) {
        let mut flights = self.flights.lock().unwrap();
        let Some(f) = flights.get_mut(key) else { return };
        f.waiters -= 1;
        if f.waiters == 0 {
            if cancel {
                f.cancel.store(true, Ordering::Relaxed);
            }
            flights.remove(key);
        }
    }
}

impl Default for OutlineCache {
    fn default() -> Self {
        Self::new()
    }
}

// MARK: Repository-wide lookups (no checkout: everything is read at a commit)

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodeMatch {
    pub path: String,
    /// 1-based
    pub line: usize,
    pub text: String,
}

/// What git grep searches: a revision, or the worktree with its untracked (not ignored) files.
fn scope(rev: Option<&str>) -> Vec<&str> {
    match rev {
        Some(r) => vec![r, "--"],
        None => vec!["--untracked", "--"],
    }
}

fn rank(candidate: &str, from: &str) -> u8 {
    if candidate == from {
        0
    } else if crate::parent(candidate) == crate::parent(from) {
        1
    } else if crate::extension(candidate) == crate::extension(from) {
        2
    } else {
        3
    }
}

fn grep_files(name: &str, rev: Option<&str>, dir: &str) -> Vec<String> {
    let mut args = vec!["grep", "-l", "-I", "-w", "--fixed-strings", "-e", name];
    args.extend(scope(rev));
    let Ok(out) = crate::git::string(&args, dir) else { return Vec::new() };
    let prefix = rev.map(|r| format!("{r}:"));
    out.lines().map(|l| prefix.as_deref().and_then(|p| l.strip_prefix(p)).unwrap_or(l).to_string()).collect()
}

/// Definitions of `name` at `rev` (None: the worktree as it is): `git grep -w` narrows the files, tree-sitter decides
/// which hits define it. Ranked: the file you're in, then its directory, then the same language, then the rest.
pub fn definitions(name: &str, rev: Option<&str>, dir: &str, from: &str, cancel: &AtomicBool) -> Vec<CodeSymbol> {
    let mut files: Vec<String> = grep_files(name, rev, dir).into_iter().filter(|f| can_navigate(f)).collect();
    files.sort_by_key(|f| rank(f, from));
    files.truncate(80);
    let found = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(files.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(f) = files.get(i) else { break };
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let text = match rev {
                        Some(r) => crate::git::string(&["show", &format!("{r}:{f}")], dir).ok(),
                        None => std::fs::read_to_string(std::path::Path::new(dir).join(f)).ok(),
                    };
                    let Some(text) = text else { continue };
                    let defs = symbols(&text, f, cancel).into_iter().filter(|s| s.is_definition && s.name == name);
                    found.lock().unwrap().extend(defs);
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap();
    found.sort_by(|a, b| (rank(&a.path, from), &a.path, a.line).cmp(&(rank(&b.path, from), &b.path, b.line)));
    found
}

/// Every whole-word occurrence of `name` at `rev` (None: the worktree, untracked files included).
pub fn references(name: &str, rev: Option<&str>, dir: &str, limit: usize) -> Vec<CodeMatch> {
    let mut args = vec!["grep", "-n", "-I", "-w", "--fixed-strings", "-e", name];
    args.extend(scope(rev));
    let Ok(out) = crate::git::string(&args, dir) else { return Vec::new() };
    let prefix = rev.map(|r| format!("{r}:"));
    let mut matches = Vec::new();
    for line in out.lines() {
        // "<rev>:<path>:<line>:<text>"
        let rest = prefix.as_deref().and_then(|p| line.strip_prefix(p)).unwrap_or(line);
        let mut f = rest.splitn(3, ':');
        let (Some(path), Some(n), Some(text)) = (f.next(), f.next(), f.next()) else { continue };
        let Ok(n) = n.parse() else { continue };
        matches.push(CodeMatch { path: path.to_string(), line: n, text: text.trim_matches([' ', '\t']).to_string() });
        if matches.len() >= limit {
            break;
        }
    }
    matches
}

/// `references`' limit in the Swift app.
pub const REFERENCE_LIMIT: usize = 2000;

#[cfg(test)]
mod tests {
    use super::*;

    fn no() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn finds_definitions_with_tree_sitter() {
        let rust = "struct Foo;\nimpl Foo { fn bar(&self) {} }\n// helper is mentioned here\nfn helper() { let f = Foo; f.bar(); }\n";
        let defs = outline(rust, "src/lib.rs", &no());
        assert_eq!(defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Foo", "bar", "helper"]);
        assert_eq!(defs.iter().map(|d| d.line).collect::<Vec<_>>(), [1, 2, 4]);
        let ts = "export function encode() {}\nclass Preset { name = '' }\ninterface Opts { a: number }\n";
        let names: Vec<_> = outline(ts, "a.ts", &no()).into_iter().map(|s| s.name).collect();
        for n in ["encode", "Preset", "Opts"] {
            assert!(names.iter().any(|x| x == n), "{n}");
        }
        assert!(symbols("x: 1", "a.yaml", &no()).is_empty()); // no grammar: nothing, no crash
    }

    #[test]
    fn every_grammar_loads_its_tags_query() {
        let samples = [
            ("a.rs", "fn f() {}"),
            ("a.swift", "func f() {}"),
            ("a.go", "package a\nfunc f() {}"),
            ("a.ts", "function f() {}"),
            ("a.tsx", "function f() { return <div/> }"),
            ("a.js", "function f() {}"),
            ("a.py", "def f():\n  pass"),
            ("a.c", "int f(void) { return 0; }"),
            ("a.cpp", "int f() { return 0; }"),
            ("a.java", "class A {\n  void f() {}\n}"),
        ];
        for (p, src) in samples {
            assert!(can_navigate(p), "{p}: tags query didn't compile");
            assert!(outline(src, p, &no()).iter().any(|s| s.name == "f"), "{p}");
        }
    }

    #[test]
    fn size_caps() {
        assert!(!is_too_large("func a() {}\n"));
        assert!(is_too_large(&"x".repeat(MAX_PARSE_BYTES + 1)));
        // Minified: few lines, each very long.
        assert!(is_too_large(&vec!["var a=1;".repeat(200); 100].join("\n")));
        // Many short lines over the line cap.
        assert!(is_too_large(&"x\n".repeat(MAX_PARSE_LINES + 10)));
        // Ordinary source of a few thousand lines.
        assert!(!is_too_large(&(0..3000).map(|i| format!("func f{i}() {{ return {i} }}")).collect::<Vec<_>>().join("\n")));
    }

    #[test]
    fn outline_still_works() {
        let Outline::Symbols(s) = outline_of(&Arc::from("struct A {}\nfunc b() {}\n"), "x.swift", &no()) else { panic!("too large?") };
        assert!(s.iter().any(|s| s.name == "b"));
        assert_eq!(outline_of(&Arc::from("var a=1;".repeat(400_000)), "x.js", &no()), Outline::TooLarge);
    }

    #[test]
    fn concurrent_calls_share_one_parse() {
        let cache = OutlineCache::new();
        let text: Arc<str> = (0..2000).map(|i| format!("func f{i}() {{}}")).collect::<Vec<_>>().join("\n").into();
        let (x, y) = std::thread::scope(|s| {
            let a = s.spawn(|| cache.outline(&text, "a.swift", &no()));
            let b = s.spawn(|| cache.outline(&text, "a.swift", &no()));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert!(x == y && !x.is_empty());
        assert_eq!(cache.parses.load(Ordering::Relaxed), 1);
        cache.outline(&text, "a.swift", &no()); // cached
        assert_eq!(cache.parses.load(Ordering::Relaxed), 1);
        assert_eq!(cache.in_flight(), 0);
    }

    #[test]
    fn cancelling_the_caller_stops_the_parse() {
        let cache = OutlineCache::new();
        // Under the caps but slow enough to still be parsing when cancelled.
        let text = (0..45_000).map(|i| format!("func f{i}(a: Int, b: String) -> [Int: String] {{ return [a: b] }}")).collect::<Vec<_>>().join("\n");
        let mut cut = MAX_PARSE_BYTES - 1;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let text: Arc<str> = text[..cut].into();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            c.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let r = cache.outline(&text, "big.swift", &cancel);
        assert!(r.is_empty());
        assert_eq!(cache.in_flight(), 0);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancelled_symbols_return_empty() {
        assert!(symbols("func a() {}\n", "a.swift", &AtomicBool::new(true)).is_empty());
    }

    #[test]
    fn navigates_at_a_commit_without_checkout() {
        let dir = std::env::temp_dir().join(format!("loupe-code-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let d = dir.to_str().unwrap();
        std::fs::write(dir.join("src/fit.rs"), "pub fn fit(edge: u32) -> u32 { edge }\n").unwrap();
        std::fs::write(dir.join("src/main.rs"), "use crate::fit::fit;\n// fit is great\nfn main() { fit(3); }\n").unwrap();
        crate::git::run(&["init", "-q"], d, None, false).unwrap();
        crate::git::run(&["add", "."], d, None, false).unwrap();
        crate::git::run(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qm", "x"], d, None, false).unwrap();
        let head = crate::git::string(&["rev-parse", "HEAD"], d).unwrap().trim().to_string();
        std::fs::remove_file(dir.join("src/fit.rs")).unwrap(); // the worktree doesn't matter: reads go to the commit
        let defs = definitions("fit", Some(&head), d, "src/main.rs", &no());
        assert_eq!(defs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["src/fit.rs"]);
        assert_eq!(defs[0].line, 1);
        let refs = references("fit", Some(&head), d, REFERENCE_LIMIT);
        assert_eq!(refs.len(), 4);
        assert!(refs.iter().any(|r| r.path == "src/main.rs" && r.line == 3));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
