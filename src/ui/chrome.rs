//! App-owned caption and native window actions for the undecorated window.

use crate::state::{App, Load, Route};
use crate::theme::*;
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use incular::runtime::WindowHandle;
use incular::widgets::StackFit;

const TITLEBAR_HEIGHT: f32 = 44.0;
const CAPTION_BUTTON: f32 = 34.0;
const CAPTION_PADDING: f32 = 3.0;
const LOGO_HEIGHT: f32 = 22.0;

fn toggle_maximized(window: &WindowHandle) {
    let maximized = window.observed_state().maximized.unwrap_or(false);
    let _ = window.set_maximized(!maximized);
}

fn caption_button(glyph: Glyph, label: &str, close: bool, action: impl Fn() + 'static) -> Widget {
    let hover = if close { Color::rgba(220, 55, 65, 220) } else { GLASS_HOVER };
    Button::with_child(icon(glyph, 16.0, TEXT).accessibility_label(label))
        .style(ButtonStyle {
            background: Some(Color::TRANSPARENT),
            background_states: Some(StateColor::new(Color::TRANSPARENT).hovered(hover)),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(8.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(CAPTION_BUTTON, CAPTION_BUTTON)),
            alignment: Some(Alignment::CENTER),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..Default::default()
        })
        .on_click(action)
        .into()
}

fn home_refresh(app: &App) -> Widget {
    let app = app.clone();
    // Subscribe only this control, keeping the search field stable as rows arrive.
    Builder::new(move |_| {
        let loading = app.board.read(|board| {
            // No generation yet means initial addon discovery is still running.
            board.generation == 0 || board.rows.iter().any(|row| matches!(row.items, Load::Loading))
        });
        let glyph: Widget = if loading {
            spinner(18.0)
        } else {
            icon(icons::rotate_ccw(), 18.0, TEXT)
        };
        let refresh = app.clone();
        super::tooltip::tooltip(if loading { "Loading catalogs…" } else { "Refresh Home" },
            icon_button(glyph, 32.0, move || refresh.load_board())
                .accessibility_label("Refresh Home"))
    }).into()
}

fn home_tools(app: &App) -> Widget {
    let (submit, typed) = (app.clone(), app.clone());
    let field = input(&app.controllers.search, "Search movies, series…", 300.0, move |query| {
        submit.submit_search(query);
        submit.go(Route::Search);
    }).size(Size::new(300.0, 32.0)).style(backdrop_field_style())
        .on_changed(move |value| typed.search_menu_typed(&value));
    let field = frosted_glass(Widget::from(field).accessibility_label("Search movies and series"), 10.0, 16.0);
    let field = super::search_menu::field(app, field, 300.0);
    let search = app.clone();
    let search_button = super::tooltip::tooltip("Search",
        icon_button(icon(icons::search(), 18.0, TEXT).accessibility_label("Search Home"), 32.0, move || {
            search.submit_search(search.controllers.search.text());
            search.go(Route::Search);
        }).accessibility_label("Search Home"));
    row(vec![field,
        hgap(4.0), search_button, hgap(8.0), home_refresh(app)]).into()
}

fn titlebar(app: &App, window: &WindowHandle) -> Widget {
    let (drag, double) = (window.clone(), window.clone());
    // Start the OS move only once a drag is recognized. A stationary double
    // click remains available for maximize/restore, including on the logo.
    let drag_area = GestureDetector::new(
        Container::new().height(TITLEBAR_HEIGHT).color(Color::TRANSPARENT)
            .padding(EdgeInsets::only((TITLEBAR_HEIGHT - LOGO_HEIGHT) / 2.0, 0.0, 0.0, 0.0))
            .child(row(vec![logo(LOGO_HEIGHT), hgap(10.0),
                text("Spoutpeak", 13.0, FontWeight::W600, TEXT).into()])),
    )
    .on_pan_start(move |_| { let _ = drag.begin_move_drag(); })
    .on_double_tap(move || toggle_maximized(&double));

    let maximized = window.observed_state().maximized.unwrap_or(false);
    let (minimize, maximize, close) = (window.clone(), window.clone(), window.clone());
    let shutdown = app.clone();
    let controls = Container::new().radius(12.0).color(Color::rgba(0, 0, 0, 55))
        .padding(EdgeInsets::all(CAPTION_PADDING))
        .child(row(vec![
            caption_button(icons::minus(), "Minimize window", false, move || {
                let _ = minimize.set_minimized(true);
            }),
            caption_button(if maximized { icons::window_restore() } else { icons::window_maximize() },
                if maximized { "Restore window" } else { "Maximize window" }, false,
                move || toggle_maximized(&maximize)),
            caption_button(icons::x(), "Close window", true, move || {
                shutdown.stop_playback();
                let _ = close.close();
            }),
        ]));
    let mut children = vec![Expanded::new(drag_area).into()];
    // Keep interactive tools outside the native drag/double-click region.
    if app.current_route() == Route::Board {
        children.extend([home_tools(app), hgap(16.0)]);
    }
    // Native caption actions should not steal the player's shortcut focus.
    let control_inset = (TITLEBAR_HEIGHT - CAPTION_BUTTON - CAPTION_PADDING * 2.0) / 2.0;
    children.extend([ExcludeFocus::new(controls).into(), hgap(control_inset)]);
    SizedBox::new().height(TITLEBAR_HEIGHT).child(Container::new()
        .color(if app.current_route() == Route::Player { Color::BLACK } else { Color::TRANSPARENT })
        .child(row(children))).into()
}

/// The ambient layer sits behind this frame, so the caption has the same
/// background as the rest of the app, with no separate Windows title strip.
pub fn frame(app: &App, content: Widget) -> Widget {
    let app = app.clone();
    LayoutBuilder::new(move |_, _| {
        let Some(window) = app.player.window.borrow().clone() else { return content.clone(); };
        // Native observed_state is a snapshot, not a reactive dependency. Use
        // the requested mode so caption removal/restoration cannot miss a resize.
        if app.player.fullscreen.get() {
            return content.clone();
        }
        let body: Widget = Column::new(vec![titlebar(&app, &window), Expanded::new(content.clone()).into()])
            .cross_axis_alignment(CrossAxisAlignment::Stretch).into();
        if window.observed_state().maximized.unwrap_or(false) { return body; }

        // Native resize gestures keep normal Windows edge/corner resizing.
        // Thin hit regions reserve only the outside five pixels of the frame.
        let edge = |direction| WindowResizeRegion::new(direction,
            Container::new().color(Color::TRANSPARENT));
        let mut regions = vec![body];
        use WindowResizeDirection::*;
        regions.extend([
            Positioned::new(edge(North)).left(8.0).right(8.0).top(0.0).height(5.0).into(),
            Positioned::new(edge(South)).left(8.0).right(8.0).bottom(0.0).height(5.0).into(),
            Positioned::new(edge(West)).left(0.0).top(8.0).bottom(8.0).width(5.0).into(),
            Positioned::new(edge(East)).right(0.0).top(8.0).bottom(8.0).width(5.0).into(),
            Positioned::new(edge(NorthWest)).left(0.0).top(0.0).width(8.0).height(8.0).into(),
            Positioned::new(edge(NorthEast)).right(0.0).top(0.0).width(8.0).height(8.0).into(),
            Positioned::new(edge(SouthWest)).left(0.0).bottom(0.0).width(8.0).height(8.0).into(),
            Positioned::new(edge(SouthEast)).right(0.0).bottom(0.0).width(8.0).height(8.0).into(),
        ]);
        Stack::new(regions).fit(StackFit::Expand).into()
    }).into()
}
