//! Lightweight, dependency-free syntax highlighting for diff lines. It tokenizes line by line (diffs are fragments,
//! so a full parser would lack context anyway) and carries block-comment / multi-line-string state across lines.
//! ponytail: hand tokenizer, not a grammar; tree-sitter-highlight was measured and lost (see tests/highlight_bench.rs).
//!
//! Offsets are UTF-8 byte offsets into the text given (gpui's text runs are byte ranges), not the UTF-16 offsets the
//! Swift version used. Every delimiter is ASCII, so spans always start and end on char boundaries.

use crate::extension;
use std::sync::LazyLock;

/// The order matters: `CodeTheme::tokens` is indexed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Macro,
    Attribute,
    Property,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxSpan {
    pub start: u32,
    pub len: u32,
    pub kind: TokenKind,
}

impl SyntaxSpan {
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

pub struct Language {
    /// Sorted, for binary search.
    keywords: Vec<&'static str>,
    line_comments: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    strings: &'static [u8],
    /// Delimiters that may span lines: `"""`, `` ` ``, `''`.
    multiline_strings: &'static [&'static str],
    /// rust: `name!`
    macro_bang: bool,
    /// `#[`, `@`
    attribute_prefix: &'static [&'static str],
    /// SQL
    case_insensitive: bool,
    /// YAML/TOML/JSON-ish: highlight keys
    key_value: bool,
    /// shell, PHP
    dollar_vars: bool,
}

const DEFAULT: Language = Language {
    keywords: Vec::new(),
    line_comments: &["//"],
    block: Some(("/*", "*/")),
    strings: b"\"'",
    multiline_strings: &[],
    macro_bang: false,
    attribute_prefix: &[],
    case_insensitive: false,
    key_value: false,
    dollar_vars: false,
};

fn lang(words: &'static str, l: Language) -> Language {
    let mut keywords: Vec<&str> = words.split(' ').collect();
    keywords.sort_unstable();
    keywords.dedup();
    Language { keywords, ..l }
}

macro_rules! language {
    ($name:ident, $words:expr $(, $field:ident: $value:expr)* $(,)?) => {
        pub static $name: LazyLock<Language> = LazyLock::new(|| lang($words, Language { $($field: $value,)* ..DEFAULT }));
    };
}

language!(RUST, "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while yield Some None Ok Err",
    macro_bang: true, attribute_prefix: &["#[", "#!["]);
language!(SWIFT, "actor any as associatedtype async await break case catch class continue default defer deinit do else enum extension fallthrough false fileprivate final for func guard if import in init inout internal is lazy let mutating nil nonisolated open operator override private protocol public repeat rethrows return self Self some static struct subscript super switch throw throws true try typealias unowned var weak where while",
    multiline_strings: &["\"\"\""], attribute_prefix: &["@"]);
language!(GO, "break case chan const continue default defer else fallthrough false for func go goto if import interface iota map nil package range return select struct switch true type var",
    multiline_strings: &["`"]);
language!(TYPESCRIPT, "abstract as async await break case catch class const continue debugger declare default delete do else enum export extends false finally for from function get if implements import in instanceof interface keyof let new null of private protected public readonly return satisfies set static super switch this throw true try type typeof undefined var void while yield",
    multiline_strings: &["`"], attribute_prefix: &["@"]);
language!(PYTHON, "and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield match case",
    line_comments: &["#"], block: None, multiline_strings: &["\"\"\"", "'''"], attribute_prefix: &["@"]);
language!(C, "auto break case char const continue default do double else enum extern float for goto if inline int long register return short signed sizeof static struct switch typedef union unsigned void volatile while bool true false NULL nullptr class namespace public private protected template typename virtual override new delete this using constexpr auto",
    attribute_prefix: &["#"]);
language!(JVM, "abstract and as break case catch class const continue data default do else enum extends false final finally for fun if implements import in interface is internal lateinit new null object open override package private protected public return sealed static super suspend switch this throw throws true try val var void when while",
    multiline_strings: &["\"\"\""], attribute_prefix: &["@"]);
language!(NIX, "let in with rec inherit if then else assert import true false null or builtins",
    line_comments: &["#"], strings: b"\"", multiline_strings: &["''"]);
