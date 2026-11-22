//! Block-style YAML as JSON-like values (objects, arrays, strings), one per document.
//! ponytail: no anchors, aliases, tags or multi-line flow collections; enough for config, CI, Kubernetes and Helm
//! files (Helm's `{{ … }}` lines are skipped like comments).

use serde_json::{Map, Value};

pub fn documents(text: &str) -> Vec<Value> {
    let mut docs: Vec<Vec<String>> = vec![Vec::new()];
    for l in text.split('\n') {
        if l == "---" || l.starts_with("--- ") || l == "..." {
            docs.push(Vec::new());
            continue;
        }
        docs.last_mut().unwrap().push(l.to_string());
    }
    docs.into_iter().filter_map(|lines| Parser { lines, i: 0 }.document()).collect()
}

fn ws(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

fn indent(l: &str) -> usize {
    l.bytes().take_while(|&b| b == b' ').count()
}

fn skipped(l: &str) -> bool {
    let t = ws(l);
    t.is_empty() || t.starts_with('#') || t.starts_with("{{")
}

fn is_item(t: &str) -> bool {
    t == "-" || t.starts_with("- ")
}

struct Parser {
    lines: Vec<String>,
    i: usize,
}

impl Parser {
    /// The next line with content: its indent and its text after the indent.
    fn peek(&mut self) -> Option<(usize, String)> {
        while self.i < self.lines.len() && skipped(&self.lines[self.i]) {
            self.i += 1;
        }
        let l = self.lines.get(self.i)?;
        let n = indent(l);
        Some((n, l[n..].to_string()))
    }

    fn document(&mut self) -> Option<Value> {
        let (n, _) = self.peek()?;
        self.node(n)
    }

    fn node(&mut self, at: usize) -> Option<Value> {
        let (n, text) = self.peek().filter(|p| p.0 >= at)?;
        if is_item(&text) {
            return Some(self.sequence(n));
        }
        if entry(&text).is_some() {
            return Some(self.mapping(n));
        }
        self.i += 1;
        Some(scalar(&text))
    }

    fn sequence(&mut self, at: usize) -> Value {
        let mut out = Vec::new();
        while let Some((n, text)) = self.peek().filter(|(n, t)| *n == at && is_item(t)) {
            let rest = text[1..].trim_start_matches(' ');
            if rest.is_empty() || rest.starts_with('#') {
                self.i += 1;
                out.push(self.node(at + 1).unwrap_or_else(empty));
            } else {
                // "- key: v" opens a mapping (and "- - x" a sequence) as deep as the text after the dash: the item's
                // later keys line up with its first.
                let deeper = n + text.len() - rest.len();
                self.lines[self.i] = " ".repeat(deeper) + rest;
                out.push(self.node(deeper).unwrap_or_else(empty));
            }
        }
        Value::Array(out)
    }

    fn mapping(&mut self, at: usize) -> Value {
        let mut out = Map::new();
        while let Some((k, v)) = self.peek().filter(|(n, t)| *n == at && !is_item(t)).and_then(|(_, t)| entry(&t)) {
            self.i += 1;
            let value = if v.is_empty() || v.starts_with('#') {
                // The value is on the lines below: deeper, or a sequence at the key's own indent ("key:\n- a").
                match self.peek() {
                    Some((n, t)) if n > at || (n == at && is_item(&t)) => self.node(n).unwrap_or_else(empty),
                    _ => empty(),
                }
            } else if v.starts_with('|') || v.starts_with('>') {
                Value::String(self.block(at, v.starts_with('>')))
            } else {
                scalar(&v)
            };
            out.insert(k, value);
        }
        Value::Object(out)
    }

    /// A `|` or `>` scalar: the lines deeper than its key, dedented.
    fn block(&mut self, at: usize, folded: bool) -> String {
        let mut body: Vec<&str> = Vec::new();
        while let Some(l) = self.lines.get(self.i) {
            if !ws(l).is_empty() && indent(l) <= at {
                break;
            }
            body.push(l);
            self.i += 1;
        }
        while body.last().is_some_and(|l| ws(l).is_empty()) {
            body.pop();
        }
        let cut = body.iter().filter(|l| !ws(l).is_empty()).map(|l| indent(l)).min().unwrap_or(0);
        body.iter().map(|l| &l[cut.min(indent(l))..]).collect::<Vec<_>>().join(if folded { " " } else { "\n" })
    }
}

fn empty() -> Value {
    Value::String(String::new())
}

/// "key: value" → (key, value), the colon outside quotes and followed by a space or the end; None otherwise.
pub(crate) fn entry(t: &str) -> Option<(String, String)> {
    let f = t.chars().next()?;
    if f == '[' || f == '{' {
        return None;
    }
    let mut quote = (f == '"' || f == '\'').then_some(f);
    let mut chars = t.char_indices().skip(if quote.is_some() { 1 } else { 0 }).peekable();
    while let Some((j, c)) = chars.next() {
        let next = chars.peek().map(|&(_, n)| n);
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == ':' && (next.is_none() || next == Some(' ')) {
            return Some((unquote(ws(&t[..j])), ws(&t[j + 1..]).to_string()));
        } else if c == ' ' && next == Some('#') {
            return None;
        }
    }
    None
}

fn unquote(s: &str) -> String {
    let f = s.chars().next();
    if s.chars().count() >= 2 && matches!(f, Some('"' | '\'')) && s.ends_with(f.unwrap()) {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

fn scalar(raw: &str) -> Value {
    let mut v = ws(raw);
    if let Some(f) = v.chars().next().filter(|&f| f == '"' || f == '\'') {
        let body = &v[1..];
        return Value::String(body.find(f).map_or(v, |e| &body[..e]).to_string());
    }
    if let Some(r) = v.find(" #") {
        v = ws(&v[..r]);
    }
    if v.starts_with('[') && v.ends_with(']') && v.len() >= 2 {
        return Value::Array(items(&v[1..v.len() - 1]).iter().map(|s| scalar(s)).collect());
    }
    if v.starts_with('{') && v.ends_with('}') && v.len() >= 2 {
        let mut d = Map::new();
        for e in items(&v[1..v.len() - 1]) {
            if let Some((k, x)) = entry(&e) {
                d.insert(k, scalar(&x));
            }
        }
        return Value::Object(d);
    }
    Value::String(v.to_string())
}

/// A flow collection's entries, split on the commas outside quotes and brackets.
fn items(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0);
    let mut quote = None;
    for c in s.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '[' || c == '{' {
            depth += 1;
        } else if c == ']' || c == '}' {
            depth -= 1;
        } else if c == ',' && depth == 0 {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
    }
    if !ws(&cur).is_empty() {
        out.push(cur);
    }
    out.iter().map(|s| ws(s).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_block_yaml() {
        let d = documents("a: 1\nb:\n  - x\n  - y: 2\n    z: 'q' # c\nc: |\n  l1\n  l2\nd: [p, \"q, r\"]\n---\n- k\n");
        assert_eq!(d[0], json!({"a": "1", "b": ["x", {"y": "2", "z": "q"}], "c": "l1\nl2", "d": ["p", "q, r"]}));
        assert_eq!(d[1], json!(["k"]));
        assert_eq!(documents("e: {f: g}\nh:\n")[0], json!({"e": {"f": "g"}, "h": ""}));
    }
}
