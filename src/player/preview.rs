//! Debounced, cancellable thumbnail decoding, separate from the playback engine.

use super::engine::{self, MediaInfo};
use crate::state::spawn;
use crate::util::hide_console;
use incular::prelude::{ImageHandle, Signal};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Read;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use std::time::{Duration, Instant};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const CACHE_SIZE: usize = 48;

#[derive(Clone, PartialEq)]
pub enum PreviewFrame { Idle, Loading, Ready(ImageHandle), Unavailable }

#[derive(Clone)]
struct Source {
    url: String,
    headers: Vec<(String, String)>,
    torrent: bool,
    info: Option<MediaInfo>,
}

#[derive(Clone)]
pub struct SeekPreview {
    pub frame: Signal<PreviewFrame>,
    source: Rc<RefCell<Option<Source>>>,
    wanted: Rc<Cell<Option<u64>>>,
    revision: Arc<AtomicU64>,
    gate: Arc<tokio::sync::Mutex<()>>,
    cache: Rc<RefCell<VecDeque<(u64, ImageHandle)>>>,
}

impl SeekPreview {
    pub fn new() -> Self {
        Self {
            frame: Signal::new(PreviewFrame::Idle),
            source: Rc::new(RefCell::new(None)), wanted: Rc::new(Cell::new(None)),
            revision: Arc::new(AtomicU64::new(0)),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            cache: Rc::new(RefCell::new(VecDeque::new())),
        }
    }

    pub fn clear(&self) {
        self.cancel();
        self.source.borrow_mut().take();
        self.cache.borrow_mut().clear();
    }

    pub fn configure(&self, url: String, headers: Vec<(String, String)>, torrent: bool) {
        self.clear();
        *self.source.borrow_mut() = Some(Source { url, headers, torrent, info: None });
    }

    pub fn set_info(&self, info: MediaInfo) {
        if let Some(source) = self.source.borrow_mut().as_mut() { source.info = Some(info); }
    }

    pub fn cancel(&self) {
        if self.wanted.take().is_some() {
            self.revision.fetch_add(1, Ordering::AcqRel);
        }
        self.frame.set(PreviewFrame::Idle);
    }

    pub fn request(&self, seconds: Option<f64>) {
        let Some(seconds) = seconds.filter(|s| s.is_finite() && *s >= 0.0) else {
            self.cancel(); return;
        };
        let Some(source) = self.source.borrow().clone() else { return };
        let Some(info) = &source.info else { return };
        if info.live || !info.has_video { self.frame.set(PreviewFrame::Unavailable); return; }
        let key = seconds.min((info.duration - 0.1).max(0.0)).floor() as u64;
        if self.wanted.get() == Some(key) { return; }
        self.wanted.set(Some(key));
        let ticket = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let mut cache = self.cache.borrow_mut();
        if let Some(index) = cache.iter().position(|(at, _)| *at == key) {
            let item = cache.remove(index).unwrap();
            self.frame.set(PreviewFrame::Ready(item.1.clone()));
            cache.push_back(item);
            return;
        }
        drop(cache);
        self.frame.set(PreviewFrame::Loading);
        let (revision, gate) = (self.revision.clone(), self.gate.clone());
        spawn(async move {
            tokio::time::sleep(Duration::from_millis(140)).await;
            if revision.load(Ordering::Acquire) != ticket { return None; }
            // Only one decoder per player, even while an old request cancels.
            let _guard = gate.lock().await;
            if revision.load(Ordering::Acquire) != ticket { return None; }
            tokio::task::spawn_blocking(move || decode(&source, key, &revision, ticket)).await.ok().flatten()
        }, move |app, image| app.player.preview.finish(ticket, key, image));
    }

    fn finish(&self, ticket: u64, key: u64, image: Option<ImageHandle>) {
        if self.revision.load(Ordering::Acquire) != ticket || self.wanted.get() != Some(key) { return; }
        if let Some(image) = image {
            let mut cache = self.cache.borrow_mut();
            if cache.len() >= CACHE_SIZE { cache.pop_front(); }
            cache.push_back((key, image.clone()));
            self.frame.set(PreviewFrame::Ready(image));
        } else {
            self.frame.set(PreviewFrame::Unavailable);
        }
    }
}

