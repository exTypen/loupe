//! Shared look: cards, pills, badges, list rows, split panes (built on theme.rs). Small functions returning
//! styled `div`s, so a view keeps chaining (`.on_click`, `.child`, `.when`) on what it gets back.
pub mod palette;
pub mod tabs;

use gpui_kit::{
    App, Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Svg, Window, div, prelude::FluentBuilder as _,
    px, svg,
};

use crate::theme;

/// Split panes: gpui-component's resizable groups, restyled by `install_theme`.
pub use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel, v_resizable};

/// gpui-component's widgets (inputs, menus, popovers, scrollbars, resize handles) in theme.rs colors, and no
/// caret blink: a focused field must not keep the window redrawing.
pub fn install_theme(cx: &mut App) {
    use gpui_kit::component::{Theme, ThemeMode};
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |t| {
        t.font_family = theme::UI_FONT.into();
        t.font_size = theme::UI_SIZE;
        t.mono_font_family = theme::CODE_FONT.into();
        t.mono_font_size = theme::CODE_SIZE;
        t.radius = px(6.);
        t.radius_lg = theme::RADIUS_CARD;
        t.shadow = false;
        t.focus_ring = false;
        t.cursor_blink = gpui_kit::component::input::CursorBlink::steady();
        let c = &mut t.colors;
        c.background = theme::canvas();
        c.foreground = theme::text();
        c.border = theme::border();
        c.input = theme::border_strong();
        c.ring = theme::border_strong();
        c.caret = theme::text();
        c.selection = theme::hunk_bg();
        c.muted = theme::raised();
        c.muted_foreground = theme::text_muted();
        c.popover = theme::raised();
        c.popover_foreground = theme::text();
        c.accent = theme::selected();
        c.accent_foreground = theme::text();
        c.primary = theme::blue();
        c.secondary = theme::raised();
        c.secondary_hover = theme::selected();
        c.secondary_active = theme::selected();
        c.secondary_foreground = theme::text();
        c.list = theme::canvas();
        c.list_hover = theme::hover();
        c.list_active = theme::selected();
        c.list_active_border = theme::selected();
        c.sidebar = theme::sidebar();
        c.title_bar = theme::sidebar();
        c.title_bar_border = theme::sidebar();
        c.scrollbar = gpui_kit::transparent_black();
        c.drag_border = theme::border_strong();
        c.overlay = gpui_kit::hsla(0., 0., 0., 0.45);
    });
}

/// A Lucide icon from the embedded set ("git-branch", "folder", …), 14px, muted.
pub fn icon(name: &str) -> Svg {
    svg().path(SharedString::from(format!("icons/{name}.svg"))).size(px(14.)).flex_none().text_color(theme::text_muted())
}

/// Rounded, 1px border, the card surface. Put a `card_header` first for a title strip.
pub fn card() -> Div {
    div().flex().flex_col().bg(theme::card()).border_1().border_color(theme::border()).rounded(theme::RADIUS_CARD).overflow_hidden()
}

/// A card's title strip: the title on the left, meta (a duration, counts, "Done 15.1s") on the right.
pub fn card_header(title: impl IntoElement, meta: impl IntoElement) -> Div {
    div().flex().items_center().gap_2().px_3().h(px(32.)).flex_none().bg(theme::card_header()).border_b_1().border_color(theme::border())
        .text_color(theme::text()).text_size(px(12.))
        .child(div().flex().items_center().gap_2().flex_1().min_w_0().overflow_hidden().child(title))
        .child(div().flex().items_center().gap_2().flex_none().text_color(theme::text_muted()).child(meta))
}

/// A card's footer status line "✓ Done 15.1s".
pub fn card_footer(child: impl IntoElement) -> Div {
    div().flex().items_center().gap_2().px_3().py_1p5().min_h(px(30.)).flex_none().border_t_1().border_color(theme::border())
        .text_size(px(12.)).text_color(theme::text_muted()).child(child)
}

/// Rounded-full outlined button ("Fork from here", "Open Repository…").
pub fn pill(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    pill_base(id, label).bg(theme::canvas()).hover(|s| s.bg(theme::raised())).active(|s| s.bg(theme::selected()))
}

// gpui allows one hover style per element, so the variants share everything but it.
fn pill_base(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).flex().flex_none().items_center().justify_center().gap_1p5().h(px(28.)).px_3()
        .rounded(theme::RADIUS_PILL).border_1().border_color(theme::border_strong())
        .text_size(px(12.5)).text_color(theme::text()).cursor_pointer()
        .child(label.into())
}