language!(SQL, "select from where and or not insert into values update set delete create table index unique primary key foreign references alter add drop column constraint default null is in as on join left right inner outer group by order having limit offset returning begin commit rollback if exists cascade check case when then else end distinct union all text integer bigint boolean timestamptz uuid jsonb varchar serial timestamp bytea date time interval numeric decimal real float double smallint char json array with view trigger function returns language sequence grant revoke type enum materialized concurrently using",
    line_comments: &["--"], strings: b"'", case_insensitive: true);
language!(SHELL, "if then else elif fi for while until do done case esac function in return export local readonly set unset source echo exit RUN FROM COPY ADD ENV ARG WORKDIR CMD ENTRYPOINT EXPOSE USER",
    line_comments: &["#"], block: None, dollar_vars: true);
language!(YAML, "true false null yes no on off", line_comments: &["#"], block: None, key_value: true);
language!(TOML, "true false", line_comments: &["#", ";"], block: None, multiline_strings: &["\"\"\""], key_value: true);
language!(JSON, "true false null", line_comments: &["//"], strings: b"\"", key_value: true);
language!(CSS, "important inherit initial unset none auto", line_comments: &["//"], attribute_prefix: &["@"], key_value: true);
language!(RUBY, "alias and begin break case class def defined do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield require attr_accessor",
    line_comments: &["#"], block: None, attribute_prefix: &["@"]);
language!(LUA, "and break do else elseif end false for function goto if in local nil not or repeat return then true until while",
    line_comments: &["--"], block: Some(("--[[", "]]")));
language!(SOLIDITY, "contract interface library function modifier event emit struct enum mapping public private internal external view pure payable returns return if else for while memory storage calldata require revert assert uint256 uint address bool bytes32 string true false constructor override virtual immutable constant");
language!(PROTO, "syntax package import option message enum service rpc returns repeated optional map oneof reserved true false type query mutation subscription fragment on input scalar interface union extend schema",
    line_comments: &["//", "#"]);
language!(PHP, "abstract and array as break case catch class const continue default do echo else elseif extends false final for foreach function if implements interface namespace new null private protected public return static switch this throw true try use while",
    line_comments: &["//", "#"], dollar_vars: true);
language!(HCL, "resource data variable output module provider locals terraform true false null for in if",
    line_comments: &["#", "//"], key_value: true);

impl Language {
    pub fn for_path(path: &str) -> Option<&'static Language> {
        let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
        if name == "dockerfile" || name.starts_with("dockerfile.") || name == "makefile" || name == "justfile" {
            return Some(&SHELL);
        }
        let ext = extension(&name);
        Some(match ext {
            "rs" => &RUST,
            "swift" => &SWIFT,
            "go" => &GO,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => &TYPESCRIPT,
            "py" | "pyi" => &PYTHON,
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "m" | "mm" => &C,
            "java" | "kt" | "kts" | "scala" | "cs" | "dart" => &JVM,
            "nix" => &NIX,
            "sql" => &SQL,
            "sh" | "bash" | "zsh" | "fish" | "envrc" => &SHELL,
            "yml" | "yaml" => &YAML,
            "toml" | "ini" | "cfg" | "conf" => &TOML,
            "json" | "jsonc" | "json5" => &JSON,
            "css" | "scss" | "sass" | "less" => &CSS,
            "rb" => &RUBY,
            "lua" => &LUA,
            "sol" => &SOLIDITY,
            "proto" | "graphql" | "gql" => &PROTO,
            "php" => &PHP,
            "tf" | "tfvars" | "hcl" => &HCL,
            _ => return None,
        })
    }

    fn is_keyword(&self, word: &[u8]) -> bool {
        // Words are ASCII-or-UTF-8 identifier bytes; keywords are ASCII, so a non-UTF-8 slice can't be one anyway.
        let Ok(w) = std::str::from_utf8(word) else { return false };
        if self.case_insensitive && w.bytes().any(|b| b.is_ascii_uppercase()) {
            return self.keywords.binary_search(&w.to_ascii_lowercase().as_str()).is_ok();
        }
        self.keywords.binary_search(&w).is_ok()
    }
}

