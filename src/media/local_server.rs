//! Built-in torrent streaming server, running inside the app.
//!
//! It answers the part of the Media streaming server's HTTP API that
//! Spoutpeak uses (`/settings`, `/{hash}/create`, `/{hash}/{file}`,
//! `/{hash}/{file}/stats.json` and `/opensubHash`), so the client in
//! [`super::server`] works against it unchanged and torrents play without
//! Media installed. The BitTorrent side is librqbit; downloaded pieces are
//! kept in a size-limited cache folder.

use super::addon::http;
use super::server::TorrentProfile;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::{DhtPersistenceConfig, Id20};
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, DhtSessionConfig, ManagedTorrent, Session, SessionOptions,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::num::NonZeroU32;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf};

pub const DEFAULT_PORT: u16 = 11470;
/// A torrent nobody has streamed for this long stops downloading.
const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(30);
/// How long `create` waits for the torrent's metadata from peers.
const METADATA_TIMEOUT: Duration = Duration::from_secs(50);
const DEFAULT_CACHE_SIZE: u64 = 2 * 1024 * 1024 * 1024;
/// Settings this server keeps; anything else a client sends is ignored.
const SETTING_KEYS: &[&str] = &[
    "cacheSize",
    "btMaxConnections",
    "btHandshakeTimeout",
    "btRequestTimeout",
    "btDownloadSpeedSoftLimit",
    "btDownloadSpeedHardLimit",
    "btMinPeersForStable",
];
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "wmv", "flv", "mpg", "mpeg", "m2ts",
];

static SERVER: tokio::sync::OnceCell<Arc<Server>> = tokio::sync::OnceCell::const_new();

/// Starts the built-in server on the port in `base` (default 11470) unless
/// it is already running in this process.
pub async fn start(base: &str) -> Result<(), String> {
    let port = port_of(base).unwrap_or(DEFAULT_PORT);
    SERVER.get_or_try_init(|| launch(port)).await.map(|_| ())
}

fn port_of(base: &str) -> Option<u16> {
    let rest = base.split_once("://").map_or(base, |(_, rest)| rest);
    let authority = rest.split('/').next()?;
    authority.rsplit_once(':')?.1.parse().ok()
}

struct Entry {
    handle: ManagedTorrentHandle,
    /// Open HTTP responses reading from this torrent.
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
}

