//! Title details: metadata on the left, episodes and streams on the right.

use super::motion;
use super::widgets::*;
use crate::state::{App, Load, Quality, StreamGroup};
use crate::addons::types::{Meta, Stream, StreamSource, Video, WatchStatus};
use crate::theme::*;
use incular::prelude::*;
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::widgets::{HitTestBehavior, StackFit};
use std::rc::Rc;

const PANEL_WIDTH: f32 = 460.0;
const PANEL_INNER: f32 = PANEL_WIDTH - 40.0;

/// Frosted-glass fills (over the blurred panel).
const GLASS: Color = Color::rgba(255, 255, 255, 12);
const GLASS_HOVER: Color = Color::rgba(255, 255, 255, 26);
/// Selected / "continue" tiles: brighter glass.
const GLASS_ACCENT: Color = Color::rgba(255, 255, 255, 40);
const GLASS_ACCENT_HOVER: Color = Color::rgba(255, 255, 255, 56);

/// Acrylic: what lies behind `child` is blurred, then `child` (a translucent
/// tint) is drawn over it, inside the rounded corners.
fn acrylic(child: impl Into<Widget>, radius: f32, sigma: f32) -> Widget {
    ClipRRect::new(radius, incular::widgets::BackdropFilter::blur(sigma, child)).into()
}

/// A list tile: a button that grows with its content.
fn list_button(child: Widget, width: f32, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let (background, hover) =
        if selected { (GLASS_ACCENT, GLASS_ACCENT_HOVER) } else { (GLASS, GLASS_HOVER) };
    SizedBox::new()
        .width(width)
        .child(
            Button::with_child(child)
                .style(ButtonStyle {
                    background: Some(background),
                    background_states: Some(StateColor::new(background).hovered(hover)),
                    foreground: Some(TEXT),
                    border: Some(Border::new(0.0, Color::TRANSPARENT)),
                    border_radius: Some(12.0),
                    padding: Some(EdgeInsets::all(12.0)),
                    alignment: Some(Alignment::CENTER_LEFT),
                    overlay_color: Some(Color::TRANSPARENT.into()),
                    animation_duration: Some(std::time::Duration::ZERO),
                    ..ButtonStyle::default()
                })
                .on_click(on_click),
        )
        .into()
}

