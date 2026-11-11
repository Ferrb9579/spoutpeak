//! Title details: metadata on the left, episodes and streams on the right.

use super::widgets::*;
use crate::state::{App, Load, Quality, StreamGroup};
use crate::media::types::{Meta, Stream, StreamSource, Video};
use crate::theme::*;
use incular::prelude::*;
use incular::widgets::{HitTestBehavior, StackFit};

const PANEL_WIDTH: f32 = 460.0;
const PANEL_INNER: f32 = PANEL_WIDTH - 40.0;

/// A list tile that grows with its content (controls `Button` has a fixed
/// height), with its own hover state.
fn list_button(child: Widget, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let hovered = Signal::new(false);
    let on_click = std::rc::Rc::new(on_click);
    Builder::new(move |_| {
        let background = if selected {
            ACCENT_DIM
        } else if hovered.get() {
            SURFACE_HOVER
        } else {
            SURFACE
        };
        let (enter, exit, click) = (hovered.clone(), hovered.clone(), on_click.clone());
        MouseRegion::new(
            GestureDetector::new(
                Container::new()
                    .width(PANEL_INNER)
                    .padding(EdgeInsets::all(12.0))
                    .decoration(
                        BoxDecoration::new()
                            .color(background)
                            .border(Border::new(1.0, if selected { ACCENT } else { LINE }))
                            .border_radius(BorderRadius::all(Radius::circular(12.0))),
                    )
                    .child(child.clone()),
            )
            .behavior(HitTestBehavior::Opaque)
            .on_tap(move || click()),
        )
        .on_enter(move |_| {
            enter.set(true);
        })
        .on_exit(move |_| {
            exit.set(false);
        })
        .into()
    })
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
    let mut actions: Vec<Widget> = vec![{
        let (app, meta) = (app.clone(), meta.clone());
        icon_text_button(
            if in_library { icons::check() } else { icons::plus() },
            if in_library { "In library" } else { "Add to library" },
            if in_library { Tone::Neutral } else { Tone::Primary },
            move || app.toggle_library(&meta),
        )
    }];
    let trailer = meta
        .trailer_streams
        .iter()
        .find_map(|s| s.yt_id.clone())
        .or_else(|| meta.trailers.first().map(|t| t.source.clone()))
        .filter(|id| !id.is_empty());
    if let Some(id) = trailer {
        actions.push(hgap(10.0));
        let app = app.clone();
        actions.push(button("Trailer", Tone::Neutral, move || {
            app.play_stream(&Stream {
                yt_id: Some(id.clone()),
                ..Stream::default()
            })
        }));
    }
    if meta.id().starts_with("tt") {
        actions.push(hgap(10.0));
        let url = format!("https://www.imdb.com/title/{}/", meta.id());
        let app = app.clone();
        actions.push(button("IMDb", Tone::Ghost, move || {
            app.play_stream(&Stream {
                external_url: Some(url.clone()),
                ..Stream::default()
            })
        }));
    }
    children.push(gap(22.0));
    children.push(row(actions).into());

    if loading {
        children.push(gap(20.0));
        children.push(muted("Loading details…"));
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
                .color(Color::rgba(214, 219, 232, 255))
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
    SingleChildScrollView::new(Padding::new(
        EdgeInsets::only(40.0, 8.0, 36.0, 40.0),
        hrow(vec![
            poster,
            hgap(30.0),
            Expanded::new(col(children)).into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Start),
    ))
    .controller(app.controllers.detail.clone())
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
    let details = stream.details();
    let mut lines: Vec<Widget> = vec![
        row(vec![
            text(
                if headline.is_empty() { "Stream".to_owned() } else { headline.replace('\n', " ") },
                13.5,
                FontWeight::W600,
                TEXT,
            )
            .into(),
            hgap(8.0),
            text(badge, 11.0, FontWeight::W600, ACCENT).into(),
        ])
        .into(),
    ];
    if !details.is_empty() {
        lines.push(gap(4.0));
        lines.push(
            Text::new(details)
                .style(style(12.0, FontWeight::W400, MUTED))
                .max_lines(Some(4))
                .overflow(TextOverflow::Ellipsis)
                .into(),
        );
    }
    let (app, stream) = (app.clone(), stream.clone());
    list_button(
        SizedBox::new()
            .width(PANEL_INNER - 26.0)
            .child(col(lines))
            .into(),
        false,
        move || app.play_stream(&stream),
    )
}

fn streams_pane(app: &App, groups: &[StreamGroup], video: &Video, is_series: bool) -> Widget {
    let mut children: Vec<Widget> = Vec::new();
    if is_series {
        let app = app.clone();
        children.push(
            row(vec![
                small_icon_button(icons::chevron_left(), Some("Episodes"), Tone::Neutral, move || {
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
                .child(SingleChildScrollView::new(row(chips)).scroll_direction(Axis::Horizontal))
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
    if loading > 0 {
        children.push(muted(format!(
            "Asking {loading} addon{}…",
            if loading == 1 { "" } else { "s" }
        )));
    } else if total == 0 {
        children.push(muted(if groups.is_empty() {
            "No installed addon provides streams for this. Install a stream addon under Addons."
        } else {
            "No streams were found for this title."
        }));
    }
    SingleChildScrollView::new(Padding::new(
        EdgeInsets::all(20.0),
        col(children),
    ))
    .controller(app.controllers.streams.clone())
    .into()
}

fn episodes_pane(app: &App, meta: &Meta, season: Option<u32>) -> Widget {
    let seasons = meta.seasons();
    let resume = app
        .library
        .read(|l| l.get(meta.id()).and_then(|i| i.state.video_id.clone()));
    let mut children: Vec<Widget> = Vec::new();
    if seasons.len() > 1 {
        let chips: Vec<Widget> = seasons
            .iter()
            .flat_map(|number| {
                let app = app.clone();
                let number = *number;
                let label = if number == 0 {
                    "Specials".to_owned()
                } else {
                    format!("Season {number}")
                };
                [
                    chip(label, season == Some(number), move || app.select_season(number)),
                    hgap(8.0),
                ]
            })
            .collect();
        children.push(
            SizedBox::new()
                .height(34.0)
                .width(PANEL_INNER)
                .child(SingleChildScrollView::new(row(chips)).scroll_direction(Axis::Horizontal))
                .into(),
        );
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
        let caption = if is_resume {
            "Continue here".to_owned()
        } else {
            released
        };
        let tile = hrow(vec![
            net_image(video.thumbnail.as_deref(), 112.0, 63.0, 8.0, 320, &video.label()),
            hgap(12.0),
            SizedBox::new()
                .width(PANEL_INNER - 112.0 - 40.0)
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
        let app = app.clone();
        children.push(list_button(tile.into(), is_resume, move || {
            app.select_video(video.clone())
        }));
        children.push(gap(8.0));
    }
    SingleChildScrollView::new(Padding::new(EdgeInsets::all(20.0), col(children)))
        .controller(app.controllers.streams.clone())
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
        (None, false) => Padding::all(20.0, muted(if loading { "Loading…" } else { "Nothing to play." })).into(),
    };

    let background = {
        let url = meta.background.clone().or_else(|| meta.poster.clone());
        LayoutBuilder::new(move |_, constraints| {
            let size = constraints.biggest();
            match &url {
                Some(url) => net_image(Some(url), size.width, size.height, 0.0, 1280, ""),
                None => SizedBox::shrink().into(),
            }
        })
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
                Container::new()
                    .width(PANEL_WIDTH)
                    .margin(EdgeInsets::only(0.0, 0.0, 24.0, 24.0))
                    .decoration(
                        BoxDecoration::new()
                            .color(Color::rgba(15, 18, 28, 235))
                            .border(Border::new(1.0, LINE))
                            .border_radius(BorderRadius::all(Radius::circular(18.0))),
                    )
                    .child(panel)
                    .into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into(),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Stretch);

    layers(vec![
        background.into(),
        Container::new().color(Color::rgba(9, 11, 17, 222)).into(),
        content.into(),
    ])
    .fit(StackFit::Expand)
    .into()
}
