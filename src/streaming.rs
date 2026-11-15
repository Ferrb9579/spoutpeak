//! Built-in torrent streaming engine.
//!
//! The app asks [`stream_url`] for a torrent file and gets back a plain HTTP
//! URL that ffmpeg (or an external player) can read with seeking. Behind it,
//! librqbit downloads the pieces around the read position first. The HTTP
//! listener only binds to 127.0.0.1 on a random port, and every URL carries
//! a random per-run token, so nothing else on the machine or network can use
//! it. Downloaded pieces stay in a cache for faster replays, limited by size
//! and by how long ago each torrent was last played.

use crate::store::TorrentSettings;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::{DhtPersistenceConfig, Id20};
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, DhtSessionConfig, FileStream, ManagedTorrent, Session,
    SessionOptions, StreamPriority,
};
use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::num::NonZeroU32;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf};

/// A torrent nobody has streamed for this long stops downloading.
const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);
/// librqbit fetches this much ahead of every open stream (its
/// `PER_STREAM_BUF_DEFAULT`), in file order.
const STREAM_WINDOW: u64 = 32 * 1024 * 1024;
/// A response counts as playback (rather than a probe) once it has sent this
/// much and stayed open this long; only then does read-ahead start.
const PLAYING_BYTES: u64 = 8 * 1024 * 1024;
const PLAYING_AFTER: Duration = Duration::from_secs(3);
/// A reader that waited this long for a piece is starving: read-ahead backs
/// off so every peer works on the pieces it needs next.
const STARVING_AFTER: Duration = Duration::from_millis(250);
const STARVED_COOLDOWN: Duration = Duration::from_secs(3);
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(30);
/// Touched in a cached torrent's folder while it is in use, so its age is
/// the time since it was last played, not since a piece was last written.
const USED_MARKER: &str = ".spoutpeak-used";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long to wait for the torrent's metadata from peers.
const METADATA_TIMEOUT: Duration = Duration::from_secs(50);
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "wmv", "flv", "mpg", "mpeg", "m2ts",
];

type Handle = Arc<ManagedTorrent>;

// ------------------------------------------------------------- public API

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TorrentStats {
    pub peers: u64,
    /// Bytes per second.
    pub download_speed: f64,
    /// Share of the streamed file downloaded so far, 0 to 1.
    pub progress: f64,
    /// Downloaded stretches of the file as (start, end) fractions, in order.
    pub ranges: Vec<(f32, f32)>,
}

/// Presets for how hard the engine pulls from the swarm.
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

    /// (max peer connections, download cap in bytes/s).
    fn values(self) -> (u32, Option<u64>) {
        match self {
            Self::Soft => (35, Some(1_677_721)),
            Self::Default => (55, Some(3_145_728)),
            Self::Fast => (200, Some(39_321_600)),
            Self::UltraFast => (400, None),
        }
    }

    pub fn apply(self, settings: &mut TorrentSettings) {
        (settings.max_connections, settings.speed_limit) = self.values();
    }

    /// The preset the settings match, if any (else "custom").
    pub fn detect(settings: &TorrentSettings) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.values() == (settings.max_connections, settings.speed_limit))
    }
}

/// Starts the engine (idempotent; a failed start can be retried).
pub async fn start() -> Result<(), String> {
    engine().await.map(|_| ())
}

/// Applies changed settings. The speed cap applies at once, the peer limit
/// to torrents opened from now on, the cache limits at the next cleanup.
pub fn configure(settings: &TorrentSettings) {
    *config().lock().unwrap() = settings.clone();
    if let Some(engine) = ENGINE.get() {
        engine.session.ratelimits.set_download_bps(speed_cap(settings));
    }
}

