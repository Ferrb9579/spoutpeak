//! Board, Discover, Library and Search.

use super::motion;
use super::widgets::*;
use crate::state::{App, FeaturedCarousel, LibrarySort, Load, ui};
use crate::addons::types::{LibraryItem, Meta};
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
fn chip_bar(app: &App, key: &str, chips: Vec<Widget>) -> Widget {
    let mut spaced = Vec::with_capacity(chips.len() * 2);
    for chip in chips {
        spaced.push(chip);
        spaced.push(hgap(8.0));
    }
    Container::new()
        .height(42.0)
        .padding(EdgeInsets::only(32.0, 0.0, 32.0, 12.0))
        .child(horizontal_scroller(&app.controllers.row(key), row(spaced)))
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
    let strip = sideways_row(
        &app.controllers.row("continue-watching"),
        count,
        size.width + CARD_GAP,
        size.height + CARD_TEXT,
        move |index| {
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
            .into()
        },
    );
    Some(
        col(vec![
            text("Continue watching", 17.0, FontWeight::W600, TEXT).into(),
            gap(10.0),
            strip,
            gap(22.0),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}

/// A near-widescreen canvas keeps landscape artwork close to its original crop.
fn hero_height(width: f32) -> f32 {
    (width * 0.5).max(340.0)
}

/// One continuous artwork surface, without a separate area reserved for copy.
fn hero_art(url: Option<&str>, width: f32, height: f32) -> Widget {
    let Some(url) = url else { return SizedBox::new().into(); };
    let picture = crate::images::picture(url, 1920);
    Builder::new(move |_| {
        match picture.get() {
            crate::images::Picture::Ready(handle) => Image::new(handle)
                .width(width).height(height).fit(BoxFit::Cover).into(),
            _ => SizedBox::new().into(),
        }
    }).into()
}

#[derive(Clone, Copy, PartialEq)]
struct HeroCaptionLayout {
    progress: f32,
    inset: f32,
    baseline: f32,
    copy_width: f32,
}

impl MultiChildLayoutDelegate for HeroCaptionLayout {
    fn perform_layout(&self, size: Size, children: &mut MultiChildLayoutContext<'_>) {
        let copy = children.layout_child("copy", Constraints::loose(
            Size::new(self.copy_width, (size.height - self.baseline - self.inset).max(0.0)),
        ));
        // Measure the wrapped synopsis, reserving the top controls and keeping
        // the entire title inside the artwork even for very long descriptions.
        let max_lift = (size.height - self.baseline - copy.height - self.inset - 44.0)
            .clamp(0.0, 160.0);
        let description = children.layout_child("description", Constraints::loose(
            Size::new(self.copy_width.min(640.0), (max_lift - 16.0).max(0.0)),
        ));
        let lift = (description.height + 16.0).min(max_lift);
        let top = size.height - self.baseline - copy.height - lift * self.progress;
        children.position_child("copy", Offset::new(self.inset, top));
        children.position_child("description", Offset::new(self.inset, top + copy.height + 16.0));
    }

    fn should_relayout(&self, old: &Self) -> bool {
        self != old
    }
}

fn hero_caption(copy: Widget, description: Option<&str>, hovered: Signal<bool>,
    width: f32, height: f32, inset: f32) -> Widget {
    let compact = width < 800.0;
    let copy_width = width - inset * 2.0 - if compact { 0.0 } else { 250.0 };
    let baseline = inset + if compact { 56.0 } else { 0.0 };
    let Some(description) = description.filter(|description| !description.trim().is_empty()) else {
        return Positioned::new(copy).left(inset).width(copy_width).bottom(baseline).into();
    };
    let description = text(description.to_owned(), 14.0, FontWeight::W400,
        Color::rgba(230, 232, 235, 255))
        .max_lines(Some(5)).overflow(TextOverflow::Ellipsis);
    let scrim: Widget = Container::new().width(width).height(height)
        .decoration(BoxDecoration::new().gradient(LinearGradient::new(
            Offset::ZERO, Offset::new(0.0, height),
            GradientStops::new(vec![
                GradientStop { offset: 0.0, color: Color::TRANSPARENT },
                GradientStop { offset: 0.40, color: Color::TRANSPARENT },
                GradientStop { offset: 0.70, color: Color::rgba(6, 9, 10, 120) },
                GradientStop { offset: 1.0, color: Color::rgba(6, 9, 10, 80) },
            ]),
        ))).into();
    Builder::new(move |_| {
        let (description, scrim) = (description.clone(), scrim.clone());
        TweenAnimationBuilder::new(
            if hovered.get() { 1.0f32 } else { 0.0 },
            std::time::Duration::from_millis(if incular::animation::disable_animations() { 0 } else { 380 }),
            move |_, progress, copy| {
                // Let the caption make room before the synopsis fades in.
                let opacity = ((progress - 0.35) / 0.65).clamp(0.0, 1.0);
                layers(vec![
                    IgnorePointer::new(Opacity::new(progress, scrim.clone())).into(),
                    CustomMultiChildLayout::new(
                        HeroCaptionLayout { progress, inset, baseline, copy_width },
                        vec![
                            LayoutId::new("copy", copy.expect("the caption is supplied as the animation child")),
                            LayoutId::new("description", ClipRect::new(IgnorePointer::new(
                                Opacity::new(opacity, description.clone()),
                            ))),
                        ],
                    ).into(),
                ]).fit(incular::widgets::StackFit::Expand)
            },
        ).curve(Curve::EaseOutCubic).child(copy.clone()).into()
    }).into()
}

fn hero(app: &App, meta: &Meta, source: &str, hovered: Signal<bool>) -> Widget {
    let (app, meta, source) = (app.clone(), meta.clone(), source.to_owned());
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width().clamp(320.0, 2400.0);
        let height = hero_height(width);
        let inset = if width >= 800.0 { 28.0 } else { 22.0 };
        let art = meta.background.as_deref().or(meta.poster.as_deref());
        let mut facts: Vec<String> = Vec::new();
        facts.extend(meta.release_line());
        if let Some(rating) = meta.imdb_rating.as_deref().filter(|r| !r.is_empty()) {
            facts.push(format!("IMDb {rating}"));
        }
        if let Some(runtime) = meta.runtime.as_deref().filter(|r| !r.is_empty()) {
            facts.push(runtime.to_owned());
        }
        facts.extend(meta.genres.iter().take(2).cloned());

        let mut copy: Vec<Widget> = vec![
            line(format!("FEATURED  ·  {}", source.to_uppercase()), 10.0, FontWeight::W700,
                Color::rgba(222, 222, 222, 255)),
            gap(8.0),
            line(meta.title().to_owned(), if width >= 800.0 { 30.0 } else { 26.0 },
                FontWeight::W700, TEXT),
        ];
        if !facts.is_empty() {
            copy.push(gap(6.0));
            copy.push(line(facts.join("   ·   "), 12.0, FontWeight::W500,
                Color::rgba(202, 204, 207, 255)));
        }
        let (open, save) = (app.clone(), app.clone());
        let (open_meta, save_meta) = (meta.clone(), meta.clone());
        let saved = app.in_library(meta.id());
        let actions = row(vec![
                button("View details", Tone::Primary, move || open.open_detail(open_meta.clone())),
                hgap(10.0),
                icon_text_button(
                    if saved { icons::check() } else { icons::plus() },
                    if saved { "In library" } else { "Library" },
                    Tone::Glass,
                    move || save.toggle_library(&save_meta),
                ),
            ]);
        let caption = hero_caption(col(copy).cross_axis_alignment(CrossAxisAlignment::Stretch).into(),
            meta.description.as_deref(), hovered.clone(), width, height, inset);
        let actions: Widget = if width >= 800.0 {
            Positioned::new(actions).right(inset).bottom(inset).into()
        } else {
            Positioned::new(actions).left(inset).bottom(inset).into()
        };

        let surface = |color: Color| -> Widget {
            Container::new()
                .width(width)
                .height(height)
                .color(color)
                .into()
        };
        SizedBox::new()
            .width(width)
            .height(height)
            .child(layers(vec![
                surface(SURFACE),
                hero_art(art, width, height),
                // A bottom scrim gives the compact caption contrast while
                // leaving the scene above it unobstructed.
                Container::new().width(width).height(height).decoration(BoxDecoration::new().gradient(LinearGradient::new(
                    Offset::ZERO, Offset::new(0.0, height),
                    GradientStops::new(vec![
                        GradientStop { offset: 0.0, color: Color::TRANSPARENT },
                        GradientStop { offset: 0.40, color: Color::TRANSPARENT },
                        GradientStop { offset: 0.65, color: Color::rgba(6, 9, 10, 55) },
                        GradientStop { offset: 0.82, color: Color::rgba(6, 9, 10, 185) },
                        GradientStop { offset: 1.0, color: Color::rgba(6, 9, 10, 245) },
                    ]),
                ))).into(),
                caption,
                actions,
            ]))
            .into()
    })
    .into()
}

fn carousel_button(glyph: Glyph, label: &str, on_click: impl Fn() + 'static) -> Widget {
    let child = icon(glyph, 15.0, TEXT).accessibility_label(label);
    let control = Container::new().radius(14.0).color(Color::rgba(255, 255, 255, 34))
        .child(icon_button(child, 28.0, on_click));
    super::tooltip::tooltip(label, frosted_glass(control, 14.0, 12.0))
}

fn carousel_indicator(carousel: &FeaturedCarousel, active: bool) -> Widget {
    let carousel = carousel.clone();
    // Only this tiny subtree subscribes to timer updates.
    Builder::new(move |_| {
        let width = if active { 30.0 } else { 5.0 };
        let progress = if active && !incular::animation::disable_animations() {
            carousel.progress.get()
        } else if active { 1.0 } else { 0.0 };
        Container::new().width(width).height(5.0).radius(2.5)
            .color(Color::rgba(255, 255, 255, 85))
            .child(Align::new(Alignment::CENTER_LEFT,
                Container::new().width(width * progress).height(5.0).radius(2.5).color(TEXT),
            )).into()
    }).into()
}

fn featured_carousel(app: &App, slides: Vec<(Meta, String)>) -> Widget {
    let app = app.clone();
    let slides = Rc::new(slides);
    let carousel = app.board.peek(|board| board.carousel.clone());
    let hovered = Signal::new(carousel.hovered.get());
    let last_ready = Rc::new(std::cell::RefCell::new(None::<Widget>));
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width().clamp(320.0, 2400.0);
        let height = hero_height(width);
        let (app, slides, carousel) = (app.clone(), slides.clone(), carousel.clone());
        let hovered = hovered.clone();
        let last_ready = last_ready.clone();
        Builder::new(move |_| {
            let count = slides.len();
            let index = carousel.index.get() % count;
            let paused = carousel.paused.get();
            let (meta, source) = &slides[index];
            // Warm the original composition and its blurred extension.
            if count > 1 {
                let next = &slides[(index + 1) % count].0;
                if let Some(url) = next.background.as_deref().or(next.poster.as_deref()) {
                    let _ = crate::images::picture(url, 1920);
                    let _ = crate::images::blurred(url);
                }
            }
            let slide_key = format!("featured:{}:{}", meta.kind, meta.id());
            let slide = hero(&app, meta, source, hovered.clone()).with_key(slide_key.as_str());
            // A fade must start with the artwork, not finish while it loads.
            let ready = meta.background.as_deref().or(meta.poster.as_deref()).is_none_or(|url| {
                let picture = crate::images::picture(url, 1920).get();
                let blurred = crate::images::blurred(url).get();
                !matches!(picture, crate::images::Picture::Loading)
                    && !matches!(blurred, crate::images::Picture::Loading)
            });
            if ready || last_ready.borrow().is_none() {
                *last_ready.borrow_mut() = Some(slide);
            }
            let mut layers = vec![Widget::from(AnimatedSwitcher::new(
                std::time::Duration::from_millis(if incular::animation::disable_animations() { 0 } else { 700 }),
                last_ready.borrow().clone(),
            ).animated_transition_builder(|child, animation| FadeTransition::from_animation(animation, child))
                .layout_builder(|current, previous| Stack::new(previous.into_iter().chain(current))
                    .fit(incular::widgets::StackFit::Expand).into()))];
            if count > 1 {
                let previous = carousel.clone();
                let mut controls = vec![carousel_button(icons::chevron_left(), "Previous featured title",
                    move || previous.advance(-1, count)), hgap(4.0)];
                // Keep all slides accessible without letting the dots cover the artwork.
                let visible = count.min(7);
                let start = index.saturating_sub(visible / 2).min(count - visible);
                for target in start..start + visible {
                    let select = carousel.clone();
                    controls.push(Widget::from(incular::controls::Button::with_child(
                        carousel_indicator(&carousel, target == index)
                            .accessibility_label(format!("Show featured title {} of {count}", target + 1)),
                    ).style(incular::controls::ButtonStyle {
                        background: Some(Color::TRANSPARENT),
                        border: Some(Border::new(0.0, Color::TRANSPARENT)),
                        padding: Some(EdgeInsets::ZERO),
                        fixed_size: Some(Size::new(if target == index { 38.0 } else { 22.0 }, 28.0)),
                        alignment: Some(Alignment::CENTER),
                        overlay_color: Some(Color::TRANSPARENT.into()),
                        ..Default::default()
                    }).on_click(move || select.select(target)))
                        .accessibility_label(format!("Show featured title {} of {count}", target + 1)));
                }
                let next = carousel.clone();
                controls.push(hgap(4.0));
                controls.push(carousel_button(icons::chevron_right(), "Next featured title",
                    move || next.advance(1, count)));
                if !incular::animation::disable_animations() {
                    let toggle = carousel.clone();
                    let label = if paused { "Resume featured rotation" } else { "Pause featured rotation" };
                    controls.push(hgap(6.0));
                    controls.push(carousel_button(if paused { icons::play() } else { icons::pause() }, label, move || {
                            toggle.toggle_pause();
                        }));
                }
                let controls = Container::new().radius(22.0)
                    .color(Color::rgba(6, 9, 10, 150))
                    .padding(EdgeInsets::symmetric(8.0, 5.0))
                    .child(row(controls));
                layers.push(Positioned::new(controls).right(18.0).top(18.0).into());
            }
            let enter = carousel.clone();
            let exit = carousel.clone();
            let reveal = hovered.clone();
            let conceal = hovered.clone();
            let swipe = carousel.clone();
            let distance = Rc::new(std::cell::Cell::new(0.0f32));
            let start = distance.clone();
            let update = distance.clone();
            let gestures = GestureDetector::new(
                ClipRRect::new(20.0, SizedBox::new().height(height).child(
                    Stack::new(layers).fit(incular::widgets::StackFit::Expand),
                )),
            )
            .on_horizontal_drag_start(move |_| start.set(0.0))
            .on_horizontal_drag_update(move |event| update.set(update.get() + event.delta.x))
            .on_horizontal_drag_end(move |_| {
                if distance.get().abs() >= 48.0 {
                    swipe.advance(if distance.get() < 0.0 { 1 } else { -1 }, count);
                }
            });
            MouseRegion::new(gestures)
                .on_enter(move |_| { enter.hover(true); reveal.set(true); })
                .on_exit(move |_| { exit.hover(false); conceal.set(false); })
                .into()
        }).into()
    }).into()
}

