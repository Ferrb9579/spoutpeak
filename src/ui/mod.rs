//! The window: navigation shell, routing and global keyboard handling.

mod addons;
mod browse;
mod detail;
mod downloads;
mod motion;
mod player;
mod settings;
mod splash;
mod widgets;

use crate::state::{App, Route};
use crate::theme::*;
use incular::controls::{Button, ButtonStyle, ControlTheme, ControlThemeScope, StateColor};
use incular::prelude::*;
use incular::widgets::StackFit;
use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

/// Wraps a screen so it rebuilds on its own when the state it reads changes.
fn island(app: &App, build: fn(&App) -> Widget) -> Widget {
    let app = app.clone();
    Builder::new(move |_| build(&app)).into()
}

const NAV_WIDTH: f32 = 196.0;

fn nav_button(app: &App, label: &str, glyph: Arc<Path>, route: Route, active: bool) -> Widget {
    let background = if active { ACCENT_DIM } else { Color::TRANSPARENT };
    let foreground = if active { TEXT } else { MUTED };
    let app = app.clone();
    let content = row(vec![
        // Active marker.
        Container::new()
            .width(3.0)
            .height(18.0)
            .radius(2.0)
            .color(if active { ACCENT } else { Color::TRANSPARENT })
            .into(),
        hgap(13.0),
        icon(glyph, 18.0, if active { ACCENT } else { MUTED }),
        hgap(13.0),
        text(
            label,
            14.0,
            if active { FontWeight::W600 } else { FontWeight::W500 },
            foreground,
        )
        .into(),
    ]);
    Button::with_child(content)
        .style(ButtonStyle {
            background: Some(background),
            background_states: Some(
                StateColor::new(background).hovered(if active { ACCENT_DIM } else { SURFACE_HI }),
            ),
            foreground: Some(foreground),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(12.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(NAV_WIDTH, 42.0)),
            alignment: Some(Alignment::CENTER_LEFT),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(move || app.go(route))
        .into()
}

fn nav_label(label: &str) -> Widget {
    Padding::new(
        EdgeInsets::only(16.0, 0.0, 0.0, 8.0),
        text(label, 10.5, FontWeight::W700, FAINT),
    )
    .into()
}

fn sidebar(app: &App, current: Route) -> Widget {
    let mut items: Vec<Widget> = vec![
        Padding::new(
            EdgeInsets::only(14.0, 6.0, 0.0, 0.0),
            row(vec![
                logo(26.0),
                hgap(11.0),
                text("Spoutpeak", 18.0, FontWeight::W700, TEXT).into(),
            ]),
        )
        .into(),
        gap(28.0),
        nav_label("BROWSE"),
    ];
    for (label, glyph, route) in [
        ("Home", icons::house(), Route::Board),
        ("Discover", icons::compass(), Route::Discover),
        ("Library", icons::bookmark(), Route::Library),
        ("Search", icons::search(), Route::Search),
        ("Downloads", icons::download(), Route::Downloads),
    ] {
        items.push(nav_button(app, label, glyph, route, route == current));
        items.push(gap(4.0));
    }
    items.push(gap(18.0));
    items.push(nav_label("MANAGE"));
    for (label, glyph, route) in [
        ("Addons", icons::puzzle(), Route::Addons),
        ("Settings", icons::settings(), Route::Settings),
    ] {
        items.push(nav_button(app, label, glyph, route, route == current));
        items.push(gap(4.0));
    }
    items.push(Spacer::new().into());

    let footer = {
        let app = app.clone();
        Builder::new(move |_| {
            let (dot, server) = match app.server_online.get() {
                Some(true) => (GOOD, "Torrent engine ready"),
                Some(false) => (DANGER, "Torrent engine stopped"),
                None => (FAINT, "Starting torrent engine"),
            };
            let titles = app.library.read(|l| l.values().filter(|i| !i.removed).count());
            let open = app.clone();
            GestureDetector::new(
                Container::new()
                    .width(NAV_WIDTH)
                    .padding(EdgeInsets::all(12.0))
                    .decoration(
                        BoxDecoration::new()
                            .color(SURFACE)
                            .border_radius(BorderRadius::all(Radius::circular(14.0))),
                    )
                    .child(col(vec![
                        line(
                            format!("{titles} in your library"),
                            12.5,
                            FontWeight::W600,
                            TEXT,
                        ),
                        gap(1.0),
                        line("Saved on this PC", 11.0, FontWeight::W400, FAINT),
                        gap(10.0),
                        row(vec![
                            Container::new().width(7.0).height(7.0).radius(3.5).color(dot).into(),
                            hgap(8.0),
                            text(server, 11.0, FontWeight::W400, MUTED).into(),
                        ])
                        .into(),
                    ])),
            )
            .on_tap(move || open.go(Route::Settings))
            .into()
        })
    };
    items.push(footer.into());

    // A floating rounded panel rather than a full-height strip.
    Padding::new(
        EdgeInsets::only(12.0, 12.0, 0.0, 12.0),
        Container::new()
            .width(NAV_WIDTH + 24.0)
            .padding(EdgeInsets::only(12.0, 16.0, 12.0, 12.0))
            .decoration(
                BoxDecoration::new()
                    .color(PANEL)
                    .border_radius(BorderRadius::all(Radius::circular(20.0))),
            )
            .child(
                Column::new(items)
                    .cross_axis_alignment(CrossAxisAlignment::Start)
                    .main_axis_size(MainAxisSize::Max),
            ),
    )
    .into()
}

fn toast(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| match app.toast.get() {
        Some(message) => Align::new(
            Alignment::BOTTOM_CENTER,
            Padding::new(
                EdgeInsets::only(0.0, 0.0, 0.0, 34.0),
                IgnorePointer::new(motion::pop_in(
                    Container::new()
                        .padding(EdgeInsets::symmetric(18.0, 11.0))
                        .decoration(
                            BoxDecoration::new()
                                .color(Color::rgba(238, 240, 247, 255))
                                .border_radius(BorderRadius::all(Radius::circular(12.0))),
                        )
                        .child(text(message, 13.5, FontWeight::W500, Color::rgba(18, 20, 30, 255))),
                    14.0,
                )),
            ),
        )
        .into(),
        None => SizedBox::shrink().into(),
    })
    .into()
}

pub(super) fn handle_key(app: &App, event: &KeyboardEvent) -> bool {
    if event.state != KeyState::Down {
        return false;
    }
    let stack = app.route.get_untracked();
    let route = stack.last().copied().unwrap_or(Route::Board);
    let shift = event.modifiers.contains(Modifiers::SHIFT);
    let control = event.modifiers.contains(Modifiers::CONTROL);
    let alt = event.modifiers.contains(Modifiers::ALT);
    if route != Route::Player {
        let target = match event.code {
            Code::Escape | Code::BrowserBack if stack.len() > 1 => {
                app.back();
                return true;
            }
            Code::ArrowLeft if alt && stack.len() > 1 => {
                app.back();
                return true;
            }
            Code::KeyK | Code::KeyF if control => Route::Search,
            Code::Digit1 if control => Route::Board,
            Code::Digit2 if control => Route::Discover,
            Code::Digit3 if control => Route::Library,
            Code::Digit4 if control => Route::Search,
            Code::Digit5 if control => Route::Addons,
            Code::Digit6 if control => Route::Settings,
            Code::Digit7 if control => Route::Downloads,
            _ => return false,
        };
        app.go(target);
        return true;
    }
    let (seek, seek_shift, volume_step) = app.profile.peek(|p| {
        let s = &p.settings;
        (f64::from(s.seek_step), f64::from(s.seek_step_shift), s.volume_step as f32 / 100.0)
    });
    let step = if shift { seek_shift } else { seek };
    match event.code {
        Code::Space | Code::KeyK => app.toggle_pause(),
        Code::ArrowLeft | Code::KeyJ => app.seek_by(-step),
        Code::ArrowRight | Code::KeyL => app.seek_by(step),
        Code::ArrowUp => {
            let volume = app.player.state.peek(|s| s.volume);
            app.set_volume(volume + volume_step);
        }
        Code::ArrowDown => {
            let volume = app.player.state.peek(|s| s.volume);
            app.set_volume(volume - volume_step);
        }
        // Frame-ish stepping and speed, as in YouTube / mpv.
        Code::Comma if shift => {
            let speed = app.player.state.peek(|s| s.speed);
            app.set_speed(((speed - 0.25).max(0.25) * 100.0).round() / 100.0);
        }
        Code::Period if shift => {
            let speed = app.player.state.peek(|s| s.speed);
            app.set_speed(((speed + 0.25).min(3.0) * 100.0).round() / 100.0);
        }
        Code::Digit0 | Code::Home => app.seek_to(0.0),
        Code::BracketLeft => app.adjust_audio_delay(-0.05),
        Code::BracketRight => app.adjust_audio_delay(0.05),
        Code::KeyC => app.cycle_subtitles(),
        Code::KeyB => app.adjust_brightness(if shift { -0.05 } else { 0.05 }),
        Code::KeyM => app.toggle_mute(),
        Code::KeyF | Code::F11 => app.toggle_fullscreen(),
        Code::KeyG => app.adjust_subtitle_delay(-0.25),
        Code::KeyH => app.adjust_subtitle_delay(0.25),
        Code::KeyN => {
            if let Some(next) = app.player.state.peek(|s| s.next.clone()) {
                app.play_next(next);
            }
        }
        Code::Escape | Code::BrowserBack => {
            if app.player.state.peek(|s| s.fullscreen) {
                app.toggle_fullscreen();
            } else {
                app.back();
            }
        }
        _ => return false,
    }
    true
}

const PAGE_ENTRANCE: Duration = Duration::from_millis(220);
const SPLASH_FADE: Duration = Duration::from_millis(320);

thread_local! {
    /// Fade and rise of the screen that just opened.
    static PAGE: motion::Entrance = motion::Entrance::new();
    static LAST_ROUTE: Cell<Option<Route>> = const { Cell::new(None) };
    /// Set once the startup screen has finished fading out.
    static SPLASH_GONE: Signal<bool> = Signal::new(false);
}

/// The startup screen; fades out once startup finishes, then leaves the tree.
fn splash_layer(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        if !app.booted.get() {
            return island(&app, splash::screen);
        }
        IgnorePointer::new(
            island(&app, splash::screen)
                .animate()
                .duration(SPLASH_FADE)
                .curve(Curve::EaseOut)
                .fade_out()
                .on_complete(|| {
                    SPLASH_GONE.with(|gone| gone.set(true));
                }),
        )
        .into()
    })
    .into()
}