/// Registers the torrent and returns a local HTTP URL for the requested
/// file, or for its largest video file when `file_idx` is `None`.
pub async fn stream_url(
    info_hash: &str,
    file_idx: Option<u32>,
    trackers: &[String],
) -> Result<String, String> {
    let engine = engine().await?;
    let (hash, id) = parse_hash(info_hash)?;
    let wanted = file_idx.map(|i| i as usize);
    let handle = engine.torrent(&hash, id, trackers.to_vec(), wanted, None, false).await?;
    let files = file_list(&handle);
    let index = match wanted {
        Some(index) if index < files.len() => index,
        Some(_) => return Err("the torrent has no such file".into()),
        None => guess_video(&files).ok_or("the torrent has no video file")?,
    };
    engine.prepare(&handle, index, false).await?;
    engine.watch(&hash, &handle, index);
    let mut url = format!("{}/{hash}/{index}", engine.base);
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

/// Swarm statistics for a URL returned by [`stream_url`].
pub fn stats(url: &str) -> Option<TorrentStats> {
    let engine = ENGINE.get()?;
    let path = url.strip_prefix(&engine.base)?.split('?').next()?;
    let (hash, file) = path.trim_start_matches('/').split_once('/')?;
    let file: usize = file.parse().ok()?;
    let handle = engine.lookup(hash)?;
    let stats = handle.stats();
    let live = stats.live.as_ref();
    let file_len = file_list(&handle).get(file).map_or(0, |f| f.1);
    let done = stats.file_progress.get(file).copied().unwrap_or(0);
    Some(TorrentStats {
        peers: live.map_or(0, |l| u64::from(l.snapshot.peer_stats.live)),
        download_speed: live.map_or(0.0, |l| l.download_speed.as_bytes() as f64),
        progress: if file_len > 0 { (done as f64 / file_len as f64).clamp(0.0, 1.0) } else { 0.0 },
        ranges: have_ranges(engine, &handle, file),
    })
}

/// Which parts of one file of a torrent are on disk, as merged
/// (start, end) fractions of the file.
fn have_ranges(engine: &Engine, handle: &Handle, file: usize) -> Vec<(f32, f32)> {
    let Some(metadata) = handle.metadata.load_full() else { return Vec::new() };
    let Some(info) = metadata.file_infos.get(file) else { return Vec::new() };
    if info.len == 0 {
        return Vec::new();
    }
    let api = librqbit::Api::new(engine.session.clone(), None);
    let Ok((haves, total)) = api.api_dump_haves(TorrentIdOrHash::Id(handle.id())) else {
        return Vec::new();
    };
    let piece_len = u64::from(metadata.lengths().default_piece_length());
    let (file_start, file_len) = (info.offset_in_torrent, info.len);
    let mut ranges: Vec<(f32, f32)> = Vec::new();
    for piece in info.piece_range.clone() {
        if piece >= total || !haves.get(piece as usize).is_some_and(|bit| *bit) {
            continue;
        }
        let start = (u64::from(piece) * piece_len).max(file_start) - file_start;
        let end = ((u64::from(piece) + 1) * piece_len).min(file_start + file_len) - file_start;
        let (a, b) = (start as f32 / file_len as f32, end as f32 / file_len as f32);
        match ranges.last_mut() {
            Some(last) if a - last.1 < 0.0005 => last.1 = b,
            _ => ranges.push((a, b)),
        }
    }
    ranges
}

/// Where torrent data is cached.
pub fn cache_folder() -> PathBuf {
    crate::util::cache_dir().join("torrents")
}

/// Deletes every cached torrent that is not being streamed or downloaded
/// right now, and returns the disk space freed.
pub async fn clear_cache() -> Result<u64, String> {
    match ENGINE.get() {
        Some(engine) => {
            let busy = engine.busy();
            let root = engine.root.clone();
            let victims = tokio::task::spawn_blocking(move || {
                scan_cache(&root).into_iter().filter(|f| !busy.contains(&f.hash)).collect()
            })
            .await
            .map_err(|e| e.to_string())?;
            Ok(engine.evict(victims).await)
        }
        None => tokio::task::spawn_blocking(|| {
            scan_cache(&cache_folder())
                .into_iter()
                .filter(|f| remove_folder(&f.path))
                .map(|f| f.size)
                .sum()
        })
        .await
        .map_err(|e| e.to_string()),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: u64,
    /// Bytes per second.
    pub speed: f64,
}

/// A torrent file downloaded in full by [`download_file`].
pub struct Downloaded {
    pub path: PathBuf,
    /// The file's name inside the torrent.
    pub name: String,
}

/// Downloads one file of a torrent completely, fetching pieces in whatever
/// order the swarm serves fastest, and returns where it is on disk.
///
/// A torrent that is not open yet is written to `staging/<hash>`, a folder
/// the caller keeps on the same drive as the final file so it can be
/// hard-linked there instead of copied. If the torrent is already open for
/// streaming, or the cache still has it, that copy is completed instead.
/// Call [`finish_download`] once the file has been saved elsewhere.
pub async fn download_file(
    info_hash: &str,
    file_idx: Option<u32>,
    trackers: &[String],
    staging: &FsPath,
    cancel: &AtomicBool,
    progress: impl Fn(DownloadProgress),
) -> Result<Downloaded, String> {
    let engine = engine().await?;
    let (hash, id) = parse_hash(info_hash)?;
    let cached = engine.root.join(&hash);
    let folder = if cached.is_dir() { cached } else { staging.join(&hash) };
    let wanted = file_idx.map(|i| i as usize);
    let handle = engine
        .torrent(&hash, id, trackers.to_vec(), wanted, Some(folder), true)
        .await?;
    let files = file_list(&handle);
    let index = match wanted {
        Some(index) if index < files.len() => index,
        Some(_) => return Err("the torrent has no such file".into()),
        None => guess_video(&files).ok_or("the torrent has no video file")?,
    };
    let (name, total, _) = files[index].clone();
    engine.prepare(&handle, index, true).await?;
    let Some((active, last_used)) = engine.counters(&hash) else {
        return Err("the torrent closed unexpectedly".into());
    };
    let _hold = Hold::new(active, last_used);
    loop {
        if cancel.load(Ordering::Acquire) {
            // Back to streaming only: deselect the file so a player still
            // using it fetches just what it reads, and stop altogether if
            // nobody is. Pieces so far stay on disk for Resume.
            engine.deselect(&handle, index).await;
            if _hold.active.load(Ordering::SeqCst) == 1 {
                let _ = engine.session.pause(&handle).await;
            }
            return Err("cancelled".into());
        }
        let stats = handle.stats();
        if let Some(error) = stats.error {
            return Err(format!("torrent error: {error}"));
        }
        let received = stats.file_progress.get(index).copied().unwrap_or(0);
        let speed = stats.live.as_ref().map_or(0.0, |l| l.download_speed.as_bytes() as f64);
        progress(DownloadProgress { received, total, speed });
        if received >= total {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let path = handle.output_folder().join(
        handle
            .metadata
            .load_full()
            .and_then(|m| m.file_infos.get(index).map(|f| f.relative_filename.clone()))
            .ok_or("the torrent lost its file list")?,
    );
    let name = name.rsplit('/').next().unwrap_or(&name).to_owned();
    Ok(Downloaded { path, name })
}

/// The download of `info_hash` has been saved (or deleted): its staging
/// copy can go once nothing streams from it any more.
pub fn finish_download(info_hash: &str, staging: &FsPath) {
    let hash = info_hash.to_ascii_lowercase();
    let open = ENGINE.get().and_then(|engine| {
        let torrents = engine.torrents.lock().unwrap();
        torrents.get(&hash).map(|e| e.discard.store(true, Ordering::SeqCst))
    });
    if open.is_none() {
        let _ = std::fs::remove_dir_all(staging.join(&hash));
    }
}

/// Counts a running download (or open stream) against its torrent.
struct Hold {
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
}

impl Hold {
    fn new(active: Arc<AtomicUsize>, last_used: Arc<Mutex<Instant>>) -> Self {
        active.fetch_add(1, Ordering::SeqCst);
        Self { active, last_used }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        *self.last_used.lock().unwrap() = Instant::now();
    }
}

// ------------------------------------------------------------- engine

static ENGINE: tokio::sync::OnceCell<Arc<Engine>> = tokio::sync::OnceCell::const_new();

fn config() -> &'static Mutex<TorrentSettings> {
    static CONFIG: OnceLock<Mutex<TorrentSettings>> = OnceLock::new();
    CONFIG.get_or_init(|| Mutex::new(TorrentSettings::default()))
}

async fn engine() -> Result<&'static Arc<Engine>, String> {
    ENGINE.get_or_try_init(launch).await
}

struct Entry {
    handle: Handle,
    /// Open HTTP responses and running downloads using this torrent.
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
    /// Set for torrents a download added: they are written to a staging
    /// folder next to the downloads instead of the cache.
    staging: Option<PathBuf>,
    /// Delete `staging` once the torrent is closed (its download finished
    /// or was deleted).
    discard: Arc<AtomicBool>,
    /// Read-ahead for each file being streamed.
    watchers: HashMap<usize, Watcher>,
}

struct Watcher {
    readahead: Arc<Readahead>,
    task: tokio::task::JoinHandle<()>,
}

impl Entry {
    /// Stops the read-ahead tasks and waits until they have let go of the
    /// torrent's files (Windows cannot delete open files).
    async fn stop_watchers(&mut self) {
        for (_, watcher) in self.watchers.drain() {
            watcher.readahead.stop.store(true, Ordering::SeqCst);
            let _ = tokio::time::timeout(Duration::from_secs(3), watcher.task).await;
        }
    }
}

struct Engine {
    session: Arc<Session>,
    /// One sub-folder per torrent, named by info hash.
    root: PathBuf,
    /// `http://127.0.0.1:<port>/<token>`.
    base: String,
    torrents: Mutex<HashMap<String, Entry>>,
    /// Serialises adding the same torrent from concurrent requests.
    adding: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// 128 random bits from the OS-seeded hasher keys, as hex.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos());
        hasher.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

async fn launch() -> Result<Arc<Engine>, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("could not open a local port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = random_token();

    let root = cache_folder();
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create the torrent cache: {e}"))?;
    let settings = config().lock().unwrap().clone();
    let options = |persist: bool| SessionOptions {
        dht: Some(DhtSessionConfig {
            persistence: persist.then(|| DhtPersistenceConfig {
                config_filename: Some(crate::util::cache_dir().join("dht.json")),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ratelimits: LimitsConfig {
            download_bps: speed_cap(&settings),
            ..Default::default()
        },
        // librqbit's blocking-I/O permits (default 8) are also held by every
        // open file stream for its whole life. A player opens up to five at
        // once (probe, video, audio, subtitles, read-ahead) and more briefly
        // during a seek, which left too few for writing received pieces.
        runtime_worker_threads: Some(32),
        ..Default::default()
    };
    // The saved DHT state pins its UDP port; a second running instance
    // cannot bind it, so that one starts a fresh, unsaved DHT instead.
    let session = match Session::new_with_opts(root.clone(), options(true)).await {
        Ok(session) => session,
        Err(_) => Session::new_with_opts(root.clone(), options(false))
            .await
            .map_err(|e| format!("could not start the torrent engine: {e:#}"))?,
    };

    let engine = Arc::new(Engine {
        session,
        root,
        base: format!("http://127.0.0.1:{port}/{token}"),
        torrents: Mutex::new(HashMap::new()),
        adding: Mutex::new(HashMap::new()),
    });

    let router = Router::new()
        .route(&format!("/{token}/{{hash}}/{{file}}"), get(serve_file))
        .with_state(engine.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let housekeeper = engine.clone();
    tokio::spawn(async move {
        loop {
            housekeeper.housekeeping().await;
            tokio::time::sleep(HOUSEKEEPING_EVERY).await;
        }
    });
    Ok(engine)
}

fn speed_cap(settings: &TorrentSettings) -> Option<NonZeroU32> {
    settings
        .speed_limit
        .and_then(|bps| NonZeroU32::new(bps.min(u64::from(u32::MAX)) as u32))
}

fn parse_hash(hash: &str) -> Result<(String, Id20), String> {
    let hash = hash.to_ascii_lowercase();
    if hash.len() != 40 {
        return Err("expected a 40-character info hash".into());
    }
    Id20::from_str(&hash).map(|id| (hash, id)).map_err(|_| "invalid info hash".into())
}

impl Engine {
    fn lookup(&self, hash: &str) -> Option<Handle> {
        let torrents = self.torrents.lock().unwrap();
        let entry = torrents.get(&hash.to_ascii_lowercase())?;
        *entry.last_used.lock().unwrap() = Instant::now();
        Some(entry.handle.clone())
    }

    /// The torrent for `hash`, adding it (and resolving metadata from
    /// peers) if needed.
    ///
    /// For streaming no file is selected, so librqbit fetches only the
    /// pieces around what is being read (it serves streams of unselected
    /// files). A selected file is downloaded whole, in whatever order peers
    /// offer, which is right for a download but competes with playback and
    /// fills the cache. A download without a file in mind starts paused
    /// until [`Engine::prepare`] picks one.
    async fn torrent(
        &self,
        hash: &str,
        id: Id20,
        trackers: Vec<String>,
        file: Option<usize>,
        folder: Option<PathBuf>,
        download: bool,
    ) -> Result<Handle, String> {
        if let Some(handle) = self.lookup(hash) {
            return Ok(handle);
        }
        let lock = self.adding.lock().unwrap().entry(hash.to_owned()).or_default().clone();
        let _adding = lock.lock().await;
        if let Some(handle) = self.lookup(hash) {
            return Ok(handle);
        }
        let folder = folder.unwrap_or_else(|| self.root.join(hash));
        let staging = (!folder.starts_with(&self.root)).then(|| folder.clone());
        if let Some(handle) = self.session.get(TorrentIdOrHash::Hash(id)) {
            return Ok(self.remember(hash, handle, staging));
        }
        let settings = config().lock().unwrap().clone();
        let options = AddTorrentOptions {
            paused: download && file.is_none(),
            only_files: Some(if download { file.into_iter().collect() } else { Vec::new() }),
            // Reuse pieces already on disk from an earlier session.
            overwrite: true,
            output_folder: Some(folder.to_string_lossy().into_owned()),
            trackers: (!trackers.is_empty()).then_some(trackers),
            peer_limit: (settings.max_connections > 0).then_some(settings.max_connections as usize),
            ..Default::default()
        };
        let magnet = format!("magnet:?xt=urn:btih:{hash}");
        let added = tokio::time::timeout(
            METADATA_TIMEOUT,
            self.session.add_torrent(AddTorrent::from_url(magnet), Some(options)),
        )
        .await
        .map_err(|_| "no peers sent the torrent's details in time".to_owned())?
        .map_err(|e| format!("could not open the torrent: {e:#}"))?;
        let handle = added.into_handle().ok_or("the torrent was not added")?;
        Ok(self.remember(hash, handle, staging))
    }

    fn remember(&self, hash: &str, handle: Handle, staging: Option<PathBuf>) -> Handle {
        if staging.is_none() {
            mark_used(&self.root.join(hash));
        }
        self.torrents.lock().unwrap().insert(
            hash.to_owned(),
            Entry {
                handle: handle.clone(),
                active: Arc::new(AtomicUsize::new(0)),
                last_used: Arc::new(Mutex::new(Instant::now())),
                staging,
                discard: Arc::new(AtomicBool::new(false)),
                watchers: HashMap::new(),
            },
        );
        handle
    }

    /// Makes sure the torrent is running, and for a download that `file` is
    /// selected so all of it comes in.
    async fn prepare(&self, handle: &Handle, file: usize, download: bool) -> Result<(), String> {
        handle.wait_until_initialized().await.map_err(|e| format!("{e:#}"))?;
        let current = handle.only_files();
        if download && current.as_ref().is_none_or(|files| !files.contains(&file)) {
            let mut files: HashSet<usize> = current.unwrap_or_default().into_iter().collect();
            files.insert(file);
            self.session
                .update_only_files(handle, &files)
                .await
                .map_err(|e| format!("{e:#}"))?;
        }
        if handle.is_paused() {
            self.session.unpause(handle).await.map_err(|e| format!("{e:#}"))?;
        }
        Ok(())
    }

    /// Stops downloading `file` as a whole (streams of it keep working).
    async fn deselect(&self, handle: &Handle, file: usize) {
        let Some(current) = handle.only_files() else { return };
        if !current.contains(&file) {
            return;
        }
        let files: HashSet<usize> = current.into_iter().filter(|&f| f != file).collect();
        let _ = self.session.update_only_files(handle, &files).await;
    }

    /// Starts read-ahead for `file` of `hash`, once.
    fn watch(&self, hash: &str, handle: &Handle, file: usize) -> Option<Arc<Readahead>> {
        let mut torrents = self.torrents.lock().unwrap();
        let entry = torrents.get_mut(hash)?;
        if let Some(watcher) = entry.watchers.get(&file) {
            return Some(watcher.readahead.clone());
        }
        let len = file_list(handle).get(file)?.1;
        let readahead = Arc::new(Readahead::default());
        let task = tokio::spawn(run_readahead(readahead.clone(), handle.clone(), file, len));
        entry.watchers.insert(file, Watcher { readahead: readahead.clone(), task });
        Some(readahead)
    }

    /// Stops a torrent and forgets it. Its files stay on disk.
    async fn close(&self, mut entry: Entry) {
        entry.stop_watchers().await;
        let _ = self.session.delete(TorrentIdOrHash::Id(entry.handle.id()), false).await;
    }

    fn counters(&self, hash: &str) -> Option<(Arc<AtomicUsize>, Arc<Mutex<Instant>>)> {
        let torrents = self.torrents.lock().unwrap();
        let entry = torrents.get(hash)?;
        Some((entry.active.clone(), entry.last_used.clone()))
    }

    /// Stops torrents nobody is watching and trims the cache to its limit.
    async fn housekeeping(&self) {
        let idle: Vec<Entry> = {
            let mut torrents = self.torrents.lock().unwrap();
            let hashes: Vec<String> = torrents
                .iter()
                .filter(|(_, e)| {
                    let unused = e.active.load(Ordering::SeqCst) == 0;
                    // A finished download's staging copy goes right away.
                    let expired = e.discard.load(Ordering::SeqCst)
                        || e.last_used.lock().unwrap().elapsed() > IDLE_AFTER;
                    unused && expired
                })
                .map(|(hash, _)| hash.clone())
                .collect();
            hashes.iter().filter_map(|h| torrents.remove(h)).collect()
        };
        for entry in idle {
            // Keep the files: in the cache they speed up replays, in a
            // staging folder they let an unfinished download resume.
            let discard = entry.staging.clone().filter(|_| entry.discard.load(Ordering::SeqCst));
            self.close(entry).await;
            if let Some(folder) = discard {
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(folder)).await;
            }
        }
        // Torrents being read or downloaded right now are protected, and
        // count as just used. A torrent that is merely still open (closed
        // less than `IDLE_AFTER` ago) is shut first if its folder has to go.
        let busy = self.busy();
        for hash in &busy {
            mark_used(&self.root.join(hash));
        }
        let (limit, max_age) = {
            let config = config().lock().unwrap();
            (config.cache_size, config.cache_max_age_days)
        };
        if limit.is_none() && max_age.is_none() {
            return;
        }
        let open: HashSet<String> = self.torrents.lock().unwrap().keys().cloned().collect();
        let root = self.root.clone();
        let victims = tokio::task::spawn_blocking(move || {
            cache_victims(scan_cache(&root), limit, max_age, &busy, &open)
        })
        .await
        .unwrap_or_default();
        self.evict(victims).await;
    }

    /// Hashes of the torrents being read or downloaded right now.
    fn busy(&self) -> HashSet<String> {
        self.torrents
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, e)| e.active.load(Ordering::SeqCst) > 0)
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    /// Closes and deletes cached torrents, skipping any that started being
    /// read since they were chosen, and returns the disk space freed. A
    /// folder that cannot be deleted yet (a file still open) is chosen
    /// again, and retried, at the next cleanup.
    async fn evict(&self, victims: Vec<CachedTorrent>) -> u64 {
        let mut freed = 0;
        for victim in victims {
            let open = {
                let mut torrents = self.torrents.lock().unwrap();
                match torrents.get(&victim.hash) {
                    Some(e) if e.active.load(Ordering::SeqCst) > 0 => continue,
                    Some(_) => torrents.remove(&victim.hash),
                    None => None,
                }
            };
            if let Some(entry) = open {
                self.close(entry).await;
            }
            let path = victim.path.clone();
            if tokio::task::spawn_blocking(move || remove_folder(&path)).await.unwrap_or(false) {
                freed += victim.size;
            }
        }
        freed
    }
}

/// Deletes a torrent folder. Windows refuses while a just-closed file is
/// still being released, so a failed attempt is retried briefly.
fn remove_folder(path: &FsPath) -> bool {
    for attempt in 0..3 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) if attempt < 2 => std::thread::sleep(Duration::from_millis(500)),
            Err(_) => {}
        }
    }
    false
}

/// Records that a cached torrent was just used (see `USED_MARKER`).
fn mark_used(folder: &FsPath) {
    if std::fs::create_dir_all(folder).is_ok() {
        let _ = std::fs::File::create(folder.join(USED_MARKER));
    }
}

/// One torrent folder in the cache.
struct CachedTorrent {
    hash: String,
    path: PathBuf,
    /// Space it takes on disk.
    size: u64,
    /// When it was last played (without a marker: last written).
    last_used: SystemTime,
}

/// Every torrent folder in the cache.
fn scan_cache(root: &FsPath) -> Vec<CachedTorrent> {
    fn walk(path: &FsPath, size: &mut u64, newest: &mut SystemTime) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), size, newest);
            } else {
                *size += size_on_disk(&entry.path(), meta.len());
                if let Ok(modified) = meta.modified() {
                    *newest = (*newest).max(modified);
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let (mut size, mut last_used) = (0, SystemTime::UNIX_EPOCH);
            walk(&e.path(), &mut size, &mut last_used);
            let hash = e.file_name().to_string_lossy().to_ascii_lowercase();
            CachedTorrent { hash, path: e.path(), size, last_used }
        })
        .collect()
}

