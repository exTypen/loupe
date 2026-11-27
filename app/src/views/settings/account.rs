//! The Add Account sheet (Settings.swift's AddAccountSheet): GitHub or Forgejo/Gitea, a token checked by asking the
//! server who it belongs to, stored through `AppModel::set_account` (Keychain, except in a scenario run).
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, Task, Window, div, prelude::FluentBuilder as _, px,
    component::{Sizable as _, input::{Input, InputEvent, InputState}},
};
use loupe_forge::{account::{self, Account}, client::Client};

use super::{Settings, foot, row};
use crate::{theme, ui};

pub struct AddAccount {
    space: String,
    github: bool,
    url: Entity<InputState>,
    token: Entity<InputState>,
    status: Option<String>,
    checking: bool,
    /// The token check or `gh auth token` in flight; dropped (its result ignored) when the sheet closes.
    task: Option<Task<()>>,
}

/// What "Add" sends: `https://` when there's no scheme, no trailing slashes.
pub fn normalize(url: &str) -> String {
    let mut base = url.trim().to_string();
    if !base.contains("://") { base = format!("https://{base}") }
    while base.ends_with('/') { base.pop(); }
    base
}

fn url_prompt(github: bool) -> &'static str { if github { "https://github.com" } else { "https://git.example.com" } }
fn token_prompt(github: bool) -> &'static str {
    if github { "Settings → Developer settings → Personal access tokens" } else { "Settings → Applications → Generate token" }
}

impl AddAccount {
    pub fn new(space: String, window: &mut Window, cx: &mut Context<Settings>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder(url_prompt(true)).default_value("https://github.com"));
        let token = cx.new(|cx| InputState::new(window, cx).placeholder(token_prompt(true)).masked(true));
        for i in [&url, &token] {
            cx.subscribe_in(i, window, |s: &mut Settings, _, e: &InputEvent, w, cx| match e {
                InputEvent::PressEnter { .. } => s.add_account(w, cx),
                InputEvent::Change => cx.notify(), // Add's enabled state
                _ => {}
            }).detach();
        }
        url.update(cx, |i, cx| i.focus(window, cx));
        Self { space, github: true, url, token, status: None, checking: false, task: None }
    }
}

impl Settings {
    /// Server switch: the URL, token and status reset (the Swift onChange).
    fn set_server(&mut self, github: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.adding.as_mut() else { return };
        if a.github == github { return }
        a.github = github;
        a.status = None;
        a.task = None;
        a.checking = false;
        a.url.update(cx, |i, cx| { i.set_value(if github { "https://github.com" } else { "" }, window, cx); i.set_placeholder(url_prompt(github), window, cx) });
        a.token.update(cx, |i, cx| { i.set_value("", window, cx); i.set_placeholder(token_prompt(github), window, cx) });
        cx.notify();
    }

    pub(super) fn add_account(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.adding.as_mut() else { return };
        let (url, token) = (a.url.read(cx).value().to_string(), a.token.read(cx).value().to_string());
        if url.is_empty() || token.is_empty() || a.checking { return }
        let (base, github, space) = (normalize(&url), a.github, a.space.clone());
        a.checking = true;
        a.status = Some("Checking…".into());
        a.task = Some(cx.spawn(async move |this, cx| {
            let (b, t) = (base.clone(), token.clone());
            let me = cx.background_executor().spawn(async move { Client::new(&b, &t, github).me() }).await;
            this.update(cx, |s, cx| {
                match me {
                    Ok(me) => {
                        // set_account replaces an account with the same id (same server, same space).
                        let acct = Account::new(&base, &me.login, github, &space);
                        s.workspace.update(cx, |w, cx| w.set_account(acct, Some(token), cx));
                        s.adding = None;
                    }
                    Err(e) => if let Some(a) = s.adding.as_mut() {
                        a.checking = false;
                        a.status = Some(e.user_message().unwrap_or_else(|| e.to_string()));
                    },
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    /// `gh auth token --hostname <host>` off the main thread; fills the token field on success.
    fn use_gh_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.adding.as_ref() else { return };
        let host = account::host_of(&a.url.read(cx).value());
        let task = cx.spawn_in(window, async move |this, cx| {
            let h = host.clone();
            let t = cx.background_executor().spawn(async move { account::gh_token(&h) }).await;
            this.update_in(cx, |s, w, cx| {
                let Some(a) = s.adding.as_mut() else { return };
                match t {
                    Some(t) => { a.token.update(cx, |i, cx| i.set_value(t, w, cx)); a.status = Some("Token read from gh.".into()) }
                    None => a.status = Some(format!("No gh login for {host}. Run `gh auth login` first.")),
                }
                cx.notify();
            }).ok();
        });
        if let Some(a) = self.adding.as_mut() { a.task = Some(task) }
    }

    pub(super) fn add_account_sheet(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let a = self.adding.as_ref().expect("sheet shown only while adding");
        let enabled = !a.url.read(cx).value().is_empty() && !a.token.read(cx).value().is_empty() && !a.checking;
        let github = a.github;
        div().absolute().inset_0().occlude().flex().flex_col().items_center().pt(px(60.)).bg(theme::backdrop())
            .child(ui::card().w(px(460.)).bg(theme::sidebar())
                .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(ui::card_header("Add Account", ""))
                .child(row("Server", ui::segmented("server", &["GitHub", "Forgejo / Gitea"], if github { 0 } else { 1 },
                    { let me = cx.entity(); move |i, w, cx| me.update(cx, |s, cx| s.set_server(i == 0, w, cx)) })))
                .child(row("Server URL", div().w(px(280.)).child(Input::new(&a.url).small())))
                .child(row("Access Token", div().w(px(280.)).child(Input::new(&a.token).small().mask_toggle())))
                .when(github, |d| d.child(row("", div().id("gh-token-tip")
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Reads the token `gh auth login` stored for this host").build(w, cx))
                    .child(ui::pill("gh-token", "Use GitHub CLI Token").on_click(cx.listener(|s, _, w, cx| s.use_gh_token(w, cx)))))))
                .child(foot("Give the token repository read and write access: Loupe uses it for pushes and fetches over HTTPS too."))
                .child(div().flex().items_center().gap_2().p_3()
                    .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(theme::text_muted()).children(a.status.clone()))
                    .child(ui::pill("add-cancel", "Cancel").on_click(cx.listener(|s, _, _, cx| { s.adding = None; cx.notify() })))
                    .child(ui::pill_primary("add-ok", "Add", theme::blue()).when(!enabled, |d| d.opacity(0.4).cursor_default())
                        .when(enabled, |d| d.on_click(cx.listener(|s, _, w, cx| s.add_account(w, cx)))))))
    }
}
