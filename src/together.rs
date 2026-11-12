//! Watch together: keeps several viewers' players in step through a small
//! relay server (`src/bin/watch-server.rs`).
//!
//! Only control messages travel through the server: which stream to play,
//! play/pause, seeks, speed and "I'm buffering". Every viewer fetches the
//! video itself, through the same torrent or URL.
//!
//! Whoever acted last leads: they send a heartbeat of their position every
//! few seconds, and the others jump to it when they drift too far.

use crate::addons::types::{Meta, Stream, Video};
use crate::player::Status;
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

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Link {
    #[default]
    Off,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, Default)]
pub struct TogetherState {
    pub link: Link,
    pub room: String,
    pub members: Vec<String>,
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

struct Inner {
    tx: RefCell<Option<mpsc::UnboundedSender<String>>>,
    /// Bumped per connection, so messages of an old one are ignored.
    connection: Cell<u64>,
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

    /// Creates a new room (empty `code`) or joins an existing one.
    pub fn together_join(&self, code: &str) {
        self.together_leave();
        let (server, name) = self.profile.peek(|p| {
            (p.settings.watch_server.trim().to_owned(), p.settings.watch_name.trim().to_owned())
        });
        let server = if server.is_empty() { DEFAULT_SERVER.to_owned() } else { server };
        let Some(handle) = crate::state::tokio() else { return };
        let together = &self.together;
        let connection = together.inner.connection.get() + 1;
        together.inner.connection.set(connection);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let _ = tx.send(json!({ "t": "hello", "name": name, "room": code.trim() }).to_string());
        *together.inner.tx.borrow_mut() = Some(tx);
        together.state.write(|s| {
            *s = TogetherState { link: Link::Connecting, ..TogetherState::default() };
        });

        handle.spawn(async move {
            let stream = match tokio::time::timeout(
                Duration::from_secs(8),
                TcpStream::connect(server.as_str()),
            )
            .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    let message = format!("Could not reach the watch-together server at {server}: {error}");
                    ui(move |app| app.together_closed(connection, Some(message)));
                    return;
                }
                Err(_) => {
                    let message = format!("The watch-together server at {server} did not answer");
                    ui(move |app| app.together_closed(connection, Some(message)));
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
            ui(move |app| app.together_closed(connection, Some("Disconnected from the watch-together room".into())));
        });
    }

    pub fn together_leave(&self) {
        let together = &self.together;
        together.inner.connection.set(together.inner.connection.get() + 1);
        // Dropping the sender ends the writer, which closes the socket.
        together.inner.tx.borrow_mut().take();
        together.inner.pending.set(None);
        together.inner.leader.set(false);
        if together.inner.auto_paused.replace(false) {
            self.together_resume_after_wait();
        }
        if together.state.peek(|s| s.link != Link::Off) {
            together.state.write(|s| *s = TogetherState::default());
        }
    }

    fn together_closed(&self, connection: u64, message: Option<String>) {
        if self.together.inner.connection.get() != connection {
            return;
        }
        self.together_leave();
        if let Some(message) = message {
            self.notify(message);
        }
    }

    fn together_message(&self, connection: u64, message: Value) {
        let together = self.together.clone();
        if together.inner.connection.get() != connection {
            return;
        }
        let from = message["from"].as_str().unwrap_or("Someone").to_owned();
        match message["t"].as_str().unwrap_or_default() {
            "welcome" => {
                let room = message["room"].as_str().unwrap_or_default().to_owned();
                together.state.write(|s| {
                    s.link = Link::Connected;
                    s.room = room.clone();
                    s.members = names(&message["members"]);
                });
                if message["media"].is_null() {
                    // A new room, or nobody has picked anything yet: share
                    // what is playing here.
                    together.inner.leader.set(true);
                    if self.player.state.peek(|s| s.status == Status::Playing) {
                        self.together_share_current();
                    }
                    self.notify(format!("Room {room} is open. Share the code to watch together."));
                } else {
                    together.inner.leader.set(false);
                    self.notify(format!("Joined room {room}"));
                    self.together_play_media(&message["media"]);
                    if let Some(pending) = parse_state(&message["state"]) {
                        self.together_apply_state(pending, false);
                    }
                }
            }
            "error" => {
                let text = message["message"].as_str().unwrap_or("The room refused the request").to_owned();
                self.together_leave();
                self.notify(text);
            }
            "members" => {
                together.state.write(|s| s.members = names(&message["members"]));
                if let Some(name) = message["joined"].as_str() {
                    self.notify(format!("{name} joined the room"));
                    // Make sure the newcomer gets a fresh position soon.
                    if together.inner.leader.get() {
                        together.inner.last_beat.set(Instant::now() - BEAT);
                    }
                } else if let Some(name) = message["left"].as_str() {
                    self.notify(format!("{name} left the room"));
                }
            }
            "media" => {
                together.inner.leader.set(false);
                self.notify(format!("{from} started a video"));
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
                    if !heartbeat && was_paused != state.paused {
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

    // ------------------------------------------------- hooks from the player

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

fn names(members: &Value) -> Vec<String> {
    members
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|m| m["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_state(value: &Value) -> Option<Pending> {
    Some(Pending {
        position: value["position"].as_f64()?,
        paused: value["paused"].as_bool().unwrap_or(true),
        speed: value["speed"].as_f64().unwrap_or(1.0),
        received: Instant::now(),
    })
}
