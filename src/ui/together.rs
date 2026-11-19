//! Watch-together UI: the Watch together page (create / join a room, then
//! the room itself) and the room panel in the player.

use super::motion;
use super::widgets::{PAGE_PADDING, page, smooth};
use crate::state::App;
use crate::theme::*;
use crate::together::{BUBBLE_SECONDS, ChatLine, Link, Role};
use incular::prelude::*;
use std::time::Duration;

fn role_color(role: Role) -> Color {
    match role {
        Role::Host => ACCENT,
        Role::Cohost => Color::rgba(200, 200, 200, 255),
        Role::Viewer => FAINT,
    }
}

fn role_glyph(role: Role) -> Glyph {
    match role {
        Role::Host => icons::crown(),
        Role::Cohost => icons::play_circle(),
        Role::Viewer => icons::eye(),
    }
}

/// What a role may do, for the badge's tooltip.
fn role_hint(role: Role, mine: bool) -> &'static str {
    match (role, mine) {
        (Role::Host, true) => "You're the host: you control playback and manage the room",
        (Role::Cohost, true) => "You're a co-host: you can control playback",
        (Role::Viewer, true) => "You're a viewer: playback follows the host",
        (Role::Host, false) => "Host: controls playback and manages the room",
        (Role::Cohost, false) => "Co-host: can control playback",
        (Role::Viewer, false) => "Viewer: follows the host",
    }
}

fn badge(role: Role, mine: bool) -> Widget {
    let color = role_color(role);
    super::tooltip::tooltip(
        role_hint(role, mine),
        Container::new()
            .padding(EdgeInsets::only(6.0, 3.0, 8.0, 3.0))
            .decoration(
                BoxDecoration::new()
                    .color(Color::rgba(255, 255, 255, 14))
                    .border_radius(BorderRadius::all(Radius::circular(9.0))),
            )
            .child(row(vec![
                icon(role_glyph(role), 11.0, color),
                hgap(4.0),
                text(role.label(), 10.5, FontWeight::W700, color).into(),
            ])),
    )
}

thread_local! {
    static CHAT_SELECTION: incular::widgets::SelectionAreaController =
        incular::widgets::SelectionAreaController::new();
    /// The room password is shown in plain text.
    static PASSWORD_SHOWN: Signal<bool> = Signal::new(false);
}

/// A small icon-only button with a tooltip.
fn tool(glyph: Glyph, tip: &str, on_click: impl Fn() + 'static) -> Widget {
    super::tooltip::tooltip(
        tip,
        Widget::from(small_icon_button(glyph, None, Tone::Ghost, on_click)).accessibility_label(tip),
    )
}

fn copy_code(room: &str) -> Widget {
    let room = room.to_owned();
    tool(icons::copy(), "Copy room code", move || {
        crate::state::copy_text(room.clone(), "Room code copied".into())
    })
}

/// The room password: masked until the eye is clicked, with a copy button.
/// Only this pill rebuilds when it is revealed.
fn password_pill(password: String, size: f32) -> Widget {
    Builder::new(move |_| {
        let shown = PASSWORD_SHOWN.with(|s| s.get());
        let value = if shown {
            password.clone()
        } else {
            "\u{2022}".repeat(password.chars().count().clamp(6, 12))
        };
        let copied = password.clone();
        Container::new()
            .padding(EdgeInsets::only(9.0, 0.0, 2.0, 0.0))
            .decoration(
                BoxDecoration::new()
                    .color(Color::rgba(255, 255, 255, 14))
                    .border_radius(BorderRadius::all(Radius::circular(10.0))),
            )
            .child(row(vec![
                super::tooltip::tooltip("Password", icon(icons::lock(), 13.0, MUTED)),
                hgap(7.0),
                text(value, size, FontWeight::W600, TEXT).max_lines(Some(1)).into(),
                hgap(4.0),
                tool(
                    if shown { icons::eye_closed() } else { icons::eye() },
                    if shown { "Hide password" } else { "Show password" },
                    move || {
                        PASSWORD_SHOWN.with(|s| {
                            s.set(!shown);
                        })
                    },
                ),
                tool(icons::copy(), "Copy password", move || {
                    crate::state::copy_text(copied.clone(), "Password copied".into())
                }),
            ]))
            .into()
    })
    .into()
}

