//! Hover tooltips, shown next to the pointer after a short hover.
//!
//! Not `incular::material::Tooltip`: its anchor is an action surface that
//! takes the taps meant for the child (a stream tile stopped opening). Here
//! the target only reports hover through a `MouseRegion`, and the bubble is
//! drawn through `OverlayPortal`, above everything after it.

use crate::theme::*;
use incular::prelude::*;
use incular::widgets::{TransientAlignment, TransientPlacement, TransientRole, TransientSide};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Hover this long before the tooltip appears.
const DELAY: Duration = Duration::from_millis(550);
const MAX_WIDTH: f32 = 420.0;

thread_local! {
    /// Delayed shows waiting for their timer, by id.
    static PENDING: std::cell::RefCell<std::collections::HashMap<u64, Box<dyn FnOnce()>>> =
        std::cell::RefCell::default();
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

fn park(show: Box<dyn FnOnce()>) -> u64 {
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    PENDING.with(|p| p.borrow_mut().insert(id, show));
    id
}

/// Run on the UI thread after the standard tooltip hover delay.
pub(super) fn after_delay(show: impl FnOnce() + 'static) {
    let id = park(Box::new(show));
    crate::state::spawn(async { tokio::time::sleep(DELAY).await }, move |_, ()| {
        if let Some(show) = PENDING.with(|p| p.borrow_mut().remove(&id)) {
            show();
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
    // Where the bubble opens (window coordinates), or `None` while hidden.
    let shown: Signal<Option<Offset>> = Signal::new(None);
    // Bumped on every enter / exit, so a stale delayed show is dropped.
    let turn = Rc::new(Cell::new(0u64));
    let pointer = Rc::new(Cell::new(Offset::new(0.0, 0.0)));

    let (enter_turn, enter_pointer, enter_shown) = (turn.clone(), pointer.clone(), shown.clone());
    let hover_pointer = pointer.clone();
    let (exit_turn, exit_shown) = (turn, shown.clone());
    // No pointer-down Listener here: wrapping a button in one stops the
    // button from seeing the pointer.
    let anchor: Widget = MouseRegion::new(child)
        .on_enter(move |event| {
            enter_pointer.set(event.position);
            enter_turn.set(enter_turn.get() + 1);
            let mine = enter_turn.get();
            let (turn, pointer, shown) = (enter_turn.clone(), enter_pointer.clone(), enter_shown.clone());
            // The completion must be `Send`: it carries an id, and the
            // show itself waits on this thread.
            after_delay(move || {
                if turn.get() == mine {
                    shown.set(Some(pointer.get()));
                }
            });
        })
        // Follow the pointer until the tooltip is up; then it stays put.
        .on_hover(move |event| hover_pointer.set(event.position))
        .on_exit(move |_| {
            exit_turn.set(exit_turn.get() + 1);
            if exit_shown.get_untracked().is_some() {
                exit_shown.set(None);
            }
        })
        .into();

    Builder::new(move |_| {
        let at = shown.get();
        let bubble = acrylic_surface(Padding::new(EdgeInsets::symmetric(11.0, 8.0),
                Text::new(text.clone())
                    .style(style(12.5, FontWeight::W500, TEXT)),
            ), 9.0);
        let bubble = ConstrainedBox::new(
            Constraints::at_most(MAX_WIDTH, f32::INFINITY),
            IgnorePointer::new(bubble),
        );
        let mut portal = OverlayPortal::new(anchor.clone())
            .overlay_child(bubble)
            .role(TransientRole::Tooltip)
            .placement(
                TransientPlacement::new()
                    .side(TransientSide::Bottom)
                    .alignment(TransientAlignment::Start)
                    .side_offset(18.0),
            )
            .show(at.is_some());
        if let Some(at) = at {
            portal = portal.anchor_point(at);
        }
        portal.into()
    })
    .into()
}
