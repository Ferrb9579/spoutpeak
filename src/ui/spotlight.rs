//! Choosing what the Board's Spotlight shows: a preview for Settings and a
//! dialog to pick its catalogs (up to three, six titles each), as in Stremio.

use super::motion;
use super::widgets::*;
use crate::addons::CatalogRef;
use crate::addons::types::Meta;
use crate::state::{App, Load, SPOTLIGHT_MAX, SPOTLIGHT_TITLES, spotlight_candidates};
use crate::theme::*;
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use incular::widgets::{HitTestBehavior, StackFit};

const DIALOG_MAX_WIDTH: f32 = 1040.0;
const COLUMNS: usize = 3;
const TILE_GAP: f32 = 16.0;

fn artwork(meta: &Meta) -> Option<&str> {
    meta.background.as_deref().or(meta.poster.as_deref()).filter(|u| !u.trim().is_empty())
}

/// A small dark pill over artwork.
fn badge(label: impl Into<String>) -> Widget {
    Container::new()
        .padding(EdgeInsets::symmetric(9.0, 4.0))
        .decoration(BoxDecoration::new().color(Color::rgba(8, 8, 8, 200)).border_radius(BorderRadius::all(Radius::circular(10.0))))
        .child(text(label, 11.0, FontWeight::W700, TEXT))
        .into()
}

/// The numbered disc on a chosen catalog.
fn order_disc(number: usize, size: f32) -> Widget {
    Container::new()
        .width(size)
        .height(size)
        .radius(size / 2.0)
        .color(SELECTED)
        .alignment(Alignment::CENTER)
        .child(text(number.to_string(), size * 0.5, FontWeight::W700, ON_SELECTED))
        .into()
}

/// A dark wash from the bottom, so captions read over any artwork.
fn bottom_scrim(width: f32, height: f32) -> Widget {
    Container::new()
        .width(width)
        .height(height)
        .decoration(BoxDecoration::new().gradient(LinearGradient::new(
            Offset::ZERO,
            Offset::new(0.0, height),
            GradientStops::new(vec![
                GradientStop { offset: 0.35, color: Color::TRANSPARENT },
                GradientStop { offset: 1.0, color: Color::rgba(6, 6, 6, 215) },
            ]),
        )))
        .into()
}

/// What the Spotlight shows now, for the Settings page: the first slide's
/// artwork, its catalogs and how many titles it rotates through.
pub fn preview(app: &App) -> Widget {
    let app = app.clone();
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width();
        let height = (width * 0.32).clamp(150.0, 260.0);
        let app = app.clone();
        Builder::new(move |_| {
            let (featured, keys, rows) = app.board.read(|b| (b.featured.clone(), b.spotlight.clone(), b.rows.clone()));
            let names: Vec<String> = keys
                .iter()
                .filter_map(|key| rows.iter().find(|r| &r.key == key).map(|r| r.title.clone()))
                .collect();
            let first = featured.first().map(|(meta, _)| meta.clone());
            let mut children: Vec<Widget> = vec![match &first {
                Some(meta) => net_image(artwork(meta), width, height, 0.0, 1280, meta.title()),
                None => Container::new().width(width).height(height).color(GLASS).into(),
            }];
            children.push(bottom_scrim(width, height));
            children.push(
                Positioned::new(badge(match featured.len() {
                    0 => "Loading titles".to_owned(),
                    1 => "1 title".to_owned(),
                    n => format!("{n} titles"),
                }))
                .right(14.0)
                .top(14.0)
                .into(),
            );
            children.push(
                Positioned::new(line(names.join(", "), 14.5, FontWeight::W600, TEXT))
                    .left(18.0)
                    .right(18.0)
                    .bottom(16.0)
                    .into(),
            );
            ClipRRect::new(14.0, SizedBox::new().width(width).height(height).child(layers(children).fit(StackFit::Expand))).into()
        })
        .into()
    })
    .into()
}

