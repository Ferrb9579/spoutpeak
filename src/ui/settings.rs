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

/// Card title with a leading library icon.
fn titled(glyph: Glyph, title: &str) -> Widget {
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

/// On / Off for a Board setting; the Board is rebuilt to match.
fn board_toggle(app: &App, on: bool, apply: fn(&mut Settings, bool)) -> Widget {
    Wrap::new([("On", true), ("Off", false)].map(|(label, value)| {
        let app = app.clone();
        chip(label, value == on, move || {
            if value != on {
                app.update_settings(|s| apply(s, value));
                app.load_board();
            }
        })
    }))
    .spacing(8.0)
    .run_spacing(8.0)
    .into()
}

/// The Board: its Spotlight and where the Spotlight's titles come from.
fn board_card(app: &App, settings: &Settings) -> Widget {
    let mut children = vec![
        titled(icons::house(), "Board"),
        gap(8.0),
        setting_row(
            "Show Spotlight",
            "The rotating titles at the top of Home.",
            board_toggle(app, settings.show_spotlight, |s, v| s.show_spotlight = v),
        ),
    ];
    if settings.show_spotlight {
        let count = app.spotlight_keys().len();
        let open = app.clone();
        children.push(setting_row(
            "Spotlight catalogs",
            "Up to 3 catalogs take turns, 6 titles each.",
            icon_text_button(
                icons::chevron_right(),
                if count == 1 { "1 catalog".to_owned() } else { format!("{count} catalogs") },
                Tone::Neutral,
                move || open.open_spotlight_picker(),
            ),
        ));
        children.push(gap(6.0));
        // Lined up with the controls column of the rows around it.
        children.push(Padding::new(EdgeInsets::only(270.0, 0.0, 0.0, 0.0), super::spotlight::preview(app)).into());
        children.push(gap(6.0));
        children.push(setting_row(
            "Show their rows on the Board",
            "Off: the Spotlight's catalogs appear only in the Spotlight.",
            board_toggle(app, settings.spotlight_rows_on_board, |s, v| s.spotlight_rows_on_board = v),
        ));
    }
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
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
            "Idle time before controls fade, including while paused",
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

fn shortcut_keys(keys: &[&str]) -> Widget {
    let mut children = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        if index > 0 { children.push(hgap(4.0)); }
        children.push(if *key == "+" {
            text("+", 14.0, FontWeight::W500, TEXT).into()
        } else if matches!(*key, "or" | "/") {
            text(*key, 10.5, FontWeight::W400, FAINT).into()
        } else {
            Container::new().height(26.0).padding(EdgeInsets::symmetric(8.0, 0.0))
                .constraints(Constraints::new(26.0, f32::INFINITY, 26.0, 26.0))
                .alignment(Alignment::CENTER)
                .decoration(BoxDecoration::new().color(SURFACE_HI)
                    .border_radius(BorderRadius::all(Radius::circular(7.0))))
                .child(Widget::from(text(if *key == "Shift" { "⇧" } else { *key },
                    12.0, FontWeight::W600, Color::rgba(188, 188, 188, 255)))
                    .accessibility_label(*key)).into()
        });
    }
    row(children).into()
}

fn shortcut_group(title: &str, shortcuts: &[(&str, &[&str])]) -> Widget {
    let mut children = vec![text(title, 13.0, FontWeight::W600, TEXT).into(), gap(10.0)];
    for (index, (label, keys)) in shortcuts.iter().enumerate() {
        if index > 0 {
            children.push(Container::new().height(1.0)
                .color(Color::rgba(255, 255, 255, 12)).into());
        }
        children.push(Padding::new(EdgeInsets::symmetric(0.0, 7.0), Row::new(vec![
            Expanded::new(text(*label, 12.5, FontWeight::W400, MUTED).max_lines(Some(2))).into(),
            hgap(12.0), shortcut_keys(keys),
        ]).cross_axis_alignment(CrossAxisAlignment::Center)).into());
    }
    Container::new().radius(12.0).color(GLASS).padding(EdgeInsets::all(16.0))
        .child(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

fn shortcuts_card() -> Widget {
    card(LayoutBuilder::new(move |_, constraints| {
        let playback = shortcut_group("Playback", &[
            ("Play or pause", &["Space", "or", "K"]),
            ("Seek backward", &["←", "or", "J"]),
            ("Seek forward", &["→", "or", "L"]),
            ("Longer seek", &["Shift", "+", "←", "/", "→"]),
            ("Playback speed − / +", &["Shift", "+", ",", "/", "."]),
            ("Back to start", &["0", "or", "Home"]),
            ("Next episode", &["N"]),
        ]);
        let audio = shortcut_group("Audio & picture", &[
            ("Volume + / −", &["↑", "/", "↓"]),
            ("Mute", &["M"]),
            ("Fullscreen", &["F", "or", "F11"]),
            ("Brightness + / −", &["B", "/", "Shift", "+", "B"]),
        ]);
        let subtitles = shortcut_group("Subtitles & sync", &[
            ("Subtitles on / off", &["C"]),
            ("Subtitle delay − / +", &["G", "/", "H"]),
            ("Audio delay − / +", &["[", "/", "]"]),
        ]);
        let groups: Widget = if constraints.max_width() >= 1080.0 {
            Row::new(vec![Expanded::new(playback).into(), hgap(16.0),
                Expanded::new(audio).into(), hgap(16.0), Expanded::new(subtitles).into()])
                .cross_axis_alignment(CrossAxisAlignment::Start).into()
        } else if constraints.max_width() >= 740.0 {
            Row::new(vec![Expanded::new(playback).into(), hgap(16.0),
                Expanded::new(col(vec![audio, gap(16.0), subtitles])
                    .cross_axis_alignment(CrossAxisAlignment::Stretch)).into()])
                .cross_axis_alignment(CrossAxisAlignment::Start).into()
        } else {
            col(vec![playback, gap(16.0), audio, gap(16.0), subtitles])
                .cross_axis_alignment(CrossAxisAlignment::Stretch).into()
        };
        col(vec![
            titled(icons::type_(), "Keyboard shortcuts"), gap(6.0),
            text("For the built-in player. Seek and volume steps follow your Controls settings.",
                12.0, FontWeight::W400, MUTED).into(),
            gap(20.0), groups, gap(18.0),
            Row::new(vec![shortcut_keys(&["Esc"]), hgap(10.0),
                Expanded::new(text("Leave fullscreen or return from the player", 12.0,
                    FontWeight::W400, MUTED)).into()]).into(),
        ]).cross_axis_alignment(CrossAxisAlignment::Stretch).into()
    })).into()
}

fn ai_card(app: &App, settings: &Settings) -> Widget {
    let model = app.ai.state.read(Clone::clone);
    let mut children: Vec<Widget> = vec![
        titled(icons::type_(), "Local AI"), gap(8.0),
        muted("Keeps subtitles in time with the audio. Runs on this PC; nothing is uploaded."),
        gap(4.0),
        setting_row("Enable local AI", "Allow the on-device tools", toggle(app,
            settings.local_ai_enabled, |s, v| s.local_ai_enabled = v)),
        setting_row("Automatic subtitle sync", "Aligns same-language subtitles to the speech as you watch",
            toggle(app, settings.automatic_subtitle_sync, |s, v| s.automatic_subtitle_sync = v)),
        gap(8.0),
    ];
    if !crate::ai::supported() {
        children.push(muted("Local AI installation currently supports Windows x64."));
    } else if model.busy {
        children.push(text(&model.stage, 13.0, FontWeight::W500, ACCENT).into());
        if model.downloading {
            children.push(muted(format!("{:.0} / {:.0} MB · {:.0}%", model.downloaded as f64 / 1e6,
                model.total as f64 / 1e6,
                100.0 * model.downloaded as f64 / model.total.max(1) as f64)));
        }
        let cancel = app.clone();
        children.push(gap(8.0));
        children.push(button("Stop task", Tone::Neutral, move || cancel.cancel_ai_task()));
    } else {
        children.push(text(if model.installed { "Model (LFM3-Omni-2B): downloaded" } else { "Model (LFM3-Omni-2B): not downloaded · about 1.6 GB" },
            13.0, FontWeight::W500, if model.installed { ACCENT } else { MUTED }).into());
        let mut actions = Vec::new();
        if settings.local_ai_enabled {
            let download = app.clone();
            actions.push(button(if model.installed { "Verify / repair" } else { "Download model" }, Tone::Primary,
                move || download.download_ai_model()));
        }
        if crate::ai::model_dir().exists() {
            let remove = app.clone();
            actions.push(hgap(8.0));
            actions.push(button("Remove download", Tone::Neutral, move || remove.remove_ai_model()));
        }
        children.push(gap(8.0));
        children.push(row(actions).into());
        children.push(gap(10.0));
        children.push(text(if model.timing_installed { "Speech timing model: downloaded" }
            else { "Speech timing model: not downloaded · about 160 MB, needed for automatic sync" },
            13.0, FontWeight::W500, if model.timing_installed { ACCENT } else { MUTED }).into());
        children.push(gap(8.0));
        let mut timing_actions = Vec::new();
        if settings.local_ai_enabled {
            let download = app.clone();
            timing_actions.push(button(if model.timing_installed { "Verify timing model" } else { "Download timing model" },
                Tone::Neutral, move || download.download_timing_model()));
        }
        if crate::player::alignment::model_dir().exists() {
            let remove = app.clone();
            timing_actions.push(hgap(8.0));
            timing_actions.push(button("Remove timing model", Tone::Neutral, move || remove.remove_timing_model()));
        }
        children.push(row(timing_actions).into());
    }
    if let Some(error) = model.error {
        children.push(gap(8.0));
        children.push(text(error, 12.0, FontWeight::W400, DANGER).max_lines(Some(4)).into());
    }
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
        "Subtitle loading",
        "Fast reads all embedded lines as soon as possible. It can use more bandwidth and may read most of the video.",
        choices(
            app,
            vec![("Balanced".to_owned(), false), ("Fast".to_owned(), true)],
            settings.fast_subtitles,
            |s, v| s.fast_subtitles = v,
        ),
    ));
    children.push(setting_row(
        "Audio language",
        "Preferred track when a file has several",
        language_choices(app, &settings.audio_lang, false, |s, v| s.audio_lang = v),
    ));
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
}

/// `2`, `1.5`: at most one decimal, none when whole.
fn amount(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 { format!("{rounded:.0}") } else { format!("{rounded:.1}") }
}

fn gib(bytes: u64) -> String {
    format!("{} GB", amount(bytes as f64 / 1_073_741_824.0))
}

fn mib_per_second(bytes: u64) -> String {
    format!("{} MB/s", amount(bytes as f64 / 1_048_576.0))
}

// ------------------------------------------------- custom streaming values

/// Splits `"1.5 GB"` / `"90s"` into a positive number and a lower-case unit.
fn number_and_unit(text: &str) -> Option<(f64, String)> {
    let text = text.trim().to_lowercase().replace(',', ".");
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let number: f64 = text[..split].parse().ok()?;
    let unit = text[split..].trim().trim_end_matches("/s").trim().to_owned();
    (number.is_finite() && number >= 0.0).then_some((number, unit))
}

/// Cache size: GB unless `MB` / `TB` is given. `0` = no cache.
fn parse_cache_size(text: &str) -> Option<Option<u64>> {
    let (number, unit) = number_and_unit(text)?;
    let scale = match unit.as_str() {
        "" | "g" | "gb" | "gib" => 1_073_741_824.0,
        "m" | "mb" | "mib" => 1_048_576.0,
        "t" | "tb" | "tib" => 1_099_511_627_776.0,
        _ => return None,
    };
    Some(Some((number * scale) as u64))
}

/// Days a cached torrent is kept: whole days, `0` = only while open.
fn parse_days(text: &str) -> Option<Option<u32>> {
    let (number, unit) = number_and_unit(text)?;
    let days = match unit.as_str() {
        "" | "d" | "day" | "days" => number,
        "w" | "week" | "weeks" => number * 7.0,
        _ => return None,
    };
    (days.fract() == 0.0 && days <= 3650.0).then_some(Some(days as u32))
}

/// Read ahead as a time (`90 s`, `3 min`, `1 h`; seconds without a unit)
/// or a size (`512 MB`, `1.5 GB`).
fn parse_read_ahead(text: &str) -> Option<ReadAhead> {
    let (number, unit) = number_and_unit(text)?;
    if number <= 0.0 {
        return None;
    }
    let seconds = |s: f64| (s >= 1.0 && s <= 24.0 * 3600.0).then_some(ReadAhead::Seconds(s.round() as u32));
    let megabytes = |mb: f64| (mb >= 1.0 && mb <= 1_048_576.0).then_some(ReadAhead::Megabytes(mb.round() as u32));
    match unit.as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => seconds(number),
        "m" | "min" | "mins" | "minute" | "minutes" => seconds(number * 60.0),
        "h" | "hr" | "hour" | "hours" => seconds(number * 3600.0),
        "mb" | "mib" => megabytes(number),
        "gb" | "gib" => megabytes(number * 1024.0),
        _ => None,
    }
}

