//! Headless interaction tests: a real DiffView in a test window, driven by key presses through gpui's dispatch.
use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Focusable as _, TestAppContext};
use loupe_app::{diff::DiffView, state::adapt};

#[gpui_kit::test]
fn select_all_and_copy_in_a_diff(cx: &mut TestAppContext) {
    cx.update(gpui_kit::component::init);
    let src = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n";
    let file = Arc::new(adapt::file_diff(&loupe_core::diff::parse(src).remove(0)));
    let view = cx.update(|cx| cx.open_window(Default::default(), |_, cx| cx.new(|cx| DiffView::new(file, cx))).unwrap());
    let focus = view.update(cx, |v, _, cx| v.focus_handle(cx)).unwrap();
    cx.update_window(view.into(), |_, window, cx| {
        window.focus(&focus, cx);
        window.render_frame(cx);
        window.press("cmd-a", cx);
        window.press("cmd-c", cx);
    }).unwrap();
    let copied = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
    assert!(copied.contains("keep") && copied.contains("new"), "{copied:?}");
}
