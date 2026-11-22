//! A Postgres-flavoured DDL reader: enough of CREATE / ALTER / DROP to replay a migrations directory into a schema,
//! so two states of a repository can be compared as schemas rather than as text. Anything it doesn't understand is
//! reported, never guessed. ponytail: hand-written subset parser, not a full SQL grammar; add statements as real
//! migrations need them.

use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

// MARK: Tokens

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Word,
    Quoted,
    String,
    Number,
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// words lowercased; quoted identifiers as written
    pub text: String,
    /// 1-based
    pub line: usize,
}

impl Token {
    pub fn is_word(&self) -> bool {
        self.kind == TokenKind::Word
    }
    pub fn is(&self, w: &str) -> bool {
        self.kind == TokenKind::Word && self.text == w
    }
    /// An identifier: a bare or quoted word.
    pub fn ident(&self) -> Option<&str> {
        matches!(self.kind, TokenKind::Word | TokenKind::Quoted).then_some(self.text.as_str())
    }
    fn sym(&self, s: &str) -> bool {
        self.kind == TokenKind::Symbol && self.text == s
    }
}

#[derive(Clone, Debug)]
pub struct Statement {
    pub tokens: Vec<Token>,
    pub line: usize,
    pub end_line: usize,
    /// whitespace-normalised source, for data statement comparison
    pub text: String,
}

/// A text's lines as `split(separator: "\n", omittingEmptySubsequences: false)` gave them.
pub fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

impl Statement {
    /// The statement as written, comments and layout kept, from the file's lines.
    pub fn source(&self, lines: &[&str]) -> String {
        if self.line < 1 || self.line > lines.len() {
            return self.text.clone();
        }
        lines[self.line - 1..self.end_line.min(lines.len())].join("\n")
    }
}

