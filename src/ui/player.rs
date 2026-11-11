//! The player screen: picture, subtitles and transport controls.

use crate::player::{Menu, Status, SubChoice};
use crate::state::App;
use crate::theme::*;
use crate::util::{format_speed, format_time, language_name};
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use incular::widgets::{HitTestBehavior, StackFit};
use std::rc::Rc;

const OVERLAY: Color = Color::rgba(8, 9, 14, 205);

/// A horizontal track; `change(fraction, committed)` fires on click and
/// while dragging, with `committed` true on click and on release.
fn slider(width: f32, fraction: f32, change: Rc<dyn Fn(f32, bool)>) -> Widget {
    let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let filled = (width * fraction).max(0.0);
    let visual = Container::new()
        .width(width)
        .height(22.0)
        .color(Color::TRANSPARENT)
        .child(layers(vec![
            Positioned::new(
                Container::new()
                    .width(width)
                    .height(4.0)
                    .radius(2.0)
                    .color(Color::rgba(255, 255, 255, 60)),
            )
            .left(0.0)
            .top(9.0)
            .into(),
            Positioned::new(
                Container::new()
                    .width(filled)
                    .height(4.0)
                    .radius(2.0)
                    .color(ACCENT),
            )
            .left(0.0)
            .top(9.0)
            .into(),
            Positioned::new(
                Container::new()
                    .width(14.0)
                    .height(14.0)
                    .radius(7.0)
                    .color(Color::WHITE),
            )
            .left((filled - 7.0).clamp(0.0, (width - 14.0).max(0.0)))
            .top(4.0)
            .into(),
        ]));
    let at = move |x: f32| (x / width.max(1.0)).clamp(0.0, 1.0);
    let last = Rc::new(std::cell::Cell::new(fraction));
    let (tap, drag, release) = (change.clone(), change.clone(), change);
    let (drag_last, release_last) = (last.clone(), last);
    GestureDetector::new(visual)
        .behavior(HitTestBehavior::Opaque)
        .on_tap_down(move |details| tap(at(details.local_position.x), true))
        .on_horizontal_drag_update(move |details| {
            let value = at(details.local_position.x);
            drag_last.set(value);
            drag(value, false);
        })
        .on_horizontal_drag_end(move |_| release(release_last.get(), true))
        .into()
}

fn seek_bar(app: &App) -> Widget {
    let app = app.clone();
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width();
        let position = app.player.position.get();
        let duration = app.player.state.peek(|s| s.duration);
        let fraction = if duration > 0.0 { (position / duration) as f32 } else { 0.0 };
        let app = app.clone();
        slider(
            width,
            fraction,
            Rc::new(move |value, committed| {
                if duration <= 0.0 {
                    return;
                }
                let target = value as f64 * duration;
                if committed {
                    app.player.scrub.set(None);
                    app.seek_to(target);
                } else {
                    app.player.scrub.set(Some(target));
                    app.player.position.set(target);
                    app.player_activity();
                }
            }),
        )
    })
    .into()
}

fn clock(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let position = app.player.position.get();
        let (duration, live) = app.player.state.peek(|s| (s.duration, s.live));
        if live {
            return row(vec![
                Container::new().width(8.0).height(8.0).radius(4.0).color(DANGER).into(),
                hgap(7.0),
                text("LIVE", 12.0, FontWeight::W700, TEXT).into(),
            ])
            .into();
        }
        let label = if duration > 0.0 {
            format!("{} / {}", format_time(position), format_time(duration))
        } else {
            format_time(position)
        };
        text(label, 12.5, FontWeight::W500, TEXT).into()
    })
    .into()
}

