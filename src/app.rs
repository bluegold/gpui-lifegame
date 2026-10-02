use gpui::{
    App, AppContext, Application, Bounds, Context, IntoElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, size,
};

struct AppView;

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

pub fn run() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.0), px(600.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| AppView),
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
