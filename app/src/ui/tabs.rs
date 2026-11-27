//! CodeBrowser.swift's shared parts: open-file tabs with back/forward through jumps (`CodeNav`), the tab bar,
//! the back/forward/outline controls and the definitions/references panel. The browsing models (a pull request
//! at its head, a worktree on disk) own a `CodeNav` and render these.
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ClipboardItem, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    uniform_list,
    component::{
        button::{Button, ButtonVariants as _},
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    },
};

use crate::{theme, ui};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodeLocation { pub path: String, pub line: usize }

/// Open tabs, the shown file and line, and the jump history.
#[derive(Clone, Debug, Default)]
pub struct CodeNav {
    pub tabs: Vec<String>,
    pub selected: Option<String>,
    pub line: Option<usize>, // the line last revealed in `selected`
    pub back: Vec<CodeLocation>,
    pub forward: Vec<CodeLocation>,
}

impl CodeNav {
    /// Shows `path` at `line`. Whatever opens (tree, jump, result) opens as a tab next to the current one; a
    /// jump elsewhere is recorded for Back.
    pub fn open(&mut self, path: &str, line: usize) { self.open_inner(path, line, true) }

    fn open_inner(&mut self, path: &str, line: usize, record: bool) {
        if let Some(cur) = self.selected.clone() {
            if record && (cur != path || self.line != Some(line)) {
                self.back.push(CodeLocation { path: cur, line: self.line.unwrap_or(1) });
                self.forward.clear();
            }
        }
        if !self.tabs.iter().any(|t| t == path) {
            let at = self.selected.as_ref().and_then(|s| self.tabs.iter().position(|t| t == s)).map_or(self.tabs.len(), |i| i + 1);
            self.tabs.insert(at.min(self.tabs.len()), path.into());
        }
        self.selected = Some(path.into());
        self.line = Some(line);
    }

    /// Compact mode: keep up with the file the change list shows, without opening a tab for it.
    pub fn follow(&mut self, path: &str) { self.selected = Some(path.into()) }

    pub fn go_back(&mut self) {
        let Some(to) = self.back.pop() else { return };
        if let Some(cur) = self.selected.clone() { self.forward.push(CodeLocation { path: cur, line: self.line.unwrap_or(1) }) }
        self.open_inner(&to.path, to.line, false);
    }

    pub fn go_forward(&mut self) {
        let Some(to) = self.forward.pop() else { return };
        if let Some(cur) = self.selected.clone() { self.back.push(CodeLocation { path: cur, line: self.line.unwrap_or(1) }) }
        self.open_inner(&to.path, to.line, false);
    }

    pub fn close_tab(&mut self, path: &str) {
        let Some(i) = self.tabs.iter().position(|t| t == path) else { return };
        self.tabs.remove(i);
        if self.selected.as_deref() == Some(path) {
            self.selected = if self.tabs.is_empty() { None } else { Some(self.tabs[i.min(self.tabs.len() - 1)].clone()) };
        }
    }

    pub fn close_other_tabs(&mut self, path: &str) {
        self.tabs.retain(|t| t == path);
        self.selected = Some(path.into());
    }

    pub fn close_all(&mut self) { self.tabs.clear(); self.selected = None }
}

/// What a tab bar or results panel asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum TabEvent { Select(String), Close(String), CloseOthers(String), CloseAll, Open(CodeLocation), Back, Forward, DismissResults }

pub type OnTabEvent = Rc<dyn Fn(TabEvent, &mut Window, &mut App)>;

fn file_name(p: &str) -> &str { p.rsplit('/').next().unwrap_or(p) }
fn parent_name(p: &str) -> &str { p.rsplit('/').nth(1).unwrap_or("") }