fn info_pane(app: &App, meta: &Meta, loading: bool, failure: Option<&str>) -> Widget {
    let mut facts: Vec<String> = Vec::new();
    facts.extend(meta.release_line());
    facts.extend(meta.runtime.clone());
    if let Some(rating) = &meta.imdb_rating {
        facts.push(format!("IMDb {rating}"));
    }
    let mut children: Vec<Widget> = vec![
        text(meta.title(), 38.0, FontWeight::W700, TEXT).into(),
        gap(10.0),
        text(facts.join("   ·   "), 14.0, FontWeight::W500, MUTED).into(),
    ];
    if !meta.genres.is_empty() {
        children.push(gap(14.0));
        children.push(
            Wrap::new(meta.genres.iter().take(6).map(|genre| {
                Container::new()
                    .padding(EdgeInsets::symmetric(11.0, 5.0))
                    .decoration(
                        BoxDecoration::new()
                            .color(Color::rgba(255, 255, 255, 22))
                            .border_radius(BorderRadius::all(Radius::circular(12.0))),
                    )
                    .child(text(genre.clone(), 12.0, FontWeight::W500, TEXT))
            }))
            .spacing(8.0)
            .run_spacing(8.0)
            .into(),
        );
    }

    let in_library = app.in_library(meta.id());
    let watched = app.is_watched(meta.id());
    let status = app.watch_status(meta.id());
    let mut actions: Vec<Widget> = vec![{
        let (app, meta) = (app.clone(), meta.clone());
        icon_text_button(
            if in_library { icons::check() } else { icons::plus() },
            if in_library { "In library" } else { "Add to library" },
            if in_library { Tone::Glass } else { Tone::Primary },
            move || app.toggle_library(&meta),
        )
    }];
    {
        let (app, meta) = (app.clone(), meta.clone());
        actions.push(icon_text_button(
            if watched { icons::check_circle() } else { icons::eye() },
            if watched { "Watched" } else { "Mark as watched" },
            Tone::Glass,
            move || app.toggle_watched(&meta),
        ));
    }
    for (panel, glyph, label) in [
        (
            DetailPanel::Status,
            status.map_or(icons::bookmark(), status_glyph),
            status.map_or("Status", WatchStatus::label),
        ),
        (DetailPanel::Playlists, icons::playlist(), "Playlists"),
    ] {
        actions.push(icon_text_button(glyph, label, Tone::Glass, move || {
            DETAIL_PANEL.with(|open| {
                let next = if open.get_untracked() == Some(panel) { None } else { Some(panel) };
                open.set(next);
            });
        }));
    }
    let trailer = meta
        .trailer_streams
        .iter()
        .find_map(|s| s.yt_id.clone())
        .or_else(|| meta.trailers.first().map(|t| t.source.clone()))
        .filter(|id| !id.is_empty());
    if let Some(id) = trailer {
        let app = app.clone();
        actions.push(button("Trailer", Tone::Glass, move || {
            app.play_stream(&Stream {
                yt_id: Some(id.clone()),
                ..Stream::default()
            })
        }));
    }
    // The page shows the art only blurred; this opens it sharp.
    if let Some(art) = meta.background.clone().or_else(|| meta.poster.clone()).filter(|u| u.starts_with("http")) {
        let app = app.clone();
        actions.push(super::tooltip::tooltip(
            "View artwork",
            Widget::from(small_icon_button(icons::maximize(), None, Tone::Glass, move || { app.artwork_view.set(Some(art.clone())); }))
                .accessibility_label("View artwork"),
        ));
    }
    if meta.id().starts_with("tt") {
        let url = format!("https://www.imdb.com/title/{}/", meta.id());
        let app = app.clone();
        actions.push(button("IMDb", Tone::Glass, move || {
            app.play_stream(&Stream {
                external_url: Some(url.clone()),
                ..Stream::default()
            })
        }));
    }
    children.push(gap(22.0));
    children.push(Wrap::new(actions).spacing(10.0).run_spacing(10.0).into());
    children.push(detail_panel(app, meta));

    if loading {
        children.push(gap(20.0));
        children.push(loading_line("Loading details…"));
    }
    if let Some(failure) = failure {
        children.push(gap(20.0));
        children.push(text(
            format!("Details unavailable: {failure}"),
            13.0,
            FontWeight::W400,
            FAINT,
        )
        .into());
    }
    if let Some(description) = &meta.description {
        children.push(gap(22.0));
        children.push(
            TextStyle::new()
                .font_size(15.0)
                .color(Color::rgba(222, 222, 222, 255))
                .line_height_absolute(23.0)
                .pipe(|style| Text::new(description.clone()).style(style))
                .into(),
        );
    }
    for (label, people) in [
        ("Cast", &meta.cast),
        ("Director", &meta.director),
        ("Writer", &meta.writer),
    ] {
        if !people.is_empty() {
            children.push(gap(16.0));
            children.push(text(label, 12.0, FontWeight::W600, FAINT).into());
            children.push(gap(4.0));
            children.push(
                text(
                    people.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                    13.5,
                    FontWeight::W400,
                    MUTED,
                )
                .into(),
            );
        }
    }

    let poster = net_image(meta.poster.as_deref(), 200.0, 300.0, 14.0, 342, meta.title());
    smooth(
        &app.controllers.detail,
        SingleChildScrollView::new(Padding::new(
            EdgeInsets::only(40.0, 8.0, 36.0, 40.0),
            hrow(vec![
                poster,
                hgap(30.0),
                Expanded::new(col(children)).into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Start),
        ))
        .physics(motion::APP_SCROLLED)
        .controller(app.controllers.detail.clone()),
    )
}

/// The pickers that open under the title's buttons.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailPanel {
    Status,
    Playlists,
}

thread_local! {
    static DETAIL_PANEL: Signal<Option<DetailPanel>> = Signal::new(None);
}

pub(super) fn status_glyph(status: WatchStatus) -> Glyph {
    match status {
        WatchStatus::Watching => icons::play_circle(),
        WatchStatus::PlanToWatch => icons::clock_circle(),
        WatchStatus::OnHold => icons::pause_circle(),
        WatchStatus::Watched => icons::check_circle(),
        WatchStatus::Dropped => icons::close_circle(),
    }
}

/// A pill with an icon: a status or a playlist to pick.
fn pick(glyph: Glyph, label: String, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let (background, hover, foreground) = if selected {
        (SELECTED, SELECTED, ON_SELECTED)
    } else {
        (Color::rgba(255, 255, 255, 22), Color::rgba(255, 255, 255, 40), TEXT)
    };
    Button::with_child(row(vec![
        icon(glyph, 14.0, foreground),
        hgap(6.0),
        text(label, 12.5, FontWeight::W600, foreground).into(),
    ]))
    .style(ButtonStyle {
        background: Some(background),
        background_states: Some(StateColor::new(background).hovered(hover)),
        foreground: Some(foreground),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        border_radius: Some(15.0),
        padding: Some(EdgeInsets::only(10.0, 0.0, 13.0, 0.0)),
        height: Some(30.0),
        overlay_color: Some(Color::TRANSPARENT.into()),
        animation_duration: Some(std::time::Duration::ZERO),
        ..ButtonStyle::default()
    })
    .on_click(on_click)
    .into()
}

