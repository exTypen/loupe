use loupe_app::{shell, state, ui};

fn main() {
    // AllAssets: the whole Lucide set, so views can use any icon by name (ui::icon).
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(|cx| {
        gpui_kit::init(cx);
        ui::install_theme(cx);
        // Before anything reads settings: a LOUPE_STRESS run picks its own settings domain here.
        let app = state::init(cx);
        state::adapt::apply_code_theme();
        shell::keys::install(cx);
        cx.on_action(|_: &shell::keys::Quit, cx| cx.quit());
        cx.on_action(|_: &shell::keys::Hide, cx| cx.hide());
        cx.on_action(|_: &shell::keys::HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &shell::keys::ShowAll, cx| cx.unhide_other_apps());
        // Closing the last window quits, as in the Swift app.
        cx.on_window_closed(|cx, _| if cx.windows().is_empty() { cx.quit() }).detach();
        shell::open_main_window(app, cx);
        cx.activate(true);
    });
}
