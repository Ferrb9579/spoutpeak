//! Saving streams to disk. Direct links are fetched as they are; torrents go
//! through the streaming server, which turns them into a plain HTTP file.
//! Partial files keep a `.part` extension and resume with a Range request.

use crate::state::{App, spawn, ui};
use crate::media::addon::http;
use crate::media::types::{Meta, Stream, StreamSource, Video};
use crate::util::data_dir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DownloadStatus {
    /// Waiting for the streaming server to resolve a torrent.
    Preparing,
    Downloading,
    Done,
    Failed(String),
    Cancelled,
}

impl DownloadStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Preparing | Self::Downloading)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Download {
    pub id: u64,
    /// "Show - S1E2 Title" or the movie name.
    pub title: String,
    /// The stream's own name (quality, addon tag).
    pub source: String,
    pub poster: Option<String>,
    pub stream: Stream,
    /// Name without extension; the extension is known once the server
    /// answers.
    pub stem: String,
    /// The finished file, once there is one.
    pub file: Option<PathBuf>,
    pub received: u64,
    pub total: Option<u64>,
    pub status: DownloadStatus,
    #[serde(skip)]
    pub speed: f64,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

impl Download {
    pub fn fraction(&self) -> Option<f32> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.received as f64 / t as f64).clamp(0.0, 1.0) as f32)
    }
}

static CUSTOM_FOLDER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Uses `dir` for new downloads; empty restores the default.
pub fn set_custom_folder(dir: &str) {
    *CUSTOM_FOLDER.lock().unwrap() = dir.trim().to_owned();
}

