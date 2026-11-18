//! The dropdown under a search field: recent searches and title
//! suggestions (see `crate::suggest`), as in Stremio.
//!
//! The field marks its place with a `CompositedTransformTarget`; the menu is
//! drawn in the root stack (`layer`), above the page, following it. Both sit
//! in one `TapRegion` group, so a tap on either keeps the menu and a tap
//! anywhere else closes it.

use crate::state::App;
use crate::theme::*;
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use incular::widgets::{CompositedTransformFollower, CompositedTransformTarget, LayerLink, TapRegion};
use std::cell::Cell;

const GROUP: &str = "search-menu";
const ROW_HEIGHT: f32 = 38.0;

thread_local! {
    static LINK: LayerLink = LayerLink::default();
    /// Width of the field the menu hangs from.
    static WIDTH: Cell<f32> = const { Cell::new(300.0) };
}

/// `field` (the search text field, `width` wide) with the dropdown wired
/// to it. Only one such field is on screen at a time.
pub fn field(app: &App, field: Widget, width: f32) -> Widget {
    WIDTH.with(|w| w.set(width));
    let (open, close) = (app.clone(), app.clone());
    let region = TapRegion::new(field)
        .group_id(GROUP)
        .on_tap_inside(move |_| open.open_search_menu())
        .on_tap_outside(move |_| close.close_search_menu());
    CompositedTransformTarget::new(LINK.with(|l| l.clone()), region).into()
}

fn section_header(title: &str, trailing: Option<Widget>) -> Widget {
    let mut children: Vec<Widget> = vec![text(title, 12.5, FontWeight::W600, MUTED).into(), Spacer::new().into()];
    children.extend(trailing);
    SizedBox::new()
        .height(32.0)
        .child(Padding::new(
            EdgeInsets::symmetric(12.0, 0.0),
            hrow(children).cross_axis_alignment(CrossAxisAlignment::Center),
        ))
        .into()
}

fn entry(app: &App, width: f32, label: String, detail: Option<String>, query: String) -> Widget {
    let mut content: Vec<Widget> = vec![Flexible::new(line(label.clone(), 13.5, FontWeight::W500, TEXT)).into()];
    if let Some(detail) = detail {
        content.push(hgap(10.0));
        content.push(text(detail, 12.0, FontWeight::W400, FAINT).max_lines(Some(1)).into());
    }
    let app = app.clone();
    Widget::from(
        Button::with_child(SizedBox::new().width(width - 48.0).child(hrow(content).cross_axis_alignment(CrossAxisAlignment::Center)))
            .style(ButtonStyle {
                background: Some(Color::TRANSPARENT),
                background_states: Some(StateColor::new(Color::TRANSPARENT).hovered(GLASS_HOVER)),
                foreground: Some(TEXT),
                border: Some(Border::new(0.0, Color::TRANSPARENT)),
                border_radius: Some(9.0),
                padding: Some(EdgeInsets::symmetric(12.0, 0.0)),
                fixed_size: Some(Size::new(width - 16.0, ROW_HEIGHT)),
                alignment: Some(Alignment::CENTER_LEFT),
                overlay_color: Some(Color::TRANSPARENT.into()),
                animation_duration: Some(std::time::Duration::ZERO),
                ..ButtonStyle::default()
            })
            .on_click(move || app.pick_search(query.clone())),
    )
    .accessibility_label(label)
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "movie" => "Movie",
        "series" => "Series",
        other => other,
    }
}

fn menu(app: &App, width: f32) -> Option<Widget> {
    let history = app.profile.read(|p| p.settings.recent_searches.clone());
    let suggestions = app.search_suggestions.get();
    if history.is_empty() && suggestions.is_empty() {
        return None;
    }
    let mut children: Vec<Widget> = Vec::new();
    if !history.is_empty() {
        let clear = app.clone();
        children.push(section_header(
            "Search history",
            Some(small_button("Clear history", Tone::Ghost, move || clear.clear_recent_searches())),
        ));
        for query in history {
            children.push(entry(app, width, query.clone(), None, query));
        }
    }
    if !suggestions.is_empty() {
        if !children.is_empty() {
            children.push(gap(8.0));
        }
        children.push(section_header("Search suggestions", None));
        for suggestion in suggestions {
            let detail = match &suggestion.year {
                Some(year) => format!("{}  ·  {year}", kind_label(&suggestion.kind)),
                None => kind_label(&suggestion.kind).to_owned(),
            };
            children.push(entry(app, width, suggestion.name.clone(), Some(detail), suggestion.name));
        }
    }
    let panel = Container::new()
        .width(width)
        .padding(EdgeInsets::all(8.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(22, 22, 22, 200))
                .border_radius(BorderRadius::all(Radius::circular(14.0)))
                .box_shadow(vec![BoxShadow::new(Color::rgba(0, 0, 0, 140), Offset::new(0.0, 10.0), 28.0, 0.0)]),
        )
        .child(vcol(children).main_axis_size(MainAxisSize::Min).cross_axis_alignment(CrossAxisAlignment::Start));
    Some(frosted_glass(panel, 14.0, 30.0))
}

/// The open dropdown, for the root stack.
pub fn layer(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        if !app.search_menu.get() {
            return SizedBox::shrink().into();
        }
        let width = WIDTH.with(Cell::get).max(320.0);
        let Some(menu) = menu(&app, width) else { return SizedBox::shrink().into() };
        let follower = CompositedTransformFollower::new(
            LINK.with(|l| l.clone()),
            TapRegion::new(super::motion::pop_in(menu, -6.0)).group_id(GROUP),
        )
        .target_anchor(Alignment::BOTTOM_LEFT)
        .follower_anchor(Alignment::TOP_LEFT)
        .offset(Offset::new(0.0, 6.0))
        .show_when_unlinked(false);
        layers(vec![Positioned::new(follower).left(0.0).top(0.0).into()]).into()
    })
    .into()
}