/// The cached torrents to delete:
/// - with a maximum age of 0 days, every one that is no longer open;
/// - otherwise every one not played for longer than the maximum age;
/// - then the least recently used ones until the rest fits in `limit`.
///
/// `busy` torrents are never chosen, so a busy torrent bigger than the limit
/// can only go once it is closed: its data is in use.
fn cache_victims(
    mut cached: Vec<CachedTorrent>,
    limit: Option<u64>,
    max_age_days: Option<u32>,
    busy: &HashSet<String>,
    open: &HashSet<String>,
) -> Vec<CachedTorrent> {
    let now = SystemTime::now();
    let expired = |t: &CachedTorrent| match max_age_days {
        None => false,
        Some(0) => !open.contains(&t.hash),
        Some(days) => now.duration_since(t.last_used).is_ok_and(|age| age > DAY * days),
    };
    let mut total: u64 = cached.iter().map(|t| t.size).sum();
    cached.sort_by_key(|t| t.last_used);
    let mut victims = Vec::new();
    for torrent in cached {
        if busy.contains(&torrent.hash) {
            continue;
        }
        if expired(&torrent) || limit.is_some_and(|limit| total > limit) {
            total = total.saturating_sub(torrent.size);
            victims.push(torrent);
        }
    }
    victims
}