pub fn board(app: &App) -> Widget {
    let (rows, featured) = app.board.read(|board| (board.rows.clone(), board.featured.clone()));
    let mut sections: Vec<Widget> = Vec::new();
    if !featured.is_empty() {
        sections.push(featured_carousel(app, featured));
        sections.push(gap(26.0));
    }
    sections.extend(continue_watching(app));
    let had_rows = !rows.is_empty();
    sections.extend(rows.iter().filter_map(|row| catalog_section(app, row)));
    if !had_rows {
        sections.push(
            SizedBox::new()
                .height(360.0)
                .child(loading_page("Loading your addons… If this stays empty, install an addon with catalogs."))
                .into(),
        );
    }
    smooth(
        &app.controllers.board,
        SingleChildScrollView::new(Padding::new(
            EdgeInsets::only(32.0, 12.0, 32.0, 32.0),
            col(sections).cross_axis_alignment(CrossAxisAlignment::Stretch),
        ))
        .physics(motion::APP_SCROLLED)
        .controller(app.controllers.board.clone()),
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
    let mut bars = vec![
        chip_bar(app, "chips:discover-kind", kind_chips),
        chip_bar(app, "chips:discover-catalog", catalog_chips),
    ];
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
            bars.push(chip_bar(app, "chips:discover-genre", chips));
        }
    }

    let body: Widget = if catalogs.is_empty() {
        Padding::new(PAGE_PADDING, notice("No installed addon offers catalogs.", MUTED)).into()
    } else if let Some(error) = error {
        Padding::new(PAGE_PADDING, notice(format!("Couldn't load: {error}"), DANGER)).into()
    } else if items.is_empty() && loading {
        loading_page("Loading…")
    } else if items.is_empty() {
        Padding::new(PAGE_PADDING, notice("Nothing here.", MUTED)).into()
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
            notice("Your library is empty. Add something from its details page.", MUTED),
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
            chip_bar(app, "chips:library-kind", kind_chips),
            chip_bar(app, "chips:library-sort", sort_chips),
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
    .on_changed(move |value| {
        typed.search_menu_typed(&value);
        typed.search_typed(value);
    });
    let go = {
        let app = app.clone();
        button("Search", Tone::Primary, move || {
            app.submit_search(app.controllers.search.text())
        })
    };
    let field = super::search_menu::field(app, field.into(), 460.0);
    let mut bar: Vec<Widget> = vec![field, hgap(10.0), go];
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
                    .child(horizontal_scroller(&app.controllers.row("chips:search"), row(chips)))
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
            Expanded::new(smooth(
                &app.controllers.search_results,
                SingleChildScrollView::new(Padding::new(
                    PAGE_PADDING,
                    col(sections).cross_axis_alignment(CrossAxisAlignment::Stretch),
                ))
                .physics(motion::APP_SCROLLED)
                .controller(app.controllers.search_results.clone()),
            ))
            .into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch)
        .into(),
    )
}
