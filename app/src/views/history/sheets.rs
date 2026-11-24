//! History's modal pieces, presented over the window with `AppModel::present`: a confirmation / alert dialog
//! (SwiftUI's confirmationDialog and NSAlert), the New Worktree sheet (App.swift's AddWorktreeSheet, opened from a
//! commit with that commit as the base) and small date helpers the rows and the header share.
use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    component::{
        Selectable as _, Sizable as _,
        button::Button,
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
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
}

impl Dialog {
    pub fn present(app: &Entity<AppModel>, title: impl Into<String>, message: impl Into<String>,
                   buttons: Vec<(String, Role, Option<Action>)>, window: &mut Window, cx: &mut App) {
        let (title, message) = (title.into(), message.into());
        let a = app.clone();
        let v = cx.new(|cx| Dialog { app: a, title, message, buttons, focus: cx.focus_handle() });
        let f = v.read(cx).focus.clone();
        f.focus(window, cx);
        app.update(cx, |a, cx| a.present(v.into(), cx));
    }

    /// "Drop stash@{0}?" with a destructive confirm and Cancel.
    pub fn confirm(app: &Entity<AppModel>, title: impl Into<String>, message: impl Into<String>, confirm: &str,
                   action: impl Fn(&mut Window, &mut App) + 'static, window: &mut Window, cx: &mut App) {
        Self::present(app, title, message, vec![
            ("Cancel".into(), Role::Cancel, None),
            (confirm.into(), Role::Destructive, Some(Rc::new(action))),
        ], window, cx)
    }

    fn press(&self, i: usize, window: &mut Window, cx: &mut App) {
        let action = self.buttons.get(i).and_then(|b| b.2.clone());
        self.app.update(cx, |a, cx| a.dismiss(cx));
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
            b.on_click(move |_, w, cx| {
                let (action, app) = { let v = me.read(cx); (v.buttons[i].2.clone(), v.app.clone()) };
                app.update(cx, |a, cx| a.dismiss(cx));
                if let Some(f) = action { f(w, cx) }
            })
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

// MARK: - New Worktree

/// "New Worktree" from a History commit (HI-20): New branch / Existing branch, Name, Based on (HEAD, the commit,
/// every branch), Location. The path follows the name until it's edited.
pub struct NewWorktree {
    app: Entity<AppModel>,
    repo: String,
    new_branch: bool,
    name: Entity<InputState>,
    location: Entity<InputState>,
    path_edited: bool,
    base: String,
    commit: String,
    branches: Vec<String>,
    error: Option<String>,
    working: bool,
    focus: FocusHandle,
    _subs: Vec<gpui_kit::Subscription>,
}

impl NewWorktree {
    pub fn present(app: &Entity<AppModel>, repo: String, commit: String, branches: Vec<String>, window: &mut Window, cx: &mut App) {
        let a = app.clone();
        let v = cx.new(|cx| {
            let name = cx.new(|cx| InputState::new(window, cx).placeholder("feature/my-change"));
            let location = cx.new(|cx| InputState::new(window, cx).placeholder("Path"));
            let suggested = crate::state::suggest_worktree_path(&repo, "");
            location.update(cx, |i, cx| i.set_value(suggested, window, cx));
            let subs = vec![
                cx.subscribe_in(&name, window, |v: &mut Self, s, e: &InputEvent, w, cx| match e {
                    InputEvent::Change => {
                        if !v.path_edited {
                            let p = crate::state::suggest_worktree_path(&v.repo, &s.read(cx).value());
                            v.location.update(cx, |i, cx| i.set_value(p, w, cx));
                        }
                        cx.notify()
                    }
                    InputEvent::PressEnter { .. } => v.create(cx),
                    _ => {}
                }),
                cx.subscribe(&location, |v: &mut Self, s, e: &InputEvent, cx| if matches!(e, InputEvent::Change) {
                    let want = crate::state::suggest_worktree_path(&v.repo, &v.name.read(cx).value());
                    if s.read(cx).value().as_ref() != want { v.path_edited = true }
                    cx.notify()
                }),
            ];
            NewWorktree { app: a, repo, new_branch: true, name, location, path_edited: false, base: commit.clone(), commit, branches,
                          error: None, working: false, focus: cx.focus_handle(), _subs: subs }
        });
        v.update(cx, |v, cx| v.name.update(cx, |t, cx| t.focus(window, cx)));
        app.update(cx, |a, cx| a.present(v.into(), cx));
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let (name, path) = (self.name.read(cx).value().trim().to_string(), self.location.read(cx).value().trim().to_string());
        if name.is_empty() || path.is_empty() || self.working { return }
        self.working = true;
        self.error = None;
        cx.notify();
        let base = if self.new_branch { self.base.clone() } else { String::new() };
        let t = self.app.update(cx, |a, cx| a.add_worktree(self.repo.clone(), path, name, base, self.new_branch, cx));
        cx.spawn(async move |this, cx| {
            let r = t.await;
            this.update(cx, |v, cx| {
                v.working = false;
                match r { Ok(()) => v.app.update(cx, |a, cx| a.dismiss(cx)), Err(e) => { v.error = Some(e); cx.notify() } }
            }).ok();
        }).detach();
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = std::path::Path::new(&self.repo).parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let name = std::path::Path::new(&self.location.read(cx).value().to_string()).file_name().map(|n| n.to_string_lossy().into_owned());
        let rx = cx.prompt_for_new_path(&dir, name.as_deref());
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(p))) = rx.await {
                this.update_in(cx, |v, w, cx| {
                    v.path_edited = true;
                    v.location.update(cx, |i, cx| i.set_value(p.to_string_lossy().into_owned(), w, cx));
                }).ok();
            }
        }).detach();
    }
}

