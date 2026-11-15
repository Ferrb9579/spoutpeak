//! Settings: playback, streaming, downloads.

use super::motion;
use super::widgets::*;
use crate::state::App;
use crate::store::{DownloadFormat, PlayerKind, ReadAhead, Settings};
use crate::streaming::TorrentProfile;
use crate::theme::*;
use crate::util::language_name;
use incular::prelude::*;

const LANGUAGES: &[&str] = &[
    "eng", "spa", "fre", "ger", "ita", "por", "pob", "rus", "jpn", "kor", "chi", "ara", "hin",
    "tur", "pol", "dut",
];

fn setting_row(label: &str, hint: &str, control: Widget) -> Widget {
    Padding::new(
        EdgeInsets::symmetric(0.0, 9.0),
        hrow(vec![
            SizedBox::new()
                .width(250.0)
                .child(col(vec![
                    text(label, 14.0, FontWeight::W500, TEXT).into(),
                    gap(2.0),
                    text(hint, 12.0, FontWeight::W400, FAINT).into(),
                ]))
                .into(),
            hgap(20.0),
            Expanded::new(Align::new(Alignment::CENTER_LEFT, control)).into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .into()
}

fn choices<T: Copy + PartialEq + 'static>(
    app: &App,
    options: Vec<(String, T)>,
    current: T,
    apply: fn(&mut Settings, T),
) -> Widget {
    Wrap::new(options.into_iter().map(|(label, value)| {
        let app = app.clone();
        chip(label, value == current, move || {
            app.update_settings(|s| apply(s, value))
        })
    }))
    .spacing(8.0)
    .run_spacing(8.0)
    .into()
}

fn language_choices(
    app: &App,
    current: &str,
    allow_off: bool,
    apply: fn(&mut Settings, String),
) -> Widget {
    let mut options: Vec<(String, String)> = Vec::new();
    if allow_off {
        options.push(("Off".to_owned(), "off".to_owned()));
    }
    options.extend(
        LANGUAGES
            .iter()
            .map(|code| (language_name(code), (*code).to_owned())),
    );
    Wrap::new(options.into_iter().map(|(label, value)| {
        let app = app.clone();
        let selected = value == current;
        chip(label, selected, move || {
            let value = value.clone();
            app.update_settings(|s| apply(s, value))
        })
    }))
    .spacing(8.0)
    .run_spacing(8.0)
    .into()
}

fn toggle(app: &App, on: bool, apply: fn(&mut Settings, bool)) -> Widget {
    choices(
        app,
        vec![("On".to_owned(), true), ("Off".to_owned(), false)],
        on,
        apply,
    )
}

/// Card title with a leading Lucide icon.
fn titled(glyph: std::sync::Arc<Path>, title: &str) -> Widget {
    row(vec![
        icon(glyph, 18.0, ACCENT),
        hgap(10.0),
        section(title),
    ])
    .into()
}

fn seconds(values: &[u32]) -> Vec<(String, u32)> {
    values.iter().map(|v| (format!("{v}s"), *v)).collect()
}

fn controls_card(app: &App, settings: &Settings) -> Widget {
    let children: Vec<Widget> = vec![
        titled(icons::gauge(), "Controls"),
        gap(6.0),
        setting_row(
            "Seek step",
            "Arrow keys, J and L",
            choices(app, seconds(&[5, 10, 15, 30]), settings.seek_step, |s, v| s.seek_step = v),
        ),
        setting_row(
            "Seek step with Shift",
            "Shift + arrow keys",
            choices(app, seconds(&[3, 5, 30, 60, 120]), settings.seek_step_shift, |s, v| {
                s.seek_step_shift = v
            }),
        ),
        setting_row(
            "Volume step",
            "Up and down arrow keys",
            choices(
                app,
                [2, 5, 10].iter().map(|v| (format!("{v}%"), *v)).collect(),
                settings.volume_step,
                |s, v| s.volume_step = v,
            ),
        ),
        setting_row(
            "Hide controls after",
            "Idle time before the player controls fade",
            choices(app, seconds(&[2, 3, 5, 8]), settings.controls_hide_seconds, |s, v| {
                s.controls_hide_seconds = v
            }),
        ),
        setting_row(
            "Default speed",
            "Every video starts at this speed",
            choices(
                app,
                [0.75, 1.0, 1.25, 1.5, 2.0]
                    .iter()
                    .map(|v| (if *v == 1.0 { "Normal".to_owned() } else { format!("{v}×") }, *v))
                    .collect(),
                settings.default_speed,
                |s, v| s.default_speed = v,
            ),
        ),
    ];
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

fn autoplay_card(app: &App, settings: &Settings) -> Widget {
    let children: Vec<Widget> = vec![
        titled(icons::skip_forward(), "Auto-play"),
        gap(6.0),
        setting_row(
            "Play next episode",
            "Continue automatically when one ends",
            toggle(app, settings.autoplay_next, |s, v| s.autoplay_next = v),
        ),
        setting_row(
            "\"Up next\" card",
            "How long before the end it appears",
            choices(
                app,
                std::iter::once(("Off".to_owned(), 0)).chain(seconds(&[10, 20, 30, 60, 90])).collect(),
                settings.up_next_seconds,
                |s, v| s.up_next_seconds = v,
            ),
        ),
        setting_row(
            "Resume playback",
            "Start where you stopped last time",
            toggle(app, settings.resume_playback, |s, v| s.resume_playback = v),
        ),
    ];
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

fn downloads_card(app: &App, format: DownloadFormat) -> Widget {
    let (save, open, reset) = (app.clone(), app.clone(), app.clone());
    let current = crate::downloads::folder().display().to_string();
    card(
        col(vec![
            titled(icons::download(), "Downloads"),
            gap(6.0),
            setting_row(
                "Download folder",
                "Leave empty for Downloads\\Spoutpeak",
                row(vec![
                    input(&app.controllers.download_dir, &current, 380.0, |_| {})
                        .on_changed(move |value| save.update_settings(|s| s.download_dir = value))
                        .into(),
                    hgap(8.0),
                    small_icon_button(icons::folder_open(), None, Tone::Neutral, move || {
                        open.open_download_folder()
                    }),
                    hgap(6.0),
                    small_icon_button(icons::rotate_ccw(), None, Tone::Neutral, move || {
                        reset.controllers.download_dir.set_text("");
                        reset.update_settings(|s| s.download_dir.clear());
                    }),
                ])
                .into(),
            ),
            setting_row(
                "Save as",
                "MP4 plays on phones and TVs; 1080p and 720p also save space",
                choices(
                    app,
                    DownloadFormat::ALL
                        .iter()
                        .map(|format| (format.label().to_owned(), *format))
                        .collect(),
                    format,
                    |s, v| s.download_format = v,
                ),
            ),
            text(
                match crate::convert::encoder_if_known() {
                    Some(Some(encoder)) => format!("Conversion uses: {}", encoder.label),
                    Some(None) => "No video encoder works on this PC, so conversion is unavailable.".to_owned(),
                    None => "Checking for a video encoder…".to_owned(),
                },
                12.0,
                FontWeight::W400,
                FAINT,
            )
            .into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch),
    )
    .into()
}

fn shortcuts_card() -> Widget {
    let keys: &[(&str, &str)] = &[
        ("Space / K", "Play or pause"),
        ("← → / J L", "Seek (Shift: longer step)"),
        ("↑ ↓", "Volume"),
        ("M", "Mute"),
        ("F / F11", "Fullscreen"),
        ("C", "Subtitles on / off"),
        ("G / H", "Subtitle delay −/+"),
        ("[ / ]", "Audio delay −/+"),
        ("Shift + , / .", "Slower / faster"),
        ("B / Shift + B", "Brighter / darker"),
        ("0 / Home", "Back to the start"),
        ("N", "Next episode"),
        ("Esc", "Leave fullscreen or the player"),
    ];
    let rows: Vec<Widget> = keys
        .iter()
        .map(|(key, action)| {
            Padding::new(
                EdgeInsets::symmetric(0.0, 5.0),
                row(vec![
                    SizedBox::new()
                        .width(160.0)
                        .child(
                            Container::new()
                                .padding(EdgeInsets::symmetric(8.0, 3.0))
                                .decoration(
                                    BoxDecoration::new()
                                        .color(SURFACE_HI)
                                        .border_radius(BorderRadius::all(Radius::circular(6.0))),
                                )
                                .child(text(*key, 12.0, FontWeight::W600, TEXT)),
                        )
                        .into(),
                    text(*action, 13.0, FontWeight::W400, MUTED).into(),
                ]),
            )
            .into()
        })
        .collect();
    let mut children = vec![titled(icons::type_(), "Keyboard shortcuts"), gap(10.0)];
    children.extend(rows);
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

fn playback_card(app: &App, settings: &Settings) -> Widget {
    let ffmpeg_ok = crate::player::engine::tools().is_some();
    let mut children: Vec<Widget> = vec![titled(icons::play(), "Playback"), gap(6.0)];
    children.push(setting_row(
        "Player",
        "Built-in plays inside the window",
        choices(
            app,
            PlayerKind::ALL
                .iter()
                .map(|kind| (kind.label().to_owned(), *kind))
                .collect(),
            settings.player,
            |s, v| s.player = v,
        ),
    ));
    if settings.player == PlayerKind::Builtin && !ffmpeg_ok {
        children.push(text(
            "ffmpeg is missing from the Spoutpeak folder. Reinstall Spoutpeak or pick an external player.",
            13.0,
            FontWeight::W500,
            DANGER,
        )
        .into());
    }
    if settings.player == PlayerKind::Custom {
        let save = app.clone();
        children.push(setting_row(
            "Player program",
            "Full path; the stream URL is passed as its argument",
            input(&app.controllers.player_path, "C:\\path\\to\\player.exe", 420.0, |_| {})
                .on_changed(move |value| save.update_settings(|s| s.custom_player_path = value))
                .into(),
        ));
    }
    children.push(setting_row(
        "Picture quality",
        "Decoded height of the built-in player",
        choices(
            app,
            vec![
                ("480p".to_owned(), 480),
                ("720p".to_owned(), 720),
                ("1080p".to_owned(), 1080),
                ("Source".to_owned(), 0),
            ],
            settings.max_video_height,
            |s, v| s.max_video_height = v,
        ),
    ));
    children.push(setting_row(
        "Hardware decoding",
        "Let ffmpeg use the GPU when it can",
        toggle(app, settings.hardware_decoding, |s, v| s.hardware_decoding = v),
    ));
    children.push(setting_row(
        "Subtitles",
        "Turned on automatically when available",
        language_choices(app, &settings.subtitle_lang, true, |s, v| s.subtitle_lang = v),
    ));
    children.push(setting_row(
        "Audio language",
        "Preferred track when a file has several",
        language_choices(app, &settings.audio_lang, false, |s, v| s.audio_lang = v),
    ));
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

fn gib(bytes: u64) -> String {
    format!("{} GB", bytes / 1_073_741_824)
}

fn mib_per_second(bytes: u64) -> String {
    format!("{:.0} MB/s", bytes as f64 / 1_048_576.0)
}

/// The built-in torrent engine: status, cache and swarm tuning.
fn streaming_card(app: &App, settings: &Settings) -> Widget {
    let torrent = &settings.torrent;
    let online = app.server_online.get();
    let (color, status) = match online {
        Some(true) => (GOOD, "Running"),
        Some(false) => (DANGER, "Not running - torrents will not play"),
        None => (FAINT, "Starting…"),
    };
    let mut status_row: Vec<Widget> = vec![
        Container::new().width(9.0).height(9.0).radius(4.5).color(color).into(),
        hgap(10.0),
        text(status, 13.0, FontWeight::W500, MUTED).into(),
    ];
    if online == Some(false) {
        let retry = app.clone();
        status_row.push(hgap(12.0));
        status_row.push(small_button("Retry", Tone::Neutral, move || {
            retry.server_online.set(None);
            retry.start_streaming();
        }));
    }
    let mut rows: Vec<Widget> = vec![
        section("Streaming"),
        gap(6.0),
        muted("Torrents stream through Spoutpeak's built-in engine. Nothing else needs to be installed."),
        gap(8.0),
        setting_row("Torrent engine", "", row(status_row).into()),
    ];

    // Cache size.
    let mut sizes: Vec<(String, Option<u64>)> = [0u64, 2, 5, 10, 20, 50, 100]
        .iter()
        .map(|g| {
            let bytes = g * 1_073_741_824;
            (if bytes == 0 { "No cache".to_owned() } else { gib(bytes) }, Some(bytes))
        })
        .collect();
    sizes.push(("Unlimited".to_owned(), None));
    if !sizes.iter().any(|(_, v)| *v == torrent.cache_size) {
        if let Some(bytes) = torrent.cache_size {
            sizes.push((gib(bytes), Some(bytes)));
        }
    }
    rows.push(setting_row(
        "Cache size",
        "Torrent data kept on disk for faster replays",
        choices(app, sizes, torrent.cache_size, |s, v| s.torrent.cache_size = v),
    ));
    let mut ages: Vec<(String, Option<u32>)> = vec![
        ("While open".to_owned(), Some(0)),
        ("1 day".to_owned(), Some(1)),
        ("3 days".to_owned(), Some(3)),
        ("7 days".to_owned(), Some(7)),
        ("30 days".to_owned(), Some(30)),
        ("Until full".to_owned(), None),
    ];
    if !ages.iter().any(|(_, v)| *v == torrent.cache_max_age_days)
        && let Some(days) = torrent.cache_max_age_days
    {
        ages.push((format!("{days} days"), Some(days)));
    }
    rows.push(setting_row(
        "Keep cached torrents",
        "Deleted this long after they were last played",
        choices(app, ages, torrent.cache_max_age_days, |s, v| s.torrent.cache_max_age_days = v),
    ));
    let mut ahead: Vec<(String, ReadAhead)> = vec![("Auto".to_owned(), ReadAhead::Auto)];
    ahead.extend([128u32, 256, 512, 1024, 2048].map(|mb| {
        let label = if mb >= 1024 { format!("{} GB", mb / 1024) } else { format!("{mb} MB") };
        (label, ReadAhead::Megabytes(mb))
    }));
    ahead.push(("Whole file".to_owned(), ReadAhead::WholeFile));
    if !ahead.iter().any(|(_, v)| *v == torrent.read_ahead)
        && let ReadAhead::Megabytes(mb) = torrent.read_ahead
    {
        ahead.push((format!("{mb} MB"), torrent.read_ahead));
    }
    rows.push(setting_row(
        "Read ahead",
        "How much past the playback position keeps downloading, also while paused. Auto is about 2.5% of the file",
        choices(app, ahead, torrent.read_ahead, |s, v| s.torrent.read_ahead = v),
    ));
    rows.push(setting_row(
        "Clear cache",
        "Delete every cached torrent that is not playing now",
        small_button("Clear now", Tone::Neutral, move || {
            crate::state::spawn(crate::streaming::clear_cache(), |app, freed| {
                app.notify(match freed {
                    Ok(0) => "The torrent cache is already empty".to_owned(),
                    Ok(bytes) => format!("Freed {}", crate::util::format_bytes(bytes)),
                    Err(error) => format!("Could not clear the cache: {error}"),
                });
            });
        }),
    ));
    rows.push(
        text(
            format!("Stored in {}", crate::streaming::cache_folder().display()),
            12.0,
            FontWeight::W400,
            FAINT,
        )
        .into(),
    );

    // Swarm profile: a preset for the two settings below.
    let detected = TorrentProfile::detect(torrent);
    let mut profiles: Vec<Widget> = TorrentProfile::ALL
        .into_iter()
        .map(|profile| {
            let app = app.clone();
            chip(profile.label(), detected == Some(profile), move || {
                app.update_settings(|s| profile.apply(&mut s.torrent))
            })
        })
        .collect();
    if detected.is_none() {
        profiles.push(chip("Custom", true, || {}));
    }
    rows.push(setting_row(
        "Torrent profile",
        "How hard the engine pulls from peers",
        Wrap::new(profiles).spacing(8.0).run_spacing(8.0).into(),
    ));

    let mut connections: Vec<(String, u32)> =
        [35u32, 55, 100, 200, 400, 800].iter().map(|n| (n.to_string(), *n)).collect();
    if !connections.iter().any(|(_, v)| *v == torrent.max_connections) {
        connections.push((torrent.max_connections.to_string(), torrent.max_connections));
    }
    rows.push(setting_row(
        "Max peer connections",
        "More peers: faster starts, more bandwidth",
        choices(app, connections, torrent.max_connections, |s, v| s.torrent.max_connections = v),
    ));

    let mut caps: Vec<(String, Option<u64>)> = [2u64, 5, 10, 25, 50, 75]
        .iter()
        .map(|mb| (format!("{mb} MB/s"), Some(mb * 1_048_576)))
        .collect();
    caps.push(("Unlimited".to_owned(), None));
    if !caps.iter().any(|(_, v)| *v == torrent.speed_limit) {
        if let Some(bps) = torrent.speed_limit {
            caps.push((mib_per_second(bps), Some(bps)));
        }
    }
    rows.push(setting_row(
        "Download speed limit",
        "Upper bound on torrent download speed",
        choices(app, caps, torrent.speed_limit, |s, v| s.torrent.speed_limit = v),
    ));
    card(col(rows).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

fn open_card(app: &App) -> Widget {
    let (submit, click) = (app.clone(), app.clone());
    let play = move |app: &App| {
        let value = app.controllers.open_url.text().trim().to_owned();
        if value.is_empty() {
            app.notify("Enter a URL or file path first");
        } else {
            app.controllers.open_url.set_text("");
            app.play_url(value);
        }
    };
    let play_click = play.clone();
    card(
        col(vec![
            section("Open media"),
            gap(6.0),
            muted("Play any direct video URL or a local file path."),
            gap(12.0),
            row(vec![
                input(
                    &app.controllers.open_url,
                    "https://… or C:\\Videos\\movie.mkv",
                    460.0,
                    move |_| play(&submit),
                )
                .into(),
                hgap(10.0),
                button("Play", Tone::Primary, move || play_click(&click)),
            ])
            .into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch),
    )
    .into()
}

pub fn screen(app: &App) -> Widget {
    let settings = app.profile.read(|p| p.settings.clone());
    let about = card(
        col(vec![
            section("About"),
            gap(6.0),
            muted(format!(
                "Spoutpeak {} - a native, addon-based media center built with Incular.",
                env!("CARGO_PKG_VERSION")
            )),
            gap(4.0),
            muted(format!("Data folder: {}", crate::util::data_dir().display())),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch),
    );
    page(
        "Settings",
        vec![],
        smooth(&app.controllers.settings, SingleChildScrollView::new(Padding::new(
            PAGE_PADDING,
            col(vec![
                playback_card(app, &settings),
                gap(16.0),
                controls_card(app, &settings),
                gap(16.0),
                autoplay_card(app, &settings),
                gap(16.0),
                streaming_card(app, &settings),
                gap(16.0),
                downloads_card(app, settings.download_format),
                gap(16.0),
                shortcuts_card(),
                gap(16.0),
                open_card(app),
                gap(16.0),
                about.into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        ))
        .physics(motion::APP_SCROLLED)
        .controller(app.controllers.settings.clone())),
    )
}