/// Lines longer than this are truncated on screen (minified files); copy still gets the full text.
pub const MAX_DRAWN_CHARS: usize = 1500;

/// Text exactly as the row draws it (truncated, tabs expanded), so span offsets line up.
pub fn display_text(text: &str) -> std::borrow::Cow<'_, str> {
    let cut = text.char_indices().nth(MAX_DRAWN_CHARS).map_or(text, |(i, _)| &text[..i]);
    if cut.contains('\t') { cut.replace('\t', "    ").into() } else { cut.into() }
}

/// What a line opened and didn't close: carried to the next line of the same side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub block: bool,
    pub string: Option<&'static str>,
}

/// A diff line's role, as far as highlighting cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    Hunk,
    Meta,
}

/// Spans for every line of a diff, the text as `display_text` gives it. Old and new sides keep separate state (a
/// removed line opening a block comment must not swallow added lines); a hunk header resets both.
pub fn highlight<'a>(lines: impl IntoIterator<Item = (LineKind, &'a str)>, lang: &Language) -> Vec<Vec<SyntaxSpan>> {
    let (mut old, mut new) = (State::default(), State::default());
    lines
        .into_iter()
        .map(|(kind, text)| match kind {
            LineKind::Hunk => {
                old = State::default();
                new = State::default();
                Vec::new()
            }
            LineKind::Meta => Vec::new(),
            LineKind::Removed => tokenize(&display_text(text), lang, &mut old),
            LineKind::Added => tokenize(&display_text(text), lang, &mut new),
            LineKind::Context => {
                let spans = tokenize(&display_text(text), lang, &mut old);
                new = old;
                spans
            }
        })
        .collect()
}

/// Consecutive lines of one text (a snippet, a file): state carries from each to the next. Empty spans for a path
/// with no language.
pub fn highlight_snippet<S: AsRef<str>>(lines: &[S], path: &str) -> Vec<Vec<SyntaxSpan>> {
    let Some(lang) = Language::for_path(path) else { return vec![Vec::new(); lines.len()] };
    let mut state = State::default();
    lines.iter().map(|l| tokenize(l.as_ref(), lang, &mut state)).collect()
}

fn starts(u: &[u8], s: &str, at: usize) -> bool {
    u.get(at..).is_some_and(|r| r.starts_with(s.as_bytes()))
}

fn find(u: &[u8], s: &str, from: usize) -> Option<usize> {
    let s = s.as_bytes();
    if from >= u.len() || s.is_empty() {
        return None;
    }
    u[from..].windows(s.len()).position(|w| w == s).map(|p| p + from)
}

/// UTF-8 length of the char starting with byte `b`.
fn char_len(b: u8) -> usize {
    match b {
        0xF0.. => 4,
        0xE0.. => 3,
        0xC0.. => 2,
        _ => 1,
    }
}