/// The space a file takes on disk. Torrent files are sparse (librqbit marks
/// them so on Windows) and their length is the full size from the start, so
/// a 17 GB film streamed for ten minutes is 17 GB long but holds far less.
#[cfg(windows)]
fn size_on_disk(path: &FsPath, len: u64) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{GetLastError, NO_ERROR};
    use windows::Win32::Storage::FileSystem::{GetCompressedFileSizeW, INVALID_FILE_SIZE};
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut high = 0u32;
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call,
    // and `high` is a valid out pointer.
    let low = unsafe { GetCompressedFileSizeW(PCWSTR(wide.as_ptr()), Some(&mut high)) };
    // INVALID_FILE_SIZE is also a valid low half; only the last error tells.
    #[allow(unused_unsafe)]
    if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != NO_ERROR {
        return len;
    }
    (u64::from(high) << 32) | u64::from(low)
}

#[cfg(unix)]
fn size_on_disk(path: &FsPath, len: u64) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map_or(len, |m| m.blocks() * 512)
}

#[cfg(not(any(windows, unix)))]
fn size_on_disk(_path: &FsPath, len: u64) -> u64 {
    len
}

// ------------------------------------------------------------- files

/// (name with `/` separators, length, is padding) for every file.
fn file_list(handle: &Handle) -> Vec<(String, u64, bool)> {
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

fn guess_video(files: &[(String, u64, bool)]) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, (name, _, padding))| {
            let lower = name.to_ascii_lowercase();
            !padding && VIDEO_EXTENSIONS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
        })
        .max_by_key(|(_, (_, len, _))| *len)
        .map(|(i, _)| i)
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
        _ => "application/octet-stream",
    }
}

