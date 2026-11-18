//! The title's background art, whole and sharp: the page shows it only as
//! the blurred ambient wash. Opened from the title page; a click anywhere
//! or Esc closes it.

use crate::images::Picture;
use crate::state::App;
use crate::theme::*;
use incular::prelude::*;
use incular::widgets::{HitTestBehavior, StackFit};

/// The viewer over the whole app while it is open, for the root stack.
pub fn layer(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let Some(url) = app.artwork_view.get() else { return SizedBox::shrink().into() };
        let picture = crate::images::picture(&url, 2560);
        let close = app.clone();
        let image = Builder::new(move |_| match picture.get() {
            Picture::Ready(handle) => Padding::new(
                EdgeInsets::all(40.0),
                Image::new(handle).fit(BoxFit::Contain),
            )
            .into(),
            Picture::Failed => super::widgets::notice("The artwork couldn't be loaded.", MUTED),
            _ => spinner(34.0),
        });
        let dismiss = app.clone();
        layers(vec![
            GestureDetector::new(
                Container::new()
                    .color(Color::rgba(0, 0, 0, 235))
                    .child(Align::new(Alignment::CENTER, image)),
            )
            .behavior(HitTestBehavior::Opaque)
            .on_tap(move || { dismiss.artwork_view.set(None); })
            .into(),
            Positioned::new(super::tooltip::tooltip(
                "Close (Esc)",
                icon_button(icon(icons::x(), 20.0, TEXT), 40.0, move || { close.artwork_view.set(None); })
                    .accessibility_label("Close artwork"),
            ))
            .right(20.0)
            .top(20.0)
            .into(),
        ])
        .fit(StackFit::Expand)
        .into()
    })
    .into()
}
