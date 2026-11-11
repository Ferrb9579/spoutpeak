//! Settings: Media account, playback, streaming server.

use super::widgets::*;
use crate::state::{App, Load};
use crate::store::{PlayerKind, Settings};
use crate::media::server::TorrentProfile;
use serde_json::{Value, json};
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

fn password_field(app: &App) -> Widget {
    let submit = app.clone();
    Container::new()
        .width(300.0)
        .height(40.0)
        .padding(EdgeInsets::symmetric(12.0, 0.0))
        .alignment(Alignment::CENTER_LEFT)
        .decoration(
            BoxDecoration::new()
                .color(SURFACE_HI)
                .border_radius(BorderRadius::all(Radius::circular(10.0))),
        )
        .child(
            EditableText::new(app.controllers.password.clone())
                .size(Size::new(276.0, 22.0))
                .style(style(14.0, FontWeight::W400, TEXT))
                .placeholder("Password")
                .placeholder_color(FAINT)
                .obscure_text(true)
                .on_submit(move |_| submit.sign_in(false)),
        )
        .into()
}

fn account_card(app: &App) -> Widget {
    let user = app.profile.read(|p| p.auth.as_ref().map(|a| a.user.email.clone()));
    let (busy, error) = app.account.read(|a| (a.busy, a.error.clone()));
    let mut children: Vec<Widget> = vec![section("Media account"), gap(6.0)];
    match user {
        Some(email) => {
            let library_count = app.library.read(|l| l.values().filter(|i| !i.removed).count());
            children.push(muted(format!(
                "Signed in as {email}. Addons and library ({library_count} titles) sync with your Media account."
            )));
            children.push(gap(14.0));
            let (sync, out) = (app.clone(), app.clone());
            children.push(
                row(vec![
                    button("Sync now", Tone::Neutral, move || {
                        sync.sync_account();
                        sync.notify("Syncing with Media…");
                    }),
                    hgap(10.0),
                    button("Sign out", Tone::Danger, move || out.sign_out()),
                ])
                .into(),
            );
        }
        None => {
            children.push(muted(
                "Sign in to bring your installed addons, library and watch progress from Media. Without an account everything stays on this device.",
            ));
            children.push(gap(14.0));
            let (login, register) = (app.clone(), app.clone());
            let mut email = input(&app.controllers.email, "Email", 300.0, |_| {});
            email = email.style(crate::theme::field_style());
            children.push(
                Wrap::new(vec![
                    email.into(),
                    password_field(app),
                    button(
                        if busy { "Signing in…" } else { "Sign in" },
                        Tone::Primary,
                        move || login.sign_in(false),
                    ),
                    button("Create account", Tone::Ghost, move || register.sign_in(true)),
                ])
                .spacing(10.0)
                .run_spacing(10.0)
                .cross_axis_alignment(WrapCrossAlignment::Center)
                .into(),
            );
            if let Some(error) = error {
                children.push(gap(10.0));
                children.push(text(error, 13.0, FontWeight::W500, DANGER).into());
            }
        }
    }
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
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

fn downloads_card(app: &App) -> Widget {
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
            "ffmpeg was not found on this computer. Install ffmpeg (or Media) or pick an external player.",
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

fn server_card(app: &App, settings: &Settings) -> Widget {
    let (color, status) = match app.server_online.get() {
        Some(true) => (GOOD, "Online"),
        Some(false) => (DANGER, "Offline - torrents will not play"),
        None => (FAINT, "Checking…"),
    };
    let (save, check, start) = (app.clone(), app.clone(), app.clone());
    let children: Vec<Widget> = vec![
        section("Streaming server"),
        gap(6.0),
        muted("Torrent streams are served by Media's streaming server. Spoutpeak uses one that is already running, or starts the one from your Media installation."),
        gap(8.0),
        setting_row(
            "Server address",
            status,
            row(vec![
                input(&app.controllers.server_url, "http://127.0.0.1:11470", 300.0, |_| {})
                    .on_changed(move |value| {
                        save.update_settings(|s| s.server_url = value.trim().to_owned())
                    })
                    .into(),
                hgap(10.0),
                Container::new().width(9.0).height(9.0).radius(4.5).color(color).into(),
                hgap(12.0),
                small_button("Check", Tone::Neutral, move || {
                    check.server_online.set(None);
                    check.check_server();
                }),
                hgap(8.0),
                small_button("Start", Tone::Neutral, move || {
                    start.server_online.set(None);
                    start.ensure_server();
                }),
            ])
            .into(),
        ),
        setting_row(
            "Start automatically",
            "Launch the local server when it is not running",
            toggle(app, settings.autostart_server, |s, v| s.autostart_server = v),
        ),
    ];
    let mut children = children;
    children.push(gap(8.0));
    children.extend(server_options(app));
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

/// Chips that write one server value.
fn server_choices(app: &App, key: &'static str, options: Vec<(String, Value)>, current: &Value) -> Widget {
    Wrap::new(options.into_iter().map(|(label, value)| {
        let app = app.clone();
        let selected = &value == current;
        chip(label, selected, move || {
            let value = value.clone();
            app.update_server_settings(|values| {
                values.insert(key.to_owned(), value);
            })
        })
    }))
    .spacing(8.0)
    .run_spacing(8.0)
    .into()
}

fn gib(bytes: u64) -> String {
    format!("{} GB", bytes / 1_073_741_824)
}

/// Settings the streaming server itself keeps: cache, torrent tuning,
/// transcoding, remote access.
fn server_options(app: &App) -> Vec<Widget> {
    let reload = app.clone();
    let header = row(vec![
        text("Server options", 14.0, FontWeight::W600, TEXT).into(),
        hgap(10.0),
        small_icon_button(icons::rotate_ccw(), Some("Reload"), Tone::Ghost, move || {
            reload.load_server_settings()
        }),
    ])
    .into();
    let settings = match app.server_settings.get() {
        Load::Ready(settings) => settings,
        Load::Failed(error) => return vec![header, gap(6.0), text(error, 13.0, FontWeight::W500, DANGER).into()],
        _ => return vec![header, gap(6.0), muted("Loading the server's settings…")],
    };
    let mut rows = vec![header, gap(4.0)];

    // Cache size: the server's own list plus larger sizes.
    let mut sizes: Vec<(String, Value)> = Vec::new();
    for bytes in [0u64, 2, 5, 10, 20, 50, 100].map(|g| g * 1_073_741_824) {
        let label = if bytes == 0 { "No cache".to_owned() } else { gib(bytes) };
        sizes.push((label, json!(bytes)));
    }
    sizes.push(("Unlimited".to_owned(), Value::Null));
    let current_size = settings.value("cacheSize");
    if !sizes.iter().any(|(_, v)| *v == current_size) {
        if let Some(bytes) = current_size.as_u64() {
            sizes.push((gib(bytes), current_size.clone()));
        }
    }
    rows.push(setting_row(
        "Cache size",
        "Torrent data kept on disk for faster replays",
        server_choices(app, "cacheSize", sizes, &current_size),
    ));

    if !settings.cache_roots.is_empty() {
        rows.push(setting_row(
            "Caching drive",
            "Where the cache is stored",
            server_choices(
                app,
                "cacheRoot",
                settings
                    .cache_roots
                    .iter()
                    .map(|c| (c.name.clone(), c.value.clone()))
                    .collect(),
                &settings.value("cacheRoot"),
            ),
        ));
        rows.push(text(
            format!("Current: {}", settings.value("cacheRoot").as_str().unwrap_or("default")),
            12.0,
            FontWeight::W400,
            FAINT,
        )
        .into());
    }

    // Torrent profile.
    let detected = TorrentProfile::detect(&settings);
    let mut profiles: Vec<Widget> = TorrentProfile::ALL
        .into_iter()
        .map(|profile| {
            let app = app.clone();
            chip(profile.label(), detected == Some(profile), move || {
                app.update_server_settings(|values| profile.apply(values))
            })
        })
        .collect();
    if detected.is_none() {
        profiles.push(chip("Custom", true, || {}));
    }
    rows.push(setting_row(
        "Torrent profile",
        "How hard the server pulls from peers",
        Wrap::new(profiles).spacing(8.0).run_spacing(8.0).into(),
    ));

    // Finer control than Media offers: connections and speed cap directly.
    let connections = settings.u64("btMaxConnections").unwrap_or(0);
    let mut connection_options: Vec<(String, Value)> =
        [35u64, 55, 100, 200, 400, 800].iter().map(|n| (n.to_string(), json!(n))).collect();
    if !connection_options.iter().any(|(_, v)| v.as_u64() == Some(connections)) {
        connection_options.push((connections.to_string(), json!(connections)));
    }
    rows.push(setting_row(
        "Max peer connections",
        "More peers: faster starts, more bandwidth",
        server_choices(app, "btMaxConnections", connection_options, &json!(connections)),
    ));
    let hard = settings.values.get("btDownloadSpeedHardLimit").and_then(Value::as_f64).unwrap_or(0.0);
    let mut caps: Vec<(String, Value)> = [2u64, 5, 10, 25, 50, 75]
        .iter()
        .map(|mb| (format!("{mb} MB/s"), json!(mb * 1_048_576)))
        .collect();
    let current_cap = caps
        .iter()
        .find(|(_, v)| v.as_f64().is_some_and(|c| (c - hard).abs() < 1024.0))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| {
            let value = json!(hard);
            caps.push((format!("{:.1} MB/s", hard / 1_048_576.0), value.clone()));
            value
        });
    rows.push(setting_row(
        "Download speed limit",
        "Upper bound on torrent download speed",
        server_choices(app, "btDownloadSpeedHardLimit", caps, &current_cap),
    ));

    // Transcoding.
    let mut transcode: Vec<(String, Value)> = vec![("Disabled".to_owned(), Value::Null)];
    transcode.extend(
        settings
            .transcode_profiles
            .iter()
            .map(|p| (p.to_uppercase(), json!(p))),
    );
    rows.push(setting_row(
        "Transcode profile",
        "GPU encoder the server uses when it must convert",
        server_choices(app, "transcodeProfile", transcode, &settings.value("transcodeProfile")),
    ));
    rows.push(setting_row(
        "Hardware transcoding",
        "Let the server use the GPU to transcode",
        server_choices(
            app,
            "transcodeHardwareAccel",
            vec![("On".to_owned(), json!(true)), ("Off".to_owned(), json!(false))],
            &settings.value("transcodeHardwareAccel"),
        ),
    ));

    if !settings.https_endpoints.is_empty() {
        rows.push(setting_row(
            "HTTPS endpoint",
            "Lets other devices on your network stream",
            server_choices(
                app,
                "remoteHttps",
                settings
                    .https_endpoints
                    .iter()
                    .map(|c| (c.name.clone(), c.value.clone()))
                    .collect(),
                &settings.value("remoteHttps"),
            ),
        ));
    }
    rows.push(setting_row(
        "Proxy streams",
        "Route direct HTTP streams through the server",
        server_choices(
            app,
            "proxyStreamsEnabled",
            vec![("On".to_owned(), json!(true)), ("Off".to_owned(), json!(false))],
            &settings.value("proxyStreamsEnabled"),
        ),
    ));
    if let Some(version) = settings.values.get("serverVersion").and_then(Value::as_str) {
        rows.push(gap(4.0));
        rows.push(text(format!("Server version {version}"), 12.0, FontWeight::W400, FAINT).into());
    }
    rows
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
                "Spoutpeak {} - a native addon-based client built with Incular.",
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
        SingleChildScrollView::new(Padding::new(
            PAGE_PADDING,
            col(vec![
                account_card(app),
                gap(16.0),
                playback_card(app, &settings),
                gap(16.0),
                controls_card(app, &settings),
                gap(16.0),
                autoplay_card(app, &settings),
                gap(16.0),
                server_card(app, &settings),
                gap(16.0),
                downloads_card(app),
                gap(16.0),
                shortcuts_card(),
                gap(16.0),
                open_card(app),
                gap(16.0),
                about.into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        ))
        .controller(app.controllers.settings.clone())
        .into(),
    )
}
