//! The file tree component (FileTree.swift): folders first, natural name order, single-child folder chains in one
//! row, a fold set per host. Changes uses it for its sections (Full mode's tree is the code browser's); History and Pull
//! Requests can use `rows` + `dir_row` + `file_row_frame` for theirs.
use std::collections::HashSet;

use gpui_kit::{Div, Hsla, ParentElement as _, Styled as _, div, px};
use loupe_core::file_tree::{self, TreeRow};
use loupe_core::git::Kind;

use crate::{theme, ui};

/// The rows to draw. `paths` in any order; `filter` (a case-insensitive substring, PR files) shows every folder open.
pub fn rows(paths: &[String], collapsed: &HashSet<String>, filter: &str) -> Vec<TreeRow> {
    let q = filter.trim().to_lowercase();
    let mut p: Vec<String> = if q.is_empty() { paths.to_vec() } else { paths.iter().filter(|p| p.to_lowercase().contains(&q)).cloned().collect() };
    file_tree::sort_paths(&mut p);
    let none = HashSet::new();
    file_tree::rows(&p, if q.is_empty() { collapsed } else { &none })
}

/// A row's indent: 10 pt per level with a 1 px guide for each.
fn indented(depth: usize) -> Div {
    let mut d = div().flex().items_center().h_full().flex_none();
    for _ in 0..depth { d = d.child(div().w(px(10.)).h_full().flex_none().child(div().ml(px(4.)).w(px(1.)).h_full().bg(theme::border()))) }
    d
}

/// A folder: chevron (turned when open), folder icon, the name dimmed and cut from the front.
pub fn dir_row(r: &TreeRow, open: bool) -> Div {
    div().flex().items_center().gap_1().h_full().w_full().min_w_0().text_color(theme::text_muted())
        .child(indented(r.depth))
        .child(ui::icon(if open { "chevron-down" } else { "chevron-right" }).size(px(11.)).text_color(theme::text_faint()))
        .child(ui::icon(if open { "folder-open" } else { "folder" }).size(px(13.)))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(r.name.clone()))
}

/// A file row's frame: its indent and the 12 pt spacer that lines names up under the folder's.
pub fn file_row_frame(depth: usize) -> Div {
    div().flex().items_center().gap(px(6.)).h_full().w_full().min_w_0().child(indented(depth)).child(div().w(px(12.)).flex_none())
}

/// MS-15: the trailing change-kind mark and its tooltip.
pub fn status_icon(kind: Kind) -> (&'static str, Hsla, &'static str) {
    match kind {
        Kind::Added => ("square-plus", theme::green(), "Added"),
        Kind::Untracked => ("square-plus", theme::green(), "Untracked"),
        Kind::Deleted => ("square-minus", theme::red(), "Deleted"),
        Kind::Renamed => ("square-arrow-right", theme::blue(), "Renamed"),
        Kind::Copied => ("square-arrow-right", theme::blue(), "Copied"),
        Kind::Conflicted => ("square-asterisk", theme::orange(), "Conflicted"), // orange as in Swift; amber reads "modified"
        Kind::TypeChanged => ("square-dot", theme::accent(), "Type changed"),
        Kind::Modified => ("square-dot", theme::accent(), "Modified"),
    }
}

/// The name, cut in the middle when it doesn't fit.
pub fn name(path: &str) -> String { path.rsplit('/').next().unwrap_or(path).to_string() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_and_folds() {
        let paths: Vec<String> = ["src/a/b/c.swift", "src/x.ts", "README.md", "f10", "f2"].iter().map(|s| s.to_string()).collect();
        let r = rows(&paths, &HashSet::new(), "");
        assert_eq!(r.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["src", "a/b", "c.swift", "x.ts", "f2", "f10", "README.md"]);
        let f = rows(&paths, &HashSet::from(["src".to_string()]), "X.T");
        assert_eq!(f.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["src", "x.ts"]); // filtering shows folders open
        assert_eq!(status_icon(Kind::Untracked).2, "Untracked");
    }
}
