//! Watch together: keeps several viewers' players in step through a small
//! relay server (`src/bin/watch-server.rs`).
//!
//! Only control messages travel through the server: which stream to play,
//! play/pause, seeks, speed, "I'm buffering", chat and room management.
//! Every viewer fetches the video itself, through the same torrent or URL.
//!
//! The room's creator is its host; the host can make others co-hosts.
//! Hosts and co-hosts control playback, viewers follow. Whoever acted last
//! leads: they send a heartbeat of their position every few seconds and the
//! others jump to it when they drift too far. A dropped connection is
//! retried with the reconnect token, which keeps the role.

use crate::addons::types::{Meta, Stream, Video};
use crate::player::{Menu, Status};
use crate::state::{App, Shared, ui};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

pub const DEFAULT_SERVER: &str = "127.0.0.1:7878";

/// Heartbeat interval of the leader.
const BEAT: Duration = Duration::from_secs(4);
/// Drift tolerated before a heartbeat moves a follower.
const BEAT_DRIFT: f64 = 2.0;
/// Drift tolerated when someone explicitly seeks / plays / pauses.
const ACTION_DRIFT: f64 = 0.75;
/// Reconnect attempts after a dropped connection, and the waits between.
const RETRY_WAITS: [u64; 8] = [1, 2, 3, 5, 8, 13, 20, 30];
/// How long a chat line floats over the video.
pub const BUBBLE_SECONDS: u64 = 8;
const CHAT_KEEP: usize = 200;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Link {
    #[default]
    Off,
    Connecting,
    /// The connection dropped; trying again.
    Reconnecting,
    Connected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    Host,
    Cohost,
    #[default]
    Viewer,
}

impl Role {
    fn parse(value: &Value) -> Self {
        match value.as_str() {
            Some("host") => Role::Host,
            Some("cohost") => Role::Cohost,
            _ => Role::Viewer,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Host => "Host",
            Role::Cohost => "Co-host",
            Role::Viewer => "Viewer",
        }
    }

    pub fn controls(self) -> bool {
        self != Role::Viewer
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub id: u64,
    pub name: String,
    pub role: Role,
}

#[derive(Clone, Debug)]
pub struct ChatLine {
    /// `None` for room notices (joins, role changes, ...).
    pub from: Option<String>,
    pub role: Role,
    pub text: String,
    /// Unix seconds.
    pub at: u64,
    /// This app's clock when it arrived (for the floating bubbles).
    pub received: Instant,
    pub mine: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TogetherState {
    pub link: Link,
    pub room: String,
    pub me: u64,
    pub role: Role,
    pub members: Vec<Member>,
    pub host_online: bool,
    /// The room has a password.
    pub protected: bool,
    pub chat: Vec<ChatLine>,
    /// Chat lines that arrived while the room panel was closed.
    pub unread: usize,
    /// Viewers whose player is waiting for data: (id, name).
    pub waiting: Vec<(u64, String)>,
}

#[derive(Clone, Copy)]
struct Pending {
    position: f64,
    paused: bool,
    speed: f64,
    received: Instant,
}

/// Where to reconnect to.
#[derive(Clone, Default)]
struct Ticket {
    room: String,
    password: String,
    token: String,
}

struct Inner {
    tx: RefCell<Option<mpsc::UnboundedSender<String>>>,
    /// Bumped per connection, so messages of an old one are ignored.
    connection: Cell<u64>,
    ticket: RefCell<Ticket>,
    retries: Cell<usize>,
    /// Set while applying a remote change, so it is not sent back.
    quiet: Cell<bool>,
    leader: Cell<bool>,
    last_beat: Cell<Instant>,
    was_stalled: Cell<bool>,
    /// This player was paused because someone else is buffering.
    auto_paused: Cell<bool>,
    /// Room state to apply once the player has opened the stream.
    pending: Cell<Option<Pending>>,
}

#[derive(Clone)]
pub struct Together {
    pub state: Shared<TogetherState>,
    inner: Rc<Inner>,
}

impl Together {
    pub fn new() -> Self {
        Self {
            state: Shared::new(TogetherState::default()),
            inner: Rc::new(Inner {
                tx: RefCell::new(None),
                connection: Cell::new(0),
                ticket: RefCell::new(Ticket::default()),
                retries: Cell::new(0),
                quiet: Cell::new(false),
                leader: Cell::new(false),
                last_beat: Cell::new(Instant::now()),
                was_stalled: Cell::new(false),
                auto_paused: Cell::new(false),
                pending: Cell::new(None),
            }),
        }
    }

    pub fn connected(&self) -> bool {
        self.state.peek(|s| s.link == Link::Connected)
    }

