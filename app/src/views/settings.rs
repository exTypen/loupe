//! The settings window (Settings.swift): General, Editor, Spaces, Shortcuts. Owner: the settings view agent
//! (docs/gpui/VIEWS.md). Spaces and accounts edit `AppModel` (saved by it), shortcuts the `Shortcuts` global (saved by
//! `keys::install`), the editor its Swift keys (editor.rs); `settingsTab`, `appearance` and the commit AI keys are
//! saved here.
mod account;
mod editor;

use std::collections::HashMap;
use std::sync::Mutex;

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, Global, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Task, Window, WindowHandle, div, prelude::FluentBuilder as _, px,
    component::{
        Sizable as _, TitleBar,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
        slider::SliderState,
    },
};
use loupe_code::theme::EditorSettings;
use loupe_forge::{account as acct, ai};

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
        // An unknown stored value falls back to System (ST-02).
        let appearance = p.string("appearance").filter(|a| ["system", "light", "dark"].contains(&a.as_str())).unwrap_or(d.appearance);
        Self { tab: p.string("settingsTab").unwrap_or(d.tab), appearance }
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

/// Model listings per harness id: a CLI takes up to 20 s to list them, so once per launch is enough.
static MODELS: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

pub struct Settings {
    pub state: SettingsState,
    workspace: Entity<AppModel>,
    selected_space: Option<String>,
    deleting: Option<String>,
    space_name: Entity<InputState>,
    recording: Option<ShortcutAction>,
    note: Option<String>,
    focus: FocusHandle,
    // General › Commit Messages
    /// harness id → executable; None while scanning.
    ai_paths: Option<HashMap<String, String>>,
    scan: Option<Task<()>>,
    /// The harness the Model field and list were loaded for ("" for none).
    ai_key: String,
    models: Vec<String>,
    models_task: Option<Task<()>>,
    model_input: Entity<InputState>,
    instructions: Entity<TextareaState>,
    // Editor
    editor: EditorSettings,
    line_slider: Entity<SliderState>,
    // Spaces › Add Account…
    adding: Option<account::AddAccount>,
    _subs: Vec<Subscription>,
}

