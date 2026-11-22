//! The settings window (Settings.swift): General, Editor, Spaces, Shortcuts. Owner: the settings view agent
//! (docs/gpui/VIEWS.md). Spaces edit `AppModel` (saved by it), shortcuts the `Shortcuts` global (saved by
//! `keys::install`); `settingsTab` and `appearance` are saved here.
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, Global, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, WindowHandle, div, prelude::FluentBuilder as _, px,
    component::{Sizable as _, TitleBar, input::{Input, InputEvent, InputState}},
};

use crate::shell::keys::{self, Shortcut, ShortcutAction, Shortcuts};
use crate::shell::model::{last_component, tilde};
use crate::state::AppModel;
use crate::{theme, ui};
use loupe_forge::settings as prefs;

/// The shown tab and the General tab's appearance: the "settingsTab" and "appearance" settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsState {
    pub tab: String,        // "general" | "editor" | "spaces" | "shortcuts"
    pub appearance: String, // "system" | "light" | "dark"
}

impl Default for SettingsState {
    fn default() -> Self { Self { tab: "spaces".into(), appearance: "system".into() } }
}

impl SettingsState {
    pub fn load() -> Self {
        let p = prefs::shared();
        let d = Self::default();
        Self { tab: p.string("settingsTab").unwrap_or(d.tab), appearance: p.string("appearance").unwrap_or(d.appearance) }
    }
    pub fn save(&self) {
        let mut p = prefs::shared();
        p.set_string("settingsTab", &self.tab);
        p.set_string("appearance", &self.appearance);
    }
}

struct SettingsWindow(WindowHandle<gpui_kit::component::Root>);
impl Global for SettingsWindow {}

/// ⌘, : brings the settings window forward, opening it the first time.
pub fn open(workspace: Entity<AppModel>, cx: &mut App) {
    if let Some(h) = cx.try_global::<SettingsWindow>().map(|w| w.0) {
        if h.update(cx, |_, w, _| w.activate_window()).is_ok() { return }
    }
    use gpui_kit::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, size};
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Settings".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(15.))) }),
        app_owns_titlebar_drag: true,
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(720.), px(640.)), cx))),
        is_resizable: false,
        ..Default::default()
    };
    if let Ok((h, _)) = gpui_kit::open_window(options, cx, |w, cx| cx.new(|cx| Settings::new(workspace, w, cx))) {
        if let Some(h) = h.downcast::<gpui_kit::component::Root>() { cx.set_global(SettingsWindow(h)) }
    }
}

pub struct Settings {
    pub state: SettingsState,
    workspace: Entity<AppModel>,
    selected_space: Option<String>,
    deleting: Option<String>,
    space_name: Entity<InputState>,
    recording: Option<ShortcutAction>,
    note: Option<String>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Settings {
    fn new(workspace: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected = Some(workspace.read(cx).current_space.clone());
        let space_name = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            // The name field writes through as you type, like the Swift TextField bound to the space.
            cx.subscribe(&space_name, |s: &mut Self, input, e: &InputEvent, cx| if matches!(e, InputEvent::Change) {
                let (name, id) = (input.read(cx).value().trim().to_string(), s.selected_space.clone());
                if let (false, Some(id)) = (name.is_empty(), id) {
                    s.workspace.update(cx, |w, cx| if let Some(sp) = w.spaces.iter_mut().find(|x| x.id == id) { sp.name = name; cx.notify() });
                }
            }),
        ];
        let mut s = Self { state: SettingsState::load(), workspace, selected_space: selected, deleting: None, space_name,
                           recording: None, note: None, focus: cx.focus_handle(), _subs: subs };
        // Focused from the start, so ⌘W and the other window actions reach this view.
        s.focus.focus(window, cx);
        s.load_space_name(window, cx);
        s
    }