/// Splits SQL into statements on top-level semicolons. Comments are dropped; strings, quoted identifiers and
/// dollar-quoted bodies stay whole.
pub fn statements(src: &str) -> Vec<Statement> {
    let u: Vec<char> = src.chars().collect();
    let n = u.len();
    let (mut i, mut line) = (0, 1);
    let mut toks: Vec<Token> = Vec::new();
    let mut out = Vec::new();
    let mut raw = String::new(); // the current statement's text
    let flush = |toks: &mut Vec<Token>, raw: &mut String, out: &mut Vec<Statement>| {
        if let Some(last) = toks.last() {
            let end_line = last.line + last.text.matches('\n').count();
            out.push(Statement { line: toks[0].line, end_line, text: raw.split_whitespace().collect::<Vec<_>>().join(" "), tokens: std::mem::take(toks) });
        }
        raw.clear();
    };
    let ident_start = |c: char| c.is_alphabetic() || c == '_';
    let ident = |c: char| ident_start(c) || c.is_ascii_digit() || c == '$';
    let text = |from: usize, to: usize| u[from..to].iter().collect::<String>();
    while i < n {
        let c = u[i];
        if c == '\n' {
            line += 1;
            raw.push(' ');
            i += 1;
            continue;
        }
        if c == ' ' || c == '\t' || (c.is_whitespace() && c != '\n' && c != '\r' && c != '\u{0B}' && c != '\u{0C}') {
            raw.push(' ');
            i += 1;
            continue;
        }
        if c == '-' && i + 1 < n && u[i + 1] == '-' {
            // -- comment
            while i < n && u[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && u[i + 1] == '*' {
            // /* comment */, nesting as Postgres does
            let mut depth = 0;
            loop {
                if u[i] == '/' && i + 1 < n && u[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if u[i] == '*' && i + 1 < n && u[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    if u[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                if !(i < n && depth > 0) {
                    break;
                }
            }
            continue;
        }
        let (start, start_line) = (i, line);
        if c == ';' {
            i += 1;
            flush(&mut toks, &mut raw, &mut out);
            continue;
        }
        if c == '\'' || ((c == 'e' || c == 'E') && i + 1 < n && u[i + 1] == '\'') {
            // 'string', E'string'
            i += if c == '\'' { 1 } else { 2 };
            while i < n {
                if u[i] == '\\' && c != '\'' {
                    i += 2;
                    continue;
                }
                if u[i] == '\'' {
                    if i + 1 < n && u[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if u[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            let t = text(start, i.min(n));
            raw += &t;
            toks.push(Token { kind: TokenKind::String, text: t, line: start_line });
            continue;
        }
        if c == '"' {
            i += 1;
            let mut name = String::new();
            while i < n {
                if u[i] == '"' {
                    if i + 1 < n && u[i + 1] == '"' {
                        name.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                name.push(u[i]);
                i += 1;
            }
            raw += &text(start, i.min(n));
            toks.push(Token { kind: TokenKind::Quoted, text: name, line: start_line });
            continue;
        }
        if c == '$' {
            // $tag$ … $tag$ (or $1 parameters)
            let mut j = i + 1;
            while j < n && ident(u[j]) && u[j] != '$' {
                j += 1;
            }
            if j < n && u[j] == '$' {
                let tag = &u[i..=j];
                let mut k = j + 1;
                while k < n {
                    if u[k] == '$' && k + tag.len() <= n && &u[k..k + tag.len()] == tag {
                        k += tag.len();
                        break;
                    }
                    if u[k] == '\n' {
                        line += 1;
                    }
                    k += 1;
                }
                i = k.min(n);
                let t = text(start, i);
                raw += &t;
                toks.push(Token { kind: TokenKind::String, text: t, line: start_line });
                continue;
            }
        }
        if ident_start(c) {
            while i < n && ident(u[i]) {
                i += 1;
            }
            let t = text(start, i);
            raw += &t;
            toks.push(Token { kind: TokenKind::Word, text: t.to_lowercase(), line: start_line });
            continue;
        }
        if c.is_ascii_digit() {
            while i < n && (u[i].is_ascii_digit() || u[i] == '.') {
                i += 1;
            }
            let t = text(start, i);
            raw += &t;
            toks.push(Token { kind: TokenKind::Number, text: t, line: start_line });
            continue;
        }
        if c == ':' && i + 1 < n && u[i + 1] == ':' {
            i += 2;
            raw += "::";
            toks.push(Token { kind: TokenKind::Symbol, text: "::".into(), line: start_line });
            continue;
        }
        i += 1;
        raw.push(c);
        toks.push(Token { kind: TokenKind::Symbol, text: c.to_string(), line: start_line });
    }
    flush(&mut toks, &mut raw, &mut out);
    out
}

// MARK: Schema

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Location {
    pub file: String,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub references: Option<String>,
    pub at: Option<Location>,
}

/// Where a column is written doesn't make it another column.
impl PartialEq for Column {
    fn eq(&self, b: &Self) -> bool {
        self.name == b.name && self.ty == b.ty && self.not_null == b.not_null && self.default_value == b.default_value && self.references == b.references
    }
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub constraints: BTreeMap<String, Constraint>,
    pub primary_key: Vec<String>,
    pub at: Option<Location>,
}

impl Table {
    pub fn column(&self, n: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == n)
    }
}

/// A named table constraint: its definition (normalised, to tell a changed one from a re-created one) and the lines
/// it's written on.
#[derive(Clone, Debug)]
pub struct Constraint {
    pub definition: String,
    pub at: Location,
    pub end_line: usize,
}

/// The constraint a table-element or ALTER TABLE ADD's tokens define, named as Postgres names it when unnamed.
pub fn constraint(part: &[Token], table: &str, file: &str) -> Option<(String, Constraint)> {
    let head = part.first()?;
    let mut body = part;
    let mut name = String::new();
    if head.is("constraint") && part.len() > 2 {
        name = part[1].ident().unwrap_or("").to_string();
        body = &part[2..];
    }
    let kind = body.first()?.text.as_str();
    if name.is_empty() {
        let mut j = body.iter().position(|t| t.text == "(").unwrap_or(body.len());
        let cols: Vec<_> = paren_list(body, &mut j).iter().filter_map(|p| p.first().and_then(|t| t.ident()).map(str::to_string)).collect();
        name = match kind {
            "primary" => format!("{table}_pkey"),
            "unique" => format!("{table}_{}_key", cols.join("_")),
            "foreign" => format!("{table}_{}_fkey", cols.join("_")),
            "check" => format!("{table}_check"),
            "exclude" => format!("{table}_excl"),
            _ => String::new(),
        };
    }
    if name.is_empty() {
        return None;
    }
    let end_line = part.last().map_or(head.line, |t| t.line);
    Some((name, Constraint { definition: render(body), at: Location { file: file.into(), line: head.line }, end_line }))
}

#[derive(Clone, Debug)]
pub struct Index {
    pub name: String,
    pub table: String,
    pub definition: String,
    pub unique: bool,
    pub at: Option<Location>,
}

impl PartialEq for Index {
    fn eq(&self, b: &Self) -> bool {
        self.table == b.table && self.definition == b.definition && self.unique == b.unique
    }
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub table: String,
    pub at: Location,
    /// when it fires and what it runs: "BEFORE UPDATE OF cidr_block · each row → f()"
    pub detail: String,
    pub function: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub body: String,
    pub at: Location,
}

#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub tables: BTreeMap<String, Table>,
    pub indexes: BTreeMap<String, Index>,
    pub enums: BTreeMap<String, Vec<String>>,
    pub enum_at: BTreeMap<String, Location>,
    pub views: BTreeMap<String, Body>,
    pub functions: BTreeMap<String, Body>,
    pub triggers: BTreeMap<String, Trigger>,
    /// Statements the reader skipped, by kind ("DO block", "CREATE POLICY"…), with where they are.
    pub skipped: Vec<(String, Location)>,
}

// MARK: Replay

/// Applies `files` (path, contents) in order.
pub fn replay<P: AsRef<str>, T: AsRef<str>>(files: &[(P, T)]) -> Schema {
    let mut s = Schema::default();
    for (path, text) in files {
        for st in statements(text.as_ref()) {
            apply(&st, path.as_ref(), &mut s);
        }
    }
    s
}

/// Strips the default `public.` schema; other schemas stay part of the name.
fn name(t: &[Token], i: &mut usize) -> Option<String> {
    let mut n = t.get(*i)?.ident()?.to_string();
    *i += 1;
    while *i + 1 < t.len() && t[*i].text == "." {
        let Some(part) = t[*i + 1].ident() else { break };
        n += ".";
        n += part;
        *i += 2;
    }
    Some(n.strip_prefix("public.").map(str::to_string).unwrap_or(n))
}

fn skip(t: &[Token], i: &mut usize, words: &[&str]) {
    while *i < t.len() && words.iter().any(|w| t[*i].is(w)) {
        *i += 1;
    }
}

/// `IF EXISTS` / `IF NOT EXISTS`, if present.
fn if_clause(t: &[Token], i: &mut usize) -> bool {
    if !t.get(*i).is_some_and(|x| x.is("if")) {
        return false;
    }
    *i += 1;
    if t.get(*i).is_some_and(|x| x.is("not")) {
        *i += 1;
    }
    if t.get(*i).is_some_and(|x| x.is("exists")) {
        *i += 1;
    }
    true
}

/// Splits tokens between parentheses (from an opening "(") at top-level commas; returns the parts and moves past ")".
fn paren_list(t: &[Token], i: &mut usize) -> Vec<Vec<Token>> {
    if !t.get(*i).is_some_and(|x| x.text == "(") {
        return Vec::new();
    }
    *i += 1;
    let mut parts: Vec<Vec<Token>> = vec![Vec::new()];
    let mut depth = 0;
    while *i < t.len() {
        let x = &t[*i];
        if x.sym("(") {
            depth += 1;
        }
        if x.sym(")") {
            if depth == 0 {
                *i += 1;
                break;
            }
            depth -= 1;
        }
        if x.sym(",") && depth == 0 {
            parts.push(Vec::new());
            *i += 1;
            continue;
        }
        parts.last_mut().unwrap().push(x.clone());
        *i += 1;
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// Splits a token run at top-level commas.
fn comma_split(t: &[Token]) -> Vec<Vec<Token>> {
    let mut parts: Vec<Vec<Token>> = vec![Vec::new()];
    let mut depth = 0;
    for x in t {
        if x.sym("(") {
            depth += 1;
        }
        if x.sym(")") {
            depth -= 1;
        }
        if x.sym(",") && depth == 0 {
            parts.push(Vec::new());
            continue;
        }
        parts.last_mut().unwrap().push(x.clone());
    }
    parts.retain(|p| !p.is_empty());
    parts
}

pub fn render<'a>(t: impl IntoIterator<Item = &'a Token>) -> String {
    let mut s = String::new();
    let mut prev: Option<&Token> = None;
    for x in t {
        let glue = prev.is_none_or(|p| {
            (p.kind == TokenKind::Symbol && ["(", ".", "::"].contains(&p.text.as_str()))
                || (x.kind == TokenKind::Symbol && [")", ",", ".", "(", "::", "[", "]"].contains(&x.text.as_str()))
                // <>, <=, !=
                || (p.kind == TokenKind::Symbol && x.kind == TokenKind::Symbol && "<>!=".contains(p.text.as_str()) && "<>=".contains(x.text.as_str()))
        });
        if !glue {
            s.push(' ');
        }
        if x.kind == TokenKind::Quoted {
            s.push('"');
            s += &x.text;
            s.push('"');
        } else {
            s += &x.text;
        }
        prev = Some(x);
    }
    s
}

const COLUMN_CONSTRAINT_WORDS: &[&str] = &["not", "null", "default", "primary", "unique", "references", "check", "constraint", "generated", "collate", "identity"];

fn constraint_word(x: &Token) -> bool {
    x.is_word() && COLUMN_CONSTRAINT_WORDS.contains(&x.text.as_str())
}

/// "name type [constraints]" → Column.
fn column(t: &[Token], at: Location) -> Option<Column> {
    let n = t.first()?.ident()?.to_string();
    let mut type_end = 1;
    let mut depth = 0;
    while type_end < t.len() {
        let x = &t[type_end];
        if x.sym("(") {
            depth += 1;
        } else if x.sym(")") {
            depth -= 1;
        }
        if depth == 0 && constraint_word(x) {
            break;
        }
        type_end += 1;
    }
    let mut c = Column { name: n, ty: render(&t[1..type_end]), not_null: false, default_value: None, references: None, at: Some(at) };
    let mut i = type_end;
    while i < t.len() {
        let x = &t[i];
        if x.is("not") && i + 1 < t.len() && t[i + 1].is("null") {
            c.not_null = true;
            i += 2;
            continue;
        }
        if x.is("primary") {
            c.not_null = true;
            i += 2;
            continue;
        }
        if x.is("default") {
            let (mut j, mut d) = (i + 1, 0);
            while j < t.len() {
                let y = &t[j];
                if y.sym("(") {
                    d += 1;
                } else if y.sym(")") {
                    d -= 1;
                }
                if d == 0 && constraint_word(y) && !(y.is("null") && j == i + 1) {
                    break;
                }
                j += 1;
            }
            c.default_value = Some(render(&t[i + 1..j]));
            i = j;
            continue;
        }
        if x.is("references") {
            let mut j = i + 1;
            if let Some(target) = name(t, &mut j) {
                let mut cols = String::new();
                if t.get(j).is_some_and(|x| x.text == "(") {
                    let p = paren_list(t, &mut j);
                    cols = format!("({})", p.iter().map(|x| render(x)).collect::<Vec<_>>().join(", "));
                }
                c.references = Some(target + &cols);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    Some(c)
}

fn is_table_constraint(t: &Token) -> bool {
    ["constraint", "primary", "unique", "foreign", "check", "exclude"].iter().any(|w| t.is(w))
}

fn trim_quotes(s: &str) -> String {
    s.trim_matches('\'').to_string()
}

fn apply(st: &Statement, file: &str, s: &mut Schema) {
    let t = &st.tokens;
    let at = Location { file: file.into(), line: st.line };
    let Some(first) = t.first().filter(|f| f.is_word()) else { return };
    let mut i = 1;
    match first.text.as_str() {
        "create" => {
            skip(t, &mut i, &["or", "replace", "unlogged", "temporary", "temp", "global", "local"]);
            if i >= t.len() {
                return;
            }
            if t[i].is("table") {
                i += 1;
                let exists = if_clause(t, &mut i);
                let Some(n) = name(t, &mut i) else { return };
                if exists && s.tables.contains_key(&n) {
                    return;
                }
                let mut table = Table { name: n.clone(), at: Some(at), ..Default::default() };
                for part in paren_list(t, &mut i) {
                    let head = &part[0];
                    if is_table_constraint(head) || head.is("like") {
                        let mut cname = if head.is("constraint") && part.len() > 1 { part[1].ident().unwrap_or("").to_string() } else { String::new() };
                        if let Some(pk) = part.iter().position(|x| x.is("primary")) {
                            let mut j = pk + 2;
                            table.primary_key = paren_list(&part, &mut j).iter().filter_map(|p| p.first().and_then(|x| x.ident()).map(str::to_string)).collect();
                            for k in table.primary_key.clone() {
                                if let Some(ci) = table.column(&k) {
                                    table.columns[ci].not_null = true;
                                }
                            }
                            if cname.is_empty() {
                                cname = format!("{n}_pkey");
                            }
                        }
                        if let Some((name, c)) = constraint(&part, &n, file) {
                            table.constraints.insert(if cname.is_empty() { name } else { cname }, c);
                        }
                        continue;
                    }
                    if let Some(c) = column(&part, Location { file: file.into(), line: head.line }) {
                        // A column's own CHECK is a table constraint too: <table>_<column>_check unless named.
                        if let Some(k) = part.iter().position(|x| x.is("check")) {
                            let named = if k >= 2 && part[k - 2].is("constraint") { part[k - 1].ident().map(str::to_string) } else { None };
                            if let Some((_, con)) = constraint(&part[k..], &n, file) {
                                table.constraints.insert(named.unwrap_or_else(|| format!("{n}_{}_check", c.name)), con);
                            }
                        }
                        if part.iter().any(|x| x.is("primary")) {
                            table.primary_key = vec![c.name.clone()];
                        }
                        table.columns.push(c);
                    }
                }
                s.tables.insert(n, table);
            } else if t[i].is("index") || (t[i].is("unique") && i + 1 < t.len() && t[i + 1].is("index")) {
                let unique = t[i].is("unique");
                i += if unique { 2 } else { 1 };
                skip(t, &mut i, &["concurrently"]);
                let exists = if_clause(t, &mut i);
                let mut n = None;
                if i < t.len() && !t[i].is("on") {
                    n = name(t, &mut i);
                }
                if !t.get(i).is_some_and(|x| x.is("on")) {
                    return;
                }
                i += 1;
                skip(t, &mut i, &["only"]);
                let Some(table) = name(t, &mut i) else { return };
                let def = render(&t[i..]);
                let idx = n.unwrap_or_else(|| {
                    let slug: String = def.chars().filter(|c| c.is_alphanumeric() || *c == '_').take(30).collect();
                    format!("{table}_{slug}_idx")
                });
                if exists && s.indexes.contains_key(&idx) {
                    return;
                }
                s.indexes.insert(idx.clone(), Index { name: idx, table, definition: def, unique, at: Some(at) });
            } else if t[i].is("type") {
                i += 1;
                let Some(n) = name(t, &mut i) else { return };
                if i + 1 < t.len() && t[i].is("as") && t[i + 1].is("enum") {
                    i += 2;
                    let vals = paren_list(t, &mut i).iter().filter_map(|p| p.first().map(|x| trim_quotes(&x.text))).collect();
                    s.enums.insert(n.clone(), vals);
                } else {
                    s.enums.entry(n.clone()).or_default();
                }
                s.enum_at.insert(n, at);
            } else if t[i].is("view") || t[i].is("materialized") {
                i += if t[i].is("materialized") { 2 } else { 1 };
                if_clause(t, &mut i);
                let Some(n) = name(t, &mut i) else { return };
                s.views.insert(n, Body { body: st.text.clone(), at });
            } else if t[i].is("function") || t[i].is("procedure") {
                i += 1;
                let Some(n) = name(t, &mut i) else { return };
                s.functions.insert(n, Body { body: st.text.clone(), at });
            } else if t[i].is("trigger") || (t[i].is("constraint") && i + 1 < t.len() && t[i + 1].is("trigger")) {
                i += if t[i].is("constraint") { 2 } else { 1 };
                let Some(n) = name(t, &mut i) else { return };
                let Some(on) = t.iter().position(|x| x.is("on")).filter(|&on| on >= i) else { return };
                let mut j = on + 1;
                let table = name(t, &mut j).unwrap_or_default();
                // The timing and events as Postgres prints them; the rest in words.
                const KEYWORDS: &[&str] = &["before", "after", "instead", "of", "insert", "update", "delete", "truncate", "or"];
                let timing: Vec<Token> = t[i..on]
                    .iter()
                    .map(|k| {
                        let mut k = k.clone();
                        if k.is_word() && KEYWORDS.contains(&k.text.as_str()) {
                            k.text = k.text.to_uppercase();
                        }
                        k
                    })
                    .collect();
                let mut parts = vec![render(&timing)];
                if let Some(e) = t.iter().position(|x| x.is("each")).filter(|e| e + 1 < t.len()) {
                    parts.push(format!("each {}", t[e + 1].text));
                }
                // EXECUTE FUNCTION|PROCEDURE f(…)
                let function = t.iter().position(|x| x.is("execute")).and_then(|e| {
                    let mut k = e + 2;
                    name(t, &mut k)
                });
                let detail = parts.join(" · ") + &function.as_ref().map(|f| format!(" → {f}()")).unwrap_or_default();
                s.triggers.insert(n, Trigger { table, at, detail, function });
            } else if !t[i].is("extension") && !t[i].is("schema") && !t[i].is("sequence") {
                s.skipped.push((format!("CREATE {}", t[i].text.to_uppercase()), at));
            }
        }
        "alter" => {
            if i >= t.len() {
                return;
            }
            if t[i].is("table") {
                i += 1;
                if_clause(t, &mut i);
                skip(t, &mut i, &["only"]);
                let Some(mut n) = name(t, &mut i).filter(|n| s.tables.contains_key(n)) else { return };
                for action in comma_split(&t[i..]) {
                    alter_table(&mut n, &action, &at, s);
                }
            } else if t[i].is("type") {
                i += 1;
                let Some(n) = name(t, &mut i) else { return };
                if i >= t.len() {
                    return;
                }
                if t[i].is("add") && i + 1 < t.len() && t[i + 1].is("value") {
                    i += 2;
                    if_clause(t, &mut i);
                    if i < t.len() {
                        let v = trim_quotes(&t[i].text);
                        let vals = s.enums.entry(n).or_default();
                        if !vals.contains(&v) {
                            vals.push(v);
                        }
                    }
                } else if t[i].is("rename") && i + 1 < t.len() {
                    if t[i + 1].is("value") && i + 4 < t.len() {
                        let (from, to) = (trim_quotes(&t[i + 2].text), trim_quotes(&t[i + 4].text));
                        if let Some(vals) = s.enums.get_mut(&n) {
                            for v in vals.iter_mut().filter(|v| **v == from) {
                                *v = to.clone();
                            }
                        }
                    } else if t[i + 1].is("to") {
                        let mut j = i + 2;
                        if let Some(to) = name(t, &mut j) {
                            // Swift assigned the removed values, so a rename of an unknown type removes the target.
                            match s.enums.remove(&n) {
                                Some(v) => s.enums.insert(to.clone(), v),
                                None => s.enums.remove(&to),
                            };
                            match s.enum_at.remove(&n) {
                                Some(v) => s.enum_at.insert(to, v),
                                None => s.enum_at.remove(&to),
                            };
                        }
                    }
                }
            } else if t[i].is("index") {
                i += 1;
                if_clause(t, &mut i);
                let Some(n) = name(t, &mut i) else { return };
                if !(i + 1 < t.len() && t[i].is("rename") && t[i + 1].is("to")) {
                    return;
                }
                let mut j = i + 2;
                if let Some(to) = name(t, &mut j) {
                    if let Some(mut idx) = s.indexes.remove(&n) {
                        idx.name = to.clone();
                        s.indexes.insert(to, idx);
                    }
                }
            }
        }
        "drop" => {
            let Some(what) = t.get(i).map(|x| x.text.clone()) else { return };
            i += 1;
            if what == "materialized" {
                i += 1;
            }
            if_clause(t, &mut i);
            if what == "index" {
                skip(t, &mut i, &["concurrently"]);
                if_clause(t, &mut i);
            }
            let names: Vec<String> = comma_split(t.get(i..).unwrap_or(&[])).iter().filter_map(|p| name(p, &mut 0)).collect();
            for n in names {
                match what.as_str() {
                    "table" => {
                        s.tables.remove(&n);
                        s.indexes.retain(|_, v| v.table != n);
                        s.triggers.retain(|_, v| v.table != n);
                    }
                    "index" => {
                        s.indexes.remove(&n);
                    }
                    "type" => {
                        s.enums.remove(&n);
                        s.enum_at.remove(&n);
                    }
                    "view" | "materialized" => {
                        s.views.remove(&n);
                    }
                    "function" | "procedure" => {
                        s.functions.remove(&n);
                    }
                    "trigger" => {
                        s.triggers.remove(&n);
                    }
                    _ => {}
                }
            }
        }
        "do" => s.skipped.push(("DO block".into(), at)),
        // data and housekeeping: compared per file, not replayed
        "insert" | "update" | "delete" | "select" | "comment" | "grant" | "revoke" | "set" | "begin" | "commit" | "with" | "truncate" | "copy" => {}
        other => s.skipped.push((other.to_uppercase(), at)),
    }
}

fn ident_list(t: &[Token], j: &mut usize) -> Vec<String> {
    paren_list(t, j).iter().filter_map(|p| p.first().and_then(|x| x.ident()).map(str::to_string)).collect()
}

fn alter_table(n: &mut String, a: &[Token], at: &Location, s: &mut Schema) {
    let (Some(mut table), Some(verb)) = (s.tables.get(n.as_str()).cloned(), a.first()) else { return };
    let mut i = 1;
    let col_at = Location { file: at.file.clone(), line: verb.line };
    match verb.text.as_str() {
        "add" => {
            if a.get(i).is_some_and(is_table_constraint) {
                let mut cname = if a[i].is("constraint") && i + 1 < a.len() { a[i + 1].ident().unwrap_or("").to_string() } else { String::new() };
                if let Some(pk) = a.iter().position(|x| x.is("primary")) {
                    let mut j = pk + 2;
                    table.primary_key = ident_list(a, &mut j);
                    for k in table.primary_key.clone() {
                        if let Some(ci) = table.column(&k) {
                            table.columns[ci].not_null = true;
                        }
                    }
                    if cname.is_empty() {
                        cname = format!("{n}_pkey");
                    }
                }
                if let Some((name, c)) = constraint(&a[i..], n, &at.file) {
                    table.constraints.insert(if cname.is_empty() { name } else { cname }, c);
                }
            } else {
                skip(a, &mut i, &["column"]);
                let exists = if_clause(a, &mut i);
                if let Some(c) = column(a.get(i..).unwrap_or(&[]), col_at) {
                    match table.column(&c.name) {
                        Some(ci) => {
                            if !exists {
                                table.columns[ci] = c;
                            }
                        }
                        None => table.columns.push(c),
                    }
                }
            }
        }
        "drop" => {
            if a.get(i).is_some_and(|x| x.is("constraint")) {
                i += 1;
                if_clause(a, &mut i);
                if let Some(c) = a.get(i).and_then(|x| x.ident()) {
                    table.constraints.remove(c);
                    if c == format!("{n}_pkey") {
                        table.primary_key.clear();
                    }
                }
            } else {
                skip(a, &mut i, &["column"]);
                if_clause(a, &mut i);
                if let Some(c) = a.get(i).and_then(|x| x.ident()) {
                    table.columns.retain(|x| x.name != c);
                }
            }
        }
        "alter" => {
            skip(a, &mut i, &["column"]);
            if let Some(ci) = a.get(i).and_then(|x| x.ident()).and_then(|c| table.column(c)) {
                i += 1;
                if i < a.len() {
                    let col = &mut table.columns[ci];
                    if a[i].is("type") || (a[i].is("set") && i + 2 < a.len() && a[i + 1].is("data")) {
                        i += if a[i].is("type") { 1 } else { 3 };
                        let end = (i..a.len()).find(|&k| a[k].is("using") || a[k].is("collate")).unwrap_or(a.len());
                        col.ty = render(a.get(i..end).unwrap_or(&[]));
                    } else if a[i].is("set") && i + 2 < a.len() && a[i + 1].is("not") && a[i + 2].is("null") {
                        col.not_null = true;
                    } else if a[i].is("drop") && i + 2 < a.len() && a[i + 1].is("not") && a[i + 2].is("null") {
                        col.not_null = false;
                    } else if a[i].is("set") && i + 1 < a.len() && a[i + 1].is("default") {
                        col.default_value = Some(render(a.get(i + 2..).unwrap_or(&[])));
                    } else if a[i].is("drop") && i + 1 < a.len() && a[i + 1].is("default") {
                        col.default_value = None;
                    }
                    col.at = Some(col_at);
                }
            }
        }
        "rename" => {
            if a.get(i).is_some_and(|x| x.is("to")) {
                let mut j = i + 1;
                if let Some(to) = name(a, &mut j) {
                    s.tables.remove(n.as_str());
                    table.name = to.clone();
                    for v in s.indexes.values_mut().filter(|v| v.table == *n) {
                        v.table = to.clone();
                    }
                    for v in s.triggers.values_mut().filter(|v| v.table == *n) {
                        v.table = to.clone();
                    }
                    *n = to;
                }
            } else if a.get(i).is_some_and(|x| x.is("constraint")) {
                if i + 3 < a.len() {
                    if let (Some(from), Some(to)) = (a[i + 1].ident(), a[i + 3].ident()) {
                        if let Some(c) = table.constraints.remove(from) {
                            table.constraints.insert(to.to_string(), c);
                        }
                    }
                }
            } else {
                skip(a, &mut i, &["column"]);
                if i + 2 < a.len() && a[i + 1].is("to") {
                    if let (Some(from), Some(to)) = (a[i].ident(), a[i + 2].ident()) {
                        if let Some(ci) = table.column(from) {
                            table.columns[ci].name = to.to_string();
                            table.columns[ci].at = Some(col_at);
                        }
                    }
                }
            }
        }
        _ => {} // OWNER TO, ENABLE TRIGGER, SET (…): no shape change
    }
    s.tables.insert(n.clone(), table);
}

// MARK: Reading what a statement does

static RAISE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\braise\s+(?:exception\s+)?'((?:[^']|'')*)'([^;]*)").unwrap());
static ERRCODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)errcode\s*=\s*'([^']+)'").unwrap());

/// The errors a function body raises — RAISE [EXCEPTION] 'message' [USING ERRCODE = 'code'] — as "“message” · code".
pub fn raises(body: &str) -> Vec<String> {
    RAISE
        .captures_iter(body)
        .map(|m| {
            let code = ERRCODE.captures(&m[2]).map(|c| format!(" · {}", &c[1])).unwrap_or_default();
            format!("“{}”{code}", m[1].replace("''", "'"))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Risk {
    pub title: String,
    pub detail: String,
}

fn risk(title: String, detail: &str) -> Risk {
    Risk { title, detail: detail.into() }
}

/// What running a statement on a live database costs beyond its effect: data lost, rows rewritten, writes blocked.
/// `existing`: the tables there before the change (an index on a new, empty table blocks nobody). `migration`: data
/// statements run on every database, not just a seeded one.
pub fn risks(st: &Statement, existing: &HashSet<String>, migration: bool) -> Vec<Risk> {
    let t = &st.tokens;
    let Some(first) = t.first().filter(|f| f.is_word()) else { return Vec::new() };
    let mut i = 1;
    let mut out = Vec::new();
    match first.text.as_str() {
        "drop" => {
            if !t.get(i).is_some_and(|x| x.is("table")) {
                return out;
            }
            i += 1;
            if_clause(t, &mut i);
            for part in comma_split(&t[i..]) {
                if let Some(n) = name(&part, &mut 0).filter(|n| existing.contains(n)) {
                    out.push(risk(format!("Drops table {n}"), "Its rows are gone once this runs."));
                }
            }
        }
        "alter" => {
            if !t.get(i).is_some_and(|x| x.is("table")) {
                return out;
            }
            i += 1;
            if_clause(t, &mut i);
            skip(t, &mut i, &["only"]);
            let Some(n) = name(t, &mut i).filter(|n| existing.contains(n)) else { return out };
            for a in comma_split(&t[i..]) {
                let Some(verb) = a.first() else { continue };
                let mut j = 1;
                match verb.text.as_str() {
                    "drop" => {
                        if j >= a.len() || a[j].is("constraint") {
                            continue;
                        }
                        skip(&a, &mut j, &["column"]);
                        if_clause(&a, &mut j);
                        if let Some(c) = a.get(j).and_then(|x| x.ident()) {
                            out.push(risk(format!("Drops column {n}.{c}"), "Its values are gone once this runs."));
                        }
                    }
                    "add" => {
                        if j >= a.len() || ["constraint", "primary", "unique", "foreign", "check", "exclude"].contains(&a[j].text.as_str()) {
                            continue;
                        }
                        skip(&a, &mut j, &["column"]);
                        if_clause(&a, &mut j);
                        let not_null = a.windows(2).any(|w| w[0].is("not") && w[1].is("null"));
                        if let Some(c) = a.get(j).and_then(|x| x.ident()) {
                            if not_null && !a.iter().any(|x| x.is("default") || x.is("generated")) {
                                out.push(risk(format!("NOT NULL column without a default: {n}.{c}"), "Fails on a table that has rows."));
                            }
                        }
                    }
                    "alter" => {
                        skip(&a, &mut j, &["column"]);
                        if j + 1 >= a.len() {
                            continue;
                        }
                        let Some(c) = a[j].ident() else { continue };
                        j += 1;
                        if a[j].is("type") || (a[j].is("set") && j + 1 < a.len() && a[j + 1].is("data")) {
                            out.push(risk(format!("Changes the type of {n}.{c}"), "Usually rewrites the table under an exclusive lock."));
                        } else if a[j].is("set") && j + 2 < a.len() && a[j + 1].is("not") && a[j + 2].is("null") {
                            out.push(risk(format!("Makes {n}.{c} NOT NULL"), "Scans the table under an exclusive lock, and fails on any null."));
                        }
                    }
                    _ => {}
                }
            }
        }
        "create" => {
            skip(t, &mut i, &["unique"]);
            if !t.get(i).is_some_and(|x| x.is("index")) || t.get(i + 1).is_some_and(|x| x.is("concurrently")) {
                return out;
            }
            let Some(on) = t.iter().position(|x| x.is("on")) else { return out };
            let mut j = on + 1;
            skip(t, &mut j, &["only"]);
            if let Some(n) = name(t, &mut j).filter(|n| existing.contains(n)) {
                out.push(Risk { title: format!("Index on {n} built without CONCURRENTLY"), detail: format!("Blocks writes to {n} until it's built.") });
            }
        }
        "update" | "delete" => {
            if !migration {
                return out;
            }
            skip(t, &mut i, &["from", "only"]);
            let Some(n) = name(t, &mut i) else { return out };
            let every = !t.iter().any(|x| x.is("where"));
            let title = if first.is("update") { format!("Updates rows in {n}") } else { format!("Deletes rows from {n}") };
            out.push(risk(title, if every { "Every row: there's no WHERE clause." } else { "Runs with the migration, on every database." }));
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraint_recreated_with_another_definition() {
        let create = "CREATE TABLE t (id int, role text CHECK (role IN ('a')), CONSTRAINT t_x_check CHECK (id > 0), UNIQUE (role));";
        let a = replay(&[("1.sql", create)]);
        let b = replay(&[("1.sql", create), ("2.sql", "ALTER TABLE t DROP CONSTRAINT t_role_check; ALTER TABLE t ADD CONSTRAINT t_role_check CHECK (role IN ('a', 'b'));")]);
        let keys: Vec<_> = a.tables["t"].constraints.keys().map(String::as_str).collect();
        assert_eq!(keys, ["t_role_check", "t_role_key", "t_x_check"]);
        assert_ne!(a.tables["t"].constraints["t_role_check"].definition, b.tables["t"].constraints["t_role_check"].definition);
        assert_eq!(a.tables["t"].constraints["t_x_check"].definition, b.tables["t"].constraints["t_x_check"].definition);
    }

    #[test]
    fn flags_risky_migration_statements() {
        let sql = "ALTER TABLE users ADD COLUMN plan text NOT NULL;
ALTER TABLE users ADD COLUMN note text NOT NULL DEFAULT '';
ALTER TABLE users ALTER COLUMN age TYPE bigint, DROP COLUMN email;
CREATE INDEX users_plan ON users (plan);
CREATE INDEX CONCURRENTLY users_note ON users (note);
CREATE INDEX fresh_x ON fresh (x);
UPDATE users SET plan = 'free';
DROP TABLE legacy;";
        let existing: HashSet<String> = ["users".into(), "legacy".into()].into();
        let risks: Vec<String> = statements(sql).iter().flat_map(|s| risks(s, &existing, true)).map(|r| r.title).collect();
        assert_eq!(
            risks,
            [
                "NOT NULL column without a default: users.plan",
                "Changes the type of users.age",
                "Drops column users.email",
                "Index on users built without CONCURRENTLY",
                "Updates rows in users",
                "Drops table legacy"
            ]
        );
    }

    #[test]
    fn describes_triggers_and_what_they_raise() {
        let s = replay(&[(
            "1.sql",
            "CREATE TABLE networks (id int, cidr_block text);
CREATE FUNCTION lock_cidr() RETURNS TRIGGER AS $$ BEGIN
    RAISE NOTICE 'hi';
    RAISE EXCEPTION 'the CIDR can''t change' USING ERRCODE = 'check_violation';
    RETURN NEW;
END; $$ LANGUAGE plpgsql;
CREATE TRIGGER lock_cidr_trg BEFORE UPDATE OF cidr_block ON networks FOR EACH ROW EXECUTE FUNCTION lock_cidr();",
        )]);
        assert_eq!(s.triggers["lock_cidr_trg"].detail, "BEFORE UPDATE OF cidr_block · each row → lock_cidr()");
        assert_eq!(raises(&s.functions["lock_cidr"].body), ["“the CIDR can't change” · check_violation"]);
    }

    #[test]
    fn statements_keep_lines_and_source() {
        let src = "-- head\nCREATE TABLE a (\n  id int\n);\n/* x /* nested */ y */ SELECT 1;";
        let st = statements(src);
        assert_eq!((st[0].line, st[0].end_line, st[1].line), (2, 4, 5));
        assert_eq!(st[0].source(&lines(src)), "CREATE TABLE a (\n  id int\n);");
        assert_eq!(st[0].text, "CREATE TABLE a ( id int )");
    }
}