impl Settings {
    fn new(workspace: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected = Some(workspace.read(cx).current_space.clone());
        let space_name = cx.new(|cx| InputState::new(window, cx));
        let model_input = cx.new(|cx| InputState::new(window, cx));
        let instr = prefs::shared().string("aiInstructions").unwrap_or_default();
        let instructions = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 3)
            .placeholder("e.g. Write in Turkish. Mention ticket numbers.").default_value(instr));
        let subs = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            // The name field writes through as you type, like the Swift TextField bound to the space. An empty or
            // blank name isn't stored (FLOWS §14: the sidebar Rename rejects it too).
            cx.subscribe(&space_name, |s: &mut Self, input, e: &InputEvent, cx| if matches!(e, InputEvent::Change) {
                let (name, id) = (input.read(cx).value().trim().to_string(), s.selected_space.clone());
                if let (false, Some(id)) = (name.is_empty(), id) {
                    s.workspace.update(cx, |w, cx| if let Some(sp) = w.spaces.iter_mut().find(|x| x.id == id) { sp.name = name; cx.notify() });
                }
            }),
            // Every edit or submit saves the trimmed model (ST-05).
            cx.subscribe(&model_input, |s: &mut Self, input, e: &InputEvent, cx| if matches!(e, InputEvent::Change | InputEvent::PressEnter { .. }) {
                if let Some(h) = ai::HARNESSES.iter().find(|h| h.id == s.ai_key) { ai::set_model(h, &input.read(cx).value(), &mut prefs::shared()) }
            }),
            cx.subscribe(&instructions, |_, input, e: &InputEvent, cx| if matches!(e, InputEvent::Change) {
                prefs::shared().set_string("aiInstructions", &input.read(cx).value());
            }),
        ];
        let editor = editor::load();
        let line_slider = editor::line_slider(editor.line_height_factor, cx);
        let mut s = Self {
            state: SettingsState::load(), workspace, selected_space: selected, deleting: None, space_name,
            recording: None, note: None, focus: cx.focus_handle(),
            ai_paths: ai::installed(), scan: None, ai_key: String::new(), models: vec![], models_task: None, model_input, instructions,
            editor, line_slider, adding: None, _subs: subs,
        };
        if s.ai_paths.is_none() { s.look_again(cx) }
        // Focused from the start, so ⌘W and the other window actions reach this view.
        s.focus.focus(window, cx);
        s.load_space_name(window, cx);
        s
    }

    fn load_space_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.selected_space.as_ref().and_then(|id| self.workspace.read(cx).spaces.iter().find(|s| &s.id == id)).map(|s| s.name.clone()).unwrap_or_default();
        self.space_name.update(cx, |i, cx| i.set_value(name, window, cx));
    }

    // MARK: General

    /// AIHarnesses.scan(): the CLIs are looked up off the main thread (a login shell may run).
    fn look_again(&mut self, cx: &mut Context<Self>) {
        self.ai_paths = None;
        self.ai_key = "\0".into(); // reload the model field after the scan
        self.scan = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async { ai::scan() }).await;
            this.update(cx, |s, cx| { s.ai_paths = Some(found); cx.notify() }).ok();
        }));
        cx.notify();
    }

    /// The Model field and list follow the selected harness (the Swift `.task(id:)`): its saved model, its built-in
    /// list, then the CLI's own listing where it has one.
    fn sync_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(paths) = &self.ai_paths else { return };
        let sel = ai::selected(&prefs::shared(), paths);
        let key = sel.as_ref().map_or("", |s| s.harness.id);
        if key == self.ai_key { return }
        self.ai_key = key.into();
        self.models_task = None;
        let Some(sel) = sel else { self.models = vec![]; return };
        self.model_input.update(cx, |i, cx| { i.set_value(sel.model.clone(), window, cx); i.set_placeholder(format!("{} default", sel.harness.name), window, cx) });
        if let Some(m) = MODELS.lock().unwrap().as_ref().and_then(|c| c.get(key)).cloned() { self.models = m; return }
        self.models = sel.harness.models.iter().map(|m| m.id.to_string()).collect();
        let (h, path) = (sel.harness, sel.path.clone());
        self.models_task = Some(cx.spawn(async move |this, cx| {
            let m = cx.background_executor().spawn(async move { ai::available_models(h, &path) }).await;
            MODELS.lock().unwrap().get_or_insert_default().insert(h.id.into(), m.clone());
            this.update(cx, |s, cx| if s.ai_key == h.id { s.models = m; cx.notify() }).ok();
        }));
    }

    /// Leaving a tab stops recording, closes its sheet and drops a model listing still running (reloaded on return).
    fn leave_tab(&mut self) {
        self.recording = None;
        self.adding = None;
        if self.models_task.take().is_some() { self.ai_key = "\0".into() }
    }

    fn set_pref(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        prefs::shared().set_string(key, value);
        cx.notify();
    }

    fn general(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_ai(window, cx);
        let modes = ["system", "light", "dark"];
        let sel = modes.iter().position(|m| *m == self.state.appearance).unwrap_or(0);
        let me = cx.entity();
        let p = prefs::shared();
        let (harness, style) = (ai::harness_id(&p), ai::style(&p));
        let current = self.ai_paths.as_ref().and_then(|paths| ai::selected(&p, paths));
        drop(p);
        let detected: Vec<&'static ai::Harness> = self.ai_paths.as_ref()
            .map(|paths| ai::HARNESSES.iter().filter(|h| paths.contains_key(h.id)).collect()).unwrap_or_default();

        let mut commit = ui::card().child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child("Commit Messages"),
            ui::link_button("look-again", "Look Again").text_size(px(11.)).map(|b| if self.ai_paths.is_none() { b.opacity(0.4) }
                else { b.on_click(cx.listener(|s, _, _, cx| s.look_again(cx))) })));
        if self.ai_paths.is_none() {
            commit = commit.child(div().flex().items_center().gap_2().p_3().text_color(theme::text_muted())
                .child(ui::icon("loader-circle")).child("Looking for AI tools…"));
        } else if detected.is_empty() {
            commit = commit.child(div().p_3().text_color(theme::text_muted()).child(format!("No AI coding tool found. Install one of: {}.",
                ai::HARNESSES.iter().map(|h| h.name).collect::<Vec<_>>().join(", "))));
        } else {
            let first = detected[0].name;
            let label = match harness.as_str() {
                ai::AUTO => format!("Automatic ({first})"),
                ai::OFF => "Off".into(),
                id => ai::HARNESSES.iter().find(|h| h.id == id).map_or(id.to_string(), |h| h.name.into()),
            };
            let write_with = {
                let (me, cur, det) = (me.clone(), harness.clone(), detected.clone());
                picker("write-with", label, move |mut m, _, _| {
                    let set = |v: &'static str| { let me = me.clone(); move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| me.update(cx, |s, cx| s.set_pref("aiHarness", v, cx)) };
                    m = m.item(PopupMenuItem::new(format!("Automatic ({first})")).checked(cur == ai::AUTO).on_click(set(ai::AUTO)));
                    for h in &det { m = m.item(PopupMenuItem::new(h.name).checked(cur == h.id).on_click(set(h.id))) }
                    m.separator().item(PopupMenuItem::new("Off").checked(cur == ai::OFF).on_click(set(ai::OFF)))
                })
            };
            commit = commit.child(row("Found on this Mac", div().text_color(theme::text_muted()).child(detected.iter().map(|h| h.name).collect::<Vec<_>>().join(", "))))
                .child(row("Write with", write_with));
            if let Some(cur) = &current {
                let h = cur.harness;
                let menu = (!self.models.is_empty()).then(|| {
                    let (models, input) = (self.models.clone(), self.model_input.clone());
                    Button::new("models").ghost().small().icon(gpui_kit::component::Icon::empty().path("icons/chevrons-up-down.svg"))
                        .dropdown_menu(move |mut m, _, _| {
                            let set = |v: String| { let input = input.clone(); move |_: &gpui_kit::ClickEvent, w: &mut Window, cx: &mut App| {
                                input.update(cx, |i, cx| i.set_value(v.clone(), w, cx));
                                ai::set_model(h, &v, &mut prefs::shared());
                            } };
                            m = m.item(PopupMenuItem::new(format!("{} default", h.name)).on_click(set(String::new()))).separator();
                            for id in &models {
                                let label = h.models.iter().find(|o| o.id == id).filter(|o| o.label != o.id).map_or(id.clone(), |o| format!("{}   {}", o.label, o.id));
                                m = m.item(PopupMenuItem::new(label).on_click(set(id.clone())));
                            }
                            m.max_h(px(360.)).scrollable(true)
                        })
                });
                let style_picker = {
                    let me = me.clone();
                    picker("style", style.title(), move |mut m, _, _| {
                        for s in ai::CommitStyle::ALL {
                            let me = me.clone();
                            m = m.item(PopupMenuItem::new(s.title()).checked(s == style).on_click(move |_, _, cx| me.update(cx, |v, cx| v.set_pref("aiStyle", s.id(), cx))));
                        }
                        m
                    })
                };
                commit = commit
                    .child(row("Model", div().flex().items_center().gap_1().child(div().w(px(220.)).child(Input::new(&self.model_input).small())).children(menu)))
                    .child(row("Style", style_picker))
                    .child(div().flex().items_start().gap_3().px_3().py_2()
                        .child(div().flex_1().pt_1().child("Extra instructions"))
                        .child(div().w(px(320.)).child(Textarea::new(&self.instructions).small())))
                    .child(foot(format!("{} The ✨ button next to the summary sends the change and recent commit subjects to {} in its read-only mode, on your own subscription.", style.summary(), h.name)));
            }
        }

        div().flex().flex_col().gap_4()
            .child(ui::card()
                .child(row("Appearance", ui::segmented("appearance", &["System", "Light", "Dark"], sel,
                    move |i, _, cx| me.update(cx, |s, cx| { s.state.appearance = modes[i].into(); s.state.save(); cx.notify() }))))
                .child(foot("Saved for every Loupe. This build draws only its dark look, whatever you pick.")))
            .child(commit)
            .child(ui::card()
                .child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child("Updates"), ""))
                .child(row("Version", div().text_color(theme::text_muted()).child(env!("CARGO_PKG_VERSION"))))
                .child(row("", div().opacity(0.4).child(ui::pill("check-updates", "Check for Updates…").cursor_default())))
                .child(foot("This build has no updater: Sparkle is part of the Swift app only. Install new versions by hand.")))
    }

    // MARK: Spaces

    /// "+": a new space, selected in the list, without switching the main window to it (FLOWS §14: Swift's
    /// `addSpace()` switched it), named "Space N" with an N no other space uses (§14: it could duplicate).
    fn new_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.workspace.update(cx, |ws, cx| {
            let id = ws.add_space_without_switch();
            let name = unique_space_name(ws.spaces.iter().filter(|s| s.id != id).map(|s| s.name.as_str()), ws.spaces.len());
            if let Some(sp) = ws.spaces.iter_mut().find(|s| s.id == id) { sp.name = name }
            cx.notify();
            id
        });
        self.selected_space = Some(id);
        self.load_space_name(window, cx);
    }

    fn spaces(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (spaces, current, accounts, tokens_loaded, tokens) = {
            let ws = self.workspace.read(cx);
            (ws.spaces.clone(), ws.current_space.clone(), ws.accounts.clone(), ws.tokens_loaded, ws.tokens.keys().cloned().collect::<Vec<_>>())
        };
        let sel = self.selected_space.clone();
        let detail = sel.as_ref().and_then(|id| spaces.iter().find(|s| &s.id == id)).cloned();
        let can_delete = sel.is_some() && spaces.len() > 1;
        div().flex().size_full().gap_4()
            .child(ui::card().w(px(200.)).flex_none()
                .child(div().flex().flex_col().flex_1().p_1().gap_px().children(spaces.iter().map(|sp| {
                    let id = sp.id.clone();
                    let drop_id = sp.id.clone();
                    ui::list_row(SharedString::from(format!("sp-{id}")), sel.as_deref() == Some(&sp.id))
                        .child(div().flex_1().truncate().child(sp.name.clone()))
                        .when(sp.id == current, |d| d.child(div().id(SharedString::from(format!("cur-{id}"))).child(ui::icon("check").size(px(12.)))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Current space").build(w, cx))))
                        // Drag to reorder (the Swift List's onMove); the sidebar dots follow.
                        .on_drag(SpaceDrag(sp.id.clone(), sp.name.clone()), |d, _, _, cx| cx.new(|_| d.clone()))
                        .drag_over::<SpaceDrag>(|s, _, _, _| s.border_t_2().border_color(theme::blue()))
                        .on_drop(cx.listener(move |s, d: &SpaceDrag, _, cx| s.workspace.update(cx, |ws, cx| {
                            move_space(&mut ws.spaces, &d.0, &drop_id);
                            cx.notify();
                        })))
                        .on_click(cx.listener(move |s, _, w, cx| { s.selected_space = Some(id.clone()); s.deleting = None; s.load_space_name(w, cx); cx.notify() }))
                })))
                .child(div().flex().items_center().gap_1().p_1().border_t_1().border_color(theme::border())
                    .child(ui::icon_button("space-add", "plus").tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("New Space").build(w, cx))
                        .on_click(cx.listener(|s, _, w, cx| s.new_space(w, cx))))
                    .child(ui::icon_button("space-delete", "minus").tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Delete Space…").build(w, cx))
                        .when(!can_delete, |d| d.opacity(0.35))
                        .when(can_delete, |d| d.on_click(cx.listener(|s, _, _, cx| { s.deleting = s.selected_space.clone(); cx.notify() }))))))
            .child(match detail {
                None => div().flex_1().child(ui::empty_state("layers", "No Space Selected", "")).into_any_element(),
                Some(sp) => {
                    let id = sp.id.clone();
                    let space_accounts: Vec<acct::Account> = acct::accounts_in(&accounts, &sp.id).cloned().collect();
                    let others: Vec<(String, String)> = spaces.iter().filter(|o| o.id != sp.id).map(|o| (o.id.clone(), o.name.clone())).collect();
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
                            .child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child("Accounts"), ui::pill("add-account", "Add Account…").h(px(24.))
                                .on_click(cx.listener({ let id = sp.id.clone(); move |s, _, w, cx| { s.adding = Some(account::AddAccount::new(id.clone(), w, cx)); cx.notify() } }))))
                            .when(space_accounts.is_empty(), |d| d.child(div().p_3().text_color(theme::text_muted()).child("No accounts. Pull requests, checks and HTTPS push and fetch need one for the repo's server.")))
                            .children(space_accounts.into_iter().enumerate().map(|(i, a)| {
                                let (aid, w) = (a.id(), self.workspace.clone());
                                let missing = tokens_loaded && !tokens.contains(&aid);
                                div().flex().items_center().gap_3().px_3().h(px(48.)).border_b_1().border_color(theme::border())
                                    .child(ui::icon(if a.is_github() { "github" } else { "server" }).size(px(16.)))
                                    .child(div().flex().flex_col().flex_1().min_w_0()
                                        .child(div().font_weight(FontWeight::MEDIUM).child(a.username.clone()))
                                        .child(div().truncate().text_size(px(11.)).text_color(theme::text_muted()).child(a.base_url.clone())))
                                    .when(missing, |d| d.child(div().flex().items_center().gap_1().text_size(px(11.)).text_color(theme::accent())
                                        .child(ui::icon("triangle-alert").size(px(12.)).text_color(theme::accent())).child("No token")))
                                    .child(ui::pill(("remove-account", i), "Remove").text_color(theme::red())
                                        .on_click(move |_, _, cx| w.update(cx, |w, cx| w.remove_account(&aid, cx))))
                            }))
                            .child(foot("Used for every repository in this space. Adding an account for a server that already has one replaces it.")))
                        .child(ui::card()
                            .child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child("Repositories"), ""))
                            .when(sp.repos.is_empty(), |d| d.child(div().p_3().text_color(theme::text_muted()).child("No repositories. Open one from the sidebar while this space is shown.")))
                            .child(div().flex().flex_col().p_1().children(sp.repos.iter().enumerate().map(|(i, repo)| {
                                let (r, w) = (repo.clone(), self.workspace.clone());
                                let move_menu = (!others.is_empty()).then(|| {
                                    let (others, r, w) = (others.clone(), repo.clone(), self.workspace.clone());
                                    Button::new(("move-to", i)).label("Move to").small().outline().dropdown_caret(true).dropdown_menu(move |mut m, _, _| {
                                        for (id, name) in &others {
                                            let (id, r, w) = (id.clone(), r.clone(), w.clone());
                                            m = m.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| w.update(cx, |w, cx| w.move_repos(&[r.clone()], &id, cx))));
                                        }
                                        m
                                    })
                                });
                                ui::list_row(("repo", i), false).h(px(40.))
                                    .child(ui::icon("folder"))
                                    .child(div().flex().flex_col().flex_1().min_w_0()
                                        .child(div().truncate().text_color(theme::text()).font_weight(FontWeight::MEDIUM).child(last_component(repo).to_string()))
                                        .child(div().truncate().text_size(px(11.)).text_color(theme::text_faint()).child(tilde(repo))))
                                    .children(move_menu)
                                    .child(div().id(("remove-tip", i)).tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Remove from Loupe. Nothing on disk changes.").build(w, cx))
                                        .child(ui::pill(("remove", i), "Remove").on_click(move |_, _, cx| w.update(cx, |w, cx| w.remove_repo(&r, cx)))))
                            }))))
                        .into_any_element()
                }
            })
    }

    // MARK: Shortcuts

    /// While recording: Esc cancels, a bare ⌫ removes the shortcut, anything else that can be one sets it. A key the
    /// app's fixed menu items own (⌘Z, ⌘Q, ⌘W, ⌘, …) is refused with a note (FLOWS §14: Swift let it collide).
    fn record_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(a) = self.recording else { return };
        cx.stop_propagation();
        let k = &e.keystroke;
        let bare = !(k.modifiers.platform || k.modifiers.shift || k.modifiers.alt || k.modifiers.control);
        if k.key == "escape" { self.recording = None; cx.notify(); return }
        let s = if k.key == "backspace" && bare { None } else {
            match Shortcut::from_keystroke(k) { Some(s) => Some(s), None => return } // not a shortcut: keep listening
        };
        if let Some(owner) = s.as_ref().and_then(reserved) {
            self.note = Some(format!("{} belongs to “{owner}”. Pick another.", s.map(|s| s.display()).unwrap_or_default()));
            cx.notify();
            return
        }
        self.assign(a, s, cx);
        self.recording = None;
        cx.notify();
    }

    /// Sets a shortcut and says which action lost it. Also used by a row's Restore, which in Swift took the default
    /// away from another action silently (FLOWS §14).
    fn assign(&mut self, a: ShortcutAction, s: Option<Shortcut>, cx: &mut Context<Self>) {
        let taken = cx.global_mut::<Shortcuts>().set(s.clone(), a);
        self.note = taken.map(|t| format!("{} was used by “{}”, which now has no shortcut.", s.map(|s| s.display()).unwrap_or_default(), t.title()));
        keys::install(cx);
    }

    fn shortcuts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sc = cx.global::<Shortcuts>();
        let mut out = div().flex().flex_col().gap_4();
        for group in ShortcutAction::GROUPS {
            let mut card = ui::card().child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child(group), ""));
            for a in ShortcutAction::ALL.into_iter().filter(|a| a.group() == group) {
                let s = sc.shortcut(a);
                let recording = self.recording == Some(a);
                let custom = sc.is_custom(a);
                card = card.child(row(a.title(), div().flex().items_center().gap_1p5()
                    .when(custom, |d| d.child(ui::icon_button(SharedString::from(format!("restore-{}", a.id())), "undo-2")
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(format!("Restore {}", a.standard().map(|s| s.display()).unwrap_or("no shortcut".into()))).build(w, cx))
                        .on_click(cx.listener(move |s, _, _, cx| { s.assign(a, a.standard(), cx); cx.notify() }))))
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

/// The fixed menu shortcuts `keys::install` binds and no action may take, with the item that owns each.
fn reserved(s: &Shortcut) -> Option<&'static str> {
    const FIXED: [(&str, &str); 9] = [
        ("cmd-z", "Undo"), ("shift-cmd-z", "Redo"), ("cmd-,", "Settings…"), ("cmd-q", "Quit Loupe"), ("cmd-h", "Hide Loupe"),
        ("alt-cmd-h", "Hide Others"), ("cmd-w", "Close Window"), ("cmd-m", "Minimize"), ("ctrl-cmd-f", "Enter Full Screen"),
    ];
    let k = s.keystroke();
    if k.strip_prefix("alt-cmd-").is_some_and(|n| n.len() == 1 && ('1'..='9').contains(&n.chars().next().unwrap_or('0'))) {
        return Some("Select Worktree");
    }
    FIXED.iter().find(|f| f.0 == k).map(|f| f.1)
}

/// "Space N" with the first N ≥ the space count no other space is named.
fn unique_space_name<'a>(names: impl Iterator<Item = &'a str> + Clone, count: usize) -> String {
    (count.max(1)..).map(|n| format!("Space {n}")).find(|c| !names.clone().any(|n| n == c)).unwrap()
}