/// "Chat" with its clear buttons: for this app, and (host and co-hosts)
/// for everyone in the room.
fn chat_title(app: &App, title: Widget, width: f32) -> Widget {
    let role = app.together.state.peek(|s| s.role);
    let mut children = vec![title, Spacer::new().into()];
    let here = app.clone();
    children.push(tool(icons::eraser(), "Clear chat for me", move || here.together_clear_chat_here()));
    if role.controls() {
        let everyone = app.clone();
        children.push(hgap(2.0));
        children.push(tool(icons::trash_2(), "Clear chat for everyone", move || {
            everyone.together_clear_chat_everywhere()
        }));
    }
    SizedBox::new()
        .width(width)
        .child(hrow(children).cross_axis_alignment(CrossAxisAlignment::Center))
        .into()
}

fn members(app: &App, width: f32) -> Widget {
    let (list, me, my_role, host_online) =
        app.together.state.peek(|s| (s.members.clone(), s.me, s.role, s.host_online));
    let mut rows: Vec<Widget> = Vec::new();
    if !host_online {
        rows.push(
            text(
                "The host is away. Co-hosts are in charge; with none, playback waits for the host.",
                11.5,
                FontWeight::W500,
                ACCENT,
            )
            .into(),
        );
        rows.push(gap(6.0));
    }
    for member in list {
        let mut line: Vec<Widget> = vec![
            Flexible::new(line(
                if member.id == me { format!("{} (you)", member.name) } else { member.name.clone() },
                13.0,
                FontWeight::W600,
                TEXT,
            ))
            .into(),
            hgap(8.0),
            badge(member.role, member.id == me),
        ];
        if my_role == Role::Host && member.id != me {
            line.push(Spacer::new().into());
            let id = member.id;
            let (toggle, promote, kick) = (app.clone(), app.clone(), app.clone());
            let (label, next) = if member.role == Role::Cohost {
                ("Make viewer", Role::Viewer)
            } else {
                ("Make co-host", Role::Cohost)
            };
            line.push(chip(label, false, move || toggle.together_set_role(id, next)));
            line.push(hgap(6.0));
            line.push(chip("Make host", false, move || promote.together_set_role(id, Role::Host)));
            line.push(hgap(6.0));
            line.push(chip("Remove", false, move || kick.together_kick(id)));
        }
        rows.push(
            SizedBox::new()
                .width(width)
                .child(hrow(line).cross_axis_alignment(CrossAxisAlignment::Center))
                .into(),
        );
        rows.push(gap(6.0));
    }
    col(rows).into()
}