/// The chosen folder, else `<Downloads>/Spoutpeak`.
pub fn folder() -> PathBuf {
    let custom = CUSTOM_FOLDER.lock().unwrap().clone();
    let dir = if custom.is_empty() {
        dirs::download_dir()
            .unwrap_or_else(|| data_dir().join("downloads"))
            .join("Spoutpeak")
    } else {
        PathBuf::from(custom)
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn part_path(stem: &str) -> PathBuf {
    folder().join(format!("{stem}.part"))
}

fn list_path() -> PathBuf {
    data_dir().join("downloads.json")
}

pub fn load() -> Vec<Download> {
    let mut list: Vec<Download> = std::fs::read(list_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    for item in &mut list {
        // The app closed mid-download; the .part file lets Retry resume.
        if item.status.is_active() {
            item.status = DownloadStatus::Failed("Interrupted".into());
        }
    }
    list
}

fn save(list: &[Download]) {
    if let Ok(bytes) = serde_json::to_vec_pretty(list) {
        let path = list_path();
        std::thread::spawn(move || {
            let temp = path.with_extension("tmp");
            if std::fs::write(&temp, bytes).is_ok() {
                let _ = std::fs::rename(&temp, &path);
            }
        });
    }
}

/// Keeps characters every file system accepts.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(['.', ' ']).chars().take(120).collect::<String>();
    if trimmed.is_empty() { "Video".to_owned() } else { trimmed }
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "wmv", "flv", "mpg", "mpeg", "m2ts",
];

fn extension_of(name: &str) -> Option<String> {
    let name = name.split(['?', '#']).next().unwrap_or(name);
    let ext = Path::new(name).extension()?.to_str()?.to_lowercase();
    VIDEO_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

fn extension_for(stream: &Stream, url: &str, content_type: Option<&str>) -> String {
    stream
        .behavior_hints
        .filename
        .as_deref()
        .and_then(extension_of)
        .or_else(|| extension_of(url))
        .or_else(|| {
            let kind = content_type?.split(';').next()?.trim().to_lowercase();
            Some(
                match kind.as_str() {
                    "video/mp4" => "mp4",
                    "video/webm" => "webm",
                    "video/x-msvideo" => "avi",
                    "video/quicktime" => "mov",
                    "video/mp2t" => "ts",
                    _ => "mkv",
                }
                .to_owned(),
            )
        })
        .unwrap_or_else(|| "mkv".to_owned())
}

/// A free `<stem>.<ext>` in the downloads folder.
fn unique_file(stem: &str, ext: &str) -> PathBuf {
    let dir = folder();
    let mut candidate = dir.join(format!("{stem}.{ext}"));
    let mut number = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({number}).{ext}"));
        number += 1;
    }
    candidate
}

enum Progress {
    Started { total: Option<u64>, received: u64 },
    Tick { received: u64, speed: f64 },
}

/// Streams `url` into `part`, resuming when the file already has bytes and
/// the server honours ranges. Returns the response's content type.
async fn fetch(
    url: String,
    headers: Vec<(String, String)>,
    part: PathBuf,
    cancel: Arc<AtomicBool>,
    report: impl Fn(Progress) + Send + 'static,
) -> Result<Option<String>, String> {
    use tokio::io::AsyncWriteExt;

    if url.contains(".m3u8") {
        return Err("Playlist (HLS) streams can't be downloaded".into());
    }
    let existing = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    let mut request = http().get(&url).timeout(Duration::from_secs(60 * 60 * 24));
    for (key, value) in &headers {
        request = request.header(key, value);
    }
    if existing > 0 {
        request = request.header("Range", format!("bytes={existing}-"));
    }
    let mut response = request
        .send()
        .await
        .map_err(|e| format!("Could not connect: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Server answered HTTP {}", status.as_u16()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if content_type.as_deref().is_some_and(|t| t.contains("mpegurl")) {
        return Err("Playlist (HLS) streams can't be downloaded".into());
    }
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT && existing > 0;
    let start = if resumed { existing } else { 0 };
    let total = response.content_length().map(|length| length + start);
    let mut file = if resumed {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        tokio::fs::File::create(&part).await
    }
    .map_err(|e| format!("Could not write the file: {e}"))?;
    report(Progress::Started { total, received: start });

    let mut received = start;
    let mut window_bytes = 0u64;
    let mut window_start = Instant::now();
    let mut last_report = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            let _ = file.flush().await;
            return Err("cancelled".into());
        }
        let chunk = match tokio::time::timeout(Duration::from_secs(120), response.chunk()).await {
            Err(_) => return Err("The download stalled".into()),
            Ok(Err(e)) => return Err(format!("Connection lost: {e}")),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
        };
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Could not write the file: {e}"))?;
        received += chunk.len() as u64;
        window_bytes += chunk.len() as u64;
        if last_report.elapsed() >= Duration::from_millis(500) {
            let elapsed = window_start.elapsed().as_secs_f64().max(0.001);
            report(Progress::Tick { received, speed: window_bytes as f64 / elapsed });
            last_report = Instant::now();
            if elapsed > 3.0 {
                window_bytes = 0;
                window_start = Instant::now();
            }
        }
    }
    file.flush().await.map_err(|e| format!("Could not write the file: {e}"))?;
    if total.is_some_and(|t| received < t) {
        return Err("The connection closed early".into());
    }
    report(Progress::Tick { received, speed: 0.0 });
    Ok(content_type)
}

impl App {
    /// Starts saving `stream` for the title open in the detail view.
    pub fn download_stream(&self, stream: &Stream) {
        if !matches!(stream.source(), StreamSource::Url(_) | StreamSource::Torrent { .. }) {
            self.notify("This stream can't be downloaded");
            return;
        }
        let (meta, video) = self.detail.peek(|d| (d.best_meta(), d.video.clone()));
        let title = download_title(meta.as_ref(), video.as_ref(), stream);
        let duplicate = self.downloads.peek(|list| {
            list.iter()
                .any(|d| d.stream == *stream && d.status != DownloadStatus::Cancelled)
        });
        if duplicate {
            self.notify("Already in Downloads");
            return;
        }
        let stem = {
            let base = sanitize(&title);
            let taken = |s: &str| self.downloads.peek(|l| l.iter().any(|d| d.stem == s));
            let mut stem = base.clone();
            let mut number = 2;
            while taken(&stem) || part_path(&stem).exists() {
                stem = format!("{base} ({number})");
                number += 1;
            }
            stem
        };
        let id = self.downloads.peek(|l| l.iter().map(|d| d.id).max().unwrap_or(0) + 1);
        let item = Download {
            id,
            title,
            source: stream.headline().replace('\n', " "),
            poster: meta.as_ref().and_then(|m| m.poster.clone()),
            stream: stream.clone(),
            stem,
            file: None,
            received: 0,
            total: None,
            status: DownloadStatus::Preparing,
            speed: 0.0,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        self.downloads.write(|list| list.insert(0, item));
        self.save_downloads();
        self.notify("Download started. Follow it under Downloads.");
        self.run_download(id);
    }

    fn save_downloads(&self) {
        self.downloads.peek(|list| save(list));
    }

    fn update_download(&self, id: u64, change: impl FnOnce(&mut Download)) {
        self.downloads.write(|list| {
            if let Some(item) = list.iter_mut().find(|d| d.id == id) {
                change(item);
            }
        });
    }

    fn run_download(&self, id: u64) {
        let Some((stream, stem, cancel)) = self.downloads.peek(|l| {
            l.iter()
                .find(|d| d.id == id)
                .map(|d| (d.stream.clone(), d.stem.clone(), d.cancel.clone()))
        }) else {
            return;
        };
        let headers: Vec<(String, String)> = stream
            .behavior_hints
            .proxy_headers
            .request
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let part = part_path(&stem);
        let source = stream.source();
        spawn(
            async move {
                let url = match source {
                    StreamSource::Url(url) => url,
                    StreamSource::Torrent { info_hash, file_idx, trackers } => {
                        crate::streaming::stream_url(&info_hash, file_idx, &trackers).await?
                    }
                    _ => return Err("This stream can't be downloaded".to_owned()),
                };
                if cancel.load(Ordering::Acquire) {
                    return Err("cancelled".into());
                }
                let report = move |progress: Progress| {
                    ui(move |app| {
                        app.update_download(id, |d| match progress {
                            Progress::Started { total, received } => {
                                d.status = DownloadStatus::Downloading;
                                d.total = total;
                                d.received = received;
                            }
                            Progress::Tick { received, speed } => {
                                d.received = received;
                                d.speed = speed;
                            }
                        })
                    })
                };
                let content_type = fetch(url.clone(), headers, part.clone(), cancel, report).await?;
                Ok((part, url, content_type))
            },
            move |app, result| match result {
                Ok((part, url, content_type)) => {
                    let ext = extension_for(&stream, &url, content_type.as_deref());
                    let target = unique_file(&stem, &ext);
                    match std::fs::rename(&part, &target) {
                        Ok(()) => {
                            app.update_download(id, |d| {
                                d.status = DownloadStatus::Done;
                                d.file = Some(target);
                                d.speed = 0.0;
                            });
                            let title = app.downloads.peek(|l| {
                                l.iter().find(|d| d.id == id).map(|d| d.title.clone())
                            });
                            app.notify(format!("Downloaded {}", title.unwrap_or_default()));
                        }
                        Err(e) => app.update_download(id, |d| {
                            d.status = DownloadStatus::Failed(format!("Could not save: {e}"));
                        }),
                    }
                    app.save_downloads();
                }
                Err(error) => {
                    app.update_download(id, |d| {
                        d.speed = 0.0;
                        d.status = if error == "cancelled" {
                            DownloadStatus::Cancelled
                        } else {
                            DownloadStatus::Failed(error)
                        };
                    });
                    app.save_downloads();
                }
            },
        );
    }

    pub fn cancel_download(&self, id: u64) {
        self.downloads.peek(|l| {
            if let Some(item) = l.iter().find(|d| d.id == id) {
                item.cancel.store(true, Ordering::Release);
            }
        });
    }

    /// Restarts a failed or cancelled download, continuing its partial file.
    pub fn retry_download(&self, id: u64) {
        self.update_download(id, |d| {
            d.cancel = Arc::new(AtomicBool::new(false));
            d.status = DownloadStatus::Preparing;
            d.speed = 0.0;
        });
        self.save_downloads();
        self.run_download(id);
    }

    /// Forgets the entry and deletes its file (finished or partial).
    pub fn delete_download(&self, id: u64) {
        let removed = self.downloads.write(|list| {
            let index = list.iter().position(|d| d.id == id)?;
            Some(list.remove(index))
        });
        if let Some(item) = removed {
            item.cancel.store(true, Ordering::Release);
            let part = part_path(&item.stem);
            let file = item.file.clone();
            // Give a running task a moment to notice the cancel and close
            // its handle before the file goes.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(800));
                let _ = std::fs::remove_file(part);
                if let Some(file) = file {
                    let _ = std::fs::remove_file(file);
                }
            });
        }
        self.save_downloads();
    }

    pub fn play_download(&self, id: u64) {
        let file = self.downloads.peek(|l| {
            l.iter().find(|d| d.id == id).and_then(|d| d.file.clone())
        });
        match file {
            Some(file) if file.exists() => {
                self.play_url(file.to_string_lossy().into_owned());
            }
            _ => self.notify("The file is missing. It may have been moved or deleted."),
        }
    }

    pub fn reveal_download(&self, id: u64) {
        let file = self.downloads.peek(|l| {
            l.iter().find(|d| d.id == id).and_then(|d| d.file.clone())
        });
        let target = file
            .and_then(|f| f.parent().map(Path::to_path_buf))
            .unwrap_or_else(folder);
        if open::that_detached(&target).is_err() {
            self.notify("Could not open the folder");
        }
    }

    pub fn open_download_folder(&self) {
        if open::that_detached(folder()).is_err() {
            self.notify("Could not open the folder");
        }
    }
}

fn download_title(meta: Option<&Meta>, video: Option<&Video>, stream: &Stream) -> String {
    let name = meta
        .map(|m| m.title().to_owned())
        .or_else(|| stream.behavior_hints.filename.clone())
        .or_else(|| stream.name.clone())
        .unwrap_or_else(|| "Video".to_owned());
    match (meta, video) {
        (Some(meta), Some(video)) if meta.videos.len() > 1 => {
            format!("{name} - {}", video.label())
        }
        _ => name,
    }
}