/// Moves space `from` to where `to` is (before it when dragged up, after it when dragged down, like onMove).
fn move_space(spaces: &mut Vec<loupe_core::model::Space>, from: &str, to: &str) {
    let (Some(i), Some(j)) = (spaces.iter().position(|s| s.id == from), spaces.iter().position(|s| s.id == to)) else { return };
    let s = spaces.remove(i);
    spaces.insert(j, s);
}

/// A space row being dragged, drawn as its name.
#[derive(Clone)]
struct SpaceDrag(String, String);
impl Render for SpaceDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded(px(6.)).bg(theme::raised()).border_1().border_color(theme::border_strong()).text_color(theme::text()).child(self.1.clone())
    }
}

/// A pop-up picker: the current value with a caret, the menu `f` builds.
fn picker(id: &'static str, label: impl Into<SharedString>, f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static) -> impl IntoElement {
    Button::new(id).label(label).small().ghost().dropdown_caret(true).dropdown_menu(move |m, w, cx| f(m, w, cx).min_w(px(200.)))
}

/// A card's caption: grey, wrapping (a form section's footer).
fn foot(text: impl Into<SharedString>) -> gpui_kit::Div {
    div().w_full().px_3().py_2().border_t_1().border_color(theme::border()).text_size(px(12.)).text_color(theme::text_muted()).child(text.into())
}

