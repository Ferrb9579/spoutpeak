//! Spoutpeak: a native, addon-based media center built with Incular.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod addons;
mod ai;
mod convert;
mod devtools;
mod diagnostics;
mod downloads;
mod images;
mod player;
mod state;
mod store;
mod streaming;
mod theme;
mod branding;
mod together;
mod ui;
mod util;
mod window;

#[cfg(test)]
mod text_tests;

use incular::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let diagnostics = diagnostics::init();
    let result = run();
    if let Err(error) = &result {
        tracing::error!(error = %error, "Application exited with an error");
    }
    if let Some(diagnostics) = diagnostics { diagnostics.finish(result.is_ok()); }
    result
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("--open-logs") => return open::that(diagnostics::directory()).map_err(Into::into),
        Some("--export-diagnostics") => {
            let report = diagnostics::export_report()?;
            println!("{}", report.display());
            if let Some(parent) = report.parent() { open::that(parent)?; }
            return Ok(());
        }
        #[cfg(debug_assertions)]
        Some("--diagnostics-test-panic") => panic!("diagnostics panic self-test"),
        #[cfg(all(debug_assertions, windows))]
        Some("--diagnostics-test-native-crash") => unsafe {
            windows::Win32::System::Diagnostics::Debug::RaiseException(0xE0000001, 1, None);
            return Err("native crash self-test unexpectedly returned".into());
        },
        _ => {}
    }
    tracing::info!("Loading profile and creating application state");
    let app_state = state::App::new();
    state::install_app(app_state.clone());

    tracing::info!("Creating native window and graphics renderer");
    let application = Application::new_with_options(
        WindowOptions {
            title: "Spoutpeak".into(),
            initial_logical_size: Size::new(1320.0, 820.0),
            minimum_logical_size: Some(Size::new(960.0, 620.0)),
            decorations: false,
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
            window::configure_corners();
            ui::root(&app_state)
        },
    )?;
    tracing::info!("Native window and graphics renderer initialized");

    // Background work needs the runtime's dispatcher, which only exists
    // once the runtime does; everything network-bound starts from here.
    let tokio = application.tokio_handle();
    application.dispatch(move |runtime| {
        state::install_bridge(tokio, runtime.dispatcher());
        state::with_app(|app| {
            app.startup();
            // `spoutpeak <url-or-file>` plays it straight away.
            if let Some(target) = std::env::args().nth(1) {
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
    tracing::info!("Entering application event loop");
    let result = incular::run(application);
    tracing::info!(success = result.is_ok(), "Application event loop stopped");
    // Native close requests (including Alt+F4) must release playback before
    // Windows begins destroying this thread's local application state.
    state::with_app(|app| { app.ai.cancel(); app.stop_playback(); });
    result.map_err(Into::into)
}