impl Render for NewWorktree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let ready = !self.name.read(cx).value().trim().is_empty() && !self.location.read(cx).value().trim().is_empty() && !self.working;
        let base_label = if self.base == self.commit { self.commit[..self.commit.len().min(10)].to_string() } else { self.base.clone() };
        let (commit, branches, m2) = (self.commit.clone(), self.branches.clone(), me.clone());
        let label = |t: &str| div().w(px(90.)).flex_none().text_color(theme::text_muted()).child(t.to_string());
        let field = |e: &Entity<InputState>| div().flex_1().h(px(28.)).px_2().flex().items_center().rounded(px(6.)).bg(theme::canvas())
            .border_1().border_color(theme::border_strong()).child(Input::new(e).appearance(false).small());
        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, _, cx| if e.keystroke.key == "escape" { v.app.update(cx, |a, cx| a.dismiss(cx)) }))
            .child(ui::card().w(px(480.)).bg(theme::raised())
                .child(ui::card_header("New Worktree", ""))
                .child(div().flex().flex_col().gap_3().p_4().text_size(px(12.5))
                    .child(div().flex().child(ui::segmented("nw-kind", &["New branch", "Existing branch"], usize::from(!self.new_branch), {
                        let me = me.clone();
                        move |i, _, cx| me.update(cx, |v, cx| { v.new_branch = i == 0; cx.notify() })
                    })))
                    .child(div().flex().items_center().gap_2().child(label("Name")).child(field(&self.name)))
                    .when(self.new_branch, |d| d.child(div().flex().items_center().gap_2().child(label("Based on"))
                        .child(Button::new("nw-base").label(base_label).small().outline().dropdown_caret(true).selected(false)
                            .dropdown_menu(move |mut menu, _, _| {
                                let pick = |b: String| { let m = m2.clone(); move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| m.update(cx, |v, cx| { v.base = b.clone(); cx.notify() }) };
                                menu = menu.item(PopupMenuItem::new("HEAD").on_click(pick("HEAD".into())))
                                    .item(PopupMenuItem::new(commit[..commit.len().min(10)].to_string()).on_click(pick(commit.clone())))
                                    .separator();
                                for b in &branches { menu = menu.item(PopupMenuItem::new(b.clone()).on_click(pick(b.clone()))) }
                                menu.scrollable(true)
                            }))))
                    .child(div().flex().items_center().gap_2().child(label("Location")).child(field(&self.location))
                        .child(ui::pill("nw-choose", "Choose…").on_click(cx.listener(|v, _, w, cx| v.choose(w, cx)))))
                    .when_some(self.error.clone(), |d, e| d.child(div().text_color(theme::red()).child(e))))
                .child(div().flex().justify_end().gap_2().p_3().border_t_1().border_color(theme::border())
                    .child(ui::pill("nw-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.app.update(cx, |a, cx| a.dismiss(cx)))))
                    .child(ui::pill_primary("nw-create", if self.working { "Creating…" } else { "Create" }, theme::blue())
                        .when(!ready, |d| d.opacity(0.4))
                        .when(ready, |d| d.on_click(cx.listener(|v, _, _, cx| v.create(cx)))))))
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
