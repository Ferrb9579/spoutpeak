//! Settings: Media account, playback, streaming server.

use super::widgets::*;
use crate::state::App;
use crate::store::{PlayerKind, Settings};
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
                .border(Border::new(1.0, LINE))
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

fn playback_card(app: &App, settings: &Settings) -> Widget {
    let ffmpeg_ok = crate::player::engine::tools().is_some();
    let mut children: Vec<Widget> = vec![section("Playback"), gap(6.0)];
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
        "Play next episode",
        "Continue automatically when one ends",
        toggle(app, settings.autoplay_next, |s, v| s.autoplay_next = v),
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
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
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
                server_card(app, &settings),
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