/// Full mode's open files. `changed` gives a changed file's status color (its dot); the selected tab is the
/// one shown. Same file name twice (mod.rs, index.ts) adds the parent directory.
pub fn tab_bar(nav: &CodeNav, changed: impl Fn(&str) -> Option<Hsla>, on: OnTabEvent) -> AnyElement {
    let tabs = nav.tabs.clone();
    div().id("file-tabs").flex().h(px(32.)).flex_none().w_full().overflow_x_scroll().bg(theme::sidebar())
        .border_b_1().border_color(theme::border())
        .children(tabs.iter().map(|path| {
            let active = nav.selected.as_deref() == Some(path);
            let name = file_name(path);
            let twin = tabs.iter().any(|t| t != path && file_name(t) == name);
            let parent = parent_name(path);
            let group: SharedString = format!("tab-{path}").into();
            let (p1, p2, p3, p4, p5) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
            let (o1, o2, o3, o4, o5) = (on.clone(), on.clone(), on.clone(), on.clone(), on.clone());
            let several = tabs.len() > 1;
            div().id(ElementId::Name(group.clone())).group(group.clone()).flex().flex_none().items_center().gap_1p5()
                .h_full().px_3().text_size(px(12.5)).cursor_pointer()
                .border_r_1().border_color(theme::border())
                .map(|d| if active { d.bg(theme::canvas()).text_color(theme::text()).border_t_2().border_color(theme::accent()) }
                         else { d.text_color(theme::text_muted()).hover(|s| s.bg(theme::hover())) })
                .on_click(move |_, w, cx| o1(TabEvent::Select(p1.clone()), w, cx))
                .child(ui::file_icon(path).size(px(13.)))
                .child(div().truncate().max_w(px(220.)).child(name.to_string()))
                .when(twin && !parent.is_empty(), |d| d.child(div().text_size(px(11.)).text_color(theme::text_faint()).child(parent.to_string())))
                .when_some(changed(path), |d, c| d.child(ui::dot(c).size(px(6.))))
                .child(div().id("close").flex().items_center().justify_center().size(px(16.)).rounded(px(4.))
                    .when(!active, |d| d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)))
                    .hover(|s| s.bg(theme::hover()))
                    .on_click(move |_, w, cx| { cx.stop_propagation(); o2(TabEvent::Close(p2.clone()), w, cx) })
                    .child(ui::icon("x").size(px(11.))))
                .context_menu(move |menu, _, _| {
                    let (a, b, c, d) = (o3.clone(), o4.clone(), o5.clone(), p3.clone());
                    let (pa, pb) = (p4.clone(), p5.clone());
                    menu.item(PopupMenuItem::new("Close Tab").on_click(move |_, w, cx| a(TabEvent::Close(pa.clone()), w, cx)))
                        .item(PopupMenuItem::new("Close Other Tabs").disabled(!several).on_click(move |_, w, cx| b(TabEvent::CloseOthers(pb.clone()), w, cx)))
                        .item(PopupMenuItem::new("Close All Tabs").on_click(move |_, w, cx| c(TabEvent::CloseAll, w, cx)))
                        .separator()
                        .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(d.clone()))))
                })
        }))
        .into_any_element()
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutlineSymbol { pub name: String, pub kind: String, pub line: usize }

/// What the outline button offers: nothing yet, the file's symbols, or "too large or minified to parse".
#[derive(Clone, Debug, PartialEq)]
pub enum Outline { None, Symbols(Vec<OutlineSymbol>), TooLarge }

/// Back / forward through jumps, the file's outline, and what's being looked up.
pub fn nav_controls(nav: &CodeNav, path: &str, outline: &Outline, busy: Option<&str>, on: OnTabEvent) -> AnyElement {
    let (b, f) = (on.clone(), on.clone());
    let can_back = !nav.back.is_empty();
    let can_forward = !nav.forward.is_empty();
    div().flex().items_center().gap_0p5()
        .when_some(busy, |d, w| d.child(div().px_1().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_muted()).truncate().child(format!("{w}…"))))
        .child(ui::icon_button("nav-back", "chevron-left").when(!can_back, |d| d.opacity(0.35))
            .when(can_back, |d| d.on_click(move |_, w, cx| b(TabEvent::Back, w, cx))))
        .child(ui::icon_button("nav-forward", "chevron-right").when(!can_forward, |d| d.opacity(0.35))
            .when(can_forward, |d| d.on_click(move |_, w, cx| f(TabEvent::Forward, w, cx))))
        .map(|d| match outline {
            Outline::TooLarge => d.child(div().id("outline").size(px(28.)).flex().items_center().justify_center()
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("No outline: this file is too large or minified to parse").build(w, cx))
                .child(ui::icon("list-tree").text_color(theme::text_faint()))),
            Outline::Symbols(syms) if !syms.is_empty() => {
                let syms = syms.clone();
                let path = path.to_string();
                d.child(Button::new("outline").ghost().icon(gpui_kit::component::Icon::empty().path("icons/list-tree.svg"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for s in &syms {
                            let (o, loc) = (on.clone(), CodeLocation { path: path.clone(), line: s.line });
                            menu = menu.item(PopupMenuItem::new(format!("{}  {} · {}", s.name, s.kind, s.line))
                                .on_click(move |_, w, cx| o(TabEvent::Open(loc.clone()), w, cx)));
                        }
                        menu.max_h(px(420.)).scrollable(true)
                    }))
            }
            _ => d,
        })
        .into_any_element()
}

#[derive(Clone, Debug, PartialEq)]
pub struct NavHit { pub path: String, pub line: usize, pub text: String }

/// Under the file: the definitions to choose from, or every reference.
#[derive(Clone, Debug, PartialEq)]
pub enum NavResults {
    Definitions(String, Vec<NavHit>),
    References(String, Vec<NavHit>, bool), // bool: looked for a definition and found none
}

