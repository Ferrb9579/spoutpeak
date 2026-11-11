//! Spoutpeak: a native, addon-based media center built with Incular.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod devtools;
mod downloads;
mod images;
mod player;
mod state;
mod store;
mod media;
mod theme;
mod ui;
mod util;

use incular::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app_state = state::App::new();
    state::install_app(app_state.clone());

    let application = Application::new_with_options(
        WindowOptions {
            title: "Spoutpeak".into(),
            initial_logical_size: Size::new(1320.0, 820.0),
            minimum_logical_size: Some(Size::new(960.0, 620.0)),
            background_color: theme::BG,
            window_icon: theme::logo_rgba()
                .and_then(|(w, h, px)| WindowIcon::from_rgba(px, w, h).ok()),
            ..WindowOptions::default()
        },
        move |cx| {
            // The window only exists once building starts; the player needs
            // its handle for fullscreen.
            if let Some(window) = cx.window_handle() {
                *app_state.player.window.borrow_mut() = Some(window);
            }
            ui::root(&app_state)
        },
    )?;

    // Background work needs the runtime's dispatcher, which only exists
    // once the runtime does; everything network-bound starts from here.
    let tokio = application.tokio_handle();
    application.dispatch(move |runtime| {
        state::install_bridge(tokio, runtime.dispatcher());
        state::with_app(|app| {
            app.startup();
            // `spoutpeak <url-or-file>` plays it straight away.
            if let Some(target) = std::env::args().nth(1) {
                app.booted.set(true);
                app.play_url(target);
            }
        });
    });

    // Fullscreen needs the window's command handle; the application is the
    // only place that can look it up.
    let main_window = application
        .active_window_ids()
        .into_iter()
        .next()
        .and_then(|id| application.window_handle(id));
    if std::env::var_os("SPOUTPEAK_DEBUG").is_some() {
        eprintln!("[window] handle available: {}", main_window.is_some());
    }
    state::with_app(|app| *app.player.window.borrow_mut() = main_window);

    devtools::start(application.simulation());
    incular::run(application)?;
    Ok(())
}