pub fn tokenize(text: &str, lang: &Language, state: &mut State) -> Vec<SyntaxSpan> {
    let u = text.as_bytes();
    let n = u.len();
    let mut spans = Vec::new();
    let mut add = |s: usize, e: usize, k: TokenKind| {
        if e > s {
            spans.push(SyntaxSpan { start: s as u32, len: (e - s) as u32, kind: k });
        }
    };
    // Bytes of a multi-byte char are all > 127, so they read as identifier characters like the UTF-16 units did.
    let ident_start = |c: u8| c.is_ascii_alphabetic() || c == b'_' || (!lang.dollar_vars && c == b'$') || c > 127;
    let ident = |c: u8| ident_start(c) || c.is_ascii_digit();
    let mut i = 0;

    // Continue constructs opened on a previous line.
    if state.block {
        if let Some((_, close)) = lang.block {
            match find(u, close, 0) {
                Some(e) => {
                    add(0, e + close.len(), TokenKind::Comment);
                    i = e + close.len();
                    state.block = false;
                }
                None => {
                    add(0, n, TokenKind::Comment);
                    return spans;
                }
            }
        }
    }
    if let Some(delim) = state.string {
        match find(u, delim, 0) {
            Some(e) => {
                add(0, e + delim.len(), TokenKind::String);
                i = e + delim.len();
                state.string = None;
            }
            None => {
                add(0, n, TokenKind::String);
                return spans;
            }
        }
    }
    // YAML/TOML/JSON keys: `key:` / `key =` / `"key":` at the start of the line.
    if lang.key_value {
        let mut p = i;
        while p < n && (u[p] == b' ' || u[p] == b'-') {
            p += 1; // indent and "- "
        }
        if p < n && u[p] == b'[' {
            // [section]
            if let Some(e) = find(u, "]", p) {
                add(p, e + 1, TokenKind::Type);
                i = e + 1;
            }
        } else {
            let quoted = p < n && u[p] == b'"';
            let mut q = if quoted { p + 1 } else { p };
            while q < n && if quoted { u[q] != b'"' } else { ident(u[q]) || u[q] == b'-' || u[q] == b'.' } {
                q += 1;
            }
            let end = if quoted { q + 1 } else { q };
            let mut r = end;
            while r < n && u[r] == b' ' {
                r += 1;
            }
            if q > p && r < n && (u[r] == b':' || u[r] == b'=') {
                add(p, end, TokenKind::Property);
                i = end;
            }
        }
    }

    while i < n {
        let c = u[i];
        if c == b' ' || c == b'\t' {
            i += 1;
            continue;
        }
        if lang.line_comments.iter().any(|s| starts(u, s, i)) {
            add(i, n, TokenKind::Comment);
            break;
        }
        if let Some((open, close)) = lang.block {
            if starts(u, open, i) {
                if let Some(e) = find(u, close, i + open.len()) {
                    add(i, e + close.len(), TokenKind::Comment);
                    i = e + close.len();
                    continue;
                }
                add(i, n, TokenKind::Comment);
                state.block = true;
                break;
            }
        }
        if let Some(&m) = lang.multiline_strings.iter().find(|s| starts(u, s, i)) {
            if let Some(e) = find(u, m, i + m.len()) {
                add(i, e + m.len(), TokenKind::String);
                i = e + m.len();
                continue;
            }
            add(i, n, TokenKind::String);
            state.string = Some(m);
            break;
        }
        if let Some(&a) = lang.attribute_prefix.iter().find(|s| starts(u, s, i)) {
            let bracket = a.ends_with('[');
            let mut e = i + a.len();
            if bracket {
                e = find(u, "]", e).unwrap_or(n - 1) + 1;
            } else {
                while e < n && (ident(u[e]) || u[e] == b'.') {
                    e += 1;
                }
            }
            if e > i + a.len() || bracket {
                add(i, e, TokenKind::Attribute);
                i = e;
                continue;
            }
        }
        if lang.dollar_vars && c == b'$' {
            let mut e = i + 1;
            if e < n && u[e] == b'{' {
                e = find(u, "}", e).unwrap_or(n - 1) + 1;
            } else {
                while e < n && ident(u[e]) {
                    e += 1;
                }
            }
            add(i, e, TokenKind::Type);
            i = e.max(i + 1);
            continue;
        }
        if lang.strings.contains(&c) {
            // Rust 'a lifetimes look like an unterminated char literal. The char after the quote may be several bytes.
            if c == b'\'' && lang.macro_bang {
                let after = if i + 1 < n { i + 1 + char_len(u[i + 1]) } else { n };
                if !(after < n && u[after] == b'\'' || i + 1 < n && u[i + 1] == b'\\') {
                    let mut e = i + 1;
                    while e < n && ident(u[e]) {
                        e += 1;
                    }
                    add(i, e, TokenKind::Attribute);
                    i = e.max(i + 1);
                    continue;
                }
            }
            let mut e = i + 1;
            while e < n && u[e] != c {
                e += if u[e] == b'\\' { 1 + char_len(u.get(e + 1).copied().unwrap_or(0)) } else { 1 };
            }
            let e = (e + 1).min(n);
            add(i, e, TokenKind::String);
            i = e;
            continue;
        }
        if c.is_ascii_digit() {
            let mut e = i + 1;
            while e < n && (ident(u[e]) || u[e] == b'.') {
                e += 1;
            }
            add(i, e, TokenKind::Number);
            i = e;
            continue;
        }
        if ident_start(c) {
            let mut e = i + 1;
            while e < n && ident(u[e]) {
                e += 1;
            }
            let mut next = e;
            while next < n && u[next] == b' ' {
                next += 1;
            }
            if lang.is_keyword(&u[i..e]) {
                add(i, e, TokenKind::Keyword);
            } else if lang.macro_bang && e < n && u[e] == b'!' {
                add(i, e + 1, TokenKind::Macro);
                e += 1;
            } else if next < n && u[next] == b'(' {
                add(i, e, TokenKind::Function);
            } else if c.is_ascii_uppercase() && !lang.case_insensitive {
                add(i, e, TokenKind::Type);
            }
            i = e;
            continue;
        }
        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_rust() {
        let mut st = State::default();
        let line = r#"    let x: Vec<&'a str> = format!("hi {}", 42); // done"#;
        let spans = tokenize(line, &RUST, &mut st);
        let kind = |w: &str| {
            let at = line.find(w).unwrap() as u32;
            spans.iter().find(|s| s.start == at).map(|s| s.kind)
        };
        assert_eq!(kind("let"), Some(TokenKind::Keyword));
        assert_eq!(kind("Vec"), Some(TokenKind::Type));
        assert_eq!(kind("'a"), Some(TokenKind::Attribute));
        assert_eq!(kind("format!"), Some(TokenKind::Macro));
        assert_eq!(kind("\"hi {}\""), Some(TokenKind::String));
        assert_eq!(kind("42"), Some(TokenKind::Number));
        assert_eq!(kind("// done"), Some(TokenKind::Comment));
        // A char literal of a multi-byte char is a string, not a lifetime.
        let s = tokenize("'é' x", &RUST, &mut State::default());
        assert_eq!((s[0].kind, s[0].len), (TokenKind::String, 4));
    }

    #[test]
    fn carries_block_comments_per_side() {
        let lines = [
            (LineKind::Hunk, "@@ -1,2 +1,2 @@"),
            (LineKind::Removed, "/* old open"),
            (LineKind::Added, "const x = 1"),
            (LineKind::Context, "end */ const y = 2"),
        ];
        let s = highlight(lines, &TYPESCRIPT);
        assert_eq!(s[1].first().map(|s| s.kind), Some(TokenKind::Comment)); // removed line opens a comment
        assert_eq!(s[2].first().map(|s| s.kind), Some(TokenKind::Keyword)); // added line is unaffected by it
        assert_eq!(s[3].first().map(|s| s.kind), Some(TokenKind::Comment)); // context continues the old side's
    }

    #[test]
    fn keys_sql_and_shell() {
        let s = tokenize("  name: \"x\" # c", &YAML, &mut State::default());
        assert_eq!(s.iter().map(|s| s.kind).collect::<Vec<_>>(), [TokenKind::Property, TokenKind::String, TokenKind::Comment]);
        let s = tokenize("SELECT id FROM t", &SQL, &mut State::default());
        assert_eq!(s.iter().filter(|s| s.kind == TokenKind::Keyword).count(), 2);
        let s = tokenize("echo ${HOME} $x", &SHELL, &mut State::default());
        assert_eq!(s.iter().map(|s| s.kind).collect::<Vec<_>>(), [TokenKind::Keyword, TokenKind::Type, TokenKind::Type]);
        assert!(Language::for_path("Dockerfile.dev").is_some() && Language::for_path("a.unknown").is_none());
        assert_eq!(display_text("\tx"), "    x");
    }

    #[test]
    fn highlights_large_file_quickly() {
        let text: Vec<String> = (0..150_000).map(|i| format!("    let value_{i} = compute({i}, \"x\"); // note")).collect();
        let lines = std::iter::once((LineKind::Hunk, "@@ -0,0 +1,150000 @@")).chain(text.iter().map(|l| (LineKind::Added, l.as_str())));
        let t = std::time::Instant::now();
        let spans = highlight(lines, &RUST);
        let elapsed = t.elapsed();
        println!("highlighted {} lines in {elapsed:?}", spans.len());
        assert_eq!(spans.len(), 150_001);
        assert!(elapsed.as_secs_f64() < 3.0);
    }
}