    fn send(&self, message: Value) {
        if let Some(tx) = self.inner.tx.borrow().as_ref() {
            let _ = tx.send(message.to_string());
        }
    }

    /// Runs `apply` without echoing the changes it makes to the room.
    fn quietly(&self, apply: impl FnOnce()) {
        let before = self.inner.quiet.replace(true);
        apply();
        self.inner.quiet.set(before);
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Media {
    stream: Stream,
    #[serde(default)]
    meta: Option<Meta>,
    #[serde(default)]
    video: Option<Video>,
}

impl App {
    // --------------------------------------------------------- connection

    /// Creates a room (empty `code`; `password` then protects it) or joins
    /// one.
    pub fn together_join(&self, code: &str, password: &str) {
        self.together_leave();
        *self.together.inner.ticket.borrow_mut() = Ticket {
            room: code.trim().to_ascii_uppercase(),
            password: password.to_owned(),
            token: String::new(),
        };
        self.together.inner.retries.set(0);
        self.together.state.write(|s| {
            *s = TogetherState { link: Link::Connecting, ..TogetherState::default() };
        });
        self.together_connect();
    }

    fn together_connect(&self) {
        let (server, name) = self.profile.peek(|p| {
            (p.settings.watch_server.trim().to_owned(), p.settings.watch_name.trim().to_owned())
        });
        let server = if server.is_empty() { DEFAULT_SERVER.to_owned() } else { server };
        let Some(handle) = crate::state::tokio() else { return };
        let together = &self.together;
        let connection = together.inner.connection.get() + 1;
        together.inner.connection.set(connection);
        let ticket = together.inner.ticket.borrow().clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let _ = tx.send(
            json!({
                "t": "hello",
                "name": name,
                "room": ticket.room,
                "password": ticket.password,
                "token": ticket.token,
            })
            .to_string(),
        );
        *together.inner.tx.borrow_mut() = Some(tx);

        handle.spawn(async move {
            let connect = tokio::time::timeout(Duration::from_secs(8), TcpStream::connect(server.as_str()));
            let stream = match connect.await {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    let message = format!("Could not reach the watch-together server at {server}: {error}");
                    ui(move |app| app.together_dropped(connection, message));
                    return;
                }
                Err(_) => {
                    let message = format!("The watch-together server at {server} did not answer");
                    ui(move |app| app.together_dropped(connection, message));
                    return;
                }
            };
            let _ = stream.set_nodelay(true);
            let (read, mut write) = stream.into_split();
            let writer = tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    if write.write_all(line.as_bytes()).await.is_err()
                        || write.write_all(b"\n").await.is_err()
                    {
                        break;
                    }
                }
            });
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(message) = serde_json::from_str::<Value>(&line) {
                    ui(move |app| app.together_message(connection, message));
                }
            }
            writer.abort();
            ui(move |app| {
                app.together_dropped(connection, "Lost the connection to the watch-together room".into())
            });
        });
    }

    /// Leaves the room (or stops trying to join).
    pub fn together_leave(&self) {
        let together = &self.together;
        together.inner.connection.set(together.inner.connection.get() + 1);
        // Dropping the sender ends the writer, which closes the socket.
        together.inner.tx.borrow_mut().take();
        *together.inner.ticket.borrow_mut() = Ticket::default();
        together.inner.pending.set(None);
        together.inner.leader.set(false);
        if together.inner.auto_paused.replace(false) {
            self.together_resume_after_wait();
        }
        if together.state.peek(|s| s.link != Link::Off) {
            together.state.write(|s| *s = TogetherState::default());
        }
        if self.player.state.peek(|s| s.menu == Menu::Together) {
            self.player.state.write(|s| s.menu = Menu::None);
        }
    }

    /// The connection ended without us leaving: retry while there is a
    /// room to go back to.
    fn together_dropped(&self, connection: u64, message: String) {
        let together = &self.together;
        if together.inner.connection.get() != connection {
            return;
        }
        together.inner.tx.borrow_mut().take();
        let can_retry = !together.inner.ticket.borrow().token.is_empty();
        let attempt = together.inner.retries.get();
        if !can_retry || attempt >= RETRY_WAITS.len() {
            self.together_leave();
            self.notify(message);
            return;
        }
        together.inner.retries.set(attempt + 1);
        if attempt == 0 {
            self.notify("Connection to the room dropped. Reconnecting…");
        }
        together.state.write(|s| {
            s.link = Link::Reconnecting;
            s.waiting.clear();
        });
        crate::state::spawn(
            async move { tokio::time::sleep(Duration::from_secs(RETRY_WAITS[attempt])).await },
            move |app, ()| {
                let together = &app.together;
                if together.inner.connection.get() == connection
                    && together.state.peek(|s| s.link == Link::Reconnecting)
                {
                    app.together_connect();
                }
            },
        );
    }

    fn together_message(&self, connection: u64, message: Value) {
        let together = self.together.clone();
        if together.inner.connection.get() != connection {
            return;
        }
        let from = message["from"].as_str().unwrap_or("Someone").to_owned();
        match message["t"].as_str().unwrap_or_default() {
            "welcome" => self.together_welcome(&message),
            "error" => {
                let text = message["message"].as_str().unwrap_or("The room refused the request").to_owned();
                if together.connected() {
                    // A refused action; still in the room.
                    self.notify(text);
                } else {
                    self.together_leave();
                    self.notify(text);
                }
            }
            "kicked" => {
                self.together_leave();
                self.notify("You were removed from the room");
            }
            // This app rejoined on a newer connection; that one carries on.
            "replaced" => {}
            "members" => self.together_members(&message),
            "chat" | "notice" => {
                let line = chat_line(&message, together.state.peek(|s| s.me));
                let open = self.player.state.peek(|s| s.menu == Menu::Together);
                let counts = line.from.is_some() && !line.mine && !open;
                together.state.write(|s| {
                    s.chat.push(line);
                    if s.chat.len() > CHAT_KEEP {
                        s.chat.remove(0);
                    }
                    if counts {
                        s.unread += 1;
                    }
                });
                // Hide the floating bubble again.
                crate::state::spawn(
                    async { tokio::time::sleep(Duration::from_secs(BUBBLE_SECONDS) + Duration::from_millis(100)).await },
                    |app, ()| app.together.state.write(|_| ()),
                );
            }
            "media" => {
                together.inner.leader.set(false);
                self.together_play_media(&message["media"]);
            }
            "state" => {
                let heartbeat = message["hb"].as_bool().unwrap_or(false);
                if !heartbeat {
                    together.inner.leader.set(false);
                }
                if let Some(state) = parse_state(&message) {
                    let was_paused = self.player.state.peek(|s| s.paused);
                    self.together_apply_state(state, heartbeat);
                    let by_person = message["id"].as_u64().unwrap_or(0) != 0;
                    if !heartbeat && by_person && was_paused != state.paused {
                        self.notify(format!("{from} {}", if state.paused { "paused" } else { "resumed" }));
                    }
                }
            }
            "buffering" => {
                let id = message["id"].as_u64().unwrap_or(0);
                let on = message["on"].as_bool().unwrap_or(false);
                together.state.write(|s| {
                    s.waiting.retain(|(other, _)| *other != id);
                    if on {
                        s.waiting.push((id, from.clone()));
                    }
                });
                let anyone = together.state.peek(|s| !s.waiting.is_empty());
                let (paused, playing) = self
                    .player
                    .state
                    .peek(|s| (s.paused, s.status == Status::Playing));
                if anyone && playing && !paused {
                    together.inner.auto_paused.set(true);
                    together.quietly(|| self.toggle_pause());
                } else if !anyone && together.inner.auto_paused.replace(false) {
                    self.together_resume_after_wait();
                }
            }
            _ => {}
        }
    }

    fn together_welcome(&self, message: &Value) {
        let together = self.together.clone();
        let rejoined = together.state.peek(|s| s.link == Link::Reconnecting);
        let room = message["room"].as_str().unwrap_or_default().to_owned();
        let role = Role::parse(&message["role"]);
        let me = message["you"].as_u64().unwrap_or(0);
        {
            let mut ticket = together.inner.ticket.borrow_mut();
            ticket.room = room.clone();
            ticket.token = message["token"].as_str().unwrap_or_default().to_owned();
        }
        together.inner.retries.set(0);
        let chat: Vec<ChatLine> = message["chat"]
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| {
                        let mut line = chat_line(line, me);
                        // History, not news: no floating bubble.
                        line.received = Instant::now()
                            .checked_sub(Duration::from_secs(3600))
                            .unwrap_or(line.received);
                        line
                    })
                    .collect()
            })
            .unwrap_or_default();
        together.state.write(|s| {
            s.link = Link::Connected;
            s.room = room.clone();
            s.me = me;
            s.role = role;
            s.members = members(&message["members"]);
            s.host_online = message["host_online"].as_bool().unwrap_or(false);
            s.protected = message["protected"].as_bool().unwrap_or(false);
            s.chat = chat;
            s.waiting.clear();
        });

        let media = &message["media"];
        if media.is_null() {
            // A new room, or nobody has picked anything yet: share what is
            // playing here.
            together.inner.leader.set(role.controls());
            if role.controls() && self.player.state.peek(|s| s.status == Status::Playing) {
                self.together_share_current();
            }
            if !rejoined {
                self.notify(format!("Room {room} is open. Share the code to watch together."));
            }
            return;
        }
        together.inner.leader.set(false);
        if rejoined {
            self.notify("Reconnected to the room");
            // Same video still open: only catch up.
            let same = self.player.current_media().is_some_and(|(stream, _, _)| {
                serde_json::to_value(&stream).ok().as_ref() == Some(&media["stream"])
            });
            if same {
                if let Some(state) = parse_state(&message["state"]) {
                    self.together_apply_state(state, false);
                }
                return;
            }
        } else {
            self.notify(format!("Joined room {room} as {}", role.label().to_lowercase()));
        }
        self.together_play_media(media);
        if let Some(state) = parse_state(&message["state"]) {
            self.together_apply_state(state, false);
        }
    }

    fn together_members(&self, message: &Value) {
        let together = &self.together;
        let list = members(&message["members"]);
        let host_online = message["host_online"].as_bool().unwrap_or(false);
        let (before, me) = together.state.peek(|s| (s.role, s.me));
        let role = list.iter().find(|m| m.id == me).map_or(before, |m| m.role);
        together.state.write(|s| {
            s.members = list.clone();
            s.host_online = host_online;
            s.role = role;
            // Someone who left is no longer holding anyone up.
            s.waiting.retain(|(id, _)| list.iter().any(|m| m.id == *id));
        });
        if role != before {
            self.notify(match role {
                Role::Host => "You are now the host",
                Role::Cohost => "You are now a co-host: you can control playback",
                Role::Viewer => "You are now a viewer",
            });
        }
        if !role.controls() {
            together.inner.leader.set(false);
        } else if !host_online {
            // The host is away: the first co-host keeps everyone in step.
            let first = list.iter().find(|m| m.role == Role::Cohost).map(|m| m.id);
            if first == Some(me) {
                together.inner.leader.set(true);
            }
        }
        if together.state.peek(|s| s.waiting.is_empty()) && together.inner.auto_paused.replace(false) {
            self.together_resume_after_wait();
        }
    }

    fn together_resume_after_wait(&self) {
        if self.player.state.peek(|s| s.paused && s.status == Status::Playing) {
            self.together.quietly(|| self.toggle_pause());
        }
    }

    fn together_play_media(&self, media: &Value) {
        let Ok(media) = serde_json::from_value::<Media>(media.clone()) else {
            self.notify("Could not open the room's video");
            return;
        };
        self.together.inner.pending.set(None);
        self.together.quietly(|| self.play_with(&media.stream, media.meta, media.video));
    }

    /// Applies the room's playback state now, or once the stream is open.
    fn together_apply_state(&self, state: Pending, heartbeat: bool) {
        let ready = self.player.state.peek(|s| s.status == Status::Playing);
        if !ready {
            self.together.inner.pending.set(Some(state));
            return;
        }
        let (paused, speed) = self.player.state.peek(|s| (s.paused, s.speed));
        let target = if state.paused {
            state.position
        } else {
            state.position + state.received.elapsed().as_secs_f64() * state.speed
        };
        let local = self.player.position.get_untracked();
        let tolerance = if heartbeat { BEAT_DRIFT } else { ACTION_DRIFT };
        self.together.quietly(|| {
            if (state.speed - speed).abs() > 0.01 {
                self.set_speed(state.speed);
            }
            if (target - local).abs() > tolerance {
                self.seek_to(target);
            }
            // Someone buffering keeps this player paused; it resumes with them.
            let held = self.together.inner.auto_paused.get();
            if state.paused != paused && !(held && !state.paused) {
                self.toggle_pause();
            }
        });
    }

    // ------------------------------------------------------ room actions

    pub fn together_chat(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() || !self.together.connected() {
            return;
        }
        self.together.send(json!({ "t": "chat", "text": text }));
    }

    pub fn together_set_role(&self, id: u64, role: Role) {
        let role = match role {
            Role::Host => "host",
            Role::Cohost => "cohost",
            Role::Viewer => "viewer",
        };
        self.together.send(json!({ "t": "role", "id": id, "role": role }));
    }

    pub fn together_kick(&self, id: u64) {
        self.together.send(json!({ "t": "kick", "id": id }));
    }

    /// Host / co-host: puts everyone at this player's position again.
    pub fn together_sync_everyone(&self) {
        if self.together.state.peek(|s| s.role.controls()) {
            self.together.inner.leader.set(true);
            self.together_send_state(false);
            self.notify("Everyone was moved to your position");
        }
    }

    pub fn together_mark_read(&self) {
        if self.together.state.peek(|s| s.unread > 0) {
            self.together.state.write(|s| s.unread = 0);
        }
    }

    // ------------------------------------------------- hooks from the player

    /// Viewers in a room follow the host: their own play / pause / seek /
    /// speed / stream changes are refused. True when refused.
    pub(crate) fn together_blocks_control(&self) -> bool {
        let together = &self.together;
        if together.inner.quiet.get() {
            return false;
        }
        let blocked = together.state.peek(|s| s.link == Link::Connected && !s.role.controls());
        if blocked {
            self.notify("Only the host and co-hosts can control playback in this room");
        }
        blocked
    }

    /// A stream was picked here.
    pub(crate) fn together_local_media(&self, stream: &Stream, meta: Option<&Meta>, video: Option<&Video>) {
        let together = &self.together;
        if !together.connected() || together.inner.quiet.get() {
            return;
        }
        together.inner.leader.set(true);
        together.inner.pending.set(None);
        together.send(json!({
            "t": "media",
            "media": Media { stream: stream.clone(), meta: meta.cloned(), video: video.cloned() },
        }));
    }

    fn together_share_current(&self) {
        if let Some((stream, meta, video)) = self.player.current_media() {
            self.together_local_media(&stream, meta.as_ref(), video.as_ref());
            self.together_send_state(false);
        }
    }

    /// Play, pause, seek or speed changed here.
    pub(crate) fn together_local_change(&self) {
        let together = &self.together;
        if !together.connected() || together.inner.quiet.get() {
            return;
        }
        if !together.state.peek(|s| s.role.controls()) {
            return;
        }
        together.inner.leader.set(true);
        // A deliberate play overrides waiting for others.
        together.inner.auto_paused.set(false);
        self.together_send_state(false);
    }

    fn together_send_state(&self, heartbeat: bool) {
        let (paused, speed) = self.player.state.peek(|s| (s.paused, s.speed));
        let position = self.player.position.get_untracked();
        self.together.inner.last_beat.set(Instant::now());
        self.together.send(json!({
            "t": "state",
            "position": position,
            "paused": paused,
            "speed": speed,
            "hb": heartbeat,
        }));
    }

    /// The player opened the stream and knows its length.
    pub(crate) fn together_player_ready(&self) {
        let together = self.together.clone();
        if !together.connected() {
            return;
        }
        if let Some(pending) = together.inner.pending.take() {
            self.together_apply_state(pending, false);
        } else if together.inner.leader.get() {
            self.together_send_state(false);
        }
    }

    /// Every player clock tick.
    pub(crate) fn together_tick(&self, stalled: bool) {
        let together = &self.together;
        if !together.connected() {
            return;
        }
        if together.inner.was_stalled.replace(stalled) != stalled {
            together.send(json!({ "t": "buffering", "on": stalled }));
        }
        let (paused, playing) = self
            .player
            .state
            .peek(|s| (s.paused, s.status == Status::Playing));
        if together.inner.leader.get()
            && together.state.peek(|s| s.role.controls())
            && playing
            && !paused
            && !stalled
            && together.inner.last_beat.get().elapsed() >= BEAT
        {
            self.together_send_state(true);
        }
    }

    /// Names of the viewers holding playback up, for the buffering card.
    pub fn together_waiting_for(&self) -> Vec<String> {
        self.together.state.peek(|s| s.waiting.iter().map(|(_, n)| n.clone()).collect())
    }
}

fn members(value: &Value) -> Vec<Member> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .map(|m| Member {
                    id: m["id"].as_u64().unwrap_or(0),
                    name: m["name"].as_str().unwrap_or("Guest").to_owned(),
                    role: Role::parse(&m["role"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn chat_line(value: &Value, me: u64) -> ChatLine {
    let notice = value["t"].as_str() == Some("notice");
    ChatLine {
        from: (!notice).then(|| value["from"].as_str().unwrap_or("Guest").to_owned()),
        role: Role::parse(&value["role"]),
        text: value["text"].as_str().unwrap_or_default().to_owned(),
        at: value["at"].as_u64().unwrap_or(0),
        received: Instant::now(),
        mine: !notice && value["id"].as_u64() == Some(me),
    }
}

fn parse_state(value: &Value) -> Option<Pending> {
    Some(Pending {
        position: value["position"].as_f64()?,
        paused: value["paused"].as_bool().unwrap_or(true),
        speed: value["speed"].as_f64().unwrap_or(1.0),
        received: Instant::now(),
    })
}