/// A label on the left, its control on the right: a grouped form row.
fn row(label: &str, control: impl IntoElement) -> impl IntoElement {
    div().flex().items_center().gap_3().px_3().min_h(px(40.)).border_b_1().border_color(theme::border())
        .child(div().flex_1().text_color(theme::text()).child(label.to_string()))
        .child(control)
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [("general", "General"), ("editor", "Editor"), ("spaces", "Spaces"), ("shortcuts", "Shortcuts")];
        let sel = tabs.iter().position(|t| t.0 == self.state.tab).unwrap_or(2);
        let me = cx.entity();
        let body = match self.state.tab.as_str() {
            "general" => self.general(window, cx).into_any_element(),
            "editor" => self.editor_tab(window, cx).into_any_element(),
            "shortcuts" => self.shortcuts(cx).into_any_element(),
            _ => self.spaces(cx).into_any_element(),
        };
        let sheet = self.adding.is_some().then(|| self.add_account_sheet(cx));
        div().track_focus(&self.focus).relative().flex().flex_col().size_full().bg(theme::sidebar()).text_color(theme::text())
            .font_family(theme::UI_FONT).text_size(theme::UI_SIZE)
            .capture_key_down(cx.listener(|s, e: &KeyDownEvent, _, cx| s.record_key(e, cx)))
            .on_key_down(cx.listener(|s, e: &KeyDownEvent, _, cx| if e.keystroke.key == "escape" && s.adding.is_some() { s.adding = None; cx.notify() }))
            .on_action(|_: &keys::CloseWindow, w, _| w.remove_window())
            .child(TitleBar::new().h(px(42.)).bg(theme::sidebar()).border_b_0()
                .child(div().flex().flex_1().justify_center().pr(px(80.))
                    .child(ui::segmented("settings-tabs", &tabs.map(|t| t.1), sel,
                        // Leaving a tab stops recording and closes its sheet (the Swift monitor ends with the view).
                        move |i, _, cx| me.update(cx, |s, cx| { s.state.tab = tabs[i].0.into(); s.state.save(); s.leave_tab(); cx.notify() })))))
            .child(div().id("settings-body").flex_1().min_h_0().overflow_y_scroll().p_5().bg(theme::canvas())
                .border_t_1().border_color(theme::border()).child(body))
            .children(sheet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keys::{COMMAND, OPTION, SHIFT};

    #[test]
    fn space_names_never_repeat() {
        assert_eq!(unique_space_name(["Default"].into_iter(), 2), "Space 2");
        assert_eq!(unique_space_name(["Default", "Space 2"].into_iter(), 2), "Space 3");
    }

    #[test]
    fn spaces_move_like_on_move() {
        let sp = |id: &str| loupe_core::model::Space { id: id.into(), name: id.into(), repos: vec![], selection: None };
        let mut v = vec![sp("a"), sp("b"), sp("c")];
        move_space(&mut v, "c", "a");
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["c", "a", "b"]);
        move_space(&mut v, "c", "b");
        assert_eq!(v.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn fixed_menu_keys_are_reserved() {
        assert_eq!(reserved(&Shortcut::new("z", COMMAND)), Some("Undo"));
        assert_eq!(reserved(&Shortcut::new("z", COMMAND | SHIFT)), Some("Redo"));
        assert_eq!(reserved(&Shortcut::new("3", COMMAND | OPTION)), Some("Select Worktree"));
        assert_eq!(reserved(&Shortcut::new("k", COMMAND)), None);
        assert_eq!(reserved(&Shortcut::new("f", COMMAND | OPTION)), None);
    }

    #[test]
    fn account_urls_are_normalised() {
        assert_eq!(account::normalize(" git.example.com/// "), "https://git.example.com");
        assert_eq!(account::normalize("http://127.0.0.1:8808/"), "http://127.0.0.1:8808");
    }
}