pub fn root(app: &App) -> Widget {
    let stack = app.route.get();
    let route = stack.last().copied().unwrap_or(Route::Board);
    let base = stack.first().copied().unwrap_or(Route::Board);

    // Each newly opened screen fades in and rises a little. Compositor-only:
    // the screen itself builds once.
    let page = PAGE.with(|p| p.clone());
    let previous = LAST_ROUTE.with(|last| last.replace(Some(route)));
    if previous.is_some_and(|previous| previous != route) {
        page.play(PAGE_ENTRANCE, if route == Route::Player { 0.0 } else { 10.0 });
    }

    let content: Widget = match route {
        Route::Player => page.wrap(island(app, player::screen)),
        Route::Detail => page.wrap(island(app, detail::screen)),
        _ => {
            let screen = match route {
                Route::Board => island(app, browse::board),
                Route::Discover => island(app, browse::discover),
                Route::Library => island(app, browse::library),
                Route::Search => island(app, browse::search),
                Route::Addons => island(app, addons::screen),
                Route::Downloads => island(app, downloads::screen),
                _ => island(app, settings::screen),
            };
            hrow(vec![sidebar(app, base), Expanded::new(page.wrap(screen)).into()])
                .cross_axis_alignment(CrossAxisAlignment::Stretch)
                .into()
        }
    };

    let mut stacked = vec![content, IgnorePointer::new(toast(app)).into()];
    if !SPLASH_GONE.with(|gone| gone.get()) && route != Route::Player {
        stacked.push(splash_layer(app));
    }
    let layered = layers(stacked).fit(StackFit::Expand);
    let keys = app.clone();
    let themed = ControlThemeScope::new(
        ControlTheme::dark(),
        DefaultTextStyle::new(
            style(14.0, FontWeight::W400, TEXT),
            // The frame clock for smooth scrolling (Incular has no public
            // ticker; see INCULAR-GAPS-2.md).
            motion::clock(Container::new().color(BG).child(layered)),
        ),
    );
    Directionality::new(
        TextDirection::Ltr,
        KeyboardListener::new(themed)
            .focus_node(app.controllers.focus.clone())
            .autofocus(true)
            .on_key(move |event| handle_key(&keys, &event)),
    )
    .into()
}