/// Peer connections per torrent.
fn parse_peers(text: &str) -> Option<u32> {
    let (number, unit) = number_and_unit(text)?;
    (unit.is_empty() && number.fract() == 0.0 && (1.0..=5000.0).contains(&number)).then_some(number as u32)
}

/// Download cap: MB/s unless `KB` / `GB` is given (`/s` optional).
fn parse_speed(text: &str) -> Option<Option<u64>> {
    let (number, unit) = number_and_unit(text)?;
    let scale = match unit.as_str() {
        "" | "m" | "mb" | "mib" => 1_048_576.0,
        "k" | "kb" | "kib" => 1024.0,
        "g" | "gb" | "gib" => 1_073_741_824.0,
        _ => return None,
    };
    let bps = (number * scale) as u64;
    (bps >= 10 * 1024).then_some(Some(bps))
}

thread_local! {
    /// The custom fields' text, kept across rebuilds of the page.
    static CUSTOM: std::cell::RefCell<std::collections::HashMap<&'static str, TextEditingController>> =
        std::cell::RefCell::default();
}

/// A small field for a value the presets lack; Enter applies it.
fn custom_field<V: 'static>(
    app: &App,
    key: &'static str,
    placeholder: &'static str,
    parse: fn(&str) -> Option<V>,
    apply: fn(&mut Settings, V),
) -> Widget {
    let controller = CUSTOM.with(|c| c.borrow_mut().entry(key).or_insert_with(TextEditingController::new).clone());
    let (app, field) = (app.clone(), controller.clone());
    let input = input(&controller, placeholder, 130.0, move |value| {
        if value.trim().is_empty() {
            return;
        }
        match parse(&value) {
            Some(parsed) => {
                app.update_settings(|s| apply(s, parsed));
                field.set_text("");
            }
            None => app.notify(format!("Couldn't use “{}”. Try something like {placeholder}.", value.trim())),
        }
    })
    .size(Size::new(130.0, 32.0));
    super::tooltip::tooltip("Type a value and press Enter", input)
}

