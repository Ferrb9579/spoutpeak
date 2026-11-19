//! Watch together: keeps several viewers' players in step through a small
//! relay server (`watch-server/`).
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
//!
//! The connection is a WebSocket to the server baked in at build time
//! (`SPOUTPEAK_WATCH_URL` in `.env`); `wss://` gets normal HTTPS
//! certificate checks.

use crate::addons::types::{Meta, Stream, Video};
use crate::player::{Menu, Status};
use crate::state::{App, Shared, ui};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// The watch-together server, from `.env` at build time.
pub const SERVER_URL: &str = match option_env!("SPOUTPEAK_WATCH_URL") {
    Some(url) => url,
    None => "ws://127.0.0.1:7878/ws",
};

/// Heartbeat interval of the leader (and of the latency probe).
const BEAT: Duration = Duration::from_secs(1);
/// Drift left alone: well under what anyone notices.
const SYNC_DRIFT: f64 = 0.04;
/// Up to this much drift is slewed away (playing a few percent fast or
/// slow); more than this is a seek.
const SLEW_LIMIT: f64 = 1.5;
/// A paused player is moved when it is further off than this.
const PAUSED_DRIFT: f64 = 0.1;
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
    /// In the room's voice chat.
    pub in_voice: bool,
    /// Their own mute / deafen.
    pub muted: bool,
    pub deafened: bool,
    /// Imposed by the host or a co-host.
    pub mod_muted: bool,
    pub mod_deafened: bool,
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
    /// The line's id in the room (for editing and deleting); 0 when unknown.
    pub mid: u64,
    pub edited: bool,
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
    /// The password this app joined or created the room with (empty when
    /// it has none), to show and copy.
    pub password: String,
    pub chat: Vec<ChatLine>,
    /// Chat lines that arrived while the room panel was closed.
    pub unread: usize,
    /// Viewers whose player is waiting for data: (id, name).
    pub waiting: Vec<(u64, String)>,
    /// What the room is watching, for the room page.
    pub now_playing: Option<NowPlaying>,
    /// Where the room's playback was at a moment, to work out where it is
    /// now without the player open.
    pub clock: Option<RoomClock>,
    pub voice: VoiceState,
}

/// This app's place in the room's voice chat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceState {
    pub joined: bool,
    pub muted: bool,
    pub deafened: bool,
    /// Room ids of the people talking now.
    pub speaking: Vec<u64>,
    /// Our own microphone is sending speech.
    pub talking: bool,
    /// Why the microphone or speakers don't work.
    pub problem: Option<String>,
}

/// What goes out on the room connection.
enum Out {
    Text(String),
    /// A voice frame.
    Binary(Vec<u8>),
}

type NetSlot = std::sync::Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<Out>>>>;
type VoiceSlot = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::voice::Voice>>>>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub episode: String,
    pub poster: Option<String>,
    /// Year, runtime and the like.
    pub facts: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RoomClock {
    pub position: f64,
    pub paused: bool,
    pub speed: f64,
    /// Length of the video, when someone in the room knows it.
    pub duration: Option<f64>,
    pub at: Instant,
}

