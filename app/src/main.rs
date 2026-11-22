use loupe_app::{shell, ui};

fn main() {
    // AllAssets: the whole Lucide set, so views can use any icon by name (ui::icon).
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(|cx| {
        gpui_kit::init(cx);
        ui::install_theme(cx);
        shell::keys::install(cx);
        cx.on_action(|_: &shell::keys::Quit, cx| cx.quit());
        cx.on_action(|_: &shell::keys::Hide, cx| cx.hide());
        cx.on_action(|_: &shell::keys::HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &shell::keys::ShowAll, cx| cx.unhide_other_apps());
        // Closing the last window quits, as in the Swift app.
        cx.on_window_closed(|cx, _| if cx.windows().is_empty() { cx.quit() }).detach();
        shell::open_main_window(shell::model::Workspace::demo(), cx);
        cx.activate(true);
    });
}