/// A pill tinted with an accent: the one primary action on screen.
pub fn pill_primary(id: impl Into<ElementId>, label: impl Into<SharedString>, color: Hsla) -> Stateful<Div> {
    pill_base(id, label).border_color(color.opacity(0.6)).bg(color.opacity(0.16)).text_color(color)
        .hover(move |s| s.bg(color.opacity(0.26))).active(move |s| s.bg(color.opacity(0.34)))
}

/// A count or small label in a rounded-full chip (change counts).
pub fn badge(text: impl Into<SharedString>) -> Div {
    div().flex_none().px(px(6.)).h(px(17.)).flex().items_center().rounded(theme::RADIUS_PILL).bg(theme::raised())
        .text_size(px(11.)).text_color(theme::text_muted()).child(text.into())
}

/// A tinted badge: the label in `color` over a faint fill of it ("Merged", ref names).
pub fn badge_tinted(text: impl Into<SharedString>, color: Hsla) -> Div {
    badge(text).bg(color.opacity(0.15)).text_color(color).font_weight(FontWeight::MEDIUM)
}

/// Inline code: a sha, ref or path in running text `fde8251`.
pub fn code_chip(text: impl Into<SharedString>) -> Div {
    div().flex_none().px(px(5.)).rounded(px(4.)).bg(theme::raised()).border_1().border_color(theme::border())
        .font_family(theme::CODE_FONT).text_size(px(11.5)).text_color(theme::accent()).child(text.into())
}

/// Faint shortcut hint, "⌘1".
pub fn kbd(text: impl Into<SharedString>) -> Div {
    div().flex_none().text_size(px(11.)).text_color(theme::text_faint()).child(text.into())
}

/// A small filled circle: unread, CI state, a tab's changed mark.
pub fn dot(color: Hsla) -> Div { div().flex_none().size(px(7.)).rounded_full().bg(color) }

/// A list row: 28px, rounded, hover fill, the soft selected fill. Children go left to right.
pub fn list_row(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div().id(id).flex().items_center().gap_2().min_h(theme::ROW_HEIGHT).px_2().rounded(theme::RADIUS_ROW)
        .text_size(theme::UI_SIZE).text_color(if selected { theme::text() } else { theme::text_muted() })
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected, |d| d.hover(|s| s.bg(theme::hover()).text_color(theme::text())))
        .cursor_pointer()
}

/// The common row: icon, a truncating title (and optional subtitle), trailing meta that keeps its width.
pub fn list_row_parts(id: impl Into<ElementId>, selected: bool, icon: impl IntoElement, title: impl Into<SharedString>,
                      subtitle: Option<SharedString>, trailing: impl IntoElement) -> Stateful<Div> {
    list_row(id, selected)
        .child(icon)
        .child(div().flex().flex_col().flex_1().min_w_0().overflow_hidden()
            .child(div().truncate().child(title.into()))
            .when_some(subtitle, |d, s| d.child(div().truncate().text_size(px(11.)).text_color(theme::text_faint()).child(s))))
        .child(div().flex().flex_none().items_center().gap_1p5().child(trailing))
}

/// "Main  4": a muted group title with a faint count, for sidebar sections.
pub fn section_header(title: impl Into<SharedString>, count: Option<usize>) -> Div {
    div().flex().items_center().h(px(26.)).px_2().gap_2().text_size(px(12.5)).text_color(theme::text_muted())
        .child(div().flex_1().truncate().child(title.into()))
        .when_some(count, |d, n| d.child(kbd(n.to_string())))
}

/// A bordered monospace block; fill it with `code_line`s.
pub fn code_block() -> Div {
    card().py_1().font_family(theme::CODE_FONT).text_size(theme::CODE_SIZE).line_height(theme::CODE_LINE_HEIGHT)
}

#[derive(Clone, Copy, PartialEq)]
pub enum LineKind { Context, Added, Removed, Hunk }

/// One line of a code block: gutter number, a +/- marker and the text, tinted per kind.
pub fn code_line(number: Option<usize>, text: impl Into<SharedString>, kind: LineKind) -> Div {
    let (bg, marker, marker_color) = match kind {
        LineKind::Added => (Some(theme::added_bg()), "+", theme::green()),
        LineKind::Removed => (Some(theme::removed_bg()), "-", theme::red()),
        LineKind::Hunk => (Some(theme::hunk_bg()), "", theme::blue()),
        LineKind::Context => (None, "", theme::text_faint()),
    };
    div().flex().w_full().min_h(theme::CODE_LINE_HEIGHT).when_some(bg, |d, c| d.bg(c))
        .child(div().w(px(44.)).flex_none().pr_2().text_right().text_color(theme::text_faint())
            .child(number.map(|n| n.to_string()).unwrap_or_default()))
        .child(div().w(px(14.)).flex_none().text_color(marker_color).child(marker))
        .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden()
            .text_color(if kind == LineKind::Hunk { theme::blue() } else { theme::text() }).child(text.into()))
}

