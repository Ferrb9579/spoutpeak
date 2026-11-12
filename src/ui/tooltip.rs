//! Hover tooltips drawn in the window's top layer.
//!
//! Incular's own `Tooltip` paints in place, under whatever comes after it
//! (a stream tile's tooltip vanished behind the next tile). Here the target
//! only reports hover; one tooltip for the whole app is drawn by
//! [`layer`], above every screen, next to the pointer.

use crate::theme::*;
use incular::prelude::*;
use std::cell::Cell;
use std::time::Duration;

/// Hover this long before the tooltip appears.
const DELAY: Duration = Duration::from_millis(550);
const MAX_WIDTH: f32 = 420.0;

#[derive(Clone, PartialEq)]
struct Tip {
    text: String,
    at: Offset,
}

thread_local! {
    static TIP: Signal<Option<Tip>> = Signal::new(None);
    /// Bumped on every enter / exit / press, so a stale delayed show is
    /// dropped.
    static TURN: Cell<u64> = const { Cell::new(0) };
    /// The latest pointer position over a tooltip target.
    static POINTER: Cell<Offset> = const { Cell::new(Offset::new(0.0, 0.0)) };
}

fn next_turn() -> u64 {
    TURN.with(|t| {
        t.set(t.get() + 1);
        t.get()
    })
}

pub fn hide() {
    next_turn();
    TIP.with(|tip| {
        if tip.get_untracked().is_some() {
            tip.set(None);
        }
    });
}

/// Shows `text` after a short hover over `child`. Empty text: no tooltip.
pub fn tooltip(text: impl Into<String>, child: impl Into<Widget>) -> Widget {
    let text: String = text.into();
    let child: Widget = child.into();
    if text.trim().is_empty() {
        return child;
    }
    // No pointer-down Listener here: wrapping a button in one stops the
    // button from seeing the pointer. A click usually navigates, and the
    // root hides the tooltip on every route change.
    MouseRegion::new(child)
        .on_enter(move |event| {
            POINTER.with(|p| p.set(event.position));
            let turn = next_turn();
            let text = text.clone();
            crate::state::spawn(async { tokio::time::sleep(DELAY).await }, move |_, ()| {
                if TURN.with(|t| t.get()) == turn {
                    let at = POINTER.with(|p| p.get());
                    TIP.with(|tip| tip.set(Some(Tip { text, at })));
                }
            });
        })
        .on_hover(|event| {
            // Follow the pointer until the tooltip is up; then it stays put.
            POINTER.with(|p| p.set(event.position));
        })
        .on_exit(|_| hide())
        .into()
}

/// The tooltip layer for the window's root stack, or `None` while nothing
/// is shown (an idle layer must not sit over the screens: in Incular an
/// upper layer can swallow clicks meant for the ones below).
pub fn layer(window: Size) -> Option<Widget> {
    let tip = TIP.with(|tip| tip.get())?;
    // Below-right of the pointer, kept inside the window.
    // Right of the pointer, or ending at its left when there is more room
    // there; never wider than the side it is on.
    let room_right = window.width - tip.at.x - 22.0;
    let room_left = tip.at.x - 14.0;
    let on_right = room_right >= MAX_WIDTH || room_right >= room_left;
    let width = MAX_WIDTH.min(if on_right { room_right } else { room_left }).max(120.0);
    let below = tip.at.y + 22.0;
    let bubble = Container::new()
        .padding(EdgeInsets::symmetric(11.0, 8.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(236, 239, 247, 248))
                .border_radius(BorderRadius::all(Radius::circular(9.0)))
                .box_shadow(vec![BoxShadow::new(
                    Color::rgba(0, 0, 0, 120),
                    Offset::new(0.0, 6.0),
                    18.0,
                    0.0,
                )]),
        )
        .child(
            Text::new(tip.text)
                .style(style(12.5, FontWeight::W500, Color::rgba(18, 20, 30, 255))),
        );
    let bubble = ConstrainedBox::new(Constraints::loose(Size::new(width, window.height.max(1.0))), bubble);
    // Near the bottom edge it goes above the pointer instead.
    let positioned = Positioned::new(bubble);
    let positioned = if on_right {
        positioned.left(tip.at.x + 14.0)
    } else {
        positioned.right((window.width - tip.at.x + 6.0).max(8.0))
    };
    let positioned = if below > window.height - 90.0 {
        positioned.bottom(window.height - tip.at.y + 10.0)
    } else {
        positioned.top(below)
    };
    let positioned: Widget = positioned.into();
    Some(IgnorePointer::new(Stack::new(vec![positioned]).fit(incular::widgets::StackFit::Expand)).into())
}