// ------------------------------------------------------------- HTTP

fn fail(status: StatusCode, message: String) -> Response {
    (status, message).into_response()
}

/// Repeated `tr=` query parameters, as [`stream_url`] appends them.
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
/// Also reports where it is reading to the file's read-ahead, and when it
/// is starved of pieces.
struct Tracked {
    inner: Box<dyn AsyncRead + Send + Unpin>,
    active: Arc<AtomicUsize>,
    last_used: Arc<Mutex<Instant>>,
    readahead: Option<Arc<Readahead>>,
    id: u64,
    position: u64,
    opened: Instant,
    sent: u64,
    /// Since when the current read has been waiting for a piece.
    waiting: Option<Instant>,
}

impl AsyncRead for Tracked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        let read = (buf.filled().len() - before) as u64;
        let this = &mut *self;
        match &polled {
            Poll::Pending => {
                let since = *this.waiting.get_or_insert_with(Instant::now);
                if since.elapsed() >= STARVING_AFTER
                    && let Some(readahead) = &this.readahead
                {
                    readahead.note_starving();
                }
            }
            Poll::Ready(_) => {
                this.waiting = None;
                this.position += read;
                this.sent += read;
                if let Some(readahead) = &this.readahead {
                    readahead.note_reader(this.id, this.position, this.sent, this.opened);
                }
            }
        }
        polled
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        *self.last_used.lock().unwrap() = Instant::now();
        if let Some(readahead) = &self.readahead {
            readahead.readers.lock().unwrap().remove(&self.id);
        }
    }
}

