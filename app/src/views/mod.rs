//! One module per view owner (docs/gpui/VIEWS.md). The shell constructs the three tab views and calls `install`
//! once per window; everything else a view needs comes from `state::AppModel` and its `WorktreeModel`s.
pub mod changes;
pub mod ci;
pub mod code;
pub mod history;
pub mod merge;
pub mod pr_create;
pub mod pulls;
pub mod settings;

use gpui_kit::{App, Entity, Subscription, Window};

use crate::state::AppModel;

/// Window-wide listeners of the views that open as overlays (checks, merge editor, PR create): each subscribes to
/// the `AppEvent`s it serves and presents itself with `AppModel::present`.
pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Vec<Subscription> {
    vec![ci::install(app, window, cx), merge::install(app, window, cx), pr_create::install(app, window, cx)]
}

/// The empty state every stub shows until its owner builds the view.
pub(crate) fn stub(icon: &str, title: &str, message: &str) -> gpui_kit::Div {
    use gpui_kit::{Styled as _, ParentElement as _};
    gpui_kit::div().size_full().child(crate::ui::empty_state(icon, title.to_string(), message.to_string()))
}

/// A centred card over a dimmed window, closed by Esc or its Close button: the stub overlays' frame.
pub(crate) fn overlay_stub<V: 'static>(app: &Entity<AppModel>, icon: &str, title: &str, detail: &str, cx: &mut gpui_kit::Context<V>) -> gpui_kit::AnyElement {
    use gpui_kit::{InteractiveElement as _, IntoElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px};
    let (a, b) = (app.clone(), app.clone());
    let _ = cx;
    div().absolute().inset_0().flex().items_center().justify_center().bg(crate::theme::backdrop())
        .on_key_down(move |e, _, cx| if e.keystroke.key == "escape" { a.update(cx, |a, cx| a.dismiss(cx)) })
        .child(crate::ui::card().w(px(520.)).h(px(360.)).bg(crate::theme::sidebar())
            .child(crate::ui::card_header(title.to_string(), crate::ui::link_button("overlay-close", "Close")
                .on_click(move |_, _, cx| b.update(cx, |a, cx| a.dismiss(cx)))))
            .child(div().flex_1().child(crate::ui::empty_state(icon, title.to_string(), detail.to_string()))))
        .into_any_element()
}