    fn load_space_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.selected_space.as_ref().and_then(|id| self.workspace.read(cx).spaces.iter().find(|s| &s.id == id)).map(|s| s.name.clone()).unwrap_or_default();
        self.space_name.update(cx, |i, cx| i.set_value(name, window, cx));
    }

    /// While recording: Esc cancels, a bare ⌫ removes the shortcut, anything else that can be one sets it.
    fn record_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(a) = self.recording else { return };
        cx.stop_propagation();
        let k = &e.keystroke;
        let bare = !(k.modifiers.platform || k.modifiers.shift || k.modifiers.alt || k.modifiers.control);
        if k.key == "escape" { self.recording = None; cx.notify(); return }
        let s = if k.key == "backspace" && bare { None } else {
            match Shortcut::from_keystroke(k) { Some(s) => Some(s), None => return } // not a shortcut: keep listening
        };
        let taken = cx.global_mut::<Shortcuts>().set(s.clone(), a);
        self.note = taken.map(|t| format!("{} was used by “{}”, which now has no shortcut.", s.map(|s| s.display()).unwrap_or_default(), t.title()));
        self.recording = None;
        keys::install(cx);
        cx.notify();
    }

    fn general(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = ["system", "light", "dark"];
        let sel = modes.iter().position(|m| *m == self.state.appearance).unwrap_or(0);
        let me = cx.entity();
        div().flex().flex_col().gap_4()
            .child(ui::card()
                .child(ui::card_header("Appearance", ""))
                .child(row("Appearance", ui::segmented("appearance", &["System", "Light", "Dark"], sel,
                    move |i, _, cx| me.update(cx, |s, cx| { s.state.appearance = modes[i].into(); s.state.save(); cx.notify() })))))
            .child(ui::card()
                .child(ui::card_header("Commit Messages", ui::link_button("look-again", "Look Again")))
                .child(div().p_3().text_color(theme::text_muted()).child("Looking for AI tools…")))
            .child(ui::card()
                .child(ui::card_header("Updates", ""))
                .child(row("Check automatically", div().text_color(theme::text_muted()).child("On"))))
    }

    fn spaces(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (spaces, current) = { let ws = self.workspace.read(cx); (ws.spaces.clone(), ws.current_space.clone()) };
        let sel = self.selected_space.clone();
        let detail = sel.as_ref().and_then(|id| spaces.iter().find(|s| &s.id == id)).cloned();
        let can_delete = sel.is_some() && spaces.len() > 1;
        div().flex().size_full().gap_4()
            .child(ui::card().w(px(200.)).flex_none()
                .child(div().flex().flex_col().flex_1().p_1().gap_px().children(spaces.iter().map(|sp| {
                    let id = sp.id.clone();
                    ui::list_row(SharedString::from(format!("sp-{id}")), sel.as_deref() == Some(&sp.id))
                        .child(div().flex_1().truncate().child(sp.name.clone()))
                        .when(sp.id == current, |d| d.child(ui::icon("check").size(px(12.))))
                        .on_click(cx.listener(move |s, _, w, cx| { s.selected_space = Some(id.clone()); s.deleting = None; s.load_space_name(w, cx); cx.notify() }))
                })))
                .child(div().flex().items_center().gap_1().p_1().border_t_1().border_color(theme::border())
                    .child(ui::icon_button("space-add", "plus").on_click(cx.listener(|s, _, w, cx| {
                        let id = s.workspace.update(cx, |ws, cx| ws.add_space(cx));
                        s.selected_space = Some(id); s.load_space_name(w, cx);
                    })))
                    .child(ui::icon_button("space-delete", "minus").when(!can_delete, |d| d.opacity(0.35))
                        .when(can_delete, |d| d.on_click(cx.listener(|s, _, _, cx| { s.deleting = s.selected_space.clone(); cx.notify() }))))))
            .child(match detail {
                None => div().flex_1().child(ui::empty_state("layers", "No Space Selected", "")).into_any_element(),
                Some(sp) => {
                    let id = sp.id.clone();
                    div().id("space-detail").flex().flex_col().flex_1().min_w_0().gap_4().overflow_y_scroll()
                        .when(self.deleting.as_deref() == Some(sp.id.as_str()), |d| d.child(ui::card().border_color(theme::red().opacity(0.5)).p_3().gap_2()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Delete space “{}”?", sp.name)))
                            .child(div().text_color(theme::text_muted()).child("Its repositories move to the neighbouring space. Its accounts are signed out and their tokens removed from the Keychain."))
                            .child(div().flex().gap_2().justify_end()
                                .child(ui::pill("del-cancel", "Cancel").on_click(cx.listener(|s, _, _, cx| { s.deleting = None; cx.notify() })))
                                .child(ui::pill_primary("del-ok", "Delete", theme::red()).on_click(cx.listener(move |s, _, w, cx| {
                                    let id = id.clone();
                                    let cur = s.workspace.update(cx, |ws, cx| { ws.delete_space(&id, cx); ws.current_space.clone() });
                                    s.deleting = None; s.selected_space = Some(cur); s.load_space_name(w, cx);
                                }))))))
                        .child(ui::card()
                            .child(row("Name", div().w(px(260.)).child(Input::new(&self.space_name).small())))
                            .when(sp.id != current, |d| {
                                let id = sp.id.clone();
                                let w = self.workspace.clone();
                                d.child(row("", ui::pill("switch-space", "Switch to This Space").on_click(move |_, _, cx| w.update(cx, |w, cx| w.switch_space_to(&id, cx)))))
                            }))
                        .child(ui::card()
                            .child(ui::card_header("Accounts", ui::link_button("add-account", "Add Account…")))
                            .child(div().p_3().text_color(theme::text_muted()).child("No accounts. Pull requests, checks and HTTPS push and fetch need one for the repo's server."))
                            .child(ui::card_footer("Used for every repository in this space. Adding an account for a server that already has one replaces it.")))
                        .child(ui::card()
                            .child(ui::card_header("Repositories", ""))
                            .when(sp.repos.is_empty(), |d| d.child(div().p_3().text_color(theme::text_muted()).child("No repositories. Open one from the sidebar while this space is shown.")))
                            .child(div().flex().flex_col().p_1().children(sp.repos.iter().enumerate().map(|(i, repo)| {
                                let (r, w) = (repo.clone(), self.workspace.clone());
                                ui::list_row(("repo", i), false).h(px(40.))
                                    .child(ui::icon("folder"))
                                    .child(div().flex().flex_col().flex_1().min_w_0()
                                        .child(div().truncate().text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(last_component(repo).to_string()))
                                        .child(div().truncate().text_size(px(11.)).text_color(theme::text_faint()).child(tilde(repo))))
                                    .child(ui::pill(("remove", i), "Remove").on_click(move |_, _, cx| w.update(cx, |w, cx| w.remove_repo(&r, cx))))
                            }))))
                        .into_any_element()
                }
            })
    }

    fn shortcuts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sc = cx.global::<Shortcuts>();
        let mut out = div().flex().flex_col().gap_4();
        for group in ShortcutAction::GROUPS {
            let mut card = ui::card().child(ui::card_header(group, ""));
            for a in ShortcutAction::ALL.into_iter().filter(|a| a.group() == group) {
                let s = sc.shortcut(a);
                let recording = self.recording == Some(a);
                let custom = sc.is_custom(a);
                card = card.child(row(a.title(), div().flex().items_center().gap_1p5()
                    .when(custom, |d| d.child(ui::icon_button(SharedString::from(format!("restore-{}", a.id())), "undo-2")
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(format!("Restore {}", a.standard().map(|s| s.display()).unwrap_or("no shortcut".into()))).build(w, cx))
                        .on_click(cx.listener(move |s, _, _, cx| { cx.global_mut::<Shortcuts>().set(a.standard(), a); s.note = None; keys::install(cx); cx.notify() }))))
                    .child(div().id(SharedString::from(format!("rec-{}", a.id()))).min_w(px(110.)).h(px(26.)).px_2().flex().items_center().justify_center()
                        .rounded(px(6.)).border_1().cursor_pointer().font_family(theme::CODE_FONT).text_size(px(12.))
                        .map(|d| if recording { d.border_color(theme::blue()).text_color(theme::blue()) }
                                 else { d.border_color(theme::border_strong()).text_color(if s.is_some() { theme::text() } else { theme::text_faint() }) })
                        .hover(|d| d.bg(theme::hover()))
                        .on_click(cx.listener(move |s, _, w, cx| {
                            s.recording = if s.recording == Some(a) { None } else { Some(a) };
                            s.focus.focus(w, cx);
                            cx.notify();
                        }))
                        .child(if recording { "Type shortcut…".to_string() } else { s.map(|s| s.display()).unwrap_or("None".into()) }))));
            }
            out = out.child(card);
        }
        out.child(div().flex().items_center().gap_2()
                .children(self.note.clone().map(|n| div().flex_1().text_size(px(12.)).text_color(theme::accent()).child(n)))
                .child(div().flex_1())
                .child(ui::pill("restore-all", "Restore Defaults").on_click(cx.listener(|s, _, _, cx| { cx.global_mut::<Shortcuts>().reset(); s.note = None; keys::install(cx); cx.notify() }))))
            .child(div().text_size(px(12.)).text_color(theme::text_faint()).child("Click a shortcut, then press the new keys. Esc cancels, ⌫ removes the shortcut."))
    }
}

