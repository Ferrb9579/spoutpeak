//! Spoutpeak watch-together relay.
//!
//! A tiny room server: every viewer streams the video themselves (the
//! torrent or URL the room picked); this only passes small JSON messages
//! between the people in a room so play, pause, seek, speed and the chosen
//! stream stay in step. No video ever goes through it.
//!
//! Roles: whoever creates a room is its **host**. The host can make
//! viewers **co-hosts** (or back), hand the host role over, and remove
//! people. Hosts and co-hosts control playback; viewers follow. When the
//! host drops, a co-host takes charge; with none, the room pauses and waits
//! for the host to reconnect (their reconnect token restores the role).
//! An empty room is kept for [`ROOM_GRACE`] so people can come back.
//!
//! Protocol: one JSON object per WebSocket text message, on `GET /ws`.
//!
//! Client to server:
//! - `hello {name, room, password, token}`: join `room` (create one when
//!   empty; `password` then protects it). `token` from an earlier `welcome`
//!   rejoins with the same role. Answered by `welcome` or `error {code}`.
//! - `media {media}`, `state {position, paused, speed, hb}`: playback, from
//!   the host and co-hosts only.
//! - `buffering {on}`: this viewer is waiting for data.
//! - `chat {text}`: a chat line.
//! - `clear_chat`: host and co-hosts; empties everyone's chat and the
//!   history new joiners get.
//! - `role {id, role}`: host only; `role` is `cohost`, `viewer` or `host`
//!   (hands the host role over).
//! - `kick {id}`: host only.
//! - `ping`.
//!
//! Server to client: `welcome {room, you, token, role, members, media,
//! state, chat, host_online, protected}`, `members {members, host_online}`,
//! `notice {text}` (also kept in the chat history), `chat {from, id, role,
//! text, at}`, `chat_cleared {from, id}`, `kicked`, `replaced`, `error {code, message}`, and the
//! `media` / `state` / `buffering` messages of others with `from` and `id`.
//!
//! The server speaks plain HTTP. Put it behind an HTTPS reverse proxy (or a
//! platform that terminates TLS) so clients connect with `wss://`.
//! `GET /health` answers `ok` for load balancers.
//!
//! Configuration: `PORT` (default 7878) or `SPOUTPEAK_WATCH_ADDR`
//! (`0.0.0.0:7878`), or the address as the first argument.

use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Longest message a client may send; a series' episode list can be large.
const MAX_LINE: usize = 4 * 1024 * 1024;
const MAX_ROOM_MEMBERS: usize = 32;
const MAX_CHAT: usize = 500;
const CHAT_HISTORY: usize = 100;
/// How long an empty room waits for people to come back.
const ROOM_GRACE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Host,
    Cohost,
    Viewer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Cohost => "cohost",
            Role::Viewer => "viewer",
        }
    }
    fn controls(self) -> bool {
        self != Role::Viewer
    }
}

struct Member {
    name: String,
    role: Role,
    token: String,
    tx: mpsc::UnboundedSender<String>,
}

struct Room {
    password: String,
    members: HashMap<u64, Member>,
    /// Reconnect token -> role, for everyone who has been in the room.
    roles: HashMap<String, Role>,
    media: Option<Value>,
    /// Last playback state and when it was set.
    state: Option<(Value, Instant)>,
    chat: VecDeque<Value>,
    empty_since: Option<Instant>,
}

#[derive(Default)]
struct Rooms {
    rooms: HashMap<String, Room>,
    next_id: u64,
    seed: u64,
}

type Shared = Arc<Mutex<Rooms>>;

