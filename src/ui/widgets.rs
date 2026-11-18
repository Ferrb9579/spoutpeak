//! Shared visual pieces: network images, poster cards, catalog rows.

use super::motion::{self, Feel, WheelAxis};
use crate::images::{self, Picture};
use crate::state::{App, CatalogRow, Load};
use crate::addons::types::Meta;
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
                .color(GLASS_HI)
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
    let shape = Some(images::Shape { width, height });
    let picture = images::shaped(url, max_pixels, shape);
    // Fade in over the placeholder only when the download finishes while
    // this is on screen; cached images appear straight away.
    let waited = std::cell::Cell::new(false);
    Builder::new(move |_| match picture.get() {
        Picture::Ready(handle) => {
            let image: Widget = Image::new(handle)
                .width(width)
                .height(height)
                .fit(BoxFit::Cover)
                .into();
            let image = if waited.replace(false) {
                let fade = motion::Entrance::new();
                fade.play(std::time::Duration::from_millis(260), 0.0);
                layers(vec![placeholder(width, height, radius, &label), fade.wrap(image)]).into()
            } else {
                image
            };
            let image: Widget = SizedBox::new().width(width).height(height).child(image).into();
            if radius > 0.0 { ClipRRect::new(radius, image).into() } else { image }
        }
        _ => {
            waited.set(true);
            placeholder(width, height, radius, &label)
        }
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
                    .color(GLASS_HI)
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
            background_states: Some(StateColor::new(Color::TRANSPARENT).hovered(GLASS_HI)),
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
                .child(line(title.clone(), 13.0, FontWeight::W600, TEXT))
                .into(),
            gap(2.0),
            SizedBox::new()
                .width(inner_width)
                .child(line(caption.clone(), 11.5, FontWeight::W400, MUTED))
                .into(),
        ]),
    );
    let full = if caption.is_empty() { title.clone() } else { format!("{title}
{caption}") };
    let (app, meta) = (app.clone(), meta.clone());
    // Inside the button: wrapped around it, a MouseRegion keeps the button
    // from seeing the pointer at all.
    card_button(
        super::tooltip::tooltip(full, content),
        Size::new(size.width, size.height + CARD_TEXT),
        move || app.open_detail(meta.clone()),
    )
}

/// A sideways-scrolling strip with smooth wheel scrolling that also follows
/// a regular (vertical) mouse wheel.
pub fn horizontal_scroller(controller: &ScrollController, child: impl Into<Widget>) -> Widget {
    motion::wheel(
        controller,
        WheelAxis::HorizontalOrVertical,
        SingleChildScrollView::new(child)
            .scroll_direction(Axis::Horizontal)
            .physics(motion::APP_SCROLLED)
            .controller(controller.clone()),
    )
}

/// A vertical page or list that scrolls with a smooth, spring-driven wheel.
/// `view` must set `motion::APP_SCROLLED` physics.
pub fn smooth(controller: &ScrollController, view: impl Into<Widget>) -> Widget {
    motion::wheel(controller, WheelAxis::Vertical, view)
}

/// A status line for loading / empty / failed states.
/// A spinner over a caption, centred in the space it is given: for a page
/// or panel whose content is still on its way.
pub fn loading_page(message: impl Into<String>) -> Widget {
    let message = message.into();
    Align::new(
        Alignment::CENTER,
        Padding::new(
            EdgeInsets::symmetric(0.0, 48.0),
            Column::new(vec![
                spinner(30.0),
                gap(14.0),
                text(message, 13.5, FontWeight::W500, MUTED).align(TextAlign::Center).into(),
            ])
            .main_axis_size(MainAxisSize::Min)
            .cross_axis_alignment(CrossAxisAlignment::Center),
        ),
    )
    .into()
}

/// A small spinner before a caption, for a part of a page still loading.
pub fn loading_line(message: impl Into<String>) -> Widget {
    row(vec![spinner(16.0), hgap(10.0), text(message, 13.0, FontWeight::W400, MUTED).into()]).into()
}

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
        for (glyph, direction) in [(icons::chevron_left(), -1.0_f32), (icons::chevron_right(), 1.0)] {
            let controller = app.controllers.row(key);
            children.push(small_icon_button(glyph, None, Tone::Neutral, move || {
                // Most of a row's visible width, so one card stays in view
                // for context.
                let width = controller.viewport_extent();
                let page = if width > 0.0 { width * 0.8 } else { 720.0 };
                motion::glide_by(&controller, direction * page, Feel::Page);
            }));
            children.push(hgap(6.0));
        }
    }
    Row::new(children)
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into()
}

/// A horizontally scrolling, lazily built row of `count` equal-width items.
/// It follows a sideways wheel or trackpad; a vertical wheel passes through
/// to the page around it.
pub fn sideways_row(
    controller: &ScrollController,
    count: usize,
    item_extent: f32,
    height: f32,
    item: impl Fn(usize) -> Widget + 'static,
) -> Widget {
    motion::wheel(
        controller,
        WheelAxis::Horizontal,
        SizedBox::new().height(height).child(
            ListView::builder(count, item)
                .scroll_direction(Axis::Horizontal)
                .item_extent(item_extent)
                .physics(motion::APP_SCROLLED)
                .controller(controller.clone()),
        ),
    )
}

/// A horizontally scrolling, lazily built strip of poster cards.
pub fn poster_strip(app: &App, key: &str, items: Rc<Vec<Meta>>, size: CardSize) -> Widget {
    let app_for_items = app.clone();
    sideways_row(
        &app.controllers.row(key),
        items.len(),
        size.width + CARD_GAP,
        size.height + CARD_TEXT,
        move |index| {
            Padding::new(
                EdgeInsets::only(0.0, 0.0, CARD_GAP, 0.0),
                poster_card(&app_for_items, &items[index], size, None, None),
            )
            .into()
        },
    )
}

/// One catalog on the Board / Search: header plus strip or status.
pub fn catalog_section(app: &App, row: &CatalogRow) -> Option<Widget> {
    if !row.on_board {
        return None;
    }
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
                    .color(GLASS),
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
    smooth(controller, GridView::builder(
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
    .physics(motion::APP_SCROLLED)
    .padding(PAGE_PADDING))
}
