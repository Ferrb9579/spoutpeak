//! Client for the Media streaming server (the `server.js` service that
//! normally listens on 127.0.0.1:11470). It turns torrents into plain HTTP
//! streams, which is all the player needs.

use super::addon::http;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:11470";

pub async fn is_online(base: &str) -> bool {
    let url = format!("{}/settings", base.trim_end_matches('/'));
    matches!(
        http().get(url).timeout(Duration::from_secs(3)).send().await,
        Ok(r) if r.status().is_success()
    )
}

#[derive(Debug, Default, Deserialize)]
struct CreateResponse {
    #[serde(default)]
    files: Vec<TorrentFile>,
    #[serde(default, rename = "guessedFileIdx")]
    guessed_file_idx: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct TorrentFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    length: u64,
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "wmv", "flv", "mpg", "mpeg", "m2ts",
];

/// Registers the torrent with the streaming server and returns a playable
/// HTTP URL for the requested (or best-guess) file.
pub async fn torrent_stream_url(
    base: &str,
    info_hash: &str,
    file_idx: Option<u32>,
    trackers: &[String],
) -> Result<String, String> {
    let base = base.trim_end_matches('/');
    let sources: Vec<String> = trackers
        .iter()
        .map(|t| format!("tracker:{t}"))
        .chain(std::iter::once(format!("dht:{info_hash}")))
        .collect();
    let body = json!({
        "torrent": { "infoHash": info_hash },
        "peerSearch": { "sources": sources, "min": 40, "max": 200 },
        "guessFileIdx": {}
    });
    let response = http()
        .post(format!("{base}/{info_hash}/create"))
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|_| {
            "the streaming server is not reachable; torrents need it (see Settings)".to_owned()
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "streaming server refused the torrent (HTTP {})",
            response.status().as_u16()
        ));
    }
    let created: CreateResponse = response.json().await.unwrap_or_default();
    let index = match file_idx {
        Some(index) => index as usize,
        None => created
            .guessed_file_idx
            .filter(|i| *i >= 0)
            .map(|i| i as usize)
            .or_else(|| {
                created
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| {
                        let lower = f.name.to_lowercase();
                        VIDEO_EXTENSIONS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
                    })
                    .max_by_key(|(_, f)| f.length)
                    .map(|(i, _)| i)
            })
            .unwrap_or(0),
    };
    let mut url = format!("{base}/{info_hash}/{index}");
    if !trackers.is_empty() {
        let query = trackers
            .iter()
            .map(|t| format!("tr={}", urlencoding::encode(t)))
            .collect::<Vec<_>>()
            .join("&");
        url = format!("{url}?{query}");
    }
    Ok(url)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TorrentStats {
    pub peers: u64,
    pub download_speed: f64,
    pub progress: f64,
}

/// `stream_url` is a URL previously returned by [`torrent_stream_url`].
pub async fn torrent_stats(stream_url: &str) -> Option<TorrentStats> {
    let path = stream_url.split('?').next()?;
    let value: Value = http()
        .get(format!("{path}/stats.json"))
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    Some(TorrentStats {
        peers: value.get("peers").and_then(Value::as_u64).unwrap_or(0),
        download_speed: value
            .get("downloadSpeed")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        progress: value
            .get("streamProgress")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    })
}

/// OpenSubtitles hash of a remote video, computed by the streaming server.
pub async fn opensub_hash(base: &str, video_url: &str) -> Option<(String, u64)> {
    let url = format!(
        "{}/opensubHash?videoUrl={}",
        base.trim_end_matches('/'),
        urlencoding::encode(video_url)
    );
    let value: Value = http()
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let result = value.get("result")?;
    Some((
        result.get("hash")?.as_str()?.to_owned(),
        result.get("size")?.as_u64()?,
    ))
}

/// A `server.js` from an existing Media installation, with a runtime
/// that can execute it.
pub fn find_local_server() -> Option<(PathBuf, PathBuf)> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        roots.push(local.join("Programs").join("Media"));
        roots.push(local.join("Programs").join("LNV").join("Media-4"));
        roots.push(local.join("Programs").join("LNV").join("Media-5"));
    }
    roots.push(PathBuf::from("/opt/media"));
    roots.push(PathBuf::from("/Applications/Media.app/Contents/MacOS"));
    for root in roots {
        let script = root.join("server.js");
        if !script.is_file() {
            continue;
        }
        for runtime in ["media-runtime.exe", "media-runtime", "node.exe", "node"] {
            let runtime = root.join(runtime);
            if runtime.is_file() {
                return Some((runtime, script));
            }
        }
        return Some((PathBuf::from("node"), script));
    }
    None
}

/// Starts the user's own installed streaming server in the background.
pub fn launch_local_server() -> Result<std::process::Child, String> {
    let (runtime, script) =
        find_local_server().ok_or("no Media streaming server (server.js) was found")?;
    let mut command = Command::new(runtime);
    command
        .arg(&script)
        .current_dir(script.parent().unwrap_or(std::path::Path::new(".")))
        .env("NO_CORS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::util::hide_console(&mut command);
    command
        .spawn()
        .map_err(|e| format!("could not start the streaming server: {e}"))
}
