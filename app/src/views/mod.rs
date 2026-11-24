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