impl NavResults {
    /// The header's words before the looked-up identifier.
    pub fn title(&self) -> (String, &str) {
        match self {
            NavResults::Definitions(w, d) => (format!("{} definitions of ", d.len()), w),
            NavResults::References(w, r, no_def) => {
                // The lookup asks for REFERENCE_LIMIT + 1, so exactly 2000 reads "2000" (Swift said "2000+", §14 #9).
                let lim = loupe_code::index::REFERENCE_LIMIT;
                let n = match r.len() { n if n > lim => format!("{lim}+ references"), 1 => "1 reference".into(), n => format!("{n} references") };
                (format!("{}{n} to ", if *no_def { "No definition found in this repository · " } else { "" }), w)
            }
        }
    }
    fn hits(&self) -> &[NavHit] { match self { NavResults::Definitions(_, h) | NavResults::References(_, h, _) => h } }
}

pub fn results_panel(results: &NavResults, on: OnTabEvent) -> AnyElement {
    let (prefix, word) = results.title();
    let hits: Rc<Vec<NavHit>> = Rc::new(results.hits().to_vec());
    let close = on.clone();
    div().flex().flex_col().size_full().bg(theme::canvas())
        .child(div().flex().items_center().gap_1().px_3().h(px(30.)).flex_none().bg(theme::card_header())
            .border_b_1().border_color(theme::border()).text_size(px(12.5))
            .child(div().text_color(theme::text_muted()).child(prefix))
            .child(div().font_family(theme::CODE_FONT).text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(word.to_string()))
            .child(div().flex_1())
            .child(ui::icon_button("results-close", "x").on_click(move |_, w, cx| close(TabEvent::DismissResults, w, cx))))
        .child(uniform_list("nav-results", hits.len(), move |range, _, _| {
            range.map(|i| {
                let h = hits[i].clone();
                let o = on.clone();
                let loc = CodeLocation { path: h.path.clone(), line: h.line };
                div().id(i).flex().items_center().gap_2().px_3().h(px(24.)).cursor_pointer().hover(|s| s.bg(theme::hover()))
                    .on_click(move |_, w, cx| o(TabEvent::Open(loc.clone()), w, cx))
                    .child(ui::file_icon(&h.path).size(px(12.)))
                    .child(div().w(px(260.)).flex_none().text_ellipsis_start().whitespace_nowrap().overflow_hidden()
                        .font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_muted()).child(format!("{}:{}", h.path, h.line)))
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::CODE_FONT).text_size(px(12.)).text_color(theme::text()).child(h.text))
                    .into_any_element()
            }).collect::<Vec<AnyElement>>()
        }).flex_1())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_open_next_to_the_current_one_and_jumps_go_back_and_forward() {
        let mut n = CodeNav::default();
        n.open("a.rs", 1);
        n.open("b.rs", 10);
        n.selected = Some("a.rs".into()); n.line = Some(1);
        n.open("c.rs", 5);
        assert_eq!(n.tabs, ["a.rs", "c.rs", "b.rs"]);
        assert_eq!(n.back.last(), Some(&CodeLocation { path: "a.rs".into(), line: 1 }));
        n.go_back();
        assert_eq!((n.selected.as_deref(), n.line), (Some("a.rs"), Some(1)));
        assert_eq!(n.forward, [CodeLocation { path: "c.rs".into(), line: 5 }]);
        n.go_forward();
        assert_eq!(n.selected.as_deref(), Some("c.rs"));
        // A new jump clears Forward.
        n.go_back(); n.open("b.rs", 3);
        assert!(n.forward.is_empty());
        // The same place again records nothing.
        let depth = n.back.len(); n.open("b.rs", 3);
        assert_eq!(n.back.len(), depth);
    }

    #[test]
    fn closing_tabs_selects_a_neighbour() {
        let mut n = CodeNav { tabs: vec!["a".into(), "b".into(), "c".into()], selected: Some("c".into()), ..Default::default() };
        n.close_tab("c");
        assert_eq!(n.selected.as_deref(), Some("b"));
        n.close_tab("a");
        assert_eq!(n.selected.as_deref(), Some("b"));
        n.close_tab("b");
        assert_eq!(n.selected, None);
        let mut n = CodeNav { tabs: vec!["a".into(), "b".into()], selected: Some("a".into()), ..Default::default() };
        n.close_other_tabs("b");
        assert_eq!((n.tabs.as_slice(), n.selected.as_deref()), (&["b".to_string()][..], Some("b")));
    }

    #[test]
    fn results_titles() {
        let hit = NavHit { path: "a".into(), line: 1, text: "x".into() };
        assert_eq!(NavResults::References("f".into(), vec![hit.clone()], false).title().0, "1 reference to ");
        assert_eq!(NavResults::References("f".into(), vec![hit.clone(); 2001], true).title().0,
                   "No definition found in this repository · 2000+ references to ");
        assert_eq!(NavResults::References("f".into(), vec![hit.clone(); 2000], false).title().0, "2000 references to ");
        assert_eq!(NavResults::Definitions("f".into(), vec![hit; 2]).title().0, "2 definitions of ");
    }
}
