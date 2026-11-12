//! Watch-together room UI: people with their roles, and the chat. Shown in
//! Settings and as a panel in the player.

use crate::state::App;
use crate::theme::*;
use crate::together::{BUBBLE_SECONDS, ChatLine, Link, Role};
use incular::prelude::*;
use std::time::Duration;

fn role_color(role: Role) -> Color {
    match role {
        Role::Host => ACCENT,
        Role::Cohost => Color::rgba(110, 190, 255, 255),
        Role::Viewer => FAINT,
    }
}

fn badge(role: Role) -> Widget {
    let color = role_color(role);
    Container::new()
        .padding(EdgeInsets::symmetric(7.0, 2.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(255, 255, 255, 14))
                .border_radius(BorderRadius::all(Radius::circular(8.0))),
        )
        .child(text(role.label(), 10.5, FontWeight::W700, color))
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
            badge(member.role),
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
        .child(SingleChildScrollView::new(col(rows)).controller(controller))
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
    row(vec![
        input(&app.controllers.watch_chat, "Message the room", width - 82.0, move |_| send(&submit)).into(),
        hgap(8.0),
        button("Send", Tone::Primary, move || send(&click)),
    ])
    .into()
}

/// Room header, people and chat. `width` is the inner width.
pub fn room_view(app: &App, width: f32, chat_height: f32) -> Widget {
    let (room, role, protected, link) =
        app.together.state.read(|s| (s.room.clone(), s.role, s.protected, s.link.clone()));
    let leave = app.clone();
    let mut header: Vec<Widget> = vec![
        icon(icons::users(), 16.0, ACCENT),
        hgap(8.0),
        text("Room", 13.0, FontWeight::W500, MUTED).into(),
        hgap(8.0),
        text(room, 18.0, FontWeight::W700, ACCENT).into(),
    ];
    if protected {
        header.push(hgap(8.0));
        header.push(text("password", 10.5, FontWeight::W600, FAINT).into());
    }
    header.push(hgap(10.0));
    header.push(badge(role));
    header.push(Spacer::new().into());
    if role.controls() && link == Link::Connected {
        let sync = app.clone();
        header.push(chip("Sync everyone", false, move || sync.together_sync_everyone()));
        header.push(hgap(6.0));
    }
    header.push(chip("Leave", false, move || leave.together_leave()));

    let mut children: Vec<Widget> = vec![
        SizedBox::new()
            .width(width)
            .child(hrow(header).cross_axis_alignment(CrossAxisAlignment::Center))
            .into(),
        gap(6.0),
    ];
    if link == Link::Reconnecting {
        children.push(text("Reconnecting to the room…", 12.0, FontWeight::W600, ACCENT).into());
        children.push(gap(6.0));
    }
    children.push(
        text(
            match role {
                Role::Host => "You're the host: you control playback and can make others co-hosts.",
                Role::Cohost => "You're a co-host: you can control playback too.",
                Role::Viewer => "You're a viewer: playback follows the host.",
            },
            11.5,
            FontWeight::W400,
            FAINT,
        )
        .into(),
    );
    children.push(gap(12.0));
    children.push(text("People", 12.0, FontWeight::W600, FAINT).into());
    children.push(gap(6.0));
    children.push(members(app, width));
    children.push(gap(8.0));
    children.push(text("Chat", 12.0, FontWeight::W600, FAINT).into());
    children.push(gap(6.0));
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
                            .color(Color::rgba(12, 14, 22, 215))
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
                .color(Color::rgba(18, 21, 32, 245))
                .border_radius(BorderRadius::all(Radius::circular(16.0))),
        )
        .child(room_view(app, WIDTH - 28.0, 220.0))
        .into()
}
