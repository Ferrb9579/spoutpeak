//! Client for a addon-based streaming server, which turns torrents
//! into plain HTTP streams. By default that is the built-in one in
//! [`super::local_server`] on 127.0.0.1:11470; a Media server or a remote
//! one speaks the same API.

use super::addon::http;
use serde::Deserialize;
use serde_json::{Value, json};
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

/// A selectable option the server advertises (cache size, cache drive,
/// HTTPS endpoint).
#[derive(Clone, Debug, PartialEq)]
pub struct ServerChoice {
    pub name: String,
    /// `Value::Null` stands for "unlimited" / "disabled".
    pub value: Value,
}

/// The streaming server's own settings (`GET /settings`). `values` is kept
/// whole so a save writes back every field it did not touch.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerSettings {
    pub values: serde_json::Map<String, Value>,
    pub cache_sizes: Vec<ServerChoice>,
    pub cache_roots: Vec<ServerChoice>,
    pub https_endpoints: Vec<ServerChoice>,
    pub transcode_profiles: Vec<String>,
}

impl ServerSettings {
    pub fn u64(&self, key: &str) -> Option<u64> {
        self.values.get(key).and_then(Value::as_u64)
    }
    pub fn value(&self, key: &str) -> Value {
        self.values.get(key).cloned().unwrap_or(Value::Null)
    }
}

/// Media's torrent profiles: how aggressively the server downloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TorrentProfile {
    Soft,
    Default,
    Fast,
    UltraFast,
}

impl TorrentProfile {
    pub const ALL: [TorrentProfile; 4] = [Self::Soft, Self::Default, Self::Fast, Self::UltraFast];

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Default => "Default",
            Self::Fast => "Fast",
            Self::UltraFast => "Ultra fast",
        }
    }

    /// (max connections, handshake ms, request ms, soft limit B/s, hard
    /// limit B/s, peers for stable), the values Media itself writes.
    fn values(self) -> (u64, u64, u64, f64, f64, u64) {
        match self {
            Self::Soft => (35, 20_000, 4_000, 1_677_721.6, 1_677_721.6, 5),
            Self::Default => (55, 20_000, 4_000, 2_621_440.0, 3_145_728.0, 5),
            Self::Fast => (200, 20_000, 4_000, 4_194_304.0, 39_321_600.0, 10),
            Self::UltraFast => (400, 25_000, 6_000, 8_388_608.0, 78_643_200.0, 10),
        }
    }

    pub fn apply(self, values: &mut serde_json::Map<String, Value>) {
        let (connections, handshake, request, soft, hard, peers) = self.values();
        values.insert("btMaxConnections".into(), json!(connections));
        values.insert("btHandshakeTimeout".into(), json!(handshake));
        values.insert("btRequestTimeout".into(), json!(request));
        values.insert("btDownloadSpeedSoftLimit".into(), json!(soft));
        values.insert("btDownloadSpeedHardLimit".into(), json!(hard));
        values.insert("btMinPeersForStable".into(), json!(peers));
    }

    /// The profile the current values match, if any (else "custom").
    pub fn detect(settings: &ServerSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| {
            let (connections, ..) = profile.values();
            settings.u64("btMaxConnections") == Some(connections)
                && settings
                    .values
                    .get("btDownloadSpeedHardLimit")
                    .and_then(Value::as_f64)
                    .is_some_and(|v| (v - profile.values().4).abs() < 1.0)
        })
    }
}

fn choices(options: &[Value], id: &str) -> Vec<ServerChoice> {
    options
        .iter()
        .find(|o| o.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|o| o.get("selections"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|s| ServerChoice {
                    name: s.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
                    value: s.get("val").cloned().unwrap_or(Value::Null),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn get_settings(base: &str) -> Result<ServerSettings, String> {
    let url = format!("{}/settings", base.trim_end_matches('/'));
    let body: Value = http()
        .get(url)
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .map_err(|_| "The streaming server is not reachable".to_owned())?
        .json()
        .await
        .map_err(|_| "The streaming server sent unreadable settings".to_owned())?;
    let values = body.get("values").and_then(Value::as_object).cloned().unwrap_or_default();
    let options = body.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
    let transcode_profiles = values
        .get("allTranscodeProfiles")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    Ok(ServerSettings {
        cache_sizes: choices(&options, "cacheSize"),
        cache_roots: choices(&options, "cacheRoot"),
        https_endpoints: choices(&options, "remoteHttps"),
        transcode_profiles,
        values,
    })
}

/// Saves the full value set back (the server replaces what it is sent).
pub async fn set_settings(base: &str, mut values: serde_json::Map<String, Value>) -> Result<(), String> {
    // Reported by the server, not settable.
    for key in ["serverVersion", "appPath", "cacheRootVersion", "cacheRootsToClean", "allTranscodeProfiles"] {
        values.remove(key);
    }
    let url = format!("{}/settings", base.trim_end_matches('/'));
    let response = http()
        .post(url)
        .json(&Value::Object(values))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| "The streaming server is not reachable".to_owned())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("The server refused the change (HTTP {})", response.status().as_u16()))
    }
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