/// Local `HH:MM` of a unix time.
fn clock(at: u64) -> String {
    chrono::DateTime::from_timestamp(at as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

fn chat_row(line: &ChatLine, width: f32) -> Widget {
    match &line.from {
        None => SizedBox::new()
            .width(width)
            .child(text(line.text.clone(), 11.5, FontWeight::W500, FAINT))
            .into(),
        Some(from) => SizedBox::new()
            .width(width)
            .child(
                hrow(vec![
                    text(clock(line.at), 11.0, FontWeight::W500, FAINT).into(),
                    hgap(6.0),
                    text(
                        from.clone(),
                        12.5,
                        FontWeight::W700,
                        if line.mine { ACCENT } else { role_color(line.role).with_alpha_if_faint() },
                    )
                    .into(),
                    hgap(8.0),
                    Expanded::new(text(line.text.clone(), 12.5, FontWeight::W400, TEXT)).into(),
                ])
                .cross_axis_alignment(CrossAxisAlignment::Start),
            )
            .into(),
    }
}

/// Faint (viewer) names are hard to read in the chat; lift them to muted.
trait Readable {
    fn with_alpha_if_faint(self) -> Color;
}
impl Readable for Color {
    fn with_alpha_if_faint(self) -> Color {
        if self == FAINT { MUTED } else { self }
    }
}

fn chat(app: &App, width: f32, height: f32) -> Widget {
    let lines = app.together.state.peek(|s| s.chat.clone());
    let mut rows: Vec<Widget> = Vec::new();
    if lines.is_empty() {
        rows.push(muted("No messages yet. Say hi!"));
    }
    for line in &lines {
        rows.push(chat_row(line, width - 16.0));
        rows.push(gap(6.0));
    }
    // Newest at the bottom: keep the view there.
    let controller = app.controllers.watch_chat_scroll.clone();
    // Lines can be selected with the mouse and copied (Ctrl+C, or the
    // right-click menu).
    let selection = CHAT_SELECTION.with(Clone::clone);
    let rows: Widget = incular::material::SelectionArea::with_controller(selection, col(rows)).into();
    controller.deferred_jump_to(f32::MAX / 4.0);
    Container::new()
        .width(width)
        .height(height)
        .padding(EdgeInsets::all(8.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(0, 0, 0, 60))
                .border_radius(BorderRadius::all(Radius::circular(10.0))),
        )
        .child(SingleChildScrollView::new(rows).controller(controller))
        .into()
}

fn composer(app: &App, width: f32) -> Widget {
    let send = |app: &App| {
        let value = app.controllers.watch_chat.text();
        if !value.trim().is_empty() {
            app.together_chat(&value);
            app.controllers.watch_chat.set_text("");
        }
    };
    let (submit, click) = (app.clone(), app.clone());
    // The field takes what the button leaves, so the row ends exactly at
    // `width` and the button keeps the chat box's margin.
    SizedBox::new()
        .width(width)
        .child(
            hrow(vec![
                Expanded::new(input(&app.controllers.watch_chat, "Message the room", width - 90.0, move |_| {
                    send(&submit)
                }))
                .into(),
                hgap(10.0),
                button("Send", Tone::Primary, move || send(&click)),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .into()
}

/// Room header, people and chat. `width` is the inner width.
pub fn room_view(app: &App, width: f32, chat_height: f32) -> Widget {
    let (room, role, password, link) =
        app.together.state.read(|s| (s.room.clone(), s.role, s.password.clone(), s.link.clone()));
    let leave = app.clone();
    let header: Vec<Widget> = vec![
        super::tooltip::tooltip("Room code", icon(icons::users(), 16.0, ACCENT)),
        hgap(8.0),
        text(room.clone(), 18.0, FontWeight::W700, ACCENT).into(),
        hgap(2.0),
        copy_code(&room),
        hgap(8.0),
        badge(role, true),
        Spacer::new().into(),
        chip("Leave", false, move || leave.together_leave()),
    ];

    let mut children: Vec<Widget> = vec![
        SizedBox::new()
            .width(width)
            .child(hrow(header).cross_axis_alignment(CrossAxisAlignment::Center))
            .into(),
        gap(6.0),
    ];
    if !password.is_empty() {
        children.push(password_pill(password, 12.5));
        children.push(gap(8.0));
    }
    if link == Link::Reconnecting {
        children.push(text("Reconnecting to the room…", 12.0, FontWeight::W600, ACCENT).into());
        children.push(gap(6.0));
    }
    children.push(gap(6.0));
    children.push(text("People", 12.0, FontWeight::W600, FAINT).into());
    children.push(gap(6.0));
    children.push(members(app, width));
    children.push(gap(2.0));
    children.push(chat_title(app, text("Chat", 12.0, FontWeight::W600, FAINT).into(), width));
    children.push(gap(4.0));
    children.push(chat(app, width, chat_height));
    children.push(gap(8.0));
    children.push(composer(app, width));
    col(children).into()
}

/// Recent chat lines floating over the video while the room panel is shut.
pub fn bubbles(app: &App) -> Option<Widget> {
    let lines: Vec<ChatLine> = app.together.state.read(|s| {
        s.chat
            .iter()
            .rev()
            .filter(|line| line.received.elapsed() < Duration::from_secs(BUBBLE_SECONDS))
            .take(3)
            .cloned()
            .collect()
    });
    if lines.is_empty() {
        return None;
    }
    let rows: Vec<Widget> = lines
        .iter()
        .rev()
        .map(|line| {
            Padding::new(
                EdgeInsets::only(0.0, 0.0, 0.0, 6.0),
                Container::new()
                    .padding(EdgeInsets::symmetric(12.0, 7.0))
                    .decoration(
                        BoxDecoration::new()
                            .color(Color::rgba(16, 16, 16, 215))
                            .border_radius(BorderRadius::all(Radius::circular(12.0))),
                    )
                    .child(chat_row(line, 320.0)),
            )
            .into()
        })
        .collect();
    Some(
        Align::new(
            Alignment::BOTTOM_LEFT,
            Padding::new(
                EdgeInsets::only(24.0, 0.0, 0.0, 130.0),
                IgnorePointer::new(
                    Column::new(rows)
                        .main_axis_size(MainAxisSize::Min)
                        .cross_axis_alignment(CrossAxisAlignment::Start),
                ),
            ),
        )
        .into(),
    )
}

/// The room panel in the player.
pub fn player_panel(app: &App) -> Widget {
    const WIDTH: f32 = 440.0;
    Container::new()
        .width(WIDTH)
        .padding(EdgeInsets::all(14.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(24, 24, 24, 245))
                .border_radius(BorderRadius::all(Radius::circular(16.0))),
        )
        .child(room_view(app, WIDTH - 28.0, 220.0))
        .into()
}

// ------------------------------------------------------------ the page

fn field(label: &str, hint: &str, control: Widget) -> Widget {
    col(vec![
        text(label, 13.0, FontWeight::W600, TEXT).into(),
        gap(2.0),
        text(hint, 11.5, FontWeight::W400, FAINT).into(),
        gap(8.0),
        control,
    ])
    .into()
}

fn save_name(app: &App) {
    let name = app.controllers.watch_name.text().trim().to_owned();
    if app.profile.peek(|p| p.settings.watch_name != name) {
        app.update_settings(|s| s.watch_name = name);
    }
}

fn create_room(app: &App) {
    save_name(app);
    let password = app.controllers.watch_password.text();
    app.together_join("", &password);
}

fn join_room(app: &App) {
    let code = app.controllers.watch_code.text().trim().to_owned();
    if code.is_empty() {
        app.notify("Enter a room code first");
        return;
    }
    save_name(app);
    let password = app.controllers.watch_join_password.text();
    app.together_join(&code, &password);
}

fn panel_title(glyph: Glyph, title: &str, subtitle: &str) -> Widget {
    hrow(vec![
        Container::new()
            .width(40.0)
            .height(40.0)
            .radius(12.0)
            .color(ACCENT_DIM)
            .alignment(Alignment::CENTER)
            .child(icon(glyph, 19.0, ACCENT))
            .into(),
        hgap(14.0),
        Expanded::new(col(vec![
            text(title, 17.0, FontWeight::W700, TEXT).into(),
            gap(2.0),
            text(subtitle, 12.5, FontWeight::W400, MUTED).into(),
        ]))
        .into(),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .into()
}

/// Not in a room: create one or join one.
fn lobby(app: &App, width: f32) -> Widget {
    let connecting = app.together.state.read(|s| s.link == Link::Connecting);
    let side_by_side = width >= 2.0 * 340.0 + 16.0;
    let panel = if side_by_side { (width - 16.0) / 2.0 } else { width };
    let field_width = (panel - 40.0).min(360.0);

    let (create_submit, create_click) = (app.clone(), app.clone());
    let create = card(
        col(vec![
            panel_title(icons::plus(), "Start a room", "You'll be the host and control playback"),
            gap(22.0),
            field(
                "Your name",
                "What others in the room see",
                input(&app.controllers.watch_name, "Guest", field_width, |_| {}).into(),
            ),
            gap(16.0),
            field(
                "Password",
                "Optional. Only people who know it can join",
                input(&app.controllers.watch_password, "No password", field_width, move |_| {
                    create_room(&create_submit)
                })
                .into(),
            ),
            gap(22.0),
            button("Create room", Tone::Primary, move || create_room(&create_click)),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .width(panel);

    let (code_submit, password_submit, join_click) = (app.clone(), app.clone(), app.clone());
    let join = card(
        col(vec![
            panel_title(icons::log_in(), "Join a room", "Use the code a friend shared with you"),
            gap(22.0),
            field(
                "Your name",
                "What others in the room see",
                input(&app.controllers.watch_name, "Guest", field_width, |_| {}).into(),
            ),
            gap(16.0),
            field(
                "Room code",
                "Six letters and digits, like K7MQ2P",
                input(&app.controllers.watch_code, "Room code", field_width, move |_| join_room(&code_submit))
                    .into(),
            ),
            gap(16.0),
            field(
                "Password",
                "Only if the room has one",
                input(&app.controllers.watch_join_password, "No password", field_width, move |_| {
                    join_room(&password_submit)
                })
                .into(),
            ),
            gap(22.0),
            button("Join room", Tone::Neutral, move || join_room(&join_click)),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .width(panel);

    let mut children = vec![
        muted(
            "Watch in sync with friends. Everyone streams the video on their own; the room keeps \
             play, pause, seeking and the chosen video in step, and carries the chat.",
        ),
        gap(20.0),
    ];
    if connecting {
        let cancel = app.clone();
        children.push(
            card(
                hrow(vec![
                    text("Connecting to the room…", 14.0, FontWeight::W600, TEXT).into(),
                    Spacer::new().into(),
                    button("Cancel", Tone::Neutral, move || cancel.together_leave()),
                ])
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .width(width)
            .into(),
        );
    } else if side_by_side {
        children.push(
            hrow(vec![create.into(), hgap(16.0), join.into()])
                .cross_axis_alignment(CrossAxisAlignment::Start)
                .into(),
        );
    } else {
        children.push(create.into());
        children.push(gap(16.0));
        children.push(join.into());
    }
    col(children).cross_axis_alignment(CrossAxisAlignment::Start).into()
}

fn room_header(app: &App, width: f32) -> Widget {
    let (room, role, password, link, count) = app
        .together
        .state
        .read(|s| (s.room.clone(), s.role, s.password.clone(), s.link.clone(), s.members.len()));
    let mut actions: Vec<Widget> = Vec::new();
    let leave = app.clone();
    actions.push(button("Leave room", Tone::Ghost, move || leave.together_leave()));

    let people = format!("{count} {}", if count == 1 { "person" } else { "people" });
    let mut facts: Vec<Widget> = vec![super::tooltip::tooltip(
        people.clone(),
        Widget::from(row(vec![
            icon(icons::users(), 14.0, MUTED),
            hgap(6.0),
            text(count.to_string(), 13.0, FontWeight::W600, MUTED).into(),
        ]))
        .accessibility_label(people),
    )];
    if !password.is_empty() {
        facts.push(hgap(12.0));
        facts.push(password_pill(password, 13.0));
    }
    let mut info: Vec<Widget> = vec![
        text("ROOM CODE", 10.5, FontWeight::W700, FAINT).into(),
        gap(4.0),
        hrow(vec![
            text(room.clone(), 30.0, FontWeight::W800, ACCENT).into(),
            hgap(6.0),
            copy_code(&room),
            hgap(10.0),
            badge(role, true),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into(),
        gap(8.0),
        hrow(facts).cross_axis_alignment(CrossAxisAlignment::Center).into(),
    ];
    if link == Link::Reconnecting {
        info.push(gap(6.0));
        info.push(text("Reconnecting to the room…", 12.5, FontWeight::W600, ACCENT).into());
    }
    card(
        hrow(vec![
            Expanded::new(col(info).cross_axis_alignment(CrossAxisAlignment::Start)).into(),
            hrow(actions).cross_axis_alignment(CrossAxisAlignment::Center).into(),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .width(width)
    .into()
}

fn now_playing(app: &App, width: f32) -> Widget {
    let (now, role) = app.together.state.read(|s| (s.now_playing.clone(), s.role));
    let body: Vec<Widget> = match now {
        Some(now) => {
            let watch = app.clone();
            let mut lines = vec![
                text("NOW PLAYING", 10.5, FontWeight::W700, FAINT).into(),
                gap(6.0),
                text(now.title.clone(), 18.0, FontWeight::W700, TEXT).max_lines(Some(1)).into(),
            ];
            if !now.episode.is_empty() {
                lines.push(gap(2.0));
                lines.push(text(now.episode.clone(), 13.0, FontWeight::W500, MUTED).max_lines(Some(1)).into());
            }
            if !now.facts.is_empty() {
                lines.push(gap(2.0));
                lines.push(text(now.facts.clone(), 12.0, FontWeight::W400, FAINT).into());
            }
            // Poster, button and gaps take this much of the card.
            let bar = (width - 40.0 - 64.0 - 16.0 - 16.0 - 110.0).clamp(140.0, 520.0);
            lines.push(gap(12.0));
            lines.push(position_line(app, bar));
            vec![
                super::widgets::net_image(now.poster.as_deref(), 64.0, 96.0, 8.0, 185, &now.title),
                hgap(16.0),
                Expanded::new(col(lines).cross_axis_alignment(CrossAxisAlignment::Start)).into(),
                hgap(16.0),
                icon_text_button(icons::play(), "Watch", Tone::Primary, move || watch.together_watch()),
            ]
        }
        None => {
            let hint = if role.controls() {
                "Play something from Home, Discover or your Library: everyone in the room follows."
            } else {
                "Waiting for the host to pick something. It opens for you as soon as they do."
            };
            let mut children: Vec<Widget> = vec![
                Expanded::new(col(vec![
                    text("NOTHING PLAYING YET", 10.5, FontWeight::W700, FAINT).into(),
                    gap(6.0),
                    text(hint, 13.5, FontWeight::W500, MUTED).into(),
                ]))
                .into(),
            ];
            if role.controls() {
                let browse = app.clone();
                children.push(hgap(16.0));
                children.push(button("Browse", Tone::Primary, move || browse.go(crate::state::Route::Board)));
            }
            children
        }
    };
    card(hrow(body).cross_axis_alignment(CrossAxisAlignment::Center)).width(width).into()
}

thread_local! {
    /// Bumped once a second while the room is playing, rebuilding only the
    /// position line (not the chat or the rest of the page).
    static TICK: Signal<u64> = Signal::new(0);
    static TICK_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Playing or paused, where the room is (`12:04 / 1:52:10`), a progress
/// bar, and who the room is waiting for.
fn position_line(app: &App, width: f32) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        TICK.with(|t| t.get());
        let (clock, waiting) = app.together.state.peek(|s| (s.clock, s.waiting.clone()));
        let Some(clock) = clock else {
            return text("Starting…", 12.5, FontWeight::W500, MUTED).into();
        };
        if !clock.paused && !TICK_PENDING.with(|p| p.replace(true)) {
            crate::state::spawn(async { tokio::time::sleep(Duration::from_secs(1)).await }, |_, ()| {
                TICK_PENDING.with(|p| p.set(false));
                TICK.with(|t| t.set(t.get_untracked().wrapping_add(1)));
            });
        }
        let at = clock.now();
        let mut time = crate::util::format_time(at);
        if let Some(duration) = clock.duration {
            time = format!("{time} / {}", crate::util::format_time(duration));
        }
        let mut state = vec![
            icon(if clock.paused { icons::pause() } else { icons::play() }, 13.0, TEXT),
            hgap(6.0),
            text(if clock.paused { "Paused" } else { "Playing" }, 12.5, FontWeight::W600, TEXT).into(),
            hgap(10.0),
            text(time, 12.5, FontWeight::W500, MUTED).into(),
        ];
        if (clock.speed - 1.0).abs() > 0.01 {
            state.push(hgap(10.0));
            state.push(text(format!("{}×", (clock.speed * 100.0).round() / 100.0), 12.5, FontWeight::W500, MUTED).into());
        }
        if let Some(duration) = clock.duration {
            let left = (duration - at).max(0.0);
            state.push(hgap(10.0));
            state.push(text(format!("{} left", crate::util::format_time(left)), 12.5, FontWeight::W400, FAINT).into());
        }
        let mut lines = vec![row(state).into()];
        if let Some(duration) = clock.duration {
            let fraction = (at / duration).clamp(0.0, 1.0) as f32;
            lines.push(gap(8.0));
            lines.push(
                layers(vec![
                    Container::new().width(width).height(4.0).radius(2.0).color(Color::rgba(255, 255, 255, 40)).into(),
                    Container::new().width((width * fraction).max(4.0)).height(4.0).radius(2.0).color(ACCENT).into(),
                ])
                .into(),
            );
        }
        if !waiting.is_empty() {
            let names: Vec<String> = waiting.into_iter().map(|(_, name)| name).collect();
            lines.push(gap(8.0));
            lines.push(text(format!("Waiting for {} to load", names.join(", ")), 12.0, FontWeight::W500, ACCENT).into());
        }
        col(lines).into()
    })
    .into()
}

fn titled(glyph: Glyph, title: &str) -> Widget {
    hrow(vec![icon(glyph, 15.0, MUTED), hgap(8.0), text(title, 14.0, FontWeight::W700, TEXT).into()])
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into()
}

fn people_card(app: &App, width: f32) -> Widget {
    card(
        col(vec![titled(icons::users(), "People"), gap(12.0), members(app, width - 40.0)])
            .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .width(width)
    .into()
}

fn chat_card(app: &App, width: f32, height: f32) -> Widget {
    let inner = width - 40.0;
    card(
        col(vec![
            chat_title(app, titled(icons::message_square(), "Chat"), inner),
            gap(10.0),
            chat(app, inner, height),
            gap(10.0),
            composer(app, inner),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .width(width)
    .into()
}

fn room_page(app: &App, width: f32) -> Widget {
    if width >= 900.0 {
        let side = 380.0;
        let main = width - side - 16.0;
        hrow(vec![
            col(vec![
                room_header(app, main),
                gap(16.0),
                now_playing(app, main),
                gap(16.0),
                people_card(app, main),
            ])
            .into(),
            hgap(16.0),
            chat_card(app, side, 420.0),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .into()
    } else {
        col(vec![
            room_header(app, width),
            gap(16.0),
            now_playing(app, width),
            gap(16.0),
            people_card(app, width),
            gap(16.0),
            chat_card(app, width, 320.0),
        ])
        .into()
    }
}

/// The Watch together page: a lobby, or the room you are in.
pub fn screen(app: &App) -> Widget {
    let app = app.clone();
    let content = LayoutBuilder::new(move |_, constraints| {
        let width = (constraints.max_width() - PAGE_PADDING.left - PAGE_PADDING.right).max(320.0);
        let in_room = app.together.state.read(|s| matches!(s.link, Link::Connected | Link::Reconnecting));
        let body = if in_room { room_page(&app, width) } else { lobby(&app, width) };
        smooth(
            &app.controllers.together,
            SingleChildScrollView::new(Padding::new(PAGE_PADDING, body))
                .physics(motion::APP_SCROLLED)
                .controller(app.controllers.together.clone()),
        )
    });
    page("Watch together", vec![], content.into())
}