impl RoomClock {
    /// Seconds into the video now.
    pub fn now(&self) -> f64 {
        let position = if self.paused {
            self.position
        } else {
            self.position + self.at.elapsed().as_secs_f64() * self.speed
        };
        match self.duration {
            Some(duration) if duration > 0.0 => position.min(duration),
            _ => position,
        }
    }
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
    /// The room's current video, to open it again from the room page.
    media: RefCell<Option<Value>>,
    /// The current connection's outgoing queue; shared with the voice
    /// thread, which sends its frames through it.
    tx: NetSlot,
    /// The running voice session, shared with the network thread that
    /// hands it incoming frames.
    voice: VoiceSlot,
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
    /// When the latency probe in flight was sent.
    ping_sent: Cell<Option<Instant>>,
    last_probe: Cell<Instant>,
    /// Smoothed one-way delay to the server, in seconds.
    half_rtt: Cell<f64>,
    /// Chat this app cleared for itself: lines sent before this unix time
    /// stay hidden, also when a reconnect brings the history back.
    chat_cleared_at: Cell<u64>,
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
                media: RefCell::new(None),
                tx: NetSlot::default(),
                voice: VoiceSlot::default(),
                connection: Cell::new(0),
                ticket: RefCell::new(Ticket::default()),
                retries: Cell::new(0),
                quiet: Cell::new(false),
                leader: Cell::new(false),
                last_beat: Cell::new(Instant::now()),
                was_stalled: Cell::new(false),
                auto_paused: Cell::new(false),
                pending: Cell::new(None),
                ping_sent: Cell::new(None),
                last_probe: Cell::new(Instant::now()),
                half_rtt: Cell::new(0.05),
                chat_cleared_at: Cell::new(0),
            }),
        }
    }

    pub fn connected(&self) -> bool {
        self.state.peek(|s| s.link == Link::Connected)
    }

    fn send(&self, message: Value) {
        if let Some(tx) = self.inner.tx.lock().unwrap().as_ref() {
            let _ = tx.send(Out::Text(message.to_string()));
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
        let (name, auth) = self.profile.peek(|p| {
            let account = p.account.as_ref();
            (
                account.map_or_else(|| p.settings.watch_name.trim().to_owned(), |a| a.name.clone()),
                account.map(|a| a.token.clone()).unwrap_or_default(),
            )
        });
        let Some(handle) = crate::state::tokio() else { return };
        let together = &self.together;
        let connection = together.inner.connection.get() + 1;
        together.inner.connection.set(connection);
        let ticket = together.inner.ticket.borrow().clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
        let _ = tx.send(Out::Text(
            json!({
                "t": "hello",
                "name": name,
                "room": ticket.room,
                "password": ticket.password,
                "token": ticket.token,
                // The server names us after the account it belongs to.
                "auth": auth,
            })
            .to_string(),
        ));
        *together.inner.tx.lock().unwrap() = Some(tx);
        let voice = together.inner.voice.clone();

        handle.spawn(async move {
            let socket = match tokio::time::timeout(Duration::from_secs(45), connect(SERVER_URL)).await {
                Ok(Ok(socket)) => socket,
                Ok(Err(error)) => {
                    let message = format!("Could not reach the watch-together server: {error}");
                    ui(move |app| app.together_dropped(connection, message));
                    return;
                }
                Err(_) => {
                    let message = "The watch-together server did not answer".to_owned();
                    ui(move |app| app.together_dropped(connection, message));
                    return;
                }
            };
            let (mut sink, mut stream) = socket.split();
            let writer = tokio::spawn(async move {
                while let Some(out) = rx.recv().await {
                    let message = match out {
                        Out::Text(line) => Message::Text(line.into()),
                        Out::Binary(frame) => Message::Binary(frame.into()),
                    };
                    if sink.send(message).await.is_err() {
                        break;
                    }
                }
                let _ = sink.close().await;
            });
            while let Some(Ok(frame)) = stream.next().await {
                match frame {
                    Message::Text(text) => {
                        if let Ok(message) = serde_json::from_str::<Value>(text.as_str()) {
                            ui(move |app| app.together_message(connection, message));
                        }
                    }
                    // Voice: decoded right here, off the UI thread.
                    Message::Binary(frame) => {
                        let session = voice.lock().unwrap().clone();
                        if let Some(session) = session {
                            session.receive(&frame);
                        }
                    }
                    Message::Close(_) => break,
                    // Pings are answered by the library.
                    _ => {}
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
        together.inner.tx.lock().unwrap().take();
        self.together_stop_voice();
        *together.inner.ticket.borrow_mut() = Ticket::default();
        together.inner.media.borrow_mut().take();
        together.inner.pending.set(None);
        together.inner.leader.set(false);
        together.inner.chat_cleared_at.set(0);
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
        together.inner.tx.lock().unwrap().take();
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
            "pong" => {
                if let Some(sent) = together.inner.ping_sent.take() {
                    let half = sent.elapsed().as_secs_f64() / 2.0;
                    let old = together.inner.half_rtt.get();
                    // Spikes come and go: follow drops at once, rises slowly.
                    together.inner.half_rtt.set(if half < old { half } else { old * 0.8 + half * 0.2 });
                }
            }
            "members" => self.together_members(&message),
            "chat_edited" => {
                let mid = message["mid"].as_u64().unwrap_or(0);
                let text = message["text"].as_str().unwrap_or_default().to_owned();
                together.state.write(|s| {
                    if let Some(line) = s.chat.iter_mut().find(|l| l.mid == mid) {
                        line.text = text;
                        line.edited = true;
                    }
                });
            }
            "chat_deleted" => {
                let mid = message["mid"].as_u64().unwrap_or(0);
                if together.state.peek(|s| s.chat.iter().any(|l| l.mid == mid)) {
                    together.state.write(|s| s.chat.retain(|l| l.mid != mid));
                }
            }
            "chat_cleared" => {
                let by_me = message["id"].as_u64() == Some(together.state.peek(|s| s.me));
                together.state.write(|s| {
                    s.chat.clear();
                    s.unread = 0;
                });
                if !by_me {
                    self.notify(format!("{from} cleared the chat"));
                }
            }
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
                together.state.write(|s| s.clock = None);
                self.together_play_media(&message["media"]);
            }
            "state" => {
                let heartbeat = message["hb"].as_bool().unwrap_or(false);
                if !heartbeat {
                    together.inner.leader.set(false);
                }
                if let Some(mut state) = parse_state(&message) {
                    self.together_set_clock(&message, state);
                    // It was sent this long ago: the sender's trip to the
                    // server, plus ours from it.
                    let delay = message["lat"].as_f64().unwrap_or(0.0).clamp(0.0, 2.0)
                        + together.inner.half_rtt.get();
                    state.received =
                        Instant::now().checked_sub(Duration::from_secs_f64(delay)).unwrap_or(state.received);
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
        let protected = message["protected"].as_bool().unwrap_or(false);
        let password = {
            let mut ticket = together.inner.ticket.borrow_mut();
            ticket.room = room.clone();
            ticket.token = message["token"].as_str().unwrap_or_default().to_owned();
            if protected { ticket.password.clone() } else { String::new() }
        };
        let cleared_at = together.inner.chat_cleared_at.get();
        together.inner.retries.set(0);
        let chat: Vec<ChatLine> = message["chat"]
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| chat_line(line, me))
                    .filter(|line| line.at >= cleared_at)
                    .map(|mut line| {
                        // History, not news: no floating bubble.
                        line.received = Instant::now()
                            .checked_sub(Duration::from_secs(3600))
                            .unwrap_or(line.received);
                        line
                    })
                    .collect()
            })
            .unwrap_or_default();
        let voice = together.state.peek(|s| s.voice.clone());
        together.state.write(|s| {
            s.link = Link::Connected;
            s.room = room.clone();
            s.me = me;
            s.role = role;
            s.members = members(&message["members"]);
            s.host_online = message["host_online"].as_bool().unwrap_or(false);
            s.protected = protected;
            s.password = password;
            s.chat = chat;
            s.waiting.clear();
        });

        if voice.joined {
            // Back in the room after a dropped connection: back in voice too.
            together.send(json!({ "t": "voice_join" }));
            together.send(json!({ "t": "voice_state", "muted": voice.muted, "deafened": voice.deafened }));
        }

        let media = &message["media"];
        self.together_remember_media(media);
        let clock = parse_state(&message["state"]);
        together.state.write(|s| s.clock = None);
        if let Some(state) = clock {
            self.together_set_clock(&message["state"], state);
        }
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
        let mine = list.iter().find(|m| m.id == me).cloned();
        if let Some(mine) = mine {
            let session = together.inner.voice.lock().unwrap().clone();
            if let Some(session) = session {
                let (muted, deafened) = together.state.peek(|s| (s.voice.muted, s.voice.deafened));
                session.set_muted(muted || mine.mod_muted);
                session.set_deafened(deafened || mine.mod_deafened);
            }
        }
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
        self.together_remember_media(media);
        let Ok(media) = serde_json::from_value::<Media>(media.clone()) else {
            self.notify("Could not open the room's video");
            return;
        };
        self.together.inner.pending.set(None);
        self.together.quietly(|| self.play_with(&media.stream, media.meta, media.video));
    }

    fn together_remember_media(&self, media: &Value) {
        *self.together.inner.media.borrow_mut() = (!media.is_null()).then(|| media.clone());
        let now = serde_json::from_value::<Media>(media.clone()).ok().map(|media| {
            let title = media
                .meta
                .as_ref()
                .and_then(|m| m.name.clone())
                .or_else(|| media.stream.title.clone())
                .or_else(|| media.stream.name.clone())
                .unwrap_or_else(|| "A video".to_owned());
            let episode = media.video.as_ref().map(|v| v.label()).unwrap_or_default();
            let poster = media
                .meta
                .as_ref()
                .and_then(|m| m.poster.clone())
                .or_else(|| media.video.as_ref().and_then(|v| v.thumbnail.clone()));
            let mut facts: Vec<String> = Vec::new();
            if let Some(meta) = &media.meta {
                facts.extend(meta.release_line());
                facts.extend(meta.runtime.clone());
            }
            NowPlaying { title, episode, poster, facts: facts.join(" · ") }
        });
        self.together.state.write(|s| s.now_playing = now);
    }

    /// Opens the room's current video again (after leaving the player).
    pub fn together_watch(&self) {
        let media = self.together.inner.media.borrow().clone();
        match media {
            Some(media) => self.together_play_media(&media),
            None => self.notify("Nothing is playing in the room yet"),
        }
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
        let local = self.player.precise_position().unwrap_or_else(|| self.player.position.get_untracked());
        let drift = target - local;
        let _ = heartbeat;
        self.together.quietly(|| {
            if (state.speed - speed).abs() > 0.01 {
                self.set_speed(state.speed);
            }
            if state.paused {
                if drift.abs() > PAUSED_DRIFT {
                    self.seek_to(target);
                }
            } else if drift.abs() > SLEW_LIMIT {
                self.seek_to(target);
            } else if drift.abs() > SYNC_DRIFT {
                self.player.nudge(drift);
            } else {
                // Close enough: stop any correction still running.
                self.player.nudge(0.0);
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

    /// Changes the text of one of our own chat lines.
    pub fn together_edit_chat(&self, mid: u64, text: &str) {
        let text = text.trim();
        if mid != 0 && !text.is_empty() {
            self.together.send(json!({ "t": "chat_edit", "mid": mid, "text": text }));
        }
    }

    /// Deletes a chat line (our own; anyone's for the host and co-hosts).
    pub fn together_delete_chat(&self, mid: u64) {
        if mid != 0 {
            self.together.send(json!({ "t": "chat_delete", "mid": mid }));
        }
    }

    // ------------------------------------------------------------ voice

    pub fn together_voice_join(&self) {
        let together = &self.together;
        if !together.connected() || together.state.peek(|s| s.voice.joined) {
            return;
        }
        let net = together.inner.tx.clone();
        let outbox: crate::voice::Outbox = Box::new(move |frame| {
            if let Some(tx) = net.lock().unwrap().as_ref() {
                let _ = tx.send(Out::Binary(frame));
            }
        });
        let (muted, deafened) = together
            .state
            .peek(|s| s.members.iter().find(|m| m.id == s.me).map(|m| (m.mod_muted, m.mod_deafened)))
            .unwrap_or_default();
        let settings = self.profile.peek(|p| p.settings.voice);
        let session = std::sync::Arc::new(crate::voice::Voice::start(outbox, muted, deafened, settings));
        *together.inner.voice.lock().unwrap() = Some(session);
        together.state.write(|s| {
            s.voice = VoiceState { joined: true, ..VoiceState::default() };
        });
        together.send(json!({ "t": "voice_join" }));
        self.together_voice_tick();
    }

    /// Voice preferences changed: the running session picks them up.
    pub fn together_voice_settings(&self, settings: crate::voice_processing::VoiceSettings) {
        self.update_settings(|s| s.voice = settings);
        if let Some(session) = self.together.inner.voice.lock().unwrap().as_ref() {
            session.set_settings(settings);
        }
    }

    pub fn together_voice_leave(&self) {
        if self.together.state.peek(|s| s.voice.joined) {
            self.together.send(json!({ "t": "voice_leave" }));
        }
        self.together_stop_voice();
    }

    fn together_stop_voice(&self) {
        if let Some(session) = self.together.inner.voice.lock().unwrap().take() {
            session.stop();
        }
        if self.together.state.peek(|s| s.voice != VoiceState::default()) {
            self.together.state.write(|s| s.voice = VoiceState::default());
        }
    }

    /// Our own moderation, as the members list says.
    fn together_moderation(&self) -> (bool, bool) {
        self.together
            .state
            .peek(|s| s.members.iter().find(|m| m.id == s.me).map(|m| (m.mod_muted, m.mod_deafened)))
            .unwrap_or_default()
    }

    pub fn together_toggle_mute(&self) {
        let (mod_muted, mod_deafened) = self.together_moderation();
        let (muted, deafened) = self.together.state.peek(|s| (s.voice.muted, s.voice.deafened));
        if muted && (mod_muted || mod_deafened) {
            self.notify("The host muted you for this room");
            return;
        }
        // Unmuting also undeafens: you can't talk without hearing.
        let (muted, deafened) = if muted { (false, false) } else { (true, deafened) };
        self.together_set_voice(muted, deafened);
    }

    pub fn together_toggle_deafen(&self) {
        let (_, mod_deafened) = self.together_moderation();
        let deafened = self.together.state.peek(|s| s.voice.deafened);
        if deafened && mod_deafened {
            self.notify("The host deafened you for this room");
            return;
        }
        // Deafening also mutes, as in most voice apps.
        if deafened { self.together_set_voice(false, false) } else { self.together_set_voice(true, true) }
    }

    fn together_set_voice(&self, muted: bool, deafened: bool) {
        let together = &self.together;
        let (mod_muted, mod_deafened) = self.together_moderation();
        if let Some(session) = together.inner.voice.lock().unwrap().as_ref() {
            session.set_muted(muted || mod_muted);
            session.set_deafened(deafened || mod_deafened);
        }
        together.state.write(|s| {
            s.voice.muted = muted;
            s.voice.deafened = deafened;
        });
        together.send(json!({ "t": "voice_state", "muted": muted, "deafened": deafened }));
    }

    /// Host and co-hosts: mutes or deafens someone (or lifts it).
    pub fn together_moderate(&self, id: u64, muted: Option<bool>, deafened: Option<bool>) {
        let mut message = json!({ "t": "voice_mod", "id": id });
        if let Some(muted) = muted {
            message["muted"] = json!(muted);
        }
        if let Some(deafened) = deafened {
            message["deafened"] = json!(deafened);
        }
        self.together.send(message);
    }

    /// While in voice: who is talking, five times a second.
    fn together_voice_tick(&self) {
        let session = self.together.inner.voice.lock().unwrap().clone();
        let Some(session) = session else { return };
        let mut speaking = session.speaking();
        speaking.sort_unstable();
        let (talking, problem) = (session.talking(), session.problem());
        let changed = self.together.state.peek(|s| {
            s.voice.speaking != speaking || s.voice.talking != talking || s.voice.problem != problem
        });
        if changed {
            self.together.state.write(|s| {
                s.voice.speaking = speaking;
                s.voice.talking = talking;
                s.voice.problem = problem;
            });
        }
        crate::state::spawn(async { tokio::time::sleep(Duration::from_millis(200)).await }, |app, ()| {
            app.together_voice_tick()
        });
    }

    /// Empties this app's copy of the chat; others keep theirs.
    pub fn together_clear_chat_here(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.together.inner.chat_cleared_at.set(now);
        self.together.state.write(|s| {
            s.chat.clear();
            s.unread = 0;
        });
    }

    /// Host and co-hosts: empties the chat for everyone in the room.
    pub fn together_clear_chat_everywhere(&self) {
        if self.together.state.peek(|s| s.role.controls()) {
            self.together.send(json!({ "t": "clear_chat" }));
        }
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
        let media = json!(Media { stream: stream.clone(), meta: meta.cloned(), video: video.cloned() });
        self.together_remember_media(&media);
        together.send(json!({ "t": "media", "media": media }));
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
        let (paused, speed, duration) = self.player.state.peek(|s| (s.paused, s.speed, s.duration));
        let position = self.player.precise_position().unwrap_or_else(|| self.player.position.get_untracked());
        self.together.inner.last_beat.set(Instant::now());
        let message = json!({
            "t": "state",
            "position": position,
            "paused": paused,
            "speed": speed,
            "duration": duration,
            "hb": heartbeat,
            "lat": self.together.inner.half_rtt.get(),
        });
        if let Some(state) = parse_state(&message) {
            self.together_set_clock(&message, state);
        }
        self.together.send(message);
    }

    /// Remembers the room's playback position (for the room page).
    fn together_set_clock(&self, message: &Value, state: Pending) {
        let duration = message["duration"].as_f64().filter(|d| d.is_finite() && *d > 0.0);
        let changed = self.together.state.peek(|s| match &s.clock {
            Some(old) => old.paused != state.paused || (old.now() - state.position).abs() > 1.5 || duration.is_some() != old.duration.is_some(),
            None => true,
        });
        let clock = RoomClock {
            position: state.position,
            paused: state.paused,
            speed: state.speed,
            duration: duration.or_else(|| self.together.state.peek(|s| s.clock.and_then(|c| c.duration))),
            at: state.received,
        };
        // Heartbeats only move the clock along: no rebuild for those.
        if changed {
            self.together.state.write(|s| s.clock = Some(clock));
        } else {
            self.together.state.write_silent(|s| s.clock = Some(clock));
        }
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
        // Latency probe, one in flight at a time (a lost one expires).
        let probe_due = match together.inner.ping_sent.get() {
            Some(sent) => sent.elapsed() > BEAT * 5,
            None => together.inner.last_probe.get().elapsed() >= BEAT,
        };
        if probe_due {
            together.inner.last_probe.set(Instant::now());
            together.inner.ping_sent.set(Some(Instant::now()));
            together.send(json!({ "t": "ping" }));
        }
        let playing = self.player.state.peek(|s| s.status == Status::Playing);
        if together.inner.leader.get()
            && together.state.peek(|s| s.role.controls())
            && playing
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
                    in_voice: m["voice"].as_bool().unwrap_or(false),
                    muted: m["muted"].as_bool().unwrap_or(false),
                    deafened: m["deafened"].as_bool().unwrap_or(false),
                    mod_muted: m["mod_muted"].as_bool().unwrap_or(false),
                    mod_deafened: m["mod_deafened"].as_bool().unwrap_or(false),
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
        // History says whose lines are ours (our id changes on reconnect).
        mine: !notice && (value["mine"].as_bool() == Some(true) || value["id"].as_u64() == Some(me)),
        mid: value["mid"].as_u64().unwrap_or(0),
        edited: value["edited"].as_bool().unwrap_or(false),
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

/// Opens the WebSocket; `wss://` checks the certificate against the usual
/// public authorities.
async fn connect(
    url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    use std::sync::Arc;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let config = Arc::new(config);
    // A host that scales to zero answers 502/503 while the server wakes up:
    // keep knocking for a while.
    let mut attempt = 0;
    loop {
        let connector = tokio_tungstenite::Connector::Rustls(config.clone());
        match tokio_tungstenite::connect_async_tls_with_config(url, None, true, Some(connector)).await {
            Ok((socket, _)) => return Ok(socket),
            Err(tokio_tungstenite::tungstenite::Error::Http(response))
                if matches!(response.status().as_u16(), 502..=504) && attempt < WAKE_RETRIES =>
            {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Retries (2 s apart) while the server is starting up.
const WAKE_RETRIES: u32 = 15;
