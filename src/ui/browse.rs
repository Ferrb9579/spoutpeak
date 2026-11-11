//! Board, Discover, Library and Search.

use super::widgets::*;
use crate::state::{App, LibrarySort, Load, ui};
use crate::media::types::{LibraryItem, Meta};
use crate::theme::*;
use incular::prelude::*;
use std::collections::HashMap;
use std::rc::Rc;

fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A single line of chips that scrolls sideways when it does not fit.
fn chip_bar(chips: Vec<Widget>) -> Widget {
    let mut spaced = Vec::with_capacity(chips.len() * 2);
    for chip in chips {
        spaced.push(chip);
        spaced.push(hgap(8.0));
    }
    Container::new()
        .height(42.0)
        .padding(EdgeInsets::only(32.0, 0.0, 32.0, 12.0))
        .child(
            SingleChildScrollView::new(row(spaced)).scroll_direction(Axis::Horizontal),
        )
        .into()
}

// ------------------------------------------------------------------- board

fn continue_watching(app: &App) -> Option<Widget> {
    let mut items: Vec<LibraryItem> = app.library.read(|library| {
        library
            .values()
            .filter(|item| item.is_in_continue_watching())
            .cloned()
            .collect()
    });
    if items.is_empty() {
        return None;
    }
    items.sort_by(|a, b| b.state.last_watched.cmp(&a.state.last_watched));
    items.truncate(24);
    let size = card_size(None);
    let items = Rc::new(items);
    let count = items.len();
    let builder_app = app.clone();
    let strip = SizedBox::new()
        .height(size.height + CARD_TEXT)
        .child(
            ListView::builder(count, move |index| {
                let item = &items[index];
                let caption = if item.state.duration > 0.0 {
                    format!("{}% watched", (item.progress() * 100.0).round())
                } else {
                    "Up next".to_owned()
                };
                Padding::new(
                    EdgeInsets::only(0.0, 0.0, CARD_GAP, 0.0),
                    poster_card(
                        &builder_app,
                        &item.as_meta(),
                        size,
                        Some(caption),
                        Some(item.progress()),
                    ),
                )
            })
            .scroll_direction(Axis::Horizontal)
            .item_extent(size.width + CARD_GAP)
            .controller(app.controllers.row("continue-watching")),
        );
    Some(
        col(vec![
            text("Continue watching", 17.0, FontWeight::W600, TEXT).into(),
            gap(10.0),
            strip.into(),
            gap(22.0),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}

/// A wide banner for the first title on the board.
fn hero(app: &App, meta: &Meta, source: &str) -> Widget {
    const HEIGHT: f32 = 300.0;
    let (app, meta, source) = (app.clone(), meta.clone(), source.to_owned());
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width().clamp(320.0, 2400.0);
        let art = meta.background.as_deref().or(meta.poster.as_deref());
        let mut facts: Vec<String> = Vec::new();
        facts.extend(meta.release_line());
        if let Some(rating) = meta.imdb_rating.as_deref().filter(|r| !r.is_empty()) {
            facts.push(format!("IMDb {rating}"));
        }
        if let Some(runtime) = meta.runtime.as_deref().filter(|r| !r.is_empty()) {
            facts.push(runtime.to_owned());
        }
        facts.extend(meta.genres.iter().take(3).cloned());

        let mut copy: Vec<Widget> = vec![
            text(format!("FEATURED  ·  {}", source.to_uppercase()), 10.5, FontWeight::W700, ACCENT)
                .into(),
            gap(8.0),
            SizedBox::new()
                .width((width - 72.0).min(620.0))
                .child(line(meta.title().to_owned(), 32.0, FontWeight::W700, TEXT))
                .into(),
        ];
        if !facts.is_empty() {
            copy.push(gap(6.0));
            copy.push(text(facts.join("   ·   "), 12.5, FontWeight::W500, MUTED).into());
        }
        if let Some(description) = meta.description.as_deref().filter(|d| !d.is_empty()) {
            copy.push(gap(10.0));
            copy.push(
                SizedBox::new()
                    .width((width - 72.0).min(560.0))
                    .child(
                        Text::new(description.to_owned())
                            .style(style(13.5, FontWeight::W400, Color::rgba(214, 219, 232, 255)))
                            .max_lines(Some(2))
                            .overflow(TextOverflow::Ellipsis),
                    )
                    .into(),
            );
        }
        let (open, save) = (app.clone(), app.clone());
        let (open_meta, save_meta) = (meta.clone(), meta.clone());
        let saved = app.in_library(meta.id());
        copy.push(gap(16.0));
        copy.push(
            row(vec![
                button("View details", Tone::Primary, move || open.open_detail(open_meta.clone())),
                hgap(10.0),
                button(
                    if saved { "✓  In library" } else { "+  Library" },
                    Tone::Neutral,
                    move || save.toggle_library(&save_meta),
                ),
            ])
            .into(),
        );

        let rounded = |color: Color| -> Widget {
            Container::new()
                .width(width)
                .height(HEIGHT)
                .decoration(
                    BoxDecoration::new()
                        .color(color)
                        .border_radius(BorderRadius::all(Radius::circular(20.0))),
                )
                .into()
        };
        SizedBox::new()
            .width(width)
            .height(HEIGHT)
            .child(layers(vec![
                rounded(SURFACE),
                net_image(art, width, HEIGHT, 20.0, 1280, ""),
                // Scrim so the copy stays readable over any artwork.
                rounded(Color::rgba(9, 11, 18, 150)),
                Positioned::new(col(copy)).left(32.0).bottom(28.0).into(),
            ]))
            .into()
    })
    .into()
}

pub fn board(app: &App) -> Widget {
    let rows = app.board.read(|board| board.rows.clone());
    let mut sections: Vec<Widget> = Vec::new();
    let featured = rows.iter().find_map(|row| match &row.items {
        Load::Ready(items) => items
            .iter()
            .find(|m| m.background.is_some() || m.poster.is_some())
            .map(|m| (m.clone(), row.title.clone())),
        _ => None,
    });
    if let Some((meta, source)) = featured {
        sections.push(hero(app, &meta, &source));
        sections.push(gap(26.0));
    }
    sections.extend(continue_watching(app));
    let had_rows = !rows.is_empty();
    sections.extend(rows.iter().filter_map(|row| catalog_section(app, row)));
    if !had_rows {
        sections.push(notice(
            "Loading your addons… If this stays empty, install an addon with catalogs.",
            MUTED,
        ));
    }
    let refresh = {
        let app = app.clone();
        small_button("Refresh", Tone::Neutral, move || app.load_board())
    };
    page(
        "Home",
        vec![refresh],
        SingleChildScrollView::new(Padding::new(
            PAGE_PADDING,
            col(sections).cross_axis_alignment(CrossAxisAlignment::Stretch),
        ))
        .controller(app.controllers.board.clone())
        .into(),
    )
}

// ---------------------------------------------------------------- discover

pub fn discover(app: &App) -> Widget {
    let catalogs = app.discover_catalogs();
    let (kind, catalog_key, genre, items, loading, exhausted, error) = app.discover.read(|d| {
        (
            d.kind.clone(),
            d.catalog_key.clone(),
            d.genre.clone(),
            Rc::new(d.items.clone()),
            d.loading,
            d.exhausted,
            d.error.clone(),
        )
    });

    let mut kinds: Vec<String> = Vec::new();
    for reference in &catalogs {
        if !kinds.contains(&reference.catalog.kind) {
            kinds.push(reference.catalog.kind.clone());
        }
    }
    let kind_chips = kinds
        .iter()
        .map(|k| {
            let (app, value) = (app.clone(), k.clone());
            chip(capitalized(k), *k == kind, move || {
                app.select_discover_kind(value.clone())
            })
        })
        .collect();

    let catalog_chips = catalogs
        .iter()
        .filter(|c| c.catalog.kind == kind)
        .map(|c| {
            let (app, kind, key) = (app.clone(), kind.clone(), c.key());
            let label = format!("{} · {}", c.catalog.title(), c.addon.manifest.name);
            chip(label, c.key() == catalog_key, move || {
                app.select_discover(kind.clone(), key.clone(), None)
            })
        })
        .collect();

    let current = catalogs.iter().find(|c| c.key() == catalog_key);
    let mut bars = vec![chip_bar(kind_chips), chip_bar(catalog_chips)];
    if let Some(current) = current {
        let options = current.catalog.genre_options();
        if !options.is_empty() {
            let mut chips = Vec::new();
            if !current.catalog.requires_extra("genre") {
                let (app, kind, key) = (app.clone(), kind.clone(), catalog_key.clone());
                chips.push(chip("All", genre.is_none(), move || {
                    app.select_discover(kind.clone(), key.clone(), None)
                }));
            }
            for option in options {
                let (app, kind, key, value) =
                    (app.clone(), kind.clone(), catalog_key.clone(), option.clone());
                chips.push(chip(
                    option.clone(),
                    genre.as_deref() == Some(option.as_str()),
                    move || app.select_discover(kind.clone(), key.clone(), Some(value.clone())),
                ));
            }
            bars.push(chip_bar(chips));
        }
    }

    let body: Widget = if catalogs.is_empty() {
        Padding::new(PAGE_PADDING, notice("No installed addon offers catalogs.", MUTED)).into()
    } else if let Some(error) = error {
        Padding::new(PAGE_PADDING, notice(format!("Couldn't load: {error}"), DANGER)).into()
    } else if items.is_empty() {
        Padding::new(
            PAGE_PADDING,
            notice(
                if loading { "Loading…" } else { "Nothing here." },
                MUTED,
            ),
        )
        .into()
    } else {
        poster_grid(
            app,
            items,
            &app.controllers.discover,
            |_| None,
            |_| None,
            move || {
                if !loading && !exhausted {
                    ui(|app| app.load_more_discover());
                }
            },
        )
    };

    let mut children = bars;
    children.push(Expanded::new(body).into());
    page(
        "Discover",
        vec![],
        Column::new(children)
            .cross_axis_alignment(CrossAxisAlignment::Stretch)
            .into(),
    )
}

// ----------------------------------------------------------------- library

pub fn library(app: &App) -> Widget {
    let (kind, sort) = app.library_view.read(|v| (v.kind.clone(), v.sort));
    let mut all: Vec<LibraryItem> = app
        .library
        .read(|library| library.values().filter(|i| !i.removed).cloned().collect());

    let mut kinds: Vec<String> = all.iter().map(|i| i.kind.clone()).collect();
    kinds.sort();
    kinds.dedup();

    let mut kind_chips = vec![{
        let app = app.clone();
        chip("All", kind.is_none(), move || {
            app.library_view.write(|v| v.kind = None)
        })
    }];
    for k in &kinds {
        let (app, value) = (app.clone(), k.clone());
        kind_chips.push(chip(
            capitalized(k),
            kind.as_deref() == Some(k.as_str()),
            move || app.library_view.write(|v| v.kind = Some(value.clone())),
        ));
    }
    let sort_chips = [
        ("Recently watched", LibrarySort::Recent),
        ("Name", LibrarySort::Name),
        ("Most watched", LibrarySort::Watched),
    ]
    .into_iter()
    .map(|(label, value)| {
        let app = app.clone();
        chip(label, sort == value, move || {
            app.library_view.write(|v| v.sort = value)
        })
    })
    .collect();

    if let Some(kind) = &kind {
        all.retain(|i| &i.kind == kind);
    }
    match sort {
        LibrarySort::Recent => all.sort_by(|a, b| {
            (&b.state.last_watched, &b.mtime).cmp(&(&a.state.last_watched, &a.mtime))
        }),
        LibrarySort::Name => all.sort_by_key(|i| i.name.to_lowercase()),
        LibrarySort::Watched => all.sort_by(|a, b| {
            b.state
                .overall_time_watched
                .total_cmp(&a.state.overall_time_watched)
        }),
    }

    let signed_in = app.profile.read(|p| p.auth.is_some());
    let progress: Rc<HashMap<String, f32>> = Rc::new(
        all.iter()
            .filter(|i| i.state.time_offset > 0.0 && i.state.duration > 0.0)
            .map(|i| (i.id.clone(), i.progress()))
            .collect(),
    );
    let metas: Rc<Vec<Meta>> = Rc::new(all.iter().map(LibraryItem::as_meta).collect());

    let body: Widget = if metas.is_empty() {
        Padding::new(
            PAGE_PADDING,
            notice(
                if signed_in {
                    "Your library is empty. Add something from its details page."
                } else {
                    "Your library is empty. Add titles from their details page; sign in under Settings to sync with your Media account."
                },
                MUTED,
            ),
        )
        .into()
    } else {
        poster_grid(
            app,
            metas,
            &app.controllers.library,
            |meta| Some(capitalized(&meta.kind)),
            move |meta| progress.get(meta.id()).copied(),
            || {},
        )
    };

    page(
        "Library",
        vec![],
        vcol(vec![
            chip_bar(kind_chips),
            chip_bar(sort_chips),
            Expanded::new(body).into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}

// ------------------------------------------------------------------ search

pub fn search(app: &App) -> Widget {
    let (query, rows) = app.search.read(|s| (s.query.clone(), s.rows.clone()));
    let recent = app.profile.read(|p| p.settings.recent_searches.clone());
    let (submit, typed) = (app.clone(), app.clone());
    let field = input(
        &app.controllers.search,
        "Search movies, series, channels…",
        460.0,
        move |value| submit.submit_search(value),
    )
    .on_changed(move |value| typed.search_typed(value));
    let go = {
        let app = app.clone();
        button("Search", Tone::Primary, move || {
            app.submit_search(app.controllers.search.text())
        })
    };
    let mut bar: Vec<Widget> = vec![field.into(), hgap(10.0), go];
    if !query.is_empty() {
        let app = app.clone();
        bar.push(hgap(8.0));
        bar.push(button("Clear", Tone::Ghost, move || {
            app.controllers.search.set_text("");
            app.run_search(String::new());
        }));
    }

    let mut sections: Vec<Widget> = Vec::new();
    if query.is_empty() {
        if recent.is_empty() {
            sections.push(notice(
                "Results appear as you type, from every installed addon that supports search.",
                MUTED,
            ));
        } else {
            let mut chips: Vec<Widget> = Vec::new();
            for entry in recent {
                let app = app.clone();
                chips.push(chip(entry.clone(), false, move || app.submit_search(entry.clone())));
                chips.push(hgap(8.0));
            }
            let clear = app.clone();
            sections.push(
                hrow(vec![
                    text("Recent searches", 15.0, FontWeight::W600, TEXT).into(),
                    hgap(12.0),
                    small_button("Clear", Tone::Ghost, move || clear.clear_recent_searches()),
                ])
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into(),
            );
            sections.push(gap(12.0));
            sections.push(
                SizedBox::new()
                    .height(34.0)
                    .child(
                        SingleChildScrollView::new(row(chips)).scroll_direction(Axis::Horizontal),
                    )
                    .into(),
            );
        }
    } else if rows.is_empty() {
        sections.push(notice("No installed addon supports search.", MUTED));
    } else {
        let pending = rows.iter().any(|r| r.items == Load::Loading);
        let found: Vec<Widget> = rows
            .iter()
            .filter(|r| !matches!(r.items, Load::Failed(_)))
            .filter_map(|row| catalog_section(app, row))
            .collect();
        if found.is_empty() && !pending {
            sections.push(notice(format!("No results for “{query}”."), MUTED));
        }
        sections.extend(found);
    }

    page(
        "Search",
        vec![],
        vcol(vec![
            Padding::new(
                EdgeInsets::only(32.0, 0.0, 32.0, 14.0),
                row(bar),
            )
            .into(),
            Expanded::new(
                SingleChildScrollView::new(Padding::new(
                    PAGE_PADDING,
                    col(sections).cross_axis_alignment(CrossAxisAlignment::Stretch),
                ))
                .controller(app.controllers.search_results.clone()),
            )
            .into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}