// ------------------------------------------------------------- read-ahead

/// Read-ahead for one streamed file.
///
/// The file is not selected for download (see [`Engine::torrent`]), so
/// librqbit fetches only `STREAM_WINDOW` past each open stream. That is a
/// few seconds of a 4K remux. Player responses are `Playback` streams; this
/// keeps one `Background` stream of its own whose window covers the stretch
/// after theirs, up to a size based on the file (`lookahead`). Background
/// pieces are only requested once no playback piece is missing (our patched
/// librqbit, `patches/librqbit`), so read-ahead never delays playback.
///
/// The stream stays open the whole time, so the torrent always has one.
/// With none, librqbit treats a torrent with nothing selected as finished:
/// peers pause for five seconds and seeds are dropped, which happens in the
/// gap between the probe and the decoders. While nothing plays its window is
/// the file's last piece, which MKV and MP4 seeking usually need.
#[derive(Default)]
struct Readahead {
    /// Open responses: id -> (position, bytes sent, opened at).
    readers: Mutex<HashMap<u64, (u64, u64, Instant)>>,
    starved_at: Mutex<Option<Instant>>,
    stop: AtomicBool,
}

impl Readahead {
    fn note_reader(&self, id: u64, position: u64, sent: u64, opened: Instant) {
        self.readers.lock().unwrap().insert(id, (position, sent, opened));
    }

