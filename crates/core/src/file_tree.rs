//! Flattened, collapsible directory tree over file paths. Flat rows (instead of nested groups) keep a 5000-file PR
//! a single virtualized list.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TreeRow {
    pub id: String,              // file path, or "dir:" + dir path
    pub name: String,            // display name; compressed chains read "a/b/c"
    pub depth: usize,
    pub dir_path: Option<String>, // Some → directory row
    pub file_path: Option<String>,
}

/// Where Finder (`localizedStandardCompare`, ICU) puts a character: ASCII punctuation in ICU's order, then digits,
/// then letters case-insensitively. Byte order would put "charge.go" before "charge_test.go"; Finder doesn't.
fn collation_key(c: char) -> (u8, char) {
    const PUNCT: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    match PUNCT.find(c) {
        Some(i) => (0, char::from(i as u8)),
        None if c.is_ascii_digit() => (1, c),
        None => (2, c.to_lowercase().next().unwrap_or(c)),
    }
}

/// Finder's order for names: case-insensitive, runs of digits by their value ("f2" before "f10").
pub fn standard_compare(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            // Equal ignoring case: lowercase first, as Finder does.
            (None, None) => return b.chars().map(|c| c.is_lowercase()).cmp(a.chars().map(|c| c.is_lowercase())).then(a.cmp(b)),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) { s.push(c); it.next(); }
                    s
                };
                let (m, n) = (num(&mut x), num(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != Ordering::Equal { return o }
            }
            (Some(c), Some(d)) => {
                let o = collation_key(c).cmp(&collation_key(d));
                if o != Ordering::Equal { return o }
                x.next();
                y.next();
            }
        }
    }
}

/// Directories before files at every level, then by name: the order the tree displays in.
pub fn tree_order(a: &str, b: &str) -> Ordering {
    let (x, y): (Vec<&str>, Vec<&str>) = (a.split('/').filter(|s| !s.is_empty()).collect(), b.split('/').filter(|s| !s.is_empty()).collect());
    for i in 0..x.len().min(y.len()) {
        if x[i] == y[i] { continue }
        let (xd, yd) = (i < x.len() - 1, i < y.len() - 1);
        if xd != yd { return if xd { Ordering::Less } else { Ordering::Greater } }
        return standard_compare(x[i], y[i]);
    }
    x.len().cmp(&y.len())
}

pub fn sort_paths(paths: &mut [String]) { paths.sort_by(|a, b| tree_order(a, b)) }

#[derive(Default)]
struct Node {
    children: HashMap<String, usize>,
    order: Vec<String>,
    file: Option<String>,
}

/// `paths` must already be in `tree_order`. Collapsed directories show without their contents.
pub fn rows(paths: &[String], collapsed: &HashSet<String>) -> Vec<TreeRow> {
    let mut nodes = vec![Node::default()];
    let child = |nodes: &mut Vec<Node>, n: usize, name: &str| -> usize {
        if let Some(&c) = nodes[n].children.get(name) { return c }
        nodes.push(Node::default());
        let c = nodes.len() - 1;
        nodes[n].children.insert(name.to_string(), c);
        nodes[n].order.push(name.to_string());
        c
    };
    for p in paths {
        let comps: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        let mut n = 0;
        for c in &comps[..comps.len().saturating_sub(1)] { n = child(&mut nodes, n, c) }
        let leaf = child(&mut nodes, n, comps.last().copied().unwrap_or(p));
        nodes[leaf].file = Some(p.clone());
    }
    let mut out = vec![];
    fn walk(nodes: &[Node], n: usize, prefix: &str, depth: usize, collapsed: &HashSet<String>, out: &mut Vec<TreeRow>) {
        for name in &nodes[n].order {
            let mut c = nodes[n].children[name];
            if let Some(f) = &nodes[c].file {
                out.push(TreeRow { id: f.clone(), name: name.clone(), depth, dir_path: None, file_path: Some(f.clone()) });
                continue;
            }
            let (mut label, mut path) = (name.clone(), format!("{prefix}{name}"));
            // Compress single-directory chains: central_api → src becomes "central_api/src".
            while nodes[c].order.len() == 1 {
                let only_name = &nodes[c].order[0];
                let only = nodes[c].children[only_name];
                if nodes[only].file.is_some() { break }
                label += &format!("/{only_name}");
                path += &format!("/{only_name}");
                c = only;
            }
            out.push(TreeRow { id: format!("dir:{path}"), name: label, depth, dir_path: Some(path.clone()), file_path: None });
            if !collapsed.contains(&path) { walk(nodes, c, &format!("{path}/"), depth + 1, collapsed, out) }
        }
    }
    walk(&nodes, 0, "", 0, collapsed, &mut out);
    out
}

/// Every directory holding a path (for Collapse All).
pub fn all_dirs(paths: &[String]) -> Vec<String> {
    let mut set = HashSet::new();
    for p in paths {
        let c: Vec<&str> = p.split('/').collect();
        for i in 1..c.len() { set.insert(c[..i].join("/")); }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_compressed_collapsible_tree() {
        let mut paths: Vec<String> = ["central_api/src/services/authn.rs", "README.md", "central_api/src/lib.rs", "central_api/src/services/x.rs"]
            .iter().map(|s| s.to_string()).collect();
        sort_paths(&mut paths);
        assert_eq!(paths, ["central_api/src/services/authn.rs", "central_api/src/services/x.rs", "central_api/src/lib.rs", "README.md"]);
        let r = rows(&paths, &HashSet::new());
        assert_eq!(r.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["central_api/src", "services", "authn.rs", "x.rs", "lib.rs", "README.md"]);
        assert_eq!(r.iter().map(|r| r.depth).collect::<Vec<_>>(), [0, 1, 2, 2, 1, 0]);
        let folded = rows(&paths, &HashSet::from(["central_api/src/services".to_string()]));
        assert_eq!(folded.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["central_api/src", "services", "lib.rs", "README.md"]);
        assert_eq!(rows(&paths, &HashSet::from(["central_api/src".to_string()])).len(), 2);
    }

    #[test]
    fn orders_names_like_finder() {
        assert_eq!(standard_compare("f2.rs", "f10.rs"), Ordering::Less);
        assert_eq!(standard_compare("Banana", "apple"), Ordering::Greater);
        // Expected values from Swift's localizedStandardCompare.
        assert_eq!(standard_compare("charge_test.go", "charge.go"), Ordering::Less);
        assert_eq!(standard_compare("a-b", "a.b"), Ordering::Less);
        assert_eq!(standard_compare("ab", "a_b"), Ordering::Greater);
        assert_eq!(standard_compare("Ab", "ab"), Ordering::Greater);
        assert_eq!(tree_order("z/a", "a"), Ordering::Less); // directories first
    }
}