/// A label on the left, its control on the right: a grouped form row.
fn row(label: &str, control: impl IntoElement) -> impl IntoElement {
    div().flex().items_center().gap_3().px_3().min_h(px(40.)).border_b_1().border_color(theme::border())
        .child(div().flex_1().text_color(theme::text()).child(label.to_string()))
        .child(control)
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [("general", "General"), ("editor", "Editor"), ("spaces", "Spaces"), ("shortcuts", "Shortcuts")];
        let sel = tabs.iter().position(|t| t.0 == self.state.tab).unwrap_or(2);
        let me = cx.entity();
        let body = match self.state.tab.as_str() {
            "general" => self.general(cx).into_any_element(),
            "editor" => ui::empty_state("code", "Editor", "Fonts, theme and line options live here once the editor settings are ported.").into_any_element(),
            "shortcuts" => self.shortcuts(cx).into_any_element(),
            _ => self.spaces(cx).into_any_element(),
        };
        div().track_focus(&self.focus).flex().flex_col().size_full().bg(theme::sidebar()).text_color(theme::text())
            .font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .capture_key_down(cx.listener(|s, e: &KeyDownEvent, _, cx| s.record_key(e, cx)))
            .on_action(|_: &keys::CloseWindow, w, _| w.remove_window())
            .child(TitleBar::new().h(px(42.)).bg(theme::sidebar()).border_b_0()
                .child(div().flex().flex_1().justify_center().pr(px(80.))
                    .child(ui::segmented("settings-tabs", &tabs.map(|t| t.1), sel,
                        move |i, _, cx| me.update(cx, |s, cx| { s.state.tab = tabs[i].0.into(); s.state.save(); s.recording = None; cx.notify() })))))
            .child(div().id("settings-body").flex_1().min_h_0().overflow_y_scroll().p_5().bg(theme::canvas())
                .border_t_1().border_color(theme::border()).child(body))
    }
}