/// [`choices`] followed by a [`custom_field`].
fn choices_or_custom<T: Copy + PartialEq + 'static>(
    app: &App,
    options: Vec<(String, T)>,
    current: T,
    apply: fn(&mut Settings, T),
    custom: Widget,
) -> Widget {
    let mut chips: Vec<Widget> = options
        .into_iter()
        .map(|(label, value)| {
            let app = app.clone();
            chip(label, value == current, move || app.update_settings(|s| apply(s, value)))
        })
        .collect();
    chips.push(custom);
    Wrap::new(chips).spacing(8.0).run_spacing(8.0).cross_axis_alignment(WrapCrossAlignment::Center).into()
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

    rows.push(setting_row(
        "Peer encryption",
        "Prefer allows unencrypted peers; Require may find fewer peers. Changes reconnect peers. Your IP remains visible.",
        choices(
            app,
            vec![
                ("Disabled".to_owned(), librqbit::EncryptionMode::Disabled),
                ("Prefer".to_owned(), librqbit::EncryptionMode::Prefer),
                ("Require".to_owned(), librqbit::EncryptionMode::Require),
            ],
            torrent.encryption,
            |s, v| s.torrent.encryption = v,
        ),
    ));

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
        choices_or_custom(app, sizes, torrent.cache_size, |s, v| s.torrent.cache_size = v,
            custom_field(app, "cache", "e.g. 15 GB", parse_cache_size, |s, v| s.torrent.cache_size = v)),
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
        choices_or_custom(app, ages, torrent.cache_max_age_days, |s, v| s.torrent.cache_max_age_days = v,
            custom_field(app, "age", "e.g. 14 days", parse_days, |s, v| s.torrent.cache_max_age_days = v)),
    ));
    let mut ahead: Vec<(String, ReadAhead)> = vec![("Auto".to_owned(), ReadAhead::Auto)];
    let seconds_label = |s: u32| {
        if s % 60 == 0 { format!("{} min", s / 60) } else { format!("{s} s") }
    };
    let megabytes_label = |mb: u32| {
        if mb >= 1024 { format!("{} GB", amount(f64::from(mb) / 1024.0)) } else { format!("{mb} MB") }
    };
    ahead.extend([30u32, 60, 120, 300, 600].map(|s| (seconds_label(s), ReadAhead::Seconds(s))));
    ahead.extend([256u32, 1024].map(|mb| (megabytes_label(mb), ReadAhead::Megabytes(mb))));
    ahead.push(("Whole file".to_owned(), ReadAhead::WholeFile));
    if !ahead.iter().any(|(_, v)| *v == torrent.read_ahead) {
        match torrent.read_ahead {
            ReadAhead::Seconds(s) => ahead.push((seconds_label(s), torrent.read_ahead)),
            ReadAhead::Megabytes(mb) => ahead.push((megabytes_label(mb), torrent.read_ahead)),
            _ => {}
        }
    }
    rows.push(setting_row(
        "Read ahead",
        "Buffered past the playback position, also while paused. Times use the file's average bitrate; Auto is about 2.5% of the file",
        choices_or_custom(app, ahead, torrent.read_ahead, |s, v| s.torrent.read_ahead = v,
            custom_field(app, "ahead", "e.g. 3 min", parse_read_ahead, |s, v| s.torrent.read_ahead = v)),
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
        choices_or_custom(app, connections, torrent.max_connections, |s, v| s.torrent.max_connections = v,
            custom_field(app, "peers", "e.g. 300", parse_peers, |s, v| s.torrent.max_connections = v)),
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
        choices_or_custom(app, caps, torrent.speed_limit, |s, v| s.torrent.speed_limit = v,
            custom_field(app, "speed", "e.g. 15 MB/s", parse_speed, |s, v| s.torrent.speed_limit = v)),
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

fn diagnostics_card(app: &App) -> Widget {
    let open_app = app.clone();
    let export_app = app.clone();
    card(col(vec![
        section("Diagnostics"),
        gap(6.0),
        muted("If Spoutpeak crashes, reopen it and export a report to share for troubleshooting."),
        gap(8.0),
        row(vec![
            small_button("Open log folder", Tone::Neutral, move || {
                if let Err(error) = open::that(crate::diagnostics::directory()) {
                    open_app.notify(format!("Could not open logs: {error}"));
                }
            }),
            hgap(8.0),
            small_button("Export diagnostics", Tone::Neutral, move || {
                export_app.notify("Preparing diagnostic report…");
                crate::state::spawn(async {
                    tokio::task::spawn_blocking(crate::diagnostics::export_report).await
                        .map_err(|e| e.to_string())?.map_err(|e| e.to_string())
                }, |app, result| match result {
                    Ok(path) => {
                        if let Some(parent) = path.parent() { let _ = open::that(parent); }
                        app.notify(format!("Report saved: {}", path.display()));
                    }
                    Err(error) => app.notify(format!("Could not export diagnostics: {error}")),
                });
            }),
        ]).into(),
        gap(8.0),
        muted("Exports recent logs and OS/GPU details to Downloads. Review the ZIP before sharing. Crash memory dumps stay in the log folder."),
        gap(4.0),
        muted(format!("Logs: {}", crate::diagnostics::directory().display())),
    ]).cross_axis_alignment(CrossAxisAlignment::Stretch)).into()
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
            gap(8.0),
            muted("Icons: Solar by 480 Design (CC BY 4.0) and MingCute (Apache 2.0)."),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Stretch),
    );
    page(
        "Settings",
        vec![],
        smooth(&app.controllers.settings, SingleChildScrollView::new(Padding::new(
            PAGE_PADDING,
            col(vec![
                board_card(app, &settings),
                gap(16.0),
                playback_card(app, &settings),
                gap(16.0),
                ai_card(app, &settings),
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
                diagnostics_card(app),
                gap(16.0),
                about.into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        ))
        .physics(motion::APP_SCROLLED)
        .controller(app.controllers.settings.clone())),
    )
}

#[cfg(test)]
mod custom_value_tests {
    use super::*;

    #[test]
    fn sizes_and_speeds_take_optional_units() {
        assert_eq!(parse_cache_size("15"), Some(Some(15 * 1_073_741_824)));
        assert_eq!(parse_cache_size("1.5 GB"), Some(Some(1_610_612_736)));
        assert_eq!(parse_cache_size("512mb"), Some(Some(512 * 1_048_576)));
        assert_eq!(parse_cache_size("0"), Some(Some(0)));
        assert_eq!(parse_cache_size("lots"), None);
        assert_eq!(parse_speed("15 MB/s"), Some(Some(15 * 1_048_576)));
        assert_eq!(parse_speed("800 kb/s"), Some(Some(800 * 1024)));
        assert_eq!(parse_speed("1,5"), Some(Some(1_572_864)));
        assert_eq!(parse_speed("1 kb"), None);
    }

    #[test]
    fn read_ahead_is_a_time_or_a_size() {
        assert_eq!(parse_read_ahead("90"), Some(ReadAhead::Seconds(90)));
        assert_eq!(parse_read_ahead("3 min"), Some(ReadAhead::Seconds(180)));
        assert_eq!(parse_read_ahead("1h"), Some(ReadAhead::Seconds(3600)));
        assert_eq!(parse_read_ahead("512 MB"), Some(ReadAhead::Megabytes(512)));
        assert_eq!(parse_read_ahead("1.5gb"), Some(ReadAhead::Megabytes(1536)));
        assert_eq!(parse_read_ahead("0"), None);
        assert_eq!(parse_read_ahead("3 parsecs"), None);
    }

    #[test]
    fn days_and_peers_are_whole_numbers() {
        assert_eq!(parse_days("14"), Some(Some(14)));
        assert_eq!(parse_days("2 weeks"), Some(Some(14)));
        assert_eq!(parse_days("0"), Some(Some(0)));
        assert_eq!(parse_days("1.5"), None);
        assert_eq!(parse_peers("300"), Some(300));
        assert_eq!(parse_peers("0"), None);
        assert_eq!(parse_peers("2.5"), None);
        assert_eq!(parse_peers("300 peers"), None);
    }

    #[test]
    fn labels_show_one_decimal_at_most() {
        assert_eq!(gib(1_610_612_736), "1.5 GB");
        assert_eq!(gib(2 * 1_073_741_824), "2 GB");
        assert_eq!(mib_per_second(15 * 1_048_576), "15 MB/s");
    }
}
