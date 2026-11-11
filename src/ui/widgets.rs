//! Shared visual pieces: network images, poster cards, catalog rows.

use crate::images::{self, Picture};
use crate::state::{App, CatalogRow, Load};
use crate::media::types::Meta;
use crate::theme::*;
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use std::rc::Rc;

fn placeholder(width: f32, height: f32, radius: f32, label: &str) -> Widget {
    let initials: String = label
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    Container::new()
        .width(width)
        .height(height)
        .alignment(Alignment::CENTER)
        .decoration(
            BoxDecoration::new()
                .color(SURFACE_HI)
                .border_radius(BorderRadius::all(Radius::circular(radius))),
        )
        .child(text(
            initials,
            (height * 0.16).clamp(12.0, 34.0),
            FontWeight::W700,
            FAINT,
        ))
        .into()
}

/// An image from the network, cropped to fill `width` x `height`. Only this
/// widget rebuilds when the download finishes.
pub fn net_image(
    url: Option<&str>,
    width: f32,
    height: f32,
    radius: f32,
    max_pixels: u32,
    label: &str,
) -> Widget {
    let label = label.to_owned();
    let Some(url) = url.filter(|u| u.starts_with("http")) else {
        return placeholder(width, height, radius, &label);
    };
    // Corners are baked into the pixels: `ClipRRect` in Incular 0.1.1 only
    // clips to the bounding rectangle.
    let shape = (radius > 0.0).then_some(images::Shape { width, height, radius });
    let picture = images::shaped(url, max_pixels, shape);
    Builder::new(move |_| match picture.get() {
        Picture::Ready(handle) => SizedBox::new()
            .width(width)
            .height(height)
            .child(
                Image::new(handle)
                    .width(width)
                    .height(height)
                    .fit(BoxFit::Cover),
            )
            .into(),
        _ => placeholder(width, height, radius, &label),
    })
    .into()
}

/// An addon logo: shown whole (never cropped) on a rounded tile.
pub fn logo_image(url: Option<&str>, size: f32, label: &str) -> Widget {
    let Some(url) = url.filter(|u| u.starts_with("http")) else {
        return placeholder(size, size, 14.0, label);
    };
    let picture = images::picture(url, 192);
    let label = label.to_owned();
    Builder::new(move |_| match picture.get() {
        Picture::Ready(handle) => Container::new()
            .width(size)
            .height(size)
            .padding(EdgeInsets::all(8.0))
            .decoration(
                BoxDecoration::new()
                    .color(SURFACE_HI)
                    .border_radius(BorderRadius::all(Radius::circular(14.0))),
            )
            .child(
                Image::new(handle)
                    .width(size - 16.0)
                    .height(size - 16.0)
                    .fit(BoxFit::Contain),
            )
            .into(),
        _ => placeholder(size, size, 14.0, &label),
    })
    .into()
}

#[derive(Clone, Copy)]
pub struct CardSize {
    pub width: f32,
    pub height: f32,
}

pub const POSTER_HEIGHT: f32 = 222.0;
pub const CARD_GAP: f32 = 14.0;
pub const CARD_TEXT: f32 = 46.0;

pub fn card_size(shape: Option<&str>) -> CardSize {
    match shape {
        Some("landscape") => CardSize {
            width: POSTER_HEIGHT * 16.0 / 9.0,
            height: POSTER_HEIGHT,
        },
        Some("square") => CardSize {
            width: POSTER_HEIGHT,
            height: POSTER_HEIGHT,
        },
        _ => CardSize {
            width: POSTER_HEIGHT * 2.0 / 3.0,
            height: POSTER_HEIGHT,
        },
    }
}

