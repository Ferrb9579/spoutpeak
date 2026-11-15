//! The window's ambient backdrop: the artwork of what you are looking at,
//! blurred into a soft wash of its colours behind every screen. The
//! interface itself is black and white, so this is the only colour besides
//! the artwork; panels on top are translucent glass (see `theme::glass`) so
//! it shows through. Without artwork the window is plain black.

use crate::images::{self, Picture};
use crate::state::{App, Load, Route};
use crate::theme::*;
use incular::prelude::*;
use incular::widgets::StackFit;
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    /// The artwork last shown, kept while screens without their own
    /// (Settings, Addons…) are open, so the mood does not flicker.
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// How strongly the blurred artwork shows through.
const ART_OPACITY: f32 = 0.9;
const CROSSFADE: Duration = Duration::from_millis(700);

/// The artwork for the current screen, if it has one.
fn artwork(app: &App, route: Route) -> Option<String> {
    let art = |m: &crate::addons::types::Meta| m.background.clone().or_else(|| m.poster.clone());
    match route {
        Route::Board => app.board.read(|board| {
            board.rows.iter().find_map(|row| match &row.items {
                Load::Ready(items) => items.iter().find_map(art),
                _ => None,
            })
        }),
        Route::Detail => app.detail.read(|d| match &d.meta {
            Load::Ready(meta) => art(meta),
            _ => d.preview.as_ref().and_then(art),
        }),
        _ => None,
    }
}

pub fn layer(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let route = app.route.get().last().copied().unwrap_or(Route::Board);
        let url = match artwork(&app, route) {
            Some(url) => {
                LAST.with(|last| *last.borrow_mut() = Some(url.clone()));
                Some(url)
            }
            None => LAST.with(|last| last.borrow().clone()),
        };
        LayoutBuilder::new(move |_, constraints| {
            let size = constraints.biggest();
            let mut children: Vec<Widget> = vec![Container::new().color(BG).into()];
            if let Some(url) = &url {
                children.push(art_layer(url, size));
            }
            children.push(scrim(size));
            layers(children).fit(StackFit::Expand).into()
        })
        .into()
    })
    .into()
}

/// The blurred artwork, fading in whenever it changes.
fn art_layer(url: &str, size: Size) -> Widget {
    let picture = images::blurred(url);
    let key = url.to_owned();
    Builder::new(move |_| match picture.get() {
        Picture::Ready(handle) => KeyedSubtree::new(
            key.as_str(),
            Opacity::new(
                ART_OPACITY,
                Image::new(handle).width(size.width).height(size.height).fit(BoxFit::Cover),
            )
            .animate()
            .duration(CROSSFADE)
            .curve(Curve::EaseOut)
            .fade_in(),
        )
        .into(),
        _ => SizedBox::shrink().into(),
    })
    .into()
}

fn stops(colors: &[(f32, Color)]) -> GradientStops {
    GradientStops::new(colors.iter().map(|&(offset, color)| GradientStop { offset, color }).collect())
}

fn painted(brush: impl Into<Brush>) -> Widget {
    Container::new().decoration(BoxDecoration::new().gradient(brush)).into()
}

/// Keeps text readable over any artwork: darker towards the bottom, where
/// lists and rows of posters sit, and a touch along the left edge.
fn scrim(size: Size) -> Widget {
    let ink = |alpha: u8| Color::rgba(0, 0, 0, alpha);
    layers(vec![
        painted(LinearGradient::new(
            Offset::new(0.0, 0.0),
            Offset::new(0.0, size.height),
            stops(&[(0.0, ink(85)), (0.45, ink(125)), (1.0, ink(205))]),
        )),
        painted(LinearGradient::new(
            Offset::new(0.0, 0.0),
            Offset::new(size.width * 0.45, 0.0),
            stops(&[(0.0, ink(70)), (1.0, ink(0))]),
        )),
    ])
    .fit(StackFit::Expand)
    .into()
}