fn decode(source: &Source, seconds: u64, revision: &AtomicU64, ticket: u64) -> Option<ImageHandle> {
    if revision.load(Ordering::Acquire) != ticket { return None; }
    let (ffmpeg, _) = engine::tools()?;
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-threads", "1", "-filter_threads", "1"]);
    if source.url.starts_with("http://") || source.url.starts_with("https://") {
        command.args(["-rw_timeout", "1500000"]);
        let mut headers: String = source.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        if source.torrent { headers.push_str("X-Spoutpeak-Preview: 1\r\n"); }
        if !headers.is_empty() { command.args(["-headers", &headers]); }
    }
    command.args(["-ss", &seconds.to_string(), "-i", &source.url]);
    let info = source.info.as_ref()?;
    let stream = format!("0:{}", info.video_index);
    let ratio = if info.width > 0 && info.height > 0 { info.width as f32 / info.height as f32 } else { WIDTH as f32 / HEIGHT as f32 };
    let width = ((HEIGHT as f32 * ratio).min(WIDTH as f32) as u32 & !1).max(2);
    let height = ((WIDTH as f32 / ratio).min(HEIGHT as f32) as u32 & !1).max(2);
    let fps = if info.fps > 0.0 { info.fps.min(30.0) } else { 30.0 };
    // Share playback's HDR/Dolby Vision conversion; keep the entire frame.
    let filter = format!("{},pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2,setsar=1",
        engine::video_filter(info, width, height, fps, false));
    command.args(["-map", &stream, "-an", "-sn", "-dn", "-vf", &filter,
        "-frames:v", "1", "-threads", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    hide_console(&mut command);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let expected = (WIDTH * HEIGHT * 4) as usize;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(expected);
        stdout.take(expected as u64 + 1).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    });
    let started = Instant::now();
    let success = loop {
        if revision.load(Ordering::Acquire) != ticket || started.elapsed() > Duration::from_secs(3) {
            let _ = child.kill(); let _ = child.wait(); break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => { let _ = child.kill(); let _ = child.wait(); break false; }
        }
    };
    let bytes = reader.join().ok().flatten()?;
    if !success || bytes.len() != expected || revision.load(Ordering::Acquire) != ticket { return None; }
    ImageHandle::from_rgba8(WIDTH, HEIGHT, bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> ImageHandle { ImageHandle::from_rgba8(1, 1, vec![255; 4]).unwrap() }

    fn configured() -> SeekPreview {
        let preview = SeekPreview::new();
        preview.configure("example.mkv".into(), Vec::new(), false);
        preview.set_info(MediaInfo { duration: 120.0, has_video: true, ..Default::default() });
        preview
    }

    #[test]
    fn stale_frames_cannot_replace_a_new_hover_or_a_new_video() {
        let preview = configured();
        preview.request(Some(10.0));
        let old = preview.revision.load(Ordering::Acquire);
        preview.request(Some(20.0));
        preview.finish(old, 10, Some(image()));
        assert!(matches!(preview.frame.get_untracked(), PreviewFrame::Loading));
        let current = preview.revision.load(Ordering::Acquire);
        preview.configure("next.mkv".into(), Vec::new(), false);
        preview.finish(current, 20, Some(image()));
        assert!(matches!(preview.frame.get_untracked(), PreviewFrame::Idle));
        assert!(preview.cache.borrow().is_empty());
    }

    #[test]
    fn returning_to_a_cached_frame_is_immediate_and_cache_is_bounded() {
        let preview = configured();
        for second in 0..60 {
            preview.request(Some(second as f64));
            preview.finish(preview.revision.load(Ordering::Acquire), second, Some(image()));
        }
        assert_eq!(preview.cache.borrow().len(), CACHE_SIZE);
        preview.request(Some(30.5));
        assert!(matches!(preview.frame.get_untracked(), PreviewFrame::Ready(_)));
        assert_eq!(preview.cache.borrow().back().unwrap().0, 30);
        preview.cancel();
        assert!(matches!(preview.frame.get_untracked(), PreviewFrame::Idle));
        assert_eq!(preview.cache.borrow().len(), CACHE_SIZE);
    }

    #[test]
    #[ignore = "requires FFmpeg on PATH; exercises real frame extraction"]
    fn real_decoder_returns_frames_from_the_requested_part_of_the_video() {
        let (ffmpeg, _) = engine::tools().expect("FFmpeg and FFprobe on PATH");
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/seek-preview-test");
        std::fs::create_dir_all(&directory).unwrap();
        let video = directory.join("red-blue.mkv");
        let mut command = Command::new(ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=160x90:d=10:r=1",
            "-f", "lavfi", "-i", "color=c=blue:s=160x90:d=10:r=1", "-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0",
            "-c:v", "ffv1"]).arg(&video);
        hide_console(&mut command);
        assert!(command.status().unwrap().success());
        let source = Source { url: video.to_string_lossy().into_owned(), headers: Vec::new(), torrent: false,
            info: Some(MediaInfo { duration: 20.0, has_video: true, ..Default::default() }) };
        let revision = AtomicU64::new(1);
        for (second, channel) in [(0, 0), (15, 2)] {
            let frame = decode(&source, second, &revision, 1).expect("decode preview frame");
            let pixels = frame.decoded().pixels();
            let centre = ((HEIGHT / 2 * WIDTH + WIDTH / 2) * 4) as usize;
            assert!(pixels[centre + channel] > 240);
            assert!(pixels[centre + 2 - channel] < 10);
            image::save_buffer(directory.join(format!("frame-{second}.png")), pixels, WIDTH, HEIGHT, image::ColorType::Rgba8).unwrap();
        }
        revision.store(2, Ordering::Release);
        assert!(decode(&source, 0, &revision, 1).is_none());
    }
}