/// One catalog in the picker: its first title's artwork, whether it is a
/// default, its place in the Spotlight, and how many titles it would give.
fn tile(app: &App, catalog: &CatalogRef, items: Option<&Load<Vec<Meta>>>, order: Option<usize>, default: bool, width: f32) -> Widget {
    let height = width * 9.0 / 16.0;
    let candidates: Vec<&Meta> = match items {
        Some(Load::Ready(items)) => spotlight_candidates(items).collect(),
        _ => Vec::new(),
    };
    let count = match items {
        Some(Load::Ready(_)) if candidates.is_empty() => "No artwork".to_owned(),
        Some(Load::Ready(_)) => format!("{} titles", candidates.len().min(SPOTLIGHT_TITLES)),
        Some(Load::Failed(_)) => "Unavailable".to_owned(),
        _ => "Loading…".to_owned(),
    };
    let usable = !candidates.is_empty();
    let art = match candidates.first() {
        Some(meta) => net_image(artwork(meta), width, height, 0.0, 720, meta.title()),
        None => Container::new().width(width).height(height).color(GLASS).into(),
    };
    let mut over: Vec<Widget> = vec![art, bottom_scrim(width, height)];
    if default {
        over.push(Positioned::new(badge("DEFAULT")).left(10.0).top(10.0).into());
    }
    if let Some(order) = order {
        over.push(Positioned::new(order_disc(order + 1, 26.0)).right(10.0).top(10.0).into());
    }
    let art: Widget = ClipRRect::new(12.0, SizedBox::new().width(width).height(height).child(layers(over).fit(StackFit::Expand))).into();
    // A light ring marks a chosen catalog.
    let ring = if order.is_some() { SELECTED } else { Color::TRANSPARENT };
    let art = Container::new().padding(EdgeInsets::all(2.5)).radius(14.0).color(ring).child(art);
    let caption = hrow(vec![
        Expanded::new(line(catalog.title(), 14.0, FontWeight::W600, if usable { TEXT } else { MUTED })).into(),
        hgap(8.0),
        text(count, 12.0, FontWeight::W400, FAINT).into(),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center);
    let content = col(vec![art.into(), gap(8.0), SizedBox::new().width(width + 5.0).child(caption).into()]);
    let (app, key) = (app.clone(), catalog.key());
    Widget::from(
        Button::with_child(content)
            .style(ButtonStyle {
                background: Some(Color::TRANSPARENT),
                background_states: Some(StateColor::new(Color::TRANSPARENT).hovered(GLASS)),
                border: Some(Border::new(0.0, Color::TRANSPARENT)),
                border_radius: Some(16.0),
                padding: Some(EdgeInsets::all(4.0)),
                overlay_color: Some(Color::TRANSPARENT.into()),
                animation_duration: Some(std::time::Duration::ZERO),
                ..ButtonStyle::default()
            })
            .enabled(usable || order.is_some())
            .on_click(move || app.toggle_spotlight_catalog(key.clone())),
    )
    .accessibility_label(format!(
        "{}{}",
        catalog.title(),
        if order.is_some() { ", in the Spotlight" } else { "" }
    ))
}

/// A chosen catalog, with a button to take it out.
fn chosen_chip(app: &App, number: usize, title: String, key: String) -> Widget {
    let app = app.clone();
    Container::new()
        .padding(EdgeInsets::only(6.0, 5.0, 6.0, 5.0))
        .decoration(BoxDecoration::new().color(GLASS_HI).border_radius(BorderRadius::all(Radius::circular(20.0))))
        .child(row(vec![
            order_disc(number, 26.0),
            hgap(10.0),
            text(title.clone(), 13.5, FontWeight::W600, TEXT).into(),
            hgap(6.0),
            super::tooltip::tooltip(
                "Remove from the Spotlight",
                icon_button(icon(icons::x(), 14.0, MUTED), 26.0, move || app.toggle_spotlight_catalog(key.clone()))
                    .accessibility_label(format!("Remove {title} from the Spotlight")),
            ),
        ]))
        .into()
}

/// The chosen catalogs and every catalog by addon, filtered by the search.
fn catalogs(app: &App, width: f32) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let filter = app.spotlight_filter.get().trim().to_lowercase();
        let preview = app.spotlight_preview.read(|p| p.clone());
        // Rebuild when the choice changes.
        let _ = app.profile.read(|p| p.settings.spotlight_catalogs.len());
        let all = app.spotlight_catalogs();
        let chosen = app.spotlight_keys();
        let defaults = App::default_spotlight_keys(&all);

        let mut children: Vec<Widget> = Vec::new();
        let mut chips: Vec<Widget> = Vec::new();
        for (index, key) in chosen.iter().enumerate() {
            if let Some(catalog) = all.iter().find(|c| &c.key() == key) {
                chips.push(chosen_chip(&app, index + 1, catalog.title(), key.clone()));
            }
        }
        children.push(
            hrow(vec![
                Expanded::new(Wrap::new(chips).spacing(10.0).run_spacing(10.0)).into(),
                hgap(12.0),
                text(format!("{} of {SPOTLIGHT_MAX}", chosen.len()), 13.0, FontWeight::W500, MUTED).into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into(),
        );
        children.push(gap(22.0));

        let tile_width = ((width - TILE_GAP * (COLUMNS as f32 - 1.0)) / COLUMNS as f32 - 13.0).max(120.0);
        let mut groups: Vec<(String, Vec<&CatalogRef>)> = Vec::new();
        for catalog in &all {
            if !filter.is_empty()
                && !catalog.title().to_lowercase().contains(&filter)
                && !catalog.addon.manifest.name.to_lowercase().contains(&filter)
            {
                continue;
            }
            let name = &catalog.addon.manifest.name;
            match groups.iter_mut().find(|(n, _)| n == name) {
                Some((_, list)) => list.push(catalog),
                None => groups.push((name.clone(), vec![catalog])),
            }
        }
        if groups.is_empty() {
            children.push(notice(
                if filter.is_empty() { "No installed addon offers catalogs." } else { "No catalog matches." },
                MUTED,
            ));
        }
        for (name, list) in groups {
            children.push(
                row(vec![
                    icon(icons::puzzle(), 18.0, TEXT),
                    hgap(10.0),
                    text(name, 16.0, FontWeight::W600, TEXT).into(),
                    hgap(10.0),
                    text(
                        if list.len() == 1 { "1 catalog".to_owned() } else { format!("{} catalogs", list.len()) },
                        13.0,
                        FontWeight::W400,
                        FAINT,
                    )
                    .into(),
                ])
                .into(),
            );
            children.push(gap(12.0));
            let tiles: Vec<Widget> = list
                .iter()
                .map(|catalog| {
                    let key = catalog.key();
                    let order = chosen.iter().position(|k| *k == key);
                    tile(&app, catalog, preview.get(&key), order, defaults.contains(&key), tile_width)
                })
                .collect();
            children.push(Wrap::new(tiles).spacing(TILE_GAP - 8.0).run_spacing(TILE_GAP - 4.0).into());
            children.push(gap(24.0));
        }
        col(children).cross_axis_alignment(CrossAxisAlignment::Stretch).into()
    })
    .into()
}

