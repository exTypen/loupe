use gpui_kit::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, WindowOptions, div};

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Hello)).expect("window");
    });
}

struct Hello;
impl Render for Hello {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("Loupe (GPUI)")
    }
}