fn pill(label: impl Into<String>, active: bool, on_click: impl Fn() + 'static) -> Widget {
    let background = if active { ACCENT_DIM } else { Color::rgba(255, 255, 255, 22) };
    let foreground = if active { ACCENT } else { TEXT };
    Button::new(label)
        .style(ButtonStyle {
            background: Some(background),
            background_states: Some(
                StateColor::new(background).hovered(Color::rgba(255, 255, 255, 48)),
            ),
            foreground: Some(foreground),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(15.0),
            padding: Some(EdgeInsets::symmetric(13.0, 0.0)),
            height: Some(30.0),
            text_style: Some(style(12.5, FontWeight::W600, foreground)),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

fn menu_item(label: String, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let background = if selected { ACCENT_DIM } else { Color::TRANSPARENT };
    Button::new(label)
        .style(ButtonStyle {
            background: Some(background),
            background_states: Some(
                StateColor::new(background).hovered(Color::rgba(255, 255, 255, 30)),
            ),
            foreground: Some(if selected { ACCENT } else { TEXT }),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(8.0),
            padding: Some(EdgeInsets::symmetric(12.0, 0.0)),
            fixed_size: Some(Size::new(296.0, 34.0)),
            alignment: Some(Alignment::CENTER_LEFT),
            text_style: Some(style(
                13.0,
                FontWeight::W500,
                if selected { ACCENT } else { TEXT },
            )),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

fn track_label(language: &str, title: &str, fallback: usize) -> String {
    let language = if language.is_empty() {
        format!("Track {}", fallback + 1)
    } else {
        language_name(language)
    };
    if title.is_empty() {
        language
    } else {
        format!("{language} - {title}")
    }
}

fn menu_panel(app: &App, menu: Menu) -> Option<Widget> {
    let mut items: Vec<Widget> = Vec::new();
    let title = match menu {
        Menu::None => return None,
        Menu::Sync => return Some(sync_panel(app)),
        Menu::Speed => {
            let current = app.player.state.peek(|s| s.speed);
            for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                let app = app.clone();
                items.push(menu_item(
                    format!("{speed}×"),
                    (current - speed).abs() < 0.01,
                    move || app.set_speed(speed),
                ));
            }
            "Speed"
        }
        Menu::Audio => {
            let (tracks, current) = app
                .player
                .state
                .peek(|s| (s.audio_tracks.clone(), s.audio_index));
            for (index, track) in tracks.iter().enumerate() {
                let app = app.clone();
                items.push(menu_item(
                    track_label(&track.language, &track.title, index),
                    index == current,
                    move || app.select_audio(index),
                ));
            }
            if tracks.is_empty() {
                items.push(muted("No audio tracks"));
            }
            "Audio"
        }
        Menu::Subtitles => {
            let (embedded, addon, choice, delay, size) = app.player.state.peek(|s| {
                (
                    s.embedded_subs.clone(),
                    s.addon_subs.clone(),
                    s.sub_choice.clone(),
                    s.sub_delay,
                    s.sub_size,
                )
            });
            {
                let app = app.clone();
                items.push(menu_item("Off".to_owned(), choice == SubChoice::Off, move || {
                    app.select_subtitle(SubChoice::Off)
                }));
            }
            for (index, track) in embedded.iter().enumerate() {
                if !track.is_text_subtitle() {
                    continue;
                }
                let app = app.clone();
                items.push(menu_item(
                    format!("{}  (embedded)", track_label(&track.language, &track.title, index)),
                    choice == SubChoice::Embedded(index),
                    move || app.select_subtitle(SubChoice::Embedded(index)),
                ));
            }
            let mut ordered: Vec<(usize, String)> = addon
                .iter()
                .enumerate()
                .map(|(i, s)| (i, language_name(s.lang.as_deref().unwrap_or(""))))
                .collect();
            ordered.sort_by(|a, b| a.1.cmp(&b.1));
            let mut seen: std::collections::HashMap<String, usize> = Default::default();
            for (index, language) in ordered {
                let number = seen.entry(language.clone()).or_insert(0);
                *number += 1;
                let app = app.clone();
                items.push(menu_item(
                    format!("{language}  #{number}"),
                    choice == SubChoice::Addon(index),
                    move || app.select_subtitle(SubChoice::Addon(index)),
                ));
            }
            if choice != SubChoice::Off {
                let app = app.clone();
                items.insert(
                    0,
                    Padding::new(
                        EdgeInsets::only(4.0, 0.0, 0.0, 10.0),
                        pill("Out of sync? Pick the line…", false, move || {
                            app.open_subtitle_sync()
                        }),
                    )
                    .into(),
                );
            }
            let (earlier, later, smaller, bigger) =
                (app.clone(), app.clone(), app.clone(), app.clone());
            items.insert(
                0,
                Padding::new(
                    EdgeInsets::only(4.0, 0.0, 0.0, 10.0),
                    col(vec![
                        row(vec![
                            SizedBox::new()
                                .width(118.0)
                                .child(text(
                                    format!("Delay {delay:+.2}s"),
                                    12.5,
                                    FontWeight::W500,
                                    MUTED,
                                ))
                                .into(),
                            pill("−", false, move || earlier.adjust_subtitle_delay(-0.25)),
                            hgap(6.0),
                            pill("+", false, move || later.adjust_subtitle_delay(0.25)),
                        ])
                        .into(),
                        gap(8.0),
                        row(vec![
                            SizedBox::new()
                                .width(118.0)
                                .child(text(
                                    format!("Size {size:.0}"),
                                    12.5,
                                    FontWeight::W500,
                                    MUTED,
                                ))
                                .into(),
                            pill("−", false, move || smaller.adjust_subtitle_size(-2.0)),
                            hgap(6.0),
                            pill("+", false, move || bigger.adjust_subtitle_size(2.0)),
                        ])
                        .into(),
                    ]),
                )
                .into(),
            );
            "Subtitles"
        }
    };
    // The scroll view fills whatever height it is offered, so offer only
    // what the entries need (up to a cap).
    let extra = if menu == Menu::Subtitles { 130.0 } else { 0.0 };
    let height = (34.0 + extra + items.len() as f32 * 34.0).min(420.0);
    let mut children: Vec<Widget> = vec![
        Padding::new(
            EdgeInsets::only(4.0, 0.0, 0.0, 10.0),
            text(title, 14.0, FontWeight::W600, TEXT),
        )
        .into(),
    ];
    children.extend(items);
    Some(
        Container::new()
            .width(320.0)
            .constraints(Constraints::tight_for(Some(320.0), None))
            .padding(EdgeInsets::all(12.0))
            .decoration(
                BoxDecoration::new()
                    .color(Color::rgba(18, 21, 32, 245))
                    .border(Border::new(1.0, LINE))
                    .border_radius(BorderRadius::all(Radius::circular(14.0))),
            )
            .child(
                ConstrainedBox::new(
                    Constraints::tight(Size::new(296.0, height)),
                    SingleChildScrollView::new(col(children)),
                ),
            )
            .into(),
    )
}

/// Every subtitle line with its time. Clicking the line being spoken right
/// now re-times the whole track, instead of guessing a delay.
fn sync_panel(app: &App) -> Widget {
    const WIDTH: f32 = 470.0;
    let cues = Rc::new(app.player.cue_list());
    let (delay, paused) = app.player.state.peek(|s| (s.sub_delay, s.paused));
    let position = app.player.position.get_untracked();
    let current = cues
        .iter()
        .position(|cue| cue.start <= position - delay && position - delay < cue.end);

    let (back, reset, pause) = (app.clone(), app.clone(), app.clone());
    let header = hrow(vec![
        text("Sync subtitles", 14.0, FontWeight::W600, TEXT).into(),
        hgap(10.0),
        text(format!("{delay:+.2}s"), 12.5, FontWeight::W600, ACCENT).into(),
        Spacer::new().into(),
        pill(if paused { "Play" } else { "Pause" }, false, move || pause.toggle_pause()),
        hgap(6.0),
        pill("Reset", false, move || reset.reset_subtitle_delay()),
        hgap(6.0),
        pill("Done", true, move || back.set_player_menu(Menu::Sync)),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center);

    let list: Widget = if cues.is_empty() {
        Padding::new(
            EdgeInsets::only(4.0, 20.0, 4.0, 0.0),
            muted("The subtitle lines are still loading. Try again in a moment."),
        )
        .into()
    } else {
        let controller = app.controllers.sync_list.clone();
        let app = app.clone();
        let count = cues.len();
        ListView::builder(count, move |index| {
            let cue = &cues[index];
            let selected = current == Some(index);
            let start = cue.start;
            let app = app.clone();
            let background = if selected { ACCENT_DIM } else { Color::TRANSPARENT };
            Button::with_child(hrow(vec![
                SizedBox::new()
                    .width(62.0)
                    .child(text(
                        format_time(cue.start),
                        12.0,
                        FontWeight::W600,
                        if selected { ACCENT } else { FAINT },
                    ))
                    .into(),
                Expanded::new(line(cue.text.replace('\n', "  "), 13.0, FontWeight::W500, TEXT))
                    .into(),
            ]))
            .style(ButtonStyle {
                background: Some(background),
                background_states: Some(
                    StateColor::new(background).hovered(Color::rgba(255, 255, 255, 30)),
                ),
                foreground: Some(TEXT),
                border: Some(Border::new(0.0, Color::TRANSPARENT)),
                border_radius: Some(8.0),
                padding: Some(EdgeInsets::symmetric(10.0, 0.0)),
                fixed_size: Some(Size::new(WIDTH - 24.0, App::SYNC_ROW - 4.0)),
                alignment: Some(Alignment::CENTER_LEFT),
                overlay_color: Some(Color::TRANSPARENT.into()),
                animation_duration: Some(std::time::Duration::ZERO),
                ..ButtonStyle::default()
            })
            .on_click(move || app.sync_subtitle_to(start))
        })
        .item_extent(App::SYNC_ROW)
        .controller(controller)
        .into()
    };

    Container::new()
        .width(WIDTH)
        .height(430.0)
        .padding(EdgeInsets::all(12.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(18, 21, 32, 245))
                .border_radius(BorderRadius::all(Radius::circular(16.0))),
        )
        .child(
            vcol(vec![
                header.into(),
                gap(6.0),
                text(
                    "Pause right as someone starts a line, then click that line below.",
                    12.0,
                    FontWeight::W400,
                    MUTED,
                )
                .into(),
                gap(10.0),
                Expanded::new(list).into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into()
}

fn top_bar(app: &App, title: &str, subtitle: &str) -> Widget {
    let back = app.clone();
    let mut titles: Vec<Widget> = vec![line(title, 16.0, FontWeight::W600, TEXT)];
    if !subtitle.is_empty() {
        titles.push(gap(2.0));
        titles.push(line(subtitle, 12.5, FontWeight::W400, MUTED));
    }
    Container::new()
        .color(OVERLAY)
        .padding(EdgeInsets::symmetric(18.0, 12.0))
        .child(
            hrow(vec![
                icon_button(icon(icons::back(), 18.0, TEXT), 40.0, move || back.back()),
                hgap(12.0),
                Expanded::new(col(titles)).into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .into()
}

fn bottom_bar(app: &App) -> Widget {
    let (live, paused, ended, volume, muted_now, speed, menu, has_next, torrent, has_subs) =
        app.player.state.peek(|s| {
            (
                s.live,
                s.paused,
                s.status == Status::Ended,
                s.volume,
                s.muted,
                s.speed,
                s.menu,
                s.next.is_some(),
                s.torrent.clone(),
                s.sub_choice != SubChoice::Off,
            )
        });
    let action = |call: fn(&App)| {
        let app = app.clone();
        move || call(&app)
    };
    let mut controls: Vec<Widget> = vec![
        icon_button(
            icon(
                if paused || ended { icons::play() } else { icons::pause() },
                20.0,
                TEXT,
            ),
            44.0,
            action(|app| app.toggle_pause()),
        ),
    ];
    if !live {
        controls.push(icon_button(
            icon(icons::rewind(), 18.0, TEXT),
            40.0,
            action(|app| app.seek_by(-10.0)),
        ));
        controls.push(icon_button(
            icon(icons::forward(), 18.0, TEXT),
            40.0,
            action(|app| app.seek_by(10.0)),
        ));
    }
    if has_next {
        controls.push(icon_button(
            icon(icons::next(), 18.0, TEXT),
            40.0,
            action(|app| {
                if let Some(next) = app.player.state.peek(|s| s.next.clone()) {
                    app.play_next(next);
                }
            }),
        ));
    }
    controls.push(hgap(6.0));
    controls.push(icon_button(
        icon(
            if muted_now || volume <= 0.0 { icons::muted() } else { icons::volume() },
            18.0,
            TEXT,
        ),
        40.0,
        action(|app| app.toggle_mute()),
    ));
    {
        let app = app.clone();
        controls.push(slider(
            96.0,
            if muted_now { 0.0 } else { volume },
            Rc::new(move |value, _| app.set_volume(value)),
        ));
    }
    controls.push(hgap(14.0));
    controls.push(clock(app));
    controls.push(Spacer::new().into());
    if let Some(stats) = torrent {
        controls.push(
            text(
                format!("{} peers · {}", stats.peers, format_speed(stats.download_speed)),
                12.0,
                FontWeight::W500,
                MUTED,
            )
            .into(),
        );
        controls.push(hgap(14.0));
    }
    for (label, target, active) in [
        (format!("{speed}×"), Menu::Speed, (speed - 1.0).abs() > 0.01),
        ("Audio".to_owned(), Menu::Audio, false),
        ("Subtitles".to_owned(), Menu::Subtitles, has_subs),
    ] {
        if live && target == Menu::Speed {
            continue;
        }
        let app = app.clone();
        controls.push(pill(label, active || menu == target, move || {
            app.set_player_menu(target)
        }));
        controls.push(hgap(8.0));
    }
    controls.push(icon_button(
        icon(icons::fullscreen(), 18.0, TEXT),
        40.0,
        action(|app| app.toggle_fullscreen()),
    ));

    Container::new()
        .color(OVERLAY)
        .padding(EdgeInsets::only(20.0, 8.0, 20.0, 12.0))
        .child(
            vcol(vec![
                if live { gap(6.0) } else { seek_bar(app) },
                gap(2.0),
                Row::new(controls)
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .into(),
            ])
            .main_axis_size(MainAxisSize::Min)
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into()
}

fn stat(label: &str, value: String) -> Widget {
    Column::new(vec![
        text(value, 15.0, FontWeight::W700, TEXT).into(),
        gap(2.0),
        text(label, 11.0, FontWeight::W500, FAINT).into(),
    ] as Vec<Widget>)
    .main_axis_size(MainAxisSize::Min)
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .into()
}

fn status_card(children: Vec<Widget>) -> Widget {
    Center::new(
        Container::new()
            .padding(EdgeInsets::symmetric(26.0, 20.0))
            .decoration(
                BoxDecoration::new()
                    .color(OVERLAY)
                    .border_radius(BorderRadius::all(Radius::circular(16.0))),
            )
            .child(
                Column::new(children)
                    .main_axis_size(MainAxisSize::Min)
                    .cross_axis_alignment(CrossAxisAlignment::Center),
            ),
    )
    .into()
}

pub fn screen(app: &App) -> Widget {
    let (stage, status, title, subtitle, visible, menu, stalled, has_video, torrent, next) =
        app.player.state.read(|s| {
            (
                s.stage.clone(),
                s.status.clone(),
                s.title.clone(),
                s.subtitle.clone(),
                s.controls_visible,
                s.menu,
                s.stalled,
                s.has_video,
                s.torrent.clone(),
                s.next.clone(),
            )
        });

    let picture = {
        let frame = app.player.frame.clone();
        LayoutBuilder::new(move |_, constraints| {
            let size = constraints.biggest();
            match frame.get() {
                Some(handle) => Image::new(handle)
                    .width(size.width)
                    .height(size.height)
                    .fit(BoxFit::Contain)
                    .into(),
                None => SizedBox::shrink().into(),
            }
        })
    };

    let captions = {
        let app = app.clone();
        Builder::new(move |_| {
            let cue = app.player.cue.get();
            if cue.is_empty() {
                return SizedBox::shrink().into();
            }
            let (size, lifted) = app
                .player
                .state
                .read(|s| (s.sub_size, s.controls_visible));
            Align::new(
                Alignment::BOTTOM_CENTER,
                Padding::new(
                    EdgeInsets::only(48.0, 0.0, 48.0, if lifted { 128.0 } else { 44.0 }),
                    IgnorePointer::new(
                        Container::new()
                            .padding(EdgeInsets::symmetric(14.0, 6.0))
                            .decoration(
                                BoxDecoration::new()
                                    .color(Color::rgba(0, 0, 0, 150))
                                    .border_radius(BorderRadius::all(Radius::circular(8.0))),
                            )
                            .child(
                                // One Text per line, centred by the column:
                                // `TextAlign::Center` on shrink-wrapped text
                                // clips in Incular 0.1.1.
                                Column::new(cue.lines().map(|line| {
                                    Text::new(line.to_owned())
                                        .style(style(size, FontWeight::W600, Color::WHITE))
                                }))
                                .main_axis_size(MainAxisSize::Min)
                                .cross_axis_alignment(CrossAxisAlignment::Center),
                            ),
                    ),
                ),
            )
            .into()
        })
    };

    // The click-to-pause surface is a sibling layer underneath the controls
    // rather than their ancestor: an ancestor GestureDetector also fires for
    // taps that land on the buttons.
    let (tap, double) = (app.clone(), app.clone());
    let surface = GestureDetector::new(Container::new().color(Color::TRANSPARENT))
        .behavior(HitTestBehavior::Opaque)
        .on_tap(move || {
            if tap.player.state.peek(|s| s.menu != Menu::None) {
                tap.set_player_menu(Menu::None);
            } else {
                tap.toggle_pause();
            }
        })
        .on_double_tap(move || double.toggle_fullscreen());

    let mut layers: Vec<Widget> = vec![
        Container::new().color(Color::BLACK).into(),
        picture.into(),
        captions.into(),
        surface.into(),
    ];

    match &status {
        Status::Opening => {
            let mut lines: Vec<Widget> = vec![
                text("OPENING", 11.0, FontWeight::W700, ACCENT).into(),
                gap(8.0),
                SizedBox::new()
                    .width(380.0)
                    .child(
                        Column::new(vec![line(title.clone(), 19.0, FontWeight::W700, TEXT)])
                            .main_axis_size(MainAxisSize::Min)
                            .cross_axis_alignment(CrossAxisAlignment::Center),
                    )
                    .into(),
            ];
            if !subtitle.is_empty() {
                lines.push(gap(3.0));
                lines.push(text(subtitle.clone(), 13.0, FontWeight::W400, MUTED).into());
            }
            lines.push(gap(18.0));
            lines.push(
                incular::controls::Progress::new()
                    .indeterminate(true)
                    .width(380.0)
                    .height(4.0)
                    .into(),
            );
            lines.push(gap(14.0));
            lines.push(text(stage.clone(), 13.0, FontWeight::W500, TEXT).into());
            if let Some(stats) = &torrent {
                lines.push(gap(12.0));
                lines.push(
                    row(vec![
                        stat("Peers", stats.peers.to_string()),
                        hgap(26.0),
                        stat("Download", format_speed(stats.download_speed)),
                    ])
                    .into(),
                );
            }
            layers.push(status_card(lines));
        }
        Status::Error(message) => {
            let back = app.clone();
            layers.push(status_card(vec![
                text("Playback failed", 17.0, FontWeight::W600, TEXT).into(),
                gap(8.0),
                SizedBox::new()
                    .width(460.0)
                    .child(
                        Text::new(message.clone()).style(style(13.5, FontWeight::W400, MUTED)),
                    )
                    .into(),
                gap(16.0),
                button("Back to streams", Tone::Primary, move || back.back()),
            ]));
        }
        Status::Ended => {
            let (replay, forward) = (app.clone(), app.clone());
            let mut actions = vec![button("Replay", Tone::Neutral, move || replay.seek_to(0.0))];
            if let Some(next) = next {
                actions.push(hgap(10.0));
                actions.push(button("Next episode", Tone::Primary, move || {
                    forward.play_next(next.clone())
                }));
            }
            layers.push(status_card(vec![
                text("Finished", 17.0, FontWeight::W600, TEXT).into(),
                gap(14.0),
                row(actions).into(),
            ]));
        }
        Status::Playing if stalled => {
            layers.push(status_card(vec![
                text("Buffering…", 15.0, FontWeight::W600, TEXT).into(),
            ]));
        }
        Status::Playing if !has_video => {
            layers.push(status_card(vec![
                text("Audio only", 15.0, FontWeight::W600, MUTED).into(),
            ]));
        }
        _ => {}
    }

    if visible {
        let mut chrome: Vec<Widget> = vec![
            top_bar(app, &title, &subtitle),
            Spacer::new().into(),
        ];
        if let Some(panel) = menu_panel(app, menu) {
            chrome.push(
                Align::new(
                    Alignment::BOTTOM_RIGHT,
                    Padding::new(EdgeInsets::only(0.0, 0.0, 20.0, 10.0), panel),
                )
                .into(),
            );
        }
        chrome.push(bottom_bar(app));
        layers.push(
            Column::new(chrome)
                .cross_axis_alignment(CrossAxisAlignment::Stretch)
                .into(),
        );
    }

    // The player owns keyboard focus while it is on screen, and its buttons
    // are kept out of the focus system: Incular has no way to hand focus
    // back programmatically, and a focused button would swallow Space.
    let (hover, keys) = (app.clone(), app.clone());
    KeyboardListener::new(
        MouseRegion::new(ExcludeFocus::new(Stack::new(layers).fit(StackFit::Expand)))
            .on_hover(move |_| hover.player_activity()),
    )
    .focus_node(app.controllers.player_focus.clone())
    .autofocus(true)
    .on_key(move |event| super::handle_key(&keys, &event))
    .into()
}
