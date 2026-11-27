//! History's modal pieces, presented over the window with `AppModel::present`: a confirmation / alert dialog
//! (SwiftUI's confirmationDialog and NSAlert) and small date helpers the rows and the header share. "New Worktree
//! from Here…" opens the shell's sheet (`AppEvent::NewWorktree`).
use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::state::AppModel;
use crate::{theme, ui};

type Action = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq)]
pub enum Role { Cancel, Default, Destructive }

/// A centred alert: title, message, buttons right-aligned. Esc is Cancel (or the only button), Return the default
/// or destructive one. Every button dismisses first, then runs its action.
pub struct Dialog {
    app: Entity<AppModel>,
    title: String,
    message: String,
    buttons: Vec<(String, Role, Option<Action>)>,
    focus: FocusHandle,
    /// Focus goes back here on dismiss: with nothing focused the window's key bindings (⌘Z, ⌘R, ⌥⌘↓ …) never reach
    /// the shell and the view, so the first ⌘Z after "Drop Stash" did nothing.
    back: FocusHandle,
}

impl Dialog {
    pub fn present(app: &Entity<AppModel>, title: impl Into<String>, message: impl Into<String>,
                   buttons: Vec<(String, Role, Option<Action>)>, back: &FocusHandle, window: &mut Window, cx: &mut App) {
        let (title, message, back) = (title.into(), message.into(), back.clone());
        let a = app.clone();
        let v = cx.new(|cx| Dialog { app: a, title, message, buttons, focus: cx.focus_handle(), back });
        let f = v.read(cx).focus.clone();
        f.focus(window, cx);
        app.update(cx, |a, cx| a.present(v.into(), cx));
    }

    /// "Drop stash@{0}?" with a destructive confirm and Cancel.
    pub fn confirm(app: &Entity<AppModel>, title: impl Into<String>, message: impl Into<String>, confirm: &str,
                   action: impl Fn(&mut Window, &mut App) + 'static, back: &FocusHandle, window: &mut Window, cx: &mut App) {
        Self::present(app, title, message, vec![
            ("Cancel".into(), Role::Cancel, None),
            (confirm.into(), Role::Destructive, Some(Rc::new(action))),
        ], back, window, cx)
    }

    fn press(&self, i: usize, window: &mut Window, cx: &mut App) {
        let action = self.buttons.get(i).and_then(|b| b.2.clone());
        self.app.update(cx, |a, cx| a.dismiss(cx));
        self.back.focus(window, cx);
        if let Some(f) = action { f(window, cx) }
    }
}

impl Render for Dialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let n = self.buttons.len();
        let esc = self.buttons.iter().position(|b| b.1 == Role::Cancel).unwrap_or(n.saturating_sub(1));
        let ret = self.buttons.iter().position(|b| b.1 != Role::Cancel);
        let buttons = self.buttons.iter().enumerate().map(|(i, (label, role, _))| {
            let me = me.clone();
            let b = match role {
                Role::Cancel => ui::pill(SharedString::from(format!("dialog-{i}")), label.clone()),
                Role::Default => ui::pill_primary(SharedString::from(format!("dialog-{i}")), label.clone(), theme::blue()),
                Role::Destructive => ui::pill_primary(SharedString::from(format!("dialog-{i}")), label.clone(), theme::red()),
            };
            b.on_click(move |_, w, cx| me.update(cx, |v, cx| v.press(i, w, cx)))
        }).collect::<Vec<_>>();
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |v, e: &gpui_kit::KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                "escape" => v.press(esc, w, cx),
                "enter" => if let Some(r) = ret { v.press(r, w, cx) },
                _ => {}
            }))
            .child(ui::card().w(px(440.)).p_4().gap_3().bg(theme::raised())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
                .when(!self.message.is_empty(), |d| d.child(div().id("dialog-text").max_h(px(320.)).overflow_y_scroll()
                    .text_size(px(12.)).text_color(theme::text_muted()).whitespace_normal().child(self.message.clone())))
                .child(div().flex().justify_end().gap_2().children(buttons)))
    }
}

// MARK: - Dates

#[repr(C)]
struct Tm { sec: i32, min: i32, hour: i32, mday: i32, mon: i32, year: i32, wday: i32, yday: i32, isdst: i32, gmtoff: i64, zone: *const i8 }
unsafe extern "C" { fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm; }

/// (year, month 1-12, day, hour, minute) in local time.
fn local(t: i64) -> (i32, usize, i32, i32, i32) {
    let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 1, mon: 0, year: 70, wday: 0, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null() };
    unsafe { localtime_r(&t, &mut tm) };
    (tm.year + 1900, tm.mon as usize + 1, tm.mday, tm.hour, tm.min)
}

/// "Nov 21, 2026 at 3:04 PM": Swift's `.formatted(date: .abbreviated, time: .shortened)` in en_US.
pub fn abbreviated(t: i64) -> String {
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, mo, d, h, mi) = local(t);
    let (h12, ap) = match h { 0 => (12, "AM"), 1..=11 => (h, "AM"), 12 => (12, "PM"), _ => (h - 12, "PM") };
    format!("{} {d}, {y} at {h12}:{mi:02} {ap}", M[mo - 1])
}

/// "yesterday", "3 days ago", "last week", "2 months ago": `.relative(presentation: .named)`, past only.
pub fn relative(t: i64, now: i64) -> String {
    let s = (now - t).max(0);
    let unit = |n: i64, last: &str, many: &str| if n == 1 { last.to_string() } else { format!("{n} {many} ago") };
    match s {
        0..=59 => if s < 1 { "now".into() } else { format!("{s} second{} ago", if s == 1 { "" } else { "s" }) },
        60..=3599 => { let m = s / 60; format!("{m} minute{} ago", if m == 1 { "" } else { "s" }) }
        3600..=86399 => { let h = s / 3600; format!("{h} hour{} ago", if h == 1 { "" } else { "s" }) }
        86400..=604799 => unit(s / 86400, "yesterday", "days"),
        604800..=2591999 => unit(s / 604800, "last week", "weeks"),
        2592000..=31535999 => unit(s / 2592000, "last month", "months"),
        _ => unit(s / 31536000, "last year", "years"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_dates_read_like_swift() {
        let now = 1_800_000_000;
        assert_eq!(relative(now - 30, now), "30 seconds ago");
        assert_eq!(relative(now - 3600, now), "1 hour ago");
        assert_eq!(relative(now - 86400, now), "yesterday");
        assert_eq!(relative(now - 3 * 86400, now), "3 days ago");
        assert_eq!(relative(now - 8 * 86400, now), "last week");
        assert_eq!(relative(now - 40 * 86400, now), "last month");
        assert_eq!(relative(now - 400 * 86400, now), "last year");
        assert!(abbreviated(now).contains(", 2027 at "));
    }
}