/// Nothing to show: an icon, a title, one line of why; add an action below with `.child`.
pub fn empty_state(icon_name: &str, title: impl Into<SharedString>, message: impl Into<SharedString>) -> Div {
    div().flex().flex_col().items_center().justify_center().gap_2().size_full().p_8()
        .child(icon(icon_name).size(px(28.)).text_color(theme::text_faint()))
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title.into()))
        .child(div().max_w(px(420.)).text_center().text_color(theme::text_muted()).child(message.into()))
}

/// A square icon button for toolbars, 28px, hover fill.
pub fn icon_button(id: impl Into<ElementId>, icon_name: &str) -> Stateful<Div> {
    div().id(id).flex().flex_none().items_center().justify_center().size(px(28.)).rounded(px(7.)).cursor_pointer()
        .hover(|s| s.bg(theme::hover())).active(|s| s.bg(theme::selected()))
        .child(icon(icon_name).size(px(15.)))
}

/// A segmented switch (Changes / History / Pull Requests): the chosen segment gets the selected fill.
/// `on_select` gets the index.
pub fn segmented(id: impl Into<SharedString>, labels: &[&str], selected: usize,
                 on_select: impl Fn(usize, &mut Window, &mut App) + 'static) -> Div {
    let id: SharedString = id.into();
    let on_select = std::rc::Rc::new(on_select);
    div().flex().flex_none().items_center().gap_0p5().p(px(2.)).h(px(30.)).rounded(px(9.)).bg(theme::canvas())
        .border_1().border_color(theme::border())
        .children(labels.iter().enumerate().map(|(i, l)| {
            let f = on_select.clone();
            div().id(ElementId::Name(format!("{id}-{i}").into())).flex().items_center().h_full().px_3().rounded(px(7.))
                .text_size(px(12.5)).cursor_pointer()
                .map(|d| if i == selected { d.bg(theme::selected()).text_color(theme::text()) }
                         else { d.text_color(theme::text_muted()).hover(|s| s.text_color(theme::text())) })
                // In the title bar a press that isn't prevented becomes a window move on the slightest drift,
                // which swallows the click (gpui-component's TitleBar).
                .on_mouse_down(gpui_kit::MouseButton::Left, |_, w, _| w.prevent_default())
                .on_click(move |_, w, cx| f(i, w, cx))
                .child(l.to_string())
        }))
}

/// The undo toast's capsule: icon, text, then whatever action (an "Undo" link) the caller adds.
pub fn toast(icon_name: &str, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap_2p5().h(px(34.)).px_4().max_w(px(560.)).rounded(theme::RADIUS_PILL)
        .bg(theme::raised()).border_1().border_color(theme::border_strong()).text_size(px(12.5)).text_color(theme::text())
        .shadow_lg()
        .child(icon(icon_name))
        .child(div().min_w_0().truncate().child(text.into()))
}

/// A text button for toasts and headers ("Undo", "Show all").
pub fn link_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).flex_none().font_weight(FontWeight::SEMIBOLD).text_color(theme::blue()).cursor_pointer()
        .hover(|s| s.text_color(theme::text())).child(label.into())
}

pub fn divider() -> Div { div().h(px(1.)).w_full().flex_none().bg(theme::border()) }

/// A file's type icon (FileIcons.swift via loupe_code::icons), drawn as an image: the SVGs are multicoloured, and
/// `svg()` would paint them as a one-colour mask. One decoded image per icon name, shared.
pub fn file_icon(path: &str) -> gpui_kit::Img {
    use std::{cell::RefCell, collections::HashMap, sync::Arc};
    use gpui_kit::{Image, ImageFormat};
    thread_local! { static CACHE: RefCell<HashMap<&'static str, Arc<Image>>> = RefCell::new(HashMap::new()); }
    let name = loupe_code::icons::icon_name(path);
    let image = CACHE.with(|c| c.borrow_mut().entry(name).or_insert_with(|| {
        let svg = loupe_code::icons::svg(name).unwrap_or_else(|| loupe_code::icons::svg_for(path));
        Arc::new(Image::from_bytes(ImageFormat::Svg, svg.as_bytes().to_vec()))
    }).clone());
    gpui_kit::img(image).size(px(14.)).flex_none()
}