fn dialog(app: &App, width: f32, height: f32) -> Widget {
    let inner = width - 56.0;
    let (close, reset, filter) = (app.clone(), app.clone(), app.clone());
    let search = input(&app.controllers.spotlight_search, "Search catalogs", 300.0, |_| {})
        .on_changed(move |value| { filter.spotlight_filter.set(value); });
    let header = hrow(vec![
        Expanded::new(section("Spotlight catalogs")).into(),
        button("Use default", Tone::Neutral, move || reset.set_spotlight_catalogs(Vec::new())),
        hgap(10.0),
        super::tooltip::tooltip(
            "Close",
            icon_button(icon(icons::x(), 18.0, TEXT), 34.0, move || close.close_spotlight_picker()).accessibility_label("Close"),
        ),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center);
    let intro = hrow(vec![
        Expanded::new(text(
            format!(
                "Pick up to {SPOTLIGHT_MAX} catalogs for the top of your Board. They take turns, up to {SPOTLIGHT_TITLES} titles each with artwork."
            ),
            13.5,
            FontWeight::W400,
            MUTED,
        ))
        .into(),
        hgap(20.0),
        search.into(),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center);
    let list = smooth(
        &app.controllers.spotlight_list,
        SingleChildScrollView::new(Padding::new(EdgeInsets::only(0.0, 0.0, 0.0, 12.0), catalogs(app, inner)))
            .physics(motion::APP_SCROLLED)
            .controller(app.controllers.spotlight_list.clone()),
    );
    let panel = Container::new()
        .width(width)
        .height(height)
        .padding(EdgeInsets::only(28.0, 24.0, 28.0, 8.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(20, 20, 20, 215))
                .border_radius(BorderRadius::all(Radius::circular(20.0)))
                .box_shadow(vec![BoxShadow::new(Color::rgba(0, 0, 0, 160), Offset::new(0.0, 18.0), 48.0, 0.0)]),
        )
        .child(
            vcol(vec![header.into(), gap(14.0), intro.into(), gap(18.0), Expanded::new(list).into()])
                .cross_axis_alignment(CrossAxisAlignment::Stretch),
        );
    frosted_glass(panel, 20.0, 40.0)
}

/// The picker over the whole app while it is open, for the root stack.
pub fn layer(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        if !app.spotlight_picker.get() {
            return SizedBox::shrink().into();
        }
        let app = app.clone();
        LayoutBuilder::new(move |_, constraints| {
            let width = (constraints.max_width() - 64.0).clamp(360.0, DIALOG_MAX_WIDTH);
            let height = (constraints.max_height() - 64.0).max(300.0);
            let close = app.clone();
            layers(vec![
                GestureDetector::new(Container::new().color(Color::rgba(0, 0, 0, 150)))
                    .behavior(HitTestBehavior::Opaque)
                    .on_tap(move || close.close_spotlight_picker())
                    .into(),
                Align::new(Alignment::CENTER, motion::pop_in(dialog(&app, width, height), 10.0)).into(),
            ])
            .fit(StackFit::Expand)
            .into()
        })
        .into()
    })
    .into()
}
