use std::io::Write as _;

use loupe_app::{shell, state, ui};
use loupe_forge::{Account, account, settings};

/// MS-01 `--api <path>`: one GET with the current space's account (else the first), through its own API root
/// (Forgejo /api/v1, GitHub api.github.com or /api/v3 — Swift always used /api/v1, FLOWS.md §14); the raw body goes to
/// stdout. In a LOUPE_STRESS run the scenario's in-memory account answers, so the flows mock can check it.
fn api(path: &str) -> i32 {
    let (acct, token) = match state::scenario::from_env() {
        Some(s) => (Some(Account::new(&s.forge, &s.user, false, account::DEFAULT_SPACE)), Some(s.token)),
        None => {
            let (accts, space) = { let p = settings::shared(); (p.accounts(), p.string("currentSpace").unwrap_or_else(|| account::DEFAULT_SPACE.into())) };
            let a = accts.iter().find(|a| a.space_id() == space).or(accts.first()).cloned();
            let t = a.as_ref().and_then(Account::read_token);
            (a, t)
        }
    };
    let (Some(a), Some(t)) = (acct, token) else { println!("no account/token"); return 1 };
    match a.client(&t).raw("GET", path, &[], None, "application/json", None) {
        Ok(r) => { std::io::stdout().write_all(&r.body).ok(); 0 }
        Err(e) => { eprintln!("{}", e.user_message().unwrap_or_else(|| format!("{e:?}"))); 1 }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // MS-02 `--lens <repo> <revA> <revB|worktree> [<upstream>]`: the change lenses' report, no window.
    if let Some(s) = loupe_app::views::code::overview::lens_cli(&args) { print!("{s}"); std::process::exit(0) }
    if let Some(p) = args.iter().position(|a| a == "--api").and_then(|i| args.get(i + 1)) { std::process::exit(api(p)) }
    // AllAssets: the whole Lucide set, so views can use any icon by name (ui::icon).
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(|cx| {
        gpui_kit::init(cx);
        ui::install_theme(cx);
        // Before anything reads settings: a LOUPE_STRESS run picks its own settings domain here.
        let app = state::init(cx);
        state::adapt::apply_code_theme();
        shell::keys::install(cx);
        // Sparkle's scheduled checks start with the app; the menu item asks right away.
        loupe_app::updates::start();
        cx.on_action(|_: &shell::keys::CheckForUpdates, _| loupe_app::updates::check_for_updates());
        cx.on_action(|_: &shell::keys::Quit, cx| cx.quit());
        cx.on_action(|_: &shell::keys::Hide, cx| cx.hide());
        cx.on_action(|_: &shell::keys::HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &shell::keys::ShowAll, cx| cx.unhide_other_apps());
        // Closing the last window quits, as in the Swift app.
        cx.on_window_closed(|cx, _| if cx.windows().is_empty() { cx.quit() }).detach();
        shell::open_main_window(app.clone(), cx);
        state::scenario::run(&app, cx); // LOUPE_STRESS steps (MS-04..10); nothing without a scenario
        // A `background` stress run stays behind the owner's windows and never takes their keystrokes.
        if !app.read(cx).scenario.as_ref().is_some_and(|s| s.background) { cx.activate(true) }
    });
}
