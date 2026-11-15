//! App-owned caption and native window actions for the undecorated window.

use crate::state::App;
use crate::theme::*;
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use incular::runtime::WindowHandle;
use incular::widgets::StackFit;

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
            fixed_size: Some(Size::new(34.0, 30.0)),
            alignment: Some(Alignment::CENTER),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..Default::default()
        })
        .on_click(action)
        .into()
}

fn titlebar(window: &WindowHandle) -> Widget {
    let (drag, double) = (window.clone(), window.clone());
    // Start the OS move only once a drag is recognized. A stationary double
    // click remains available for maximize/restore, including on the logo.
    let drag_area = GestureDetector::new(
        Container::new().height(44.0).color(Color::TRANSPARENT)
            .padding(EdgeInsets::only(22.0, 0.0, 0.0, 0.0))
            .child(row(vec![logo(22.0), hgap(10.0),
                text("Spoutpeak", 13.0, FontWeight::W600, TEXT).into()])),
    )
    .on_pan_start(move |_| { let _ = drag.begin_move_drag(); })
    .on_double_tap(move || toggle_maximized(&double));

    let maximized = window.observed_state().maximized.unwrap_or(false);
    let (minimize, maximize, close) = (window.clone(), window.clone(), window.clone());
    let controls = Container::new().radius(12.0).color(Color::rgba(0, 0, 0, 55))
        .padding(EdgeInsets::all(3.0))
        .child(row(vec![
            caption_button(icons::minus(), "Minimize window", false, move || {
                let _ = minimize.set_minimized(true);
            }),
            caption_button(if maximized { icons::window_restore() } else { icons::window_maximize() },
                if maximized { "Restore window" } else { "Maximize window" }, false,
                move || toggle_maximized(&maximize)),
            caption_button(icons::x(), "Close window", true, move || { let _ = close.close(); }),
        ]));
    SizedBox::new().height(44.0).child(row(vec![
        Expanded::new(drag_area).into(),
        controls.into(), hgap(10.0),
    ])).into()
}

/// The ambient layer sits behind this frame, so the caption has the same
/// background as the rest of the app, with no separate Windows title strip.
pub fn frame(app: &App, content: Widget) -> Widget {
    let app = app.clone();
    LayoutBuilder::new(move |_, _| {
        let Some(window) = app.player.window.borrow().clone() else { return content.clone(); };
        if window.observed_state().fullscreen.unwrap_or(false) {
            return content.clone();
        }
        let body: Widget = Column::new(vec![titlebar(&window), Expanded::new(content.clone()).into()])
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
