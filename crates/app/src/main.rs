mod app;
mod chat;
mod login;

use gpui_kit::*;

use crate::app::FrillogramApp;

fn main() {
    let config = frillogram_core::Config::from_env();

    // There's no menu, Cmd+Q or Dock reopen, so on macOS a process left
    // without its window would keep TDLib running with no way back to it.
    gpui_kit::application().with_quit_mode(QuitMode::LastWindowClosed).run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1180.), px(760.)), cx)),
            titlebar: Some(TitlebarOptions { title: Some("Frillogram".into()), ..Default::default() }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| FrillogramApp::new(config, window, cx)))
            .expect("failed to open the main window");
        cx.activate(true);
    });
}