/// The open status or playlist picker. Its own builder: opening it does
/// not rebuild the page.
fn detail_panel(app: &App, meta: &Meta) -> Widget {
    let (app, meta) = (app.clone(), meta.clone());
    Builder::new(move |_| {
        let Some(panel) = DETAIL_PANEL.with(|open| open.get()) else {
            return SizedBox::shrink().into();
        };
        let mut picks: Vec<Widget> = Vec::new();
        let title = match panel {
            DetailPanel::Status => {
                let current = app.watch_status(meta.id());
                for status in WatchStatus::ALL {
                    let (app, meta) = (app.clone(), meta.clone());
                    let selected = current == Some(status);
                    picks.push(pick(status_glyph(status), status.label().to_owned(), selected, move || {
                        app.set_watch_status(&meta, if selected { None } else { Some(status) });
                    }));
                }
                "Where are you with it?"
            }
            DetailPanel::Playlists => {
                let lists = app.playlists.read(|all| {
                    all.iter().map(|p| (p.id.clone(), p.name.clone(), p.items.len())).collect::<Vec<_>>()
                });
                for (id, name, _) in lists {
                    let inside = app.in_playlist(&id, meta.id());
                    let (app, meta) = (app.clone(), meta.clone());
                    picks.push(pick(
                        if inside { icons::check() } else { icons::plus() },
                        name,
                        inside,
                        move || app.toggle_in_playlist(&id, &meta),
                    ));
                }
                "Add to a playlist"
            }
        };
        let mut children: Vec<Widget> = vec![
            text(title, 12.0, FontWeight::W600, FAINT).into(),
            gap(10.0),
        ];
        if !picks.is_empty() {
            children.push(Wrap::new(picks).spacing(8.0).run_spacing(8.0).into());
        }
        if panel == DetailPanel::Playlists {
            let (submit, click) = ((app.clone(), meta.clone()), (app.clone(), meta.clone()));
            let create = |app: &App, meta: &Meta| {
                if let Some(id) = app.create_playlist(&app.controllers.playlist_name.text()) {
                    app.controllers.playlist_name.set_text("");
                    app.toggle_in_playlist(&id, meta);
                }
            };
            if !children.is_empty() {
                children.push(gap(12.0));
            }
            children.push(
                row(vec![
                    input(&app.controllers.playlist_name, "New playlist", 220.0, move |_| create(&submit.0, &submit.1))
                        .size(Size::new(220.0, 32.0))
                        .into(),
                    hgap(8.0),
                    small_icon_button(icons::plus(), Some("Create"), Tone::Glass, move || create(&click.0, &click.1)),
                ])
                .into(),
            );
        }
        Padding::new(
            EdgeInsets::only(0.0, 14.0, 0.0, 0.0),
            Container::new()
                .padding(EdgeInsets::all(14.0))
                .decoration(
                    BoxDecoration::new()
                        .color(Color::rgba(0, 0, 0, 70))
                        .border_radius(BorderRadius::all(Radius::circular(14.0))),
                )
                .child(col(children)),
        )
        .into()
    })
    .into()
}

trait Pipe: Sized {
    fn pipe<R>(self, apply: impl FnOnce(Self) -> R) -> R {
        apply(self)
    }
}
impl<T> Pipe for T {}

