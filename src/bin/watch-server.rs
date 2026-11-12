//! Spoutpeak watch-together relay.
//!
//! A tiny room server: every viewer streams the video themselves (the
//! torrent or URL the room picked); this only passes small JSON messages
//! between the people in a room so play, pause, seek, speed and the chosen
//! stream stay in step. No video ever goes through it.
//!
//! Protocol: one JSON object per line over TCP.
//!
//! Client to server:
//! - `{"t":"hello","name":"Ana","room":""}`: join `room`, or create a new
//!   room when it is empty. Answered with `welcome`.
//! - `{"t":"media","media":{...}}`: the stream everyone should play.
//! - `{"t":"state","position":12.5,"paused":false,"speed":1.0,"hb":false}`:
//!   playback state; `hb` marks a periodic heartbeat rather than an action.
//! - `{"t":"buffering","on":true}`: this viewer is waiting for data.
//! - `{"t":"ping"}`.
//!
//! Server to client: `welcome` (room, your id, members, last media and
//! state, the state's position already moved on by how long it has been
//! playing), `members`, and the `media` / `state` / `buffering` messages of
//! others with `from` (name) and `id` added.
//!
//! Usage: `watch-server [address]`, default `0.0.0.0:7878` (or the
//! `SPOUTPEAK_WATCH_ADDR` environment variable).

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// Longest line a client may send; a series' episode list can be large.
const MAX_LINE: usize = 4 * 1024 * 1024;
const MAX_ROOM_MEMBERS: usize = 32;

struct Member {
    name: String,
    tx: mpsc::UnboundedSender<String>,
}

#[derive(Default)]
struct Room {
    members: HashMap<u64, Member>,
    media: Option<Value>,
    /// Last playback state and when it was set.
    state: Option<(Value, Instant)>,
}

#[derive(Default)]
struct Rooms {
    rooms: HashMap<String, Room>,
    next_id: u64,
}

type Shared = Arc<Mutex<Rooms>>;

