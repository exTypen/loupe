//! The command palette frame the quick switcher (and any later picker) sits in: a dimmed backdrop, a card near
//! the top with the query field, a divider and the results. Keys and matching stay with the caller.
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Styled as _, div, px,
};

use crate::theme;

/// `on_dismiss` runs on a click outside the card.
pub fn palette(input: impl IntoElement, results: impl IntoElement, footer: Option<AnyElement>,
               on_dismiss: impl Fn(&mut gpui_kit::Window, &mut App) + 'static) -> impl IntoElement {
    div().absolute().inset_0().flex().flex_col().items_center().pt(px(90.)).bg(gpui_kit::hsla(0., 0., 0., 0.35))
        .on_mouse_down(MouseButton::Left, move |_, w, cx| on_dismiss(w, cx))
        .child(
            div().w(px(600.)).h(px(380.)).flex().flex_col().overflow_hidden()
                .bg(theme::sidebar()).border_1().border_color(theme::border_strong()).rounded(theme::RADIUS_CARD).shadow_lg()
                // Clicks inside don't dismiss.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().flex().items_center().gap_2().px_3().h(px(46.)).flex_none()
                    .child(crate::ui::icon("search").size(px(16.)))
                    .child(div().flex_1().child(input)))
                .child(crate::ui::divider())
                .child(div().flex_1().min_h_0().p_1p5().child(results))
                .children(footer.map(|f| div().flex().items_center().gap_3().px_3().h(px(28.)).flex_none()
                    .border_t_1().border_color(theme::border()).text_size(px(11.)).text_color(theme::text_faint()).child(f))),
        )
}