impl Rooms {
    fn random(&mut self, length: usize, alphabet: &[u8]) -> String {
        if self.seed == 0 {
            self.seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x1234_5678, |d| d.as_nanos() as u64)
                | 1;
        }
        (0..length)
            .map(|_| {
                // xorshift64*
                self.seed ^= self.seed >> 12;
                self.seed ^= self.seed << 25;
                self.seed ^= self.seed >> 27;
                let value = self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
                alphabet[((value >> 33) % alphabet.len() as u64) as usize] as char
            })
            .collect()
    }

    fn room_code(&mut self) -> String {
        // No 0/O/1/I: codes get read out loud.
        loop {
            let code = self.random(6, b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789");
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }
    }

    fn token(&mut self) -> String {
        self.random(24, b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
    }
}

impl Room {
    fn host_online(&self) -> bool {
        self.members.values().any(|m| m.role == Role::Host)
    }

    fn members_json(&self) -> Value {
        let mut list: Vec<(&u64, &Member)> = self.members.iter().collect();
        list.sort_by_key(|(id, m)| (m.role as u8, **id));
        Value::Array(
            list.into_iter()
                .map(|(id, m)| json!({ "id": id, "name": m.name, "role": m.role.as_str() }))
                .collect(),
        )
    }

    fn broadcast(&self, except: Option<u64>, message: &Value) {
        let line = message.to_string();
        for (id, member) in &self.members {
            if Some(*id) != except {
                let _ = member.tx.send(line.clone());
            }
        }
    }

    fn send_members(&self) {
        self.broadcast(
            None,
            &json!({ "t": "members", "members": self.members_json(), "host_online": self.host_online() }),
        );
    }

    /// A system line in the chat.
    fn notice(&mut self, text: String) {
        let line = json!({ "t": "notice", "text": text, "at": unix_now() });
        self.remember(line.clone());
        self.broadcast(None, &line);
    }

    fn remember(&mut self, line: Value) {
        self.chat.push_back(line);
        while self.chat.len() > CHAT_HISTORY {
            self.chat.pop_front();
        }
    }

    /// The stored state with its position moved forward by the time it has
    /// been playing since.
    fn current_state(&self) -> Value {
        match &self.state {
            Some((state, at)) => {
                let mut state = state.clone();
                if !state["paused"].as_bool().unwrap_or(true) {
                    let speed = state["speed"].as_f64().unwrap_or(1.0);
                    let position = state["position"].as_f64().unwrap_or(0.0);
                    state["position"] = json!(position + at.elapsed().as_secs_f64() * speed);
                }
                state
            }
            None => Value::Null,
        }
    }

    /// Stops playback for everyone where it is now.
    fn pause_all(&mut self, by: &str) {
        let state = self.current_state();
        if state.is_null() || state["paused"].as_bool().unwrap_or(true) {
            return;
        }
        let mut state = state;
        state["paused"] = json!(true);
        self.state = Some((state.clone(), Instant::now()));
        state["t"] = json!("state");
        state["from"] = json!(by);
        state["id"] = json!(0);
        state["hb"] = json!(false);
        self.broadcast(None, &state);
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn error(tx: &mpsc::UnboundedSender<String>, code: &str, message: &str) {
    let _ = tx.send(json!({ "t": "error", "code": code, "message": message }).to_string());
}

fn clean(value: &Value, limit: usize) -> String {
    value
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

struct Joined {
    code: String,
    id: u64,
}

/// Handles `hello`; returns where the client joined.
fn hello(all: &mut Rooms, message: &Value, tx: &mpsc::UnboundedSender<String>, peer: &str) -> Option<Joined> {
    let name = clean(&message["name"], 32);
    let name = if name.is_empty() { "Guest".to_owned() } else { name };
    let wanted = clean(&message["room"], 12).to_ascii_uppercase();
    let password = message["password"].as_str().unwrap_or("").to_owned();
    let offered_token = message["token"].as_str().unwrap_or("").to_owned();

    let (code, role, token) = if wanted.is_empty() {
        let code = all.room_code();
        let token = all.token();
        all.rooms.insert(
            code.clone(),
            Room {
                password: password.chars().take(128).collect(),
                members: HashMap::new(),
                roles: HashMap::from([(token.clone(), Role::Host)]),
                media: None,
                state: None,
                chat: VecDeque::new(),
                empty_since: None,
            },
        );
        (code, Role::Host, token)
    } else {
        let Some(room) = all.rooms.get(&wanted) else {
            error(tx, "no_room", "No room with that code");
            return None;
        };
        let known = room.roles.get(&offered_token).copied();
        if known.is_none() && !room.password.is_empty() && room.password != password {
            let text = if password.is_empty() { "This room needs a password" } else { "Wrong password" };
            error(tx, "password", text);
            return None;
        }
        if room.members.len() >= MAX_ROOM_MEMBERS {
            error(tx, "full", "That room is full");
            return None;
        }
        match known {
            Some(role) => (wanted, role, offered_token),
            None => {
                let token = all.token();
                (wanted, Role::Viewer, token)
            }
        }
    };

    all.next_id += 1;
    let id = all.next_id;
    let room = all.rooms.get_mut(&code)?;
    // The same person connected twice (a stale socket): drop the old one.
    let stale: Vec<u64> = room
        .members
        .iter()
        .filter(|(_, m)| m.token == token)
        .map(|(id, _)| *id)
        .collect();
    let returning = !stale.is_empty();
    for old in stale {
        if let Some(member) = room.members.remove(&old) {
            let _ = member.tx.send(json!({ "t": "replaced" }).to_string());
        }
    }
    room.roles.insert(token.clone(), role);
    room.empty_since = None;
    room.members.insert(id, Member { name: name.clone(), role, token: token.clone(), tx: tx.clone() });
    let _ = tx.send(
        json!({
            "t": "welcome",
            "room": code,
            "you": id,
            "token": token,
            "role": role.as_str(),
            "members": room.members_json(),
            "media": room.media.clone().unwrap_or(Value::Null),
            "state": room.current_state(),
            "chat": Value::Array(room.chat.iter().cloned().collect()),
            "host_online": room.host_online(),
            "protected": !room.password.is_empty(),
        })
        .to_string(),
    );
    if !returning {
        let text = match role {
            Role::Host if room.media.is_some() || room.chat.len() > 1 => format!("{name} (host) is back"),
            Role::Host => format!("{name} created the room"),
            Role::Cohost => format!("{name} (co-host) is back"),
            Role::Viewer => format!("{name} joined"),
        };
        room.notice(text);
    }
    room.send_members();
    println!("{peer} joined {code} as {name} ({})", role.as_str());
    Some(Joined { code, id })
}

/// Handles a message from someone in a room. `false` ends the connection.
fn in_room(all: &mut Rooms, me: &Joined, kind: &str, message: &Value, tx: &mpsc::UnboundedSender<String>) -> bool {
    let id = me.id;
    let Some(room) = all.rooms.get_mut(&me.code) else { return false };
    // Gone: replaced by a newer connection, or removed.
    let Some(sender) = room.members.get(&id) else { return false };
    let (name, role) = (sender.name.clone(), sender.role);
    match kind {
        "media" | "state" => {
            if !role.controls() {
                error(tx, "forbidden", "Only the host and co-hosts can control playback");
                return true;
            }
            let mut out = message.clone();
            out["from"] = json!(name);
            out["id"] = json!(id);
            if kind == "media" {
                room.media = Some(message["media"].clone());
                // A new video starts from its own beginning.
                room.state = None;
            } else {
                let mut state = json!({
                    "position": message["position"].as_f64().unwrap_or(0.0),
                    "paused": message["paused"].as_bool().unwrap_or(true),
                    "speed": message["speed"].as_f64().unwrap_or(1.0),
                });
                if let Some(duration) = message["duration"].as_f64().filter(|d| d.is_finite() && *d > 0.0) {
                    state["duration"] = json!(duration);
                }
                room.state = Some((state, Instant::now()));
            }
            room.broadcast(Some(id), &out);
        }
        "buffering" => {
            let mut out = message.clone();
            out["from"] = json!(name);
            out["id"] = json!(id);
            room.broadcast(Some(id), &out);
        }
        "chat" => {
            let text = clean(&message["text"], MAX_CHAT);
            if !text.is_empty() {
                let line = json!({
                    "t": "chat",
                    "from": name,
                    "id": id,
                    "role": role.as_str(),
                    "text": text,
                    "at": unix_now(),
                });
                room.remember(line.clone());
                room.broadcast(None, &line);
            }
        }
        "clear_chat" => {
            if !role.controls() {
                error(tx, "forbidden", "Only the host and co-hosts can clear the chat");
                return true;
            }
            room.chat.clear();
            room.broadcast(None, &json!({ "t": "chat_cleared", "from": name, "id": id }));
            room.notice(format!("{name} cleared the chat"));
        }
        "role" => {
            if role != Role::Host {
                error(tx, "forbidden", "Only the host can change roles");
                return true;
            }
            let target = message["id"].as_u64().unwrap_or(0);
            let wanted = match message["role"].as_str() {
                Some("cohost") => Role::Cohost,
                Some("viewer") => Role::Viewer,
                Some("host") => Role::Host,
                _ => return true,
            };
            if target == id {
                return true;
            }
            let Some(member) = room.members.get_mut(&target) else { return true };
            member.role = wanted;
            let (target_name, target_token) = (member.name.clone(), member.token.clone());
            room.roles.insert(target_token, wanted);
            if wanted == Role::Host {
                // Handing over: the old host becomes a co-host.
                if let Some(me) = room.members.get_mut(&id) {
                    me.role = Role::Cohost;
                    let token = me.token.clone();
                    room.roles.insert(token, Role::Cohost);
                }
            }
            room.notice(match wanted {
                Role::Host => format!("{name} made {target_name} the host"),
                Role::Cohost => format!("{name} made {target_name} a co-host"),
                Role::Viewer => format!("{target_name} is now a viewer"),
            });
            room.send_members();
        }
        "kick" => {
            if role != Role::Host {
                error(tx, "forbidden", "Only the host can remove people");
                return true;
            }
            let target = message["id"].as_u64().unwrap_or(0);
            if target != id {
                if let Some(member) = room.members.remove(&target) {
                    // A removed person cannot rejoin with their token.
                    room.roles.remove(&member.token);
                    let _ = member.tx.send(json!({ "t": "kicked" }).to_string());
                    room.notice(format!("{} was removed by {name}", member.name));
                    room.send_members();
                }
            }
        }
        _ => {}
    }
    true
}

fn left(all: &mut Rooms, me: &Joined) {
    let Some(room) = all.rooms.get_mut(&me.code) else { return };
    let Some(member) = room.members.remove(&me.id) else { return };
    if room.members.is_empty() {
        room.empty_since = Some(Instant::now());
        room.pause_all(&member.name);
        return;
    }
    // Whoever was waiting on this viewer can go on.
    room.broadcast(None, &json!({ "t": "buffering", "on": false, "from": member.name, "id": me.id }));
    if member.role == Role::Host {
        let cohost = room.members.values().find(|m| m.role == Role::Cohost).map(|m| m.name.clone());
        match cohost {
            Some(cohost) => room.notice(format!(
                "The host {} disconnected. {cohost} (co-host) is in charge until they return.",
                member.name
            )),
            None => {
                room.pause_all(&member.name);
                room.notice(format!("The host {} disconnected. Paused until they reconnect.", member.name));
            }
        }
    } else {
        room.notice(format!("{} left", member.name));
    }
    room.send_members();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(rooms): State<Shared>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Behind a proxy the socket peer is the proxy; prefer what it forwards.
    let peer = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_owned())
        .unwrap_or_else(|| peer.to_string());
    ws.max_message_size(MAX_LINE)
        .max_frame_size(MAX_LINE)
        .on_upgrade(move |socket| client(socket, rooms, peer))
}

async fn client(socket: WebSocket, rooms: Shared, peer: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        // Keeps idle proxies from closing the connection.
        let mut keepalive = tokio::time::interval(Duration::from_secs(25));
        keepalive.tick().await;
        loop {
            let message = tokio::select! {
                line = rx.recv() => match line {
                    Some(line) => Message::Text(line.into()),
                    None => break,
                },
                _ = keepalive.tick() => Message::Ping(Vec::new().into()),
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let mut joined: Option<Joined> = None;
    while let Some(Ok(frame)) = stream.next().await {
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(message) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let kind = message["t"].as_str().unwrap_or_default();
        if kind == "ping" {
            let _ = tx.send(json!({ "t": "pong" }).to_string());
            continue;
        }
        let mut all = rooms.lock().unwrap();
        match &joined {
            None if kind == "hello" => joined = hello(&mut all, &message, &tx, &peer),
            None => {}
            Some(me) => {
                if !in_room(&mut all, me, kind, &message, &tx) {
                    // Replaced or removed: leave without announcing.
                    joined = None;
                    break;
                }
            }
        }
    }

    if let Some(me) = joined {
        left(&mut rooms.lock().unwrap(), &me);
        println!("{peer} left {}", me.code);
    }
    // Let the last messages (kicked / replaced) go out, then close.
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}

fn address() -> String {
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "-h" || arg == "--help" {
            println!("watch-server [address]   (or PORT / SPOUTPEAK_WATCH_ADDR)");
            std::process::exit(0);
        }
        return arg;
    }
    if let Ok(address) = std::env::var("SPOUTPEAK_WATCH_ADDR") {
        return address;
    }
    let port = std::env::var("PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(7878);
    format!("0.0.0.0:{port}")
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let address = address();
    let rooms: Shared = Arc::default();

    let sweeper = rooms.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            sweeper.lock().unwrap().rooms.retain(|code, room| {
                let expired = room.empty_since.is_some_and(|t| t.elapsed() > ROOM_GRACE);
                if expired {
                    println!("{code} closed");
                }
                !expired
            });
        }
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .route("/", get(|| async { "Spoutpeak watch-together server. Connect with a WebSocket on /ws." }))
        .with_state(rooms);

    let listener = TcpListener::bind(&address).await?;
    println!("Spoutpeak watch-together server listening on http://{address} (WebSocket on /ws)");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await
}