    fn note_starving(&self) {
        *self.starved_at.lock().unwrap() = Some(Instant::now());
    }

    /// The furthest position of a response that is playing (not a probe).
    fn playhead(&self) -> Option<u64> {
        self.readers
            .lock()
            .unwrap()
            .values()
            .filter(|(_, sent, opened)| *sent >= PLAYING_BYTES && opened.elapsed() >= PLAYING_AFTER)
            .map(|(position, _, _)| *position)
            .max()
    }

    fn starving(&self) -> bool {
        self.starved_at
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < STARVED_COOLDOWN)
    }
}

/// How far ahead of playback to fetch: about 2% of the file, so roughly the
/// same play time for any quality (a 2 h film: about 2.5 minutes).
fn lookahead(len: u64) -> u64 {
    (len / 40).clamp(64 * 1024 * 1024, 512 * 1024 * 1024)
}

async fn run_readahead(readahead: Arc<Readahead>, handle: Handle, file: usize, len: u64) {
    let Ok(mut stream) = handle.clone().stream(file).await else { return };
    stream.set_priority(StreamPriority::Background);
    let ahead = lookahead(len);
    let park = |stream: &mut FileStream| {
        stream.set_window(1);
        stream.set_position_now(len.saturating_sub(1));
    };
    park(&mut stream);
    while !readahead.stop.load(Ordering::SeqCst) {
        // While playback waits for a piece, even background requests take
        // peers' upload slots away from it: park the window.
        match readahead.playhead().filter(|_| !readahead.starving()) {
            // librqbit already fetches the first window past the playhead.
            Some(playhead) if playhead + STREAM_WINDOW < len => {
                let from = playhead + STREAM_WINDOW;
                stream.set_position_now(from);
                stream.set_window((playhead + ahead).min(len) - from);
            }
            _ => park(&mut stream),
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn next_reader_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

async fn serve_file(
    State(engine): State<Arc<Engine>>,
    Path((hash, file)): Path<(String, usize)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let (hash, id) = match parse_hash(&hash) {
        Ok(parsed) => parsed,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error),
    };
    // The torrent may have been stopped while idle (e.g. a long pause):
    // reopen it with the trackers carried in the URL.
    let trackers = trackers_from_query(query.as_deref());
    let handle = match engine.torrent(&hash, id, trackers, Some(file), None, false).await {
        Ok(handle) => handle,
        Err(error) => return fail(StatusCode::BAD_GATEWAY, error),
    };
    let files = file_list(&handle);
    let Some((name, _, _)) = files.get(file) else {
        return fail(StatusCode::NOT_FOUND, "the torrent has no such file".into());
    };
    if let Err(error) = engine.prepare(&handle, file, false).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, error);
    }
    let readahead = engine.watch(&hash, &handle, file);
    let Some((active, last_used)) = engine.counters(&hash) else {
        return fail(StatusCode::NOT_FOUND, "the torrent is no longer open".into());
    };
    let mut reader = match handle.clone().stream(file).await {
        Ok(reader) => reader,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    };
    // A player is waiting on this response: its pieces come before any
    // read-ahead.
    reader.set_priority(StreamPriority::Playback);
    let len = reader.len();

    let mut out = HeaderMap::new();
    out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime_type(name)));
    let mut start = 0;
    let (status, body): (StatusCode, Box<dyn AsyncRead + Send + Unpin>) =
        match parse_range(headers.get(header::RANGE), len) {
            Some(Err(())) => {
                out.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{len}")).unwrap(),
                );
                return (StatusCode::RANGE_NOT_SATISFIABLE, out).into_response();
            }
            Some(Ok((from, end))) => {
                start = from;
                if let Err(e) = reader.seek(SeekFrom::Start(start)).await {
                    return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("seek failed: {e}"));
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
    let tracked = Tracked {
        inner: body,
        active,
        last_used,
        readahead,
        id: next_reader_id(),
        position: start,
        opened: Instant::now(),
        sent: 0,
        waiting: None,
    };
    let chunks = tokio_util::io::ReaderStream::with_capacity(tracked, 64 * 1024);
    (status, out, Body::from_stream(chunks)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(hash: &str, size: u64, days_ago: u32) -> CachedTorrent {
        CachedTorrent {
            hash: hash.to_owned(),
            path: PathBuf::from(hash),
            size,
            last_used: SystemTime::now() - DAY * days_ago,
        }
    }

    fn victims(
        torrents: Vec<CachedTorrent>,
        limit: Option<u64>,
        max_age: Option<u32>,
        busy: &[&str],
        open: &[&str],
    ) -> Vec<String> {
        let set = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<HashSet<_>>();
        cache_victims(torrents, limit, max_age, &set(busy), &set(open))
            .into_iter()
            .map(|t| t.hash)
            .collect()
    }

    #[test]
    fn size_limit_evicts_least_recently_used() {
        let all = vec![cached("new", 40, 0), cached("old", 40, 2), cached("mid", 40, 1)];
        assert_eq!(victims(all, Some(80), None, &[], &[]), ["old"]);
    }

    #[test]
    fn max_age_evicts_stale_torrents_under_the_size_limit() {
        let all = vec![cached("fresh", 1, 1), cached("stale", 1, 8)];
        assert_eq!(victims(all, Some(1000), Some(7), &[], &[]), ["stale"]);
    }

    #[test]
    fn zero_days_keeps_only_open_torrents() {
        let all = vec![cached("open", 1, 0), cached("closed", 1, 0)];
        assert_eq!(victims(all, None, Some(0), &[], &["open"]), ["closed"]);
    }

    #[test]
    fn busy_torrents_are_never_evicted() {
        let all = vec![cached("busy", 100, 30), cached("idle", 10, 0)];
        assert_eq!(victims(all, Some(50), Some(7), &["busy"], &["busy"]), ["idle"]);
    }

    #[test]
    fn no_limits_keep_everything() {
        let all = vec![cached("a", 1 << 40, 365)];
        assert!(victims(all, None, None, &[], &[]).is_empty());
    }
}