fn stream_tile(app: &App, stream: &Stream) -> Widget {
    let badge = match stream.source() {
        StreamSource::Torrent { .. } => "P2P",
        StreamSource::YouTube(_) => "YouTube",
        StreamSource::External(_) => "Opens in browser",
        StreamSource::Url(_) => "Direct",
        StreamSource::Unsupported => "Unsupported",
    };
    let headline = stream.headline();
    let headline = if headline.is_empty() { "Stream".to_owned() } else { headline.replace('\n', " ") };
    let details = stream.details();
    let parts = super::stream_details::split(&details);
    let mut lines: Vec<Widget> = vec![
        hrow(vec![
            Flexible::new(line(headline.clone(), 13.5, FontWeight::W600, TEXT)).into(),
            hgap(8.0),
            text(badge, 11.0, FontWeight::W600, ACCENT).into(),
        ])
        .into(),
    ];
    if !parts.description.is_empty() {
        lines.push(gap(4.0));
        lines.push(
            Text::new(breakable(parts.description))
                .style(style(12.0, FontWeight::W400, MUTED))
                // Limit only the release name. Unknown provider formats keep
                // all lines rather than silently losing their technical data.
                .max_lines(if parts.metadata.is_empty() { None } else { Some(2) })
                .overflow(TextOverflow::Ellipsis)
                .into(),
        );
    }
    if !parts.metadata.is_empty() {
        lines.push(gap(6.0));
        lines.push(Text::new(breakable(parts.metadata))
            .style(style(12.0, FontWeight::W500, TEXT))
            .into());
    } else if let Some(size) = stream.behavior_hints.video_size.filter(|size| *size > 0) {
        lines.push(gap(6.0));
        lines.push(text(format!("Size {}", crate::util::format_bytes(size)), 12.0, FontWeight::W500, TEXT).into());
    }
    let downloadable = matches!(
        stream.source(),
        StreamSource::Url(_) | StreamSource::Torrent { .. }
    );
    // The download button sits beside the tile, not inside it: a tap on a
    // nested button would also reach the tile's own gesture detector.
    let tile_width = if downloadable { PANEL_INNER - DOWNLOAD_BUTTON - 8.0 } else { PANEL_INNER };
    let (play, play_stream) = (app.clone(), stream.clone());
    // The full, untruncated text on hover.
    let full = if details.is_empty() { headline } else { format!("{headline}
{details}") };
    let tile = list_button(
        super::tooltip::tooltip(full, SizedBox::new().width(tile_width - 26.0).child(col(lines))),
        tile_width,
        false,
        move || play.play_stream(&play_stream),
    );
    if !downloadable {
        return tile;
    }
    let (save, save_stream) = (app.clone(), stream.clone());
    hrow(vec![
        tile,
        hgap(8.0),
        download_button(move || save.download_stream(&save_stream)),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .into()
}

/// Release names like `Movie.2025.1080p.WEBRip-GROUP` have no spaces, so
/// they cannot wrap and spill out of the tile. A zero-width space after each
/// separator gives the line breaker somewhere to break.
fn breakable(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + value.len() / 4);
    for ch in value.chars() {
        out.push(ch);
        if matches!(ch, '.' | '_' | '-' | '/' | '[' | ']' | '(' | ')' | ',') {
            out.push('\u{200B}');
        }
    }
    out
}

const DOWNLOAD_BUTTON: f32 = 40.0;
const WATCHED_BUTTON: f32 = 40.0;

fn download_button(on_click: impl Fn() + 'static) -> Widget {
    side_button(icon(icons::download(), 18.0, MUTED), on_click)
}

/// A square glass button beside a list tile.
fn side_button(child: Widget, on_click: impl Fn() + 'static) -> Widget {
    Button::with_child(child)
        .style(ButtonStyle {
            background: Some(GLASS),
            background_states: Some(StateColor::new(GLASS).hovered(GLASS_HOVER)),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(12.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(DOWNLOAD_BUTTON, DOWNLOAD_BUTTON)),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

fn streams_pane(app: &App, groups: &[StreamGroup], video: &Video, is_series: bool) -> Widget {
    let mut children: Vec<Widget> = Vec::new();
    if is_series {
        let app = app.clone();
        children.push(
            row(vec![
                small_icon_button(icons::chevron_left(), Some("Episodes"), Tone::Glass, move || {
                    app.clear_video()
                }),
                hgap(12.0),
                SizedBox::new()
                    .width(PANEL_INNER - 130.0)
                    .child(line(video.label(), 13.5, FontWeight::W600, TEXT))
                    .into(),
            ])
            .into(),
        );
        children.push(gap(14.0));
    }
    children.push(section("Streams"));
    children.push(gap(10.0));

    // The stream played last time for this title / episode, pinned on top.
    // The addon's fresh copy is preferred (its seeders etc. are current).
    let last = app.detail.peek(|d| {
        let meta = d.best_meta()?;
        app.last_stream(meta.id(), d.video.as_ref().map(|v| v.id()))
    });
    let last = last.map(|saved| {
        groups
            .iter()
            .filter_map(|g| match &g.items {
                Load::Ready(streams) => Some((g, streams)),
                _ => None,
            })
            .find_map(|(g, streams)| {
                streams
                    .iter()
                    .find(|s| s.same_source(&saved))
                    .map(|s| (Some(g.addon_name.clone()), s.clone()))
            })
            .unwrap_or((None, saved))
    });
    if let Some((addon, stream)) = &last {
        let label = match addon {
            Some(addon) => format!("LAST USED · {addon}"),
            None => "LAST USED".to_owned(),
        };
        children.push(
            row(vec![
                icon(icons::rotate_ccw(), 13.0, ACCENT),
                hgap(6.0),
                text(label, 11.0, FontWeight::W700, ACCENT).into(),
            ])
            .into(),
        );
        children.push(gap(6.0));
        children.push(stream_tile(app, stream));
        children.push(gap(16.0));
    }

    let loading = groups.iter().filter(|g| g.items == Load::Loading).count();
    let quality = app.detail.peek(|d| d.quality);
    let everything: usize = groups
        .iter()
        .map(|g| match &g.items {
            Load::Ready(streams) => streams.len(),
            _ => 0,
        })
        .sum();
    // Worth filtering only when there is a long list to wade through.
    if everything > 6 {
        let mut chips: Vec<Widget> = Vec::new();
        for option in Quality::ALL {
            let count = groups
                .iter()
                .filter_map(|g| match &g.items {
                    Load::Ready(streams) => Some(streams),
                    _ => None,
                })
                .flatten()
                .filter(|s| option == Quality::All || Quality::of(s) == option)
                .count();
            if count == 0 {
                continue;
            }
            let app = app.clone();
            chips.push(chip(
                format!("{} {count}", option.label()),
                quality == option,
                move || app.detail.write(|d| d.quality = option),
            ));
            chips.push(hgap(6.0));
        }
        children.push(
            SizedBox::new()
                .width(PANEL_INNER)
                .height(32.0)
                .child(horizontal_scroller(&app.controllers.row("chips:quality"), row(chips)))
                .into(),
        );
        children.push(gap(14.0));
    }
    let mut total = 0;
    for group in groups {
        if let Load::Ready(streams) = &group.items {
            let streams: Vec<&Stream> = streams
                .iter()
                .filter(|s| quality == Quality::All || Quality::of(s) == quality)
                // Already shown on top.
                .filter(|s| !last.as_ref().is_some_and(|(_, l)| l.same_source(s)))
                .collect();
            if streams.is_empty() {
                continue;
            }
            total += streams.len();
            children.push(text(group.addon_name.clone(), 12.0, FontWeight::W600, FAINT).into());
            children.push(gap(6.0));
            for stream in streams {
                children.push(stream_tile(app, stream));
                children.push(gap(8.0));
            }
            children.push(gap(8.0));
        }
    }
    let asking = format!("Asking {loading} addon{}…", if loading == 1 { "" } else { "s" });
    if loading > 0 && total == 0 {
        // Nothing to list yet: the spinner takes the middle of the panel.
        return Padding::all(
            20.0,
            vcol(vec![col(children).into(), Expanded::new(loading_page(asking)).into()])
                .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into();
    }
    if loading > 0 {
        children.push(gap(8.0));
        children.push(SizedBox::new().width(PANEL_INNER).child(Align::new(Alignment::CENTER, loading_line(asking))).into());
    } else if total == 0 {
        children.push(muted(if groups.is_empty() {
            "No installed addon provides streams for this. Install a stream addon under Addons."
        } else {
            "No streams were found for this title."
        }));
    }
    smooth(
        &app.controllers.streams,
        SingleChildScrollView::new(Padding::new(EdgeInsets::all(20.0), col(children)))
            .physics(motion::APP_SCROLLED)
            .controller(app.controllers.streams.clone()),
    )
}

/// "Continue" for the episode watched last: resumes it with the stream used
/// last time, or opens its streams when none is remembered.
fn continue_card(app: &App, meta: &Meta, resume: Option<&str>) -> Option<Widget> {
    let video = meta.videos.iter().find(|v| Some(v.id()) == resume)?.clone();
    let progress = app.library.read(|l| l.get(meta.id()).map(|i| i.progress())).unwrap_or(0.0);
    let last = app.last_stream(meta.id(), Some(video.id()));
    let detail = match (&last, progress > 0.01) {
        (Some(_), true) => format!("{:.0}% watched · resumes with the stream you used", progress * 100.0),
        (Some(_), false) => "Plays with the stream you used last time".to_owned(),
        (None, true) => format!("{:.0}% watched · pick a stream to resume", progress * 100.0),
        (None, false) => "Pick a stream to start".to_owned(),
    };
    let content = hrow(vec![
        Container::new()
            .width(38.0)
            .height(38.0)
            .radius(19.0)
            .color(ACCENT)
            .alignment(Alignment::CENTER)
            .child(icon(icons::play(), 16.0, ON_ACCENT))
            .into(),
        hgap(14.0),
        SizedBox::new()
            .width(PANEL_INNER - 38.0 - 14.0 - 32.0)
            .child(col(vec![
                Text::new(format!("Continue {}", video.label()))
                    .style(style(14.0, FontWeight::W700, TEXT))
                    .max_lines(Some(1))
                    .overflow(TextOverflow::Ellipsis)
                    .into(),
                gap(3.0),
                text(detail, 11.5, FontWeight::W500, ACCENT).into(),
            ]))
            .into(),
    ])
    .main_axis_size(MainAxisSize::Min)
    .cross_axis_alignment(CrossAxisAlignment::Center);
    let (app, meta) = (app.clone(), meta.clone());
    Some(list_button(content.into(), PANEL_INNER, true, move || {
        // Open the episode either way: its streams (with the last one on
        // top) are there when the player is closed again.
        app.select_video(video.clone());
        if let Some(stream) = app.last_stream(meta.id(), Some(video.id())) {
            let meta = app.detail.peek(|d| d.best_meta()).unwrap_or_else(|| meta.clone());
            app.play_with(&stream, Some(meta), Some(video.clone()));
        }
    }))
}

fn episodes_pane(app: &App, meta: &Meta, season: Option<u32>) -> Widget {
    let seasons = meta.seasons();
    let resume = app
        .library
        .read(|l| l.get(meta.id()).and_then(|i| i.state.video_id.clone()));
    let mut children: Vec<Widget> = Vec::new();
    let picker = seasons.len() > 1;
    let entries: Rc<Vec<SeasonEntry>> = Rc::new(
        seasons
            .iter()
            .map(|&number| SeasonEntry {
                number,
                label: season_label(number),
                episodes: meta.episodes(Some(number)).len(),
            })
            .collect(),
    );
    if let Some(card) = continue_card(app, meta, resume.as_deref()) {
        children.push(card);
        children.push(gap(14.0));
    }
    if picker {
        children.push(season_trigger(app, entries.clone(), season));
        children.push(gap(14.0));
    }
    let season = if seasons.is_empty() { None } else { season };
    for video in meta.episodes(season) {
        let is_resume = resume.as_deref() == Some(video.id());
        let released = video
            .released
            .as_deref()
            .map(|r| r.chars().take(10).collect::<String>())
            .unwrap_or_default();
        let watched = app.video_watched(meta.id(), video.id());
        let caption = if is_resume {
            "Continue here".to_owned()
        } else if watched {
            if released.is_empty() { "Watched".to_owned() } else { format!("Watched · {released}") }
        } else {
            released
        };
        // The watched toggle sits beside the tile, not inside it (a nested
        // button would also open the episode).
        let tile_width = PANEL_INNER - WATCHED_BUTTON - 8.0;
        let tile = hrow(vec![
            net_image(video.thumbnail.as_deref(), 112.0, 63.0, 8.0, 320, &video.label()),
            hgap(12.0),
            SizedBox::new()
                .width(tile_width - 112.0 - 40.0)
                .child(col(vec![
                    Text::new(video.label())
                        .style(style(13.5, FontWeight::W600, TEXT))
                        .max_lines(Some(2))
                        .overflow(TextOverflow::Ellipsis)
                        .into(),
                    gap(4.0),
                    text(
                        caption,
                        11.5,
                        FontWeight::W500,
                        if is_resume { ACCENT } else { FAINT },
                    )
                    .into(),
                ]))
                .into(),
        ])
        .main_axis_size(MainAxisSize::Min)
        .cross_axis_alignment(CrossAxisAlignment::Center);
        let (open, mark, marked_meta, video_id) = (app.clone(), app.clone(), meta.clone(), video.id().to_owned());
        let label = video.label();
        let tip = if watched { "Mark as not watched" } else { "Mark as watched" };
        children.push(
            hrow(vec![
                list_button(super::tooltip::tooltip(label, tile), tile_width, is_resume, move || {
                    open.select_video(video.clone())
                }),
                hgap(8.0),
                super::tooltip::tooltip(
                    tip,
                    Widget::from(side_button(
                        icon(icons::check_circle(), 20.0, if watched { GOOD } else { Color::rgba(255, 255, 255, 70) }),
                        move || mark.toggle_video_watched(&marked_meta, &video_id),
                    ))
                    .accessibility_label(tip),
                ),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into(),
        );
        children.push(gap(8.0));
    }
    let list: Widget = smooth(
        &app.controllers.streams,
        SingleChildScrollView::new(Padding::new(EdgeInsets::all(20.0), col(children)))
            .physics(motion::APP_SCROLLED)
            .controller(app.controllers.streams.clone()),
    );
    if !picker {
        return list;
    }
    // The dropdown layer exists only while open.
    let app = app.clone();
    Builder::new(move |_| {
        if app.season_open.get() {
            layers(vec![
                list.clone(),
                season_overlay(&app, entries.clone(), season),
            ])
                .fit(StackFit::Expand)
                .into()
        } else {
            list.clone()
        }
    })
    .into()
}

const PICKER_HEIGHT: f32 = 40.0;
const PICKER_ROW: f32 = 38.0;

struct SeasonEntry {
    number: u32,
    label: String,
    episodes: usize,
}

fn season_label(number: u32) -> String {
    if number == 0 { "Specials".to_owned() } else { format!("Season {number}") }
}

fn episodes_label(count: usize) -> String {
    format!("{count} episode{}", if count == 1 { "" } else { "s" })
}

/// Entries matching the search text: by label ("season 3", "specials") or
/// by bare number ("3").
fn matching(entries: &[SeasonEntry], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            query.is_empty()
                || e.label.to_lowercase().contains(&query)
                || e.number.to_string() == query.trim_start_matches(['s', 'S'])
        })
        .map(|(i, _)| i)
        .collect()
}

/// The combobox trigger, at the top of the episode list. Its own builder,
/// so opening the picker doesn't rebuild the episodes.
fn season_trigger(app: &App, entries: Rc<Vec<SeasonEntry>>, current: Option<u32>) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let open = app.season_open.get();
        let selected = entries.iter().find(|e| Some(e.number) == current);
        let toggle = app.clone();
        Button::with_child(
            SizedBox::new()
                .width(PANEL_INNER - 28.0)
                .child(
                    hrow(vec![
                        text(
                            selected.map_or("Choose a season".to_owned(), |e| e.label.clone()),
                            14.0,
                            FontWeight::W600,
                            TEXT,
                        )
                        .into(),
                        hgap(10.0),
                        text(
                            selected.map_or(String::new(), |e| episodes_label(e.episodes)),
                            12.0,
                            FontWeight::W400,
                            FAINT,
                        )
                        .into(),
                        Spacer::new().into(),
                        icon(
                            if open { icons::chevron_up() } else { icons::chevron_down() },
                            18.0,
                            MUTED,
                        ),
                    ])
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                ),
        )
        .style(ButtonStyle {
            background: Some(GLASS),
            background_states: Some(StateColor::new(GLASS).hovered(GLASS_HOVER)),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(12.0),
            padding: Some(EdgeInsets::symmetric(14.0, 0.0)),
            fixed_size: Some(Size::new(PANEL_INNER, PICKER_HEIGHT)),
            alignment: Some(Alignment::CENTER_LEFT),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(move || toggle.toggle_season_picker())
        .into()
    })
    .into()
}

/// The open dropdown, floating over the episode list just under the
/// trigger, with a click-away barrier.
fn season_overlay(app: &App, entries: Rc<Vec<SeasonEntry>>, current: Option<u32>) -> Widget {
    let close = app.clone();
    layers(vec![
        GestureDetector::new(Container::new().color(Color::rgba(12, 12, 12, 120)))
            .behavior(HitTestBehavior::Opaque)
            .on_tap(move || close.close_season_picker())
            .into(),
        Positioned::new(motion::pop_in(season_dropdown(app, entries, current), -6.0))
            .left(20.0)
            .top(20.0 + PICKER_HEIGHT + 6.0)
            .into(),
    ])
    .fit(StackFit::Expand)
    .into()
}

fn season_dropdown(app: &App, entries: Rc<Vec<SeasonEntry>>, current: Option<u32>) -> Widget {
    let (submit_app, submit_entries) = (app.clone(), entries.clone());
    let changed = app.clone();
    let search = input(
        &app.controllers.season_search,
        "Search seasons",
        PANEL_INNER - 20.0,
        move |query| {
            // Enter picks the first match.
            if let Some(&first) = matching(&submit_entries, &query).first() {
                submit_app.select_season(submit_entries[first].number);
            }
        },
    )
    .on_changed(move |query| changed.filter_seasons(query));

    // The results rebuild on their own as the filter changes, leaving the
    // text field (and its focus) alone.
    let results = {
        let app = app.clone();
        Builder::new(move |_| {
            let found = Rc::new(matching(&entries, &app.season_filter.get()));
            if found.is_empty() {
                return SizedBox::new()
                    .height(PICKER_ROW)
                    .child(Padding::new(
                        EdgeInsets::only(12.0, 10.0, 0.0, 0.0),
                        muted("No season matches"),
                    ))
                    .into();
            }
            let height = (found.len() as f32 * PICKER_ROW).min(PICKER_ROW * 7.0);
            let controller = app.controllers.season_list.clone();
            let (entries, app) = (entries.clone(), app.clone());
            let count = found.len();
            SizedBox::new()
                .height(height)
                .child(smooth(
                    &controller,
                    ListView::builder(count, move |index| {
                        let entry = &entries[found[index]];
                        season_option(&app, entry, Some(entry.number) == current)
                    })
                    .item_extent(PICKER_ROW)
                    .physics(motion::APP_SCROLLED)
                    .controller(controller.clone()),
                ))
                .into()
        })
    };

    let menu = Container::new()
        .width(PANEL_INNER)
        .padding(EdgeInsets::all(10.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(36, 36, 36, 170))
                .border_radius(BorderRadius::all(Radius::circular(14.0)))
                .box_shadow(vec![BoxShadow::new(
                    Color::rgba(0, 0, 0, 140),
                    Offset::new(0.0, 10.0),
                    28.0,
                    0.0,
                )]),
        )
        .child(
            vcol(vec![search.into(), gap(8.0), results.into()])
                .main_axis_size(MainAxisSize::Min)
                .cross_axis_alignment(CrossAxisAlignment::Start),
        );
    acrylic(menu, 14.0, 32.0)
}

fn season_option(app: &App, entry: &SeasonEntry, selected: bool) -> Widget {
    let background = if selected { GLASS_ACCENT } else { Color::TRANSPARENT };
    let color = if selected { ACCENT } else { TEXT };
    let mut content: Vec<Widget> = vec![
        text(entry.label.clone(), 13.5, FontWeight::W600, color).into(),
        Spacer::new().into(),
        text(episodes_label(entry.episodes), 12.0, FontWeight::W400, FAINT).into(),
    ];
    content.push(hgap(10.0));
    content.push(if selected {
        icon(icons::check(), 16.0, ACCENT)
    } else {
        SizedBox::new().width(16.0).into()
    });
    let (app, number) = (app.clone(), entry.number);
    Button::with_child(
        SizedBox::new()
            .width(PANEL_INNER - 44.0)
            .child(hrow(content).cross_axis_alignment(CrossAxisAlignment::Center)),
    )
    .style(ButtonStyle {
        background: Some(background),
        background_states: Some(StateColor::new(background).hovered(if selected {
            GLASS_ACCENT_HOVER
        } else {
            GLASS_HOVER
        })),
        foreground: Some(color),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        border_radius: Some(9.0),
        padding: Some(EdgeInsets::symmetric(12.0, 0.0)),
        fixed_size: Some(Size::new(PANEL_INNER - 20.0, PICKER_ROW - 4.0)),
        alignment: Some(Alignment::CENTER_LEFT),
        overlay_color: Some(Color::TRANSPARENT.into()),
        animation_duration: Some(std::time::Duration::ZERO),
        ..ButtonStyle::default()
    })
    .on_click(move || app.select_season(number))
    .into()
}

pub fn screen(app: &App) -> Widget {
    let (state, preview, season, video, groups) = app.detail.read(|d| {
        (
            d.meta.clone(),
            d.preview.clone(),
            d.season,
            d.video.clone(),
            d.streams.clone(),
        )
    });
    let (meta, loading, failure) = match &state {
        Load::Ready(meta) => (meta.clone(), false, None),
        Load::Failed(error) => (preview.unwrap_or_default(), false, Some(error.clone())),
        _ => (preview.unwrap_or_default(), true, None),
    };
    let is_series = meta.videos.len() > 1;

    let panel: Widget = match (&video, is_series) {
        (Some(video), _) => streams_pane(app, &groups, video, is_series),
        (None, true) => episodes_pane(app, &meta, season),
        (None, false) if loading => loading_page("Loading…"),
        (None, false) => Padding::all(20.0, muted("Nothing to play.")).into(),
    };

    let back = {
        let app = app.clone();
        icon_button(icon(icons::arrow_left(), 20.0, TEXT), 40.0, move || app.back())
    };

    let content = vcol(vec![
        Padding::new(EdgeInsets::only(28.0, 18.0, 28.0, 8.0), row(vec![back])).into(),
        Expanded::new(
            hrow(vec![
                Expanded::new(info_pane(app, &meta, loading, failure.as_deref())).into(),
                Padding::new(
                    EdgeInsets::only(0.0, 0.0, 24.0, 24.0),
                    acrylic(
                        Container::new()
                            .width(PANEL_WIDTH)
                            .decoration(
                                BoxDecoration::new()
                                    .color(Color::rgba(19, 19, 19, 105))
                                    .border_radius(BorderRadius::all(Radius::circular(18.0))),
                            )
                            .child(panel),
                        18.0,
                        40.0,
                    ),
                )
                .into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into(),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Stretch);

    // The title's artwork shows as the window's blurred ambient backdrop
    // (`ui::ambient`), so the page itself is transparent.
    content.into()
}