fn room_code(rooms: &Rooms) -> String {
    // No 0/O/1/I: codes get read out loud.
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
        ^ (rooms.next_id.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    loop {
        let code: String = (0..6)
            .map(|_| {
                // xorshift
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                ALPHABET[(seed % ALPHABET.len() as u64) as usize] as char
            })
            .collect();
        if !rooms.rooms.contains_key(&code) {
            return code;
        }
    }
}

fn members_json(room: &Room) -> Value {
    let mut list: Vec<(&u64, &Member)> = room.members.iter().collect();
    list.sort_by_key(|(id, _)| **id);
    Value::Array(
        list.into_iter()
            .map(|(id, m)| json!({ "id": id, "name": m.name }))
            .collect(),
    )
}

fn broadcast(room: &Room, except: Option<u64>, message: &Value) {
    let line = message.to_string();
    for (id, member) in &room.members {
        if Some(*id) != except {
            let _ = member.tx.send(line.clone());
        }
    }
}

/// The stored state with its position moved forward by the time it has
/// been playing since.
fn current_state(room: &Room) -> Value {
    match &room.state {
        Some((state, at)) => {
            let mut state = state.clone();
            let paused = state["paused"].as_bool().unwrap_or(true);
            if !paused {
                let speed = state["speed"].as_f64().unwrap_or(1.0);
                let position = state["position"].as_f64().unwrap_or(0.0);
                state["position"] = json!(position + at.elapsed().as_secs_f64() * speed);
            }
            state
        }
        None => Value::Null,
    }
}

async fn client(stream: TcpStream, rooms: Shared) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let _ = stream.set_nodelay(true);
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err()
                || write.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
    });

    let mut reader = BufReader::new(read);
    let mut joined: Option<(String, u64, String)> = None;
    let mut buffer = String::new();
    loop {
        buffer.clear();
        let read = match (&mut reader).take(MAX_LINE as u64).read_line(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if read >= MAX_LINE && !buffer.ends_with('\n') {
            break;
        }
        let Ok(message) = serde_json::from_str::<Value>(buffer.trim()) else {
            continue;
        };
        let kind = message["t"].as_str().unwrap_or_default();
        let mut rooms_guard = rooms.lock().unwrap();
        let rooms_ref = &mut *rooms_guard;
        match (kind, &joined) {
            ("hello", None) => {
                let name: String = message["name"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(32)
                    .collect();
                let name = if name.is_empty() { "Guest".to_owned() } else { name };
                let wanted = message["room"].as_str().unwrap_or("").trim().to_ascii_uppercase();
                let code = if wanted.is_empty() {
                    let code = room_code(rooms_ref);
                    rooms_ref.rooms.insert(code.clone(), Room::default());
                    code
                } else if rooms_ref.rooms.contains_key(&wanted) {
                    wanted
                } else {
                    let _ = tx.send(json!({ "t": "error", "message": "No room with that code" }).to_string());
                    continue;
                };
                rooms_ref.next_id += 1;
                let id = rooms_ref.next_id;
                let room = rooms_ref.rooms.get_mut(&code).unwrap();
                if room.members.len() >= MAX_ROOM_MEMBERS {
                    let _ = tx.send(json!({ "t": "error", "message": "That room is full" }).to_string());
                    continue;
                }
                room.members.insert(id, Member { name: name.clone(), tx: tx.clone() });
                let _ = tx.send(
                    json!({
                        "t": "welcome",
                        "room": code,
                        "you": id,
                        "members": members_json(room),
                        "media": room.media.clone().unwrap_or(Value::Null),
                        "state": current_state(room),
                    })
                    .to_string(),
                );
                broadcast(
                    room,
                    Some(id),
                    &json!({ "t": "members", "members": members_json(room), "joined": name }),
                );
                println!("{peer} joined {code} as {name}");
                joined = Some((code, id, name));
            }
            ("ping", _) => {
                let _ = tx.send(json!({ "t": "pong" }).to_string());
            }
            ("media" | "state" | "buffering", Some((code, id, name))) => {
                let Some(room) = rooms_ref.rooms.get_mut(code) else { break };
                let mut out = message.clone();
                out["from"] = json!(name);
                out["id"] = json!(id);
                match kind {
                    "media" => {
                        room.media = Some(message["media"].clone());
                        // A new video starts from its own beginning.
                        room.state = None;
                    }
                    "state" => {
                        let state = json!({
                            "position": message["position"].as_f64().unwrap_or(0.0),
                            "paused": message["paused"].as_bool().unwrap_or(true),
                            "speed": message["speed"].as_f64().unwrap_or(1.0),
                        });
                        room.state = Some((state, Instant::now()));
                    }
                    _ => {}
                }
                broadcast(room, Some(*id), &out);
            }
            _ => {}
        }
    }

    if let Some((code, id, name)) = joined {
        let mut rooms = rooms.lock().unwrap();
        if let Some(room) = rooms.rooms.get_mut(&code) {
            room.members.remove(&id);
            if room.members.is_empty() {
                rooms.rooms.remove(&code);
                println!("{code} closed");
            } else {
                broadcast(room, None, &json!({ "t": "members", "members": members_json(room), "left": name }));
                // Whoever was waiting on this viewer can go on.
                broadcast(room, None, &json!({ "t": "buffering", "on": false, "from": name, "id": id }));
            }
        }
        println!("{peer} left {code}");
    }
    writer.abort();
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("SPOUTPEAK_WATCH_ADDR").ok())
        .unwrap_or_else(|| "0.0.0.0:7878".to_owned());
    let listener = TcpListener::bind(&address).await?;
    println!("Spoutpeak watch-together server listening on {address}");
    let rooms: Shared = Arc::default();
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(client(stream, rooms.clone()));
    }
}
