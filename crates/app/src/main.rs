mod app;
mod args;
mod chat;
mod login;
mod media;
mod theme;

use std::borrow::Cow;

use gpui_kit::*;

use crate::{
    app::FrillogramApp,
    args::{Command, USAGE},
};

fn main() {
    let config_path = match args::parse(std::env::args_os().skip(1)) {
        Ok(Command::Run { config }) => config,
        Ok(Command::PrintDefaultConfig) => {
            print!("{}", frillogram_core::config::EXAMPLE);
            return;
        }
        Ok(Command::Help) => {
            println!("{USAGE}");
            return;
        }
        Err(error) => {
            eprintln!("frillogram: {error}\nRun `frillogram --help` for usage.");
            std::process::exit(2);
        }
    };
    // Before the window opens: `api_hash_command` may ask for a passphrase.
    let mut loaded = frillogram_core::config::load(config_path);

    // There's no menu, Cmd+Q or Dock reopen, so on macOS a process left
    // without its window would keep TDLib running with no way back to it.
    gpui_kit::application().with_assets(Icons).with_quit_mode(QuitMode::LastWindowClosed).run(move |cx| {
        gpui_kit::init(cx);
        if loaded.settings.appearance.font_family.is_some() {
            loaded.check_font(&cx.text_system().all_font_names());
        }
        let layout = &loaded.settings.layout;
        let window_size = size(px(layout.window_width as f32), px(layout.window_height as f32));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(window_size, cx)),
            titlebar: Some(TitlebarOptions { title: Some("Frillogram".into()), ..Default::default() }),
            ..Default::default()
        };
        let frillogram_core::Loaded { settings, config, .. } = loaded;
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| FrillogramApp::new(settings, config, window, cx)))
            .expect("failed to open the main window");
        cx.activate(true);
    });
}

// Only the selected SVGs are embedded, not the whole Lucide catalog.
gpui_kit::assets::icon_assets!(ExtraIcons, [Lock, MessageSquareText, MessagesSquare, Paperclip]);

/// The icons gpui-component draws in its own controls, plus the ones above.
struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