fn card_button(child: Widget, size: Size, on_click: impl Fn() + 'static) -> Widget {
    Button::with_child(child)
        .style(ButtonStyle {
            background: Some(Color::TRANSPARENT),
            background_states: Some(StateColor::new(Color::TRANSPARENT).hovered(SURFACE_HI)),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(12.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(size),
            alignment: Some(Alignment::TOP_LEFT),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

/// Poster with title and a caption; `progress` draws a watched bar.
pub fn poster_card(
    app: &App,
    meta: &Meta,
    size: CardSize,
    caption: Option<String>,
    progress: Option<f32>,
) -> Widget {
    let title = meta.title().to_owned();
    let inner_width = size.width - 8.0;
    let inner_height = size.height - 8.0;
    let image = net_image(
        meta.poster.as_deref(),
        inner_width,
        inner_height,
        10.0,
        342,
        &title,
    );
    let art: Widget = match progress {
        Some(fraction) => layers(vec![
            image,
            Positioned::new(
                Container::new()
                    .width(inner_width - 16.0)
                    .height(4.0)
                    .radius(2.0)
                    .color(Color::rgba(0, 0, 0, 170)),
            )
            .left(8.0)
            .bottom(8.0)
            .into(),
            Positioned::new(
                Container::new()
                    .width(((inner_width - 16.0) * fraction.clamp(0.02, 1.0)).max(4.0))
                    .height(4.0)
                    .radius(2.0)
                    .color(ACCENT),
            )
            .left(8.0)
            .bottom(8.0)
            .into(),
        ])
        .into(),
        None => image,
    };
    let caption = caption
        .or_else(|| meta.release_line())
        .unwrap_or_default();
    let content = Padding::all(
        4.0,
        col(vec![
            art,
            gap(7.0),
            SizedBox::new()
                .width(inner_width)
                .child(line(title, 13.0, FontWeight::W600, TEXT))
                .into(),
            gap(2.0),
            SizedBox::new()
                .width(inner_width)
                .child(line(caption, 11.5, FontWeight::W400, MUTED))
                .into(),
        ]),
    );
    let (app, meta) = (app.clone(), meta.clone());
    card_button(
        content.into(),
        Size::new(size.width, size.height + CARD_TEXT),
        move || app.open_detail(meta.clone()),
    )
}

/// A sideways-scrolling strip that also follows a regular (vertical) mouse
/// wheel. Incular's horizontal viewports only read the wheel's x delta, which
/// a plain mouse never produces.
pub fn horizontal_scroller(controller: &ScrollController, child: impl Into<Widget>) -> Widget {
    let wheel = controller.clone();
    Listener::new(
        SingleChildScrollView::new(child)
            .scroll_direction(Axis::Horizontal)
            .controller(controller.clone()),
    )
    .on_pointer_signal(move |event| {
        if event.scroll_delta.y != 0.0 {
            wheel.scroll_by(event.scroll_delta.y);
        }
    })
    .into()
}

/// A status line for loading / empty / failed states.
pub fn notice(message: impl Into<String>, color: Color) -> Widget {
    Container::new()
        .padding(EdgeInsets::symmetric(0.0, 18.0))
        .child(text(message, 13.5, FontWeight::W400, color))
        .into()
}

fn row_header(app: &App, title: &str, subtitle: &str, key: &str, scrollable: bool) -> Widget {
    let mut children: Vec<Widget> = vec![
        text(title, 17.0, FontWeight::W600, TEXT).into(),
        hgap(10.0),
        text(subtitle, 12.0, FontWeight::W400, FAINT).into(),
        Spacer::new().into(),
    ];
    if scrollable {
        for (glyph, delta) in [(icons::chevron_left(), -720.0_f32), (icons::chevron_right(), 720.0)] {
            let controller = app.controllers.row(key);
            children.push(small_icon_button(glyph, None, Tone::Neutral, move || {
                controller.scroll_by(delta);
            }));
            children.push(hgap(6.0));
        }
    }
    Row::new(children)
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into()
}

/// A horizontally scrolling, lazily built strip of poster cards.
pub fn poster_strip(app: &App, key: &str, items: Rc<Vec<Meta>>, size: CardSize) -> Widget {
    let app_for_items = app.clone();
    let count = items.len();
    SizedBox::new()
        .height(size.height + CARD_TEXT)
        .child(
            ListView::builder(count, move |index| {
                Padding::new(
                    EdgeInsets::only(0.0, 0.0, CARD_GAP, 0.0),
                    poster_card(&app_for_items, &items[index], size, None, None),
                )
            })
            .scroll_direction(Axis::Horizontal)
            .item_extent(size.width + CARD_GAP)
            .controller(app.controllers.row(key)),
        )
        .into()
}

/// One catalog on the Board / Search: header plus strip or status.
pub fn catalog_section(app: &App, row: &CatalogRow) -> Option<Widget> {
    let body: Widget = match &row.items {
        Load::Ready(items) if items.is_empty() => return None,
        Load::Ready(items) => {
            let size = card_size(items[0].poster_shape.as_deref());
            poster_strip(app, &row.key, Rc::new(items.clone()), size)
        }
        Load::Failed(error) => notice(format!("Couldn't load: {error}"), FAINT),
        _ => Row::new((0..7).map(|_| {
            Padding::new(
                EdgeInsets::only(0.0, 0.0, CARD_GAP, 0.0),
                Container::new()
                    .width(POSTER_HEIGHT * 2.0 / 3.0 - 8.0)
                    .height(POSTER_HEIGHT - 8.0)
                    .radius(10.0)
                    .color(SURFACE),
            )
        }))
        .main_axis_size(MainAxisSize::Min)
        .into(),
    };
    let scrollable = matches!(row.items, Load::Ready(_));
    Some(
        col(vec![
            row_header(app, &row.title, &row.addon_name, &row.key, scrollable),
            gap(10.0),
            body,
            gap(22.0),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}

/// Page frame: title bar on top, scrolling content underneath.
pub fn page(title: &str, trailing: Vec<Widget>, content: Widget) -> Widget {
    let mut header: Vec<Widget> = vec![heading(title), Spacer::new().into()];
    header.extend(trailing);
    vcol(vec![
        Container::new()
            .padding(EdgeInsets::only(32.0, 26.0, 32.0, 14.0))
            .child(Row::new(header).cross_axis_alignment(CrossAxisAlignment::Center))
            .into(),
        Expanded::new(content).into(),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Stretch)
    .into()
}

pub const PAGE_PADDING: EdgeInsets = EdgeInsets::only(32.0, 6.0, 32.0, 32.0);

/// Lazily built poster grid that asks for more when the end comes near.
pub fn poster_grid(
    app: &App,
    items: Rc<Vec<Meta>>,
    controller: &ScrollController,
    caption: impl Fn(&Meta) -> Option<String> + 'static,
    progress: impl Fn(&Meta) -> Option<f32> + 'static,
    near_end: impl Fn() + 'static,
) -> Widget {
    let size = card_size(items.first().and_then(|m| m.poster_shape.as_deref()));
    let app = app.clone();
    let count = items.len();
    GridView::builder(
        count,
        SliverGridDelegate::max_cross_axis_extent(size.width + CARD_GAP)
            .main_axis_extent(size.height + CARD_TEXT + CARD_GAP),
        move |index| {
            if index + 12 >= count {
                near_end();
            }
            let meta = &items[index];
            Align::new(
                Alignment::TOP_LEFT,
                poster_card(&app, meta, size, caption(meta), progress(meta)),
            )
        },
    )
    .controller(controller.clone())
    .padding(PAGE_PADDING)
    .into()
}