struct Server {
    session: Arc<Session>,
    /// One sub-folder per torrent, named by info hash.
    root: PathBuf,
    settings_path: PathBuf,
    values: Mutex<Map<String, Value>>,
    torrents: Mutex<HashMap<String, Entry>>,
    /// Serialises adding the same torrent from concurrent requests.
    adding: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

type Shared = Arc<Server>;
type ManagedTorrentHandle = Arc<ManagedTorrent>;

async fn launch(port: u16) -> Result<Arc<Server>, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("port {port} is already in use: {e}"))?;

    let cache = crate::util::cache_dir();
    let root = cache.join("torrents");
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create the torrent cache: {e}"))?;
    let settings_path = crate::util::data_dir().join("streaming-server.json");
    let values = load_values(&settings_path);

    let options = SessionOptions {
        dht: Some(DhtSessionConfig {
            persistence: Some(DhtPersistenceConfig {
                config_filename: Some(cache.join("dht.json")),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ratelimits: LimitsConfig {
            download_bps: speed_limit(&values),
            ..Default::default()
        },
        ..Default::default()
    };
    let session = Session::new_with_opts(root.clone(), options)
        .await
        .map_err(|e| format!("could not start the torrent engine: {e:#}"))?;

    let server = Arc::new(Server {
        session,
        root,
        settings_path,
        values: Mutex::new(values),
        torrents: Mutex::new(HashMap::new()),
        adding: Mutex::new(HashMap::new()),
    });

    let router = Router::new()
        .route("/settings", get(get_settings).post(post_settings))
        .route("/opensubHash", get(opensub_hash))
        .route("/{hash}/create", post(create))
        .route("/{hash}/{file}", get(stream))
        .route("/{hash}/{file}/stats.json", get(stats))
        .with_state(server.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let housekeeper = server.clone();
    tokio::spawn(async move {
        loop {
            housekeeper.housekeeping().await;
            tokio::time::sleep(HOUSEKEEPING_EVERY).await;
        }
    });
    Ok(server)
}

// ------------------------------------------------------------- settings

fn default_values() -> Map<String, Value> {
    let mut values = Map::new();
    values.insert("cacheSize".into(), json!(DEFAULT_CACHE_SIZE));
    TorrentProfile::Fast.apply(&mut values);
    values
}

fn load_values(path: &FsPath) -> Map<String, Value> {
    let mut values = default_values();
    let saved: Option<Map<String, Value>> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if let Some(saved) = saved {
        merge_known(&mut values, saved);
    }
    values
}

fn merge_known(values: &mut Map<String, Value>, incoming: Map<String, Value>) {
    for (key, value) in incoming {
        if SETTING_KEYS.contains(&key.as_str()) {
            values.insert(key, value);
        }
    }
}

/// `btDownloadSpeedHardLimit` as a real cap; missing or 0 means unlimited.
fn speed_limit(values: &Map<String, Value>) -> Option<NonZeroU32> {
    let bps = values.get("btDownloadSpeedHardLimit")?.as_f64()?;
    NonZeroU32::new(bps.clamp(0.0, u32::MAX as f64) as u32)
}

fn peer_limit(values: &Map<String, Value>) -> Option<usize> {
    values
        .get("btMaxConnections")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .map(|n| n as usize)
}

/// `None` = unlimited.
fn cache_limit(values: &Map<String, Value>) -> Option<u64> {
    match values.get("cacheSize") {
        Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().unwrap_or(DEFAULT_CACHE_SIZE)),
        None => Some(DEFAULT_CACHE_SIZE),
    }
}

async fn get_settings(State(server): State<Shared>) -> Response {
    let mut values = server.values.lock().unwrap().clone();
    values.insert(
        "serverVersion".into(),
        json!(format!("Spoutpeak built-in {}", env!("CARGO_PKG_VERSION"))),
    );
    axum::Json(json!({ "options": [], "values": values })).into_response()
}

async fn post_settings(State(server): State<Shared>, body: Bytes) -> Response {
    let Ok(incoming) = serde_json::from_slice::<Map<String, Value>>(&body) else {
        return fail(StatusCode::BAD_REQUEST, "settings must be a JSON object");
    };
    let values = {
        let mut values = server.values.lock().unwrap();
        merge_known(&mut values, incoming);
        values.clone()
    };
    server.session.ratelimits.set_download_bps(speed_limit(&values));
    if let Ok(bytes) = serde_json::to_vec_pretty(&values) {
        if let Err(error) = std::fs::write(&server.settings_path, bytes) {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not save: {error}"));
        }
    }
    let housekeeper = server.clone();
    tokio::spawn(async move { housekeeper.housekeeping().await });
    axum::Json(json!({ "success": true })).into_response()
}

// ------------------------------------------------------------- torrents

fn fail(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}

fn parse_hash(hash: &str) -> Result<(String, Id20), Response> {
    let hash = hash.to_ascii_lowercase();
    if hash.len() != 40 {
        return Err(fail(StatusCode::BAD_REQUEST, "expected a 40-character info hash"));
    }
    Id20::from_str(&hash)
        .map(|id| (hash, id))
        .map_err(|_| fail(StatusCode::BAD_REQUEST, "invalid info hash"))
}

impl Server {
    fn lookup(&self, hash: &str) -> Option<ManagedTorrentHandle> {
        let torrents = self.torrents.lock().unwrap();
        let entry = torrents.get(hash)?;
        *entry.last_used.lock().unwrap() = Instant::now();
        Some(entry.handle.clone())
    }

    /// The managed torrent for `hash`, adding it (and resolving metadata
    /// from peers) if needed. A torrent added without a file in mind starts
    /// paused so nothing downloads until a file is actually streamed.
    async fn torrent(
        &self,
        hash: &str,
        id: Id20,
        trackers: Vec<String>,
        file: Option<usize>,
    ) -> Result<ManagedTorrentHandle, Response> {
        if let Some(handle) = self.lookup(hash) {
            return Ok(handle);
        }
        let lock = self.adding.lock().unwrap().entry(hash.to_owned()).or_default().clone();
        let _adding = lock.lock().await;
        if let Some(handle) = self.lookup(hash) {
            return Ok(handle);
        }
        if let Some(handle) = self.session.get(TorrentIdOrHash::Hash(id)) {
            return Ok(self.remember(hash, handle));
        }

        let values = self.values.lock().unwrap().clone();
        let options = AddTorrentOptions {
            paused: file.is_none(),
            only_files: file.map(|f| vec![f]),
            // Reuse pieces already in the cache from an earlier session.
            overwrite: true,
            output_folder: Some(self.root.join(hash).to_string_lossy().into_owned()),
            trackers: (!trackers.is_empty()).then_some(trackers),
            peer_limit: peer_limit(&values),
            ..Default::default()
        };
        let magnet = format!("magnet:?xt=urn:btih:{hash}");
        let added = tokio::time::timeout(
            METADATA_TIMEOUT,
            self.session.add_torrent(AddTorrent::from_url(magnet), Some(options)),
        )
        .await
        .map_err(|_| fail(StatusCode::GATEWAY_TIMEOUT, "no peers sent the torrent's metadata in time"))?
        .map_err(|e| fail(StatusCode::BAD_GATEWAY, &format!("could not add the torrent: {e:#}")))?;
        let handle = added
            .into_handle()
            .ok_or_else(|| fail(StatusCode::INTERNAL_SERVER_ERROR, "torrent was not added"))?;
        Ok(self.remember(hash, handle))
    }

    fn remember(&self, hash: &str, handle: ManagedTorrentHandle) -> ManagedTorrentHandle {
        self.torrents.lock().unwrap().insert(
            hash.to_owned(),
            Entry {
                handle: handle.clone(),
                active: Arc::new(AtomicUsize::new(0)),
                last_used: Arc::new(Mutex::new(Instant::now())),
            },
        );
        handle
    }

    /// Makes sure `file` is being downloaded and the torrent is running.
    async fn select(&self, handle: &ManagedTorrentHandle, file: usize) -> Result<(), Response> {
        handle
            .wait_until_initialized()
            .await
            .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")))?;
        let current = handle.only_files();
        if current.as_ref().is_none_or(|files| !files.contains(&file)) {
            let mut files: HashSet<usize> = current.unwrap_or_default().into_iter().collect();
            files.insert(file);
            self.session
                .update_only_files(handle, &files)
                .await
                .map_err(|e| fail(StatusCode::NOT_FOUND, &format!("{e:#}")))?;
        }
        if handle.is_paused() {
            self.session
                .unpause(handle)
                .await
                .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")))?;
        }
        Ok(())
    }

    fn counters(&self, hash: &str) -> Option<(Arc<AtomicUsize>, Arc<Mutex<Instant>>)> {
        let torrents = self.torrents.lock().unwrap();
        let entry = torrents.get(hash)?;
        Some((entry.active.clone(), entry.last_used.clone()))
    }

    /// Stops torrents nobody is watching and trims the cache to its limit.
    async fn housekeeping(&self) {
        let idle: Vec<ManagedTorrentHandle> = {
            let mut torrents = self.torrents.lock().unwrap();
            let hashes: Vec<String> = torrents
                .iter()
                .filter(|(_, e)| {
                    e.active.load(Ordering::SeqCst) == 0
                        && e.last_used.lock().unwrap().elapsed() > IDLE_AFTER
                })
                .map(|(hash, _)| hash.clone())
                .collect();
            hashes.iter().filter_map(|h| torrents.remove(h)).map(|e| e.handle).collect()
        };
        for handle in idle {
            // Keep the files: they are the cache.
            let _ = self.session.delete(TorrentIdOrHash::Id(handle.id()), false).await;
        }

        let limit = cache_limit(&self.values.lock().unwrap());
        let Some(limit) = limit else { return };
        let in_use: HashSet<String> = self.torrents.lock().unwrap().keys().cloned().collect();
        let root = self.root.clone();
        let _ = tokio::task::spawn_blocking(move || trim_cache(&root, limit, &in_use)).await;
    }
}

/// Deletes the least recently written torrent folders until the cache fits
/// in `limit` bytes. Folders of torrents in use are never touched.
fn trim_cache(root: &FsPath, limit: u64, in_use: &HashSet<String>) {
    fn walk(path: &FsPath, size: &mut u64, newest: &mut SystemTime) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), size, newest);
            } else {
                *size += meta.len();
                if let Ok(modified) = meta.modified() {
                    *newest = (*newest).max(modified);
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut folders: Vec<(PathBuf, u64, SystemTime, bool)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let (mut size, mut newest) = (0, SystemTime::UNIX_EPOCH);
            walk(&e.path(), &mut size, &mut newest);
            let busy = in_use.contains(&e.file_name().to_string_lossy().to_ascii_lowercase());
            (e.path(), size, newest, busy)
        })
        .collect();
    let mut total: u64 = folders.iter().map(|f| f.1).sum();
    folders.sort_by_key(|f| f.2);
    for (path, size, _, busy) in folders {
        if total <= limit {
            break;
        }
        if !busy && std::fs::remove_dir_all(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

fn is_video(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIDEO_EXTENSIONS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}

fn mime_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" | "m2ts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "mpg" | "mpeg" => "video/mpeg",
        "srt" => "application/x-subrip",
        "vtt" => "text/vtt",
        _ => "application/octet-stream",
    }
}

/// (name with `/` separators, length, is padding) for every file.
fn file_list(handle: &ManagedTorrentHandle) -> Vec<(String, u64, bool)> {
    let Some(metadata) = handle.metadata.load_full() else {
        return Vec::new();
    };
    metadata
        .file_infos
        .iter()
        .map(|f| {
            let name = f
                .relative_filename
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (name, f.len, f.attrs.padding)
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct CreateBody {
    #[serde(default, rename = "peerSearch")]
    peer_search: Option<PeerSearch>,
}

#[derive(Default, Deserialize)]
struct PeerSearch {
    #[serde(default)]
    sources: Vec<String>,
}

async fn create(State(server): State<Shared>, Path(hash): Path<String>, body: Bytes) -> Response {
    let (hash, id) = match parse_hash(&hash) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let body: CreateBody = serde_json::from_slice(&body).unwrap_or_default();
    let trackers = body
        .peer_search
        .unwrap_or_default()
        .sources
        .into_iter()
        .filter_map(|s| s.strip_prefix("tracker:").map(str::to_owned))
        .collect();
    let handle = match server.torrent(&hash, id, trackers, None).await {
        Ok(handle) => handle,
        Err(response) => return response,
    };
    let files = file_list(&handle);
    let guessed = files
        .iter()
        .enumerate()
        .filter(|(_, (name, _, padding))| !padding && is_video(name))
        .max_by_key(|(_, (_, len, _))| *len)
        .map_or(-1, |(i, _)| i as i64);
    // Start fetching the likely file now so peers are connected by the time
    // the player asks for it. A different file is added on its first request.
    if guessed >= 0 {
        let _ = server.select(&handle, guessed as usize).await;
    }
    axum::Json(json!({
        "infoHash": hash,
        "name": handle.name(),
        "files": files
            .iter()
            .map(|(name, length, _)| json!({ "name": name, "path": name, "length": length }))
            .collect::<Vec<_>>(),
        "guessedFileIdx": guessed,
    }))
    .into_response()
}

/// Repeated `tr=` query parameters, as the client appends them.
fn trackers_from_query(query: Option<&str>) -> Vec<String> {
    query
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.strip_prefix("tr="))
        .filter_map(|value| urlencoding::decode(value).ok().map(|v| v.into_owned()))
        .collect()
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` as a half-open range.
fn parse_range(header: Option<&HeaderValue>, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header?.to_str().ok()?.strip_prefix("bytes=")?;
    let spec = spec.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    let range = if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        (len.saturating_sub(suffix), len)
    } else {
        let start: u64 = start.parse().ok()?;
        let end = if end.is_empty() {
            len
        } else {
            end.parse::<u64>().ok()?.saturating_add(1).min(len)
        };
        (start, end)
    };
    Some(if range.0 < range.1 { Ok(range) } else { Err(()) })
}

/// Counts an open response against its torrent so it is not stopped or
/// evicted mid-stream.
struct Tracked {
    inner: Box<dyn AsyncRead + Send + Unpin>,
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
}

impl AsyncRead for Tracked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        *self.last_used.lock().unwrap() = Instant::now();
    }
}

async fn stream(
    State(server): State<Shared>,
    Path((hash, file)): Path<(String, usize)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let (hash, id) = match parse_hash(&hash) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let trackers = trackers_from_query(query.as_deref());
    let handle = match server.torrent(&hash, id, trackers, Some(file)).await {
        Ok(handle) => handle,
        Err(response) => return response,
    };
    let files = file_list(&handle);
    let Some((name, _, _)) = files.get(file) else {
        return fail(StatusCode::NOT_FOUND, "the torrent has no such file");
    };
    if let Err(response) = server.select(&handle, file).await {
        return response;
    }
    let Some((active, last_used)) = server.counters(&hash) else {
        return fail(StatusCode::NOT_FOUND, "torrent is no longer active");
    };
    let mut reader = match handle.clone().stream(file).await {
        Ok(reader) => reader,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
    };
    let len = reader.len();

    let mut out = HeaderMap::new();
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime_type(name)));
    let (status, body): (StatusCode, Box<dyn AsyncRead + Send + Unpin>) =
        match parse_range(headers.get(header::RANGE), len) {
            Some(Err(())) => {
                out.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{len}")).unwrap(),
                );
                return (StatusCode::RANGE_NOT_SATISFIABLE, out).into_response();
            }
            Some(Ok((start, end))) => {
                if let Err(e) = reader.seek(SeekFrom::Start(start)).await {
                    return fail(StatusCode::INTERNAL_SERVER_ERROR, &format!("seek failed: {e}"));
                }
                out.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
                out.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {start}-{}/{len}", end - 1)).unwrap(),
                );
                (StatusCode::PARTIAL_CONTENT, Box::new(reader.take(end - start)))
            }
            None => {
                out.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
                (StatusCode::OK, Box::new(reader))
            }
        };

    active.fetch_add(1, Ordering::SeqCst);
    let tracked = Tracked { inner: body, active, last_used };
    let chunks = tokio_util::io::ReaderStream::with_capacity(tracked, 64 * 1024);
    (status, out, Body::from_stream(chunks)).into_response()
}

async fn stats(State(server): State<Shared>, Path((hash, file)): Path<(String, usize)>) -> Response {
    let hash = hash.to_ascii_lowercase();
    let Some(handle) = server.lookup(&hash) else {
        return fail(StatusCode::NOT_FOUND, "torrent is not active");
    };
    let stats = handle.stats();
    let live = stats.live.as_ref();
    let file_len = file_list(&handle).get(file).map_or(0, |f| f.1);
    let file_done = stats.file_progress.get(file).copied().unwrap_or(0);
    let progress = if file_len > 0 { file_done as f64 / file_len as f64 } else { 0.0 };
    axum::Json(json!({
        "infoHash": hash,
        "name": handle.name(),
        "peers": live.map_or(0, |l| l.snapshot.peer_stats.live),
        "downloadSpeed": live.map_or(0, |l| l.download_speed.as_bytes()),
        "uploadSpeed": live.map_or(0, |l| l.upload_speed.as_bytes()),
        "downloaded": stats.progress_bytes,
        "streamLen": file_len,
        "streamProgress": progress.clamp(0.0, 1.0),
    }))
    .into_response()
}

// ------------------------------------------------------------- subtitles

#[derive(Deserialize)]
struct HashQuery {
    #[serde(rename = "videoUrl")]
    video_url: String,
}

/// The OpenSubtitles hash: file size plus the 64-bit little-endian word
/// sums of the first and last 64 KiB.
async fn opensub_hash(Query(query): Query<HashQuery>) -> Response {
    const CHUNK: u64 = 64 * 1024;
    async fn fetch(url: &str, start: u64, end: u64) -> Result<(Vec<u8>, Option<u64>), String> {
        let response = http()
            .get(url)
            .header(header::RANGE.as_str(), format!("bytes={start}-{}", end - 1))
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let total = response
            .headers()
            .get(header::CONTENT_RANGE.as_str())
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse().ok())
            .or_else(|| response.content_length().filter(|_| response.status() == 200));
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        let wanted = (end - start) as usize;
        Ok((bytes[..bytes.len().min(wanted)].to_vec(), total))
    }
    fn sum(hash: &mut u64, bytes: &[u8]) {
        for word in bytes.chunks(8) {
            let mut padded = [0u8; 8];
            padded[..word.len()].copy_from_slice(word);
            *hash = hash.wrapping_add(u64::from_le_bytes(padded));
        }
    }
    let url = query.video_url;
    let result = async {
        let (head, total) = fetch(&url, 0, CHUNK).await?;
        let size = total.ok_or("the video size is unknown")?;
        let tail = if size > CHUNK {
            fetch(&url, size - CHUNK, size).await?.0
        } else {
            head.clone()
        };
        let mut hash = size;
        sum(&mut hash, &head);
        sum(&mut hash, &tail);
        Ok::<_, String>((format!("{hash:016x}"), size))
    }
    .await;
    match result {
        Ok((hash, size)) => axum::Json(json!({ "result": { "hash": hash, "size": size } })).into_response(),
        Err(error) => fail(StatusCode::BAD_GATEWAY, &error),
    }
}
