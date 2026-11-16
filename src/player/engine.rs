//! Playback engine.
//!
//! Incular has no video surface, so decoding is delegated to `ffmpeg`
//! child processes: one writes raw RGBA frames to a pipe, one writes PCM
//! audio, and an optional third extracts an embedded subtitle track. Audio
//! is the master clock (samples actually consumed by the sound card);
//! frames are presented when the clock reaches them and dropped when late.
//! Pausing simply stops reading the pipes, which blocks ffmpeg.

use super::subtitles::{Cue, parse_block};
use crate::util::hide_console;
use incular::prelude::ImageHandle;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SAMPLE_RATE: u32 = 48_000;
/// Interleaved stereo samples buffered ahead of the sound card (~0.5 s).
const AUDIO_QUEUE_TARGET: usize = SAMPLE_RATE as usize;

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub language: String,
    pub title: String,
    pub codec: String,
    /// Index of the stream in the container (`-map 0:<index>`).
    pub stream_index: usize,
}

impl Track {
    pub fn is_text_subtitle(&self) -> bool {
        !matches!(
            self.codec.as_str(),
            "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "xsub"
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// The video uses an HDR transfer (PQ / HDR10, or HLG), so it must be
    /// tone-mapped to SDR before it is shown as plain RGBA.
    pub hdr: bool,
    /// Dolby Vision whose base layer is not HDR10/HLG/SDR (profile 5, or
    /// compatibility id 0): its picture is only right after Dolby Vision
    /// reshaping, without which it looks green and purple.
    pub dolby_vision: bool,
    /// The video's matrix is not tagged, so ffmpeg would guess BT.601.
    pub untagged: bool,
    pub has_video: bool,
    /// Container index of the video stream chosen for playback.
    pub video_index: usize,
    /// No known duration over the network: play from the live edge, no seeking.
    pub live: bool,
    pub audio: Vec<Track>,
    pub subtitles: Vec<Track>,
}

impl MediaInfo {
    pub fn preferred_audio_index(&self, preferred: &str) -> usize {
        self.audio.iter().position(|track| crate::util::same_language(&track.language, preferred))
            .or_else(|| self.audio.iter().position(|track| self.has_video && track.stream_index > self.video_index))
            .unwrap_or(0)
    }
}

pub enum Event {
    Info(MediaInfo),
    /// A new frame is waiting in [`Engine::take_frame`].
    Frame,
    Position { generation: u64, seconds: f64, stalled: bool },
    /// Cues from the selected subtitle revision; `reset` clears earlier ones.
    Cues { revision: u64, cues: Vec<Cue>, reset: bool },
    /// The whole-file subtitle pass has read up to `until` seconds;
    /// `done` once it reached the end.
    SubtitleProgress { revision: u64, until: f64, done: bool },
    Ended { generation: u64 },
    Error { generation: u64, message: String },
}

#[derive(Clone, Debug)]
pub struct OpenRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub start: f64,
    pub max_height: u32,
    pub hardware_decoding: bool,
    pub volume: f32,
    pub preferred_audio: String,
}

#[derive(Clone)]
struct Config {
    audio_track: usize,
    subtitle_track: Option<usize>,
    subtitle_revision: u64,
    fast_subtitles: bool,
    speed: f64,
    /// Seconds the sound is held back against the picture (negative plays
    /// it earlier).
    audio_delay: f64,
}

struct ClockState {
    base: f64,
    speed: f64,
    audio: bool,
    anchor: Instant,
    anchor_position: f64,
}

struct Inner {
    request: OpenRequest,
    emit: Box<dyn Fn(Event) + Send + Sync>,
    alive: AtomicBool,
    generation: AtomicU64,
    /// Only one restart may reset the clock and replace decoder processes.
    restart_lock: Mutex<()>,
    pending_seek: Mutex<Option<(u64, f64)>>,
    paused: AtomicBool,
    /// Set once the first frame after a (re)start is ready, so sound and
    /// picture begin together.
    started: AtomicBool,
    consumed_frames: AtomicU64,
    /// Audio frames still to skip (positive) or repeat (negative) to slew
    /// the clock without a seek (watch-together drift correction).
    trim_frames: AtomicI64,
    clock: Mutex<ClockState>,
    audio_queue: Mutex<VecDeque<i16>>,
    audio_device: AtomicBool,
    volume: AtomicU32,
    children: Mutex<Vec<Child>>,
    frame: Mutex<Option<ImageHandle>>,
    frame_pending: AtomicBool,
    info: Mutex<Option<MediaInfo>>,
    config: Mutex<Config>,
    last_error: Mutex<String>,
    last_restart: Mutex<Instant>,
    /// First media timestamps seen by the live video / audio decoders; their
    /// difference lines the two independent connections up.
    live_video_start: Mutex<Option<f64>>,
    live_audio_start: Mutex<Option<f64>>,
    /// Bumped to cancel the whole-file subtitle pass.
    sub_pass: AtomicU64,
    /// The whole-file subtitle reader, kept apart from `children` so seeks
    /// do not restart it.
    sub_child: Mutex<Option<Child>>,
    sub_fast: AtomicBool,
    sub_fast_revision: AtomicU64,
}

pub struct Engine {
    inner: Arc<Inner>,
}

/// Locates `ffmpeg`/`ffprobe`: bundled next to the app (in `ffmpeg\`, as the
/// installer lays it out), else on PATH.
pub fn tools() -> Option<(PathBuf, PathBuf)> {
    static TOOLS: OnceLock<Option<(PathBuf, PathBuf)>> = OnceLock::new();
    TOOLS
        .get_or_init(|| {
            let works = |program: &PathBuf| {
                let mut command = Command::new(program);
                command
                    .arg("-version")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                hide_console(&mut command);
                command.status().is_ok_and(|s| s.success())
            };
            let exe = if cfg!(windows) { ".exe" } else { "" };
            let in_dir = |dir: &std::path::Path| {
                (dir.join(format!("ffmpeg{exe}")), dir.join(format!("ffprobe{exe}")))
            };
            let app_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(PathBuf::from));
            if let Some(app_dir) = app_dir {
                for dir in [app_dir.join("ffmpeg"), app_dir] {
                    let bundled = in_dir(&dir);
                    if works(&bundled.0) && works(&bundled.1) {
                        return Some(bundled);
                    }
                }
            }
            let on_path = (PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"));
            (works(&on_path.0) && works(&on_path.1)).then_some(on_path)
        })
        .clone()
}

impl Engine {
    #[cfg(test)]
    pub(super) fn test_stub(generation: u64) -> Self {
        let inner = Arc::new(Inner::new(OpenRequest {
            url: String::new(), headers: Vec::new(), start: 0.0, max_height: 90,
            hardware_decoding: false, volume: 1.0, preferred_audio: String::new(),
        }, |_| {}));
        inner.generation.store(generation, Ordering::Release);
        Self { inner }
    }

    pub fn open(request: OpenRequest, emit: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let inner = Arc::new(Inner::new(request, emit));
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("player-open".into())
            .spawn(move || worker.open())
            .expect("player thread");
        Self { inner }
    }

    pub fn take_frame(&self) -> Option<ImageHandle> {
        self.inner.frame_pending.store(false, Ordering::Release);
        self.inner.frame.lock().unwrap().take()
    }

    pub fn position(&self) -> f64 {
        self.inner.now()
    }

    pub fn is_current_generation(&self, generation: u64) -> bool {
        self.inner.current(generation)
    }

    fn restart_position(&self) -> f64 {
        self.inner.pending_seek.lock().unwrap().map_or_else(|| self.position(), |(_, position)| position)
    }

    /// Moves playback `seconds` forward (or back, when negative) gradually:
    /// with sound, by running a few percent fast or slow until caught up;
    /// without, at once. Replaces any correction still under way.
    pub fn nudge(&self, seconds: f64) {
        let mut clock = self.inner.clock.lock().unwrap();
        if clock.audio {
            let frames = seconds / clock.speed.max(0.1) * SAMPLE_RATE as f64;
            self.inner.trim_frames.store(frames.round() as i64, Ordering::Release);
        } else {
            clock.anchor_position += seconds;
        }
    }

    pub fn set_paused(&self, paused: bool) {
        let was_paused = self.inner.set_paused(paused);
        // A paused live stream falls out of the broadcast window: resuming
        // rejoins at the live edge.
        if was_paused && !paused && self.inner.is_live() {
            self.inner.queue_restart(0.0);
        }
    }

    pub fn seek(&self, seconds: f64) {
        self.inner.queue_restart(seconds);
    }

    pub fn set_volume(&self, volume: f32) {
        self.inner
            .volume
            .store(volume.clamp(0.0, MAX_VOLUME).to_bits(), Ordering::Release);
    }

    pub fn set_speed(&self, speed: f64) {
        self.inner.config.lock().unwrap().speed = speed;
        self.seek(self.restart_position());
    }

    pub fn set_audio_track(&self, index: usize) {
        self.inner.config.lock().unwrap().audio_track = index;
        self.seek(self.restart_position());
    }

    /// Shifts the sound against the picture; restarts decoding in place.
    pub fn set_audio_delay(&self, seconds: f64) {
        self.inner.config.lock().unwrap().audio_delay = seconds;
        if !self.inner.is_live() {
            self.seek(self.restart_position());
        }
    }

    /// Selects an embedded subtitle track to extract, or none.
    pub fn set_subtitle_track(&self, index: Option<usize>, revision: u64) {
        let fast = {
            let mut config = self.inner.config.lock().unwrap();
            config.subtitle_track = index;
            config.subtitle_revision = revision;
            config.fast_subtitles
        };
        self.seek(self.restart_position());
        self.inner.queue_full_subtitles(fast);
    }

    pub fn set_subtitle_loading(&self, fast: bool) {
        let selected = {
            let mut config = self.inner.config.lock().unwrap();
            if config.fast_subtitles == fast { return; }
            config.fast_subtitles = fast;
            config.subtitle_track.is_some()
        };
        if selected {
            // Changing this preference replaces only the subtitle scan.
            self.inner.queue_full_subtitles(fast);
        }
    }

    /// Reads the whole subtitle track again, as fast as the source allows.
    pub fn load_all_subtitles(&self) {
        self.inner.queue_full_subtitles(true);
    }

    pub fn stop(&self) {
        self.inner.alive.store(false, Ordering::Release);
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        self.inner.sub_pass.fetch_add(1, Ordering::AcqRel);
        self.inner.kill_children();
        if let Some(mut child) = self.inner.sub_child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Inner {
    fn new(request: OpenRequest, emit: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self {
            volume: AtomicU32::new(request.volume.to_bits()),
            request,
            emit: Box::new(emit),
            alive: AtomicBool::new(true),
            generation: AtomicU64::new(0),
            restart_lock: Mutex::new(()),
            pending_seek: Mutex::new(None),
            paused: AtomicBool::new(false),
            started: AtomicBool::new(false),
            consumed_frames: AtomicU64::new(0),
            trim_frames: AtomicI64::new(0),
            clock: Mutex::new(ClockState {
                base: 0.0,
                speed: 1.0,
                audio: false,
                anchor: Instant::now(),
                anchor_position: 0.0,
            }),
            audio_queue: Mutex::new(VecDeque::new()),
            audio_device: AtomicBool::new(false),
            children: Mutex::new(Vec::new()),
            frame: Mutex::new(None),
            frame_pending: AtomicBool::new(false),
            info: Mutex::new(None),
            config: Mutex::new(Config {
                audio_track: 0,
                subtitle_track: None,
                subtitle_revision: 0,
                fast_subtitles: false,
                speed: 1.0,
                audio_delay: 0.0,
            }),
            last_error: Mutex::new(String::new()),
            last_restart: Mutex::new(Instant::now()),
            live_video_start: Mutex::new(None),
            live_audio_start: Mutex::new(None),
            sub_pass: AtomicU64::new(0),
            sub_child: Mutex::new(None),
            sub_fast: AtomicBool::new(false),
            sub_fast_revision: AtomicU64::new(0),
        }
    }

    fn emit(&self, event: Event) {
        if self.alive.load(Ordering::Acquire) {
            (self.emit)(event);
        }
    }

    fn is_live(&self) -> bool {
        self.info.lock().unwrap().as_ref().is_some_and(|i| i.live)
    }

    fn current(&self, generation: u64) -> bool {
        self.alive.load(Ordering::Acquire) && self.generation.load(Ordering::Acquire) == generation
    }

    fn now(&self) -> f64 {
        let clock = self.clock.lock().unwrap();
        self.clock_position(&clock)
    }

    /// The caller holds the clock lock across reading and changing its anchor.
    fn clock_position(&self, clock: &ClockState) -> f64 {
        if clock.audio {
            clock.base
                + self.consumed_frames.load(Ordering::Acquire) as f64 / SAMPLE_RATE as f64
                    * clock.speed
        } else if self.paused.load(Ordering::Acquire) || !self.started.load(Ordering::Acquire) {
            clock.anchor_position
        } else {
            clock.anchor_position + clock.anchor.elapsed().as_secs_f64() * clock.speed
        }
    }

    fn set_paused(&self, paused: bool) -> bool {
        let mut clock = self.clock.lock().unwrap();
        let was_paused = self.paused.load(Ordering::Acquire);
        if was_paused == paused {
            return was_paused;
        }
        if !clock.audio {
            clock.anchor_position = self.clock_position(&clock);
            clock.anchor = Instant::now();
        }
        self.paused.store(paused, Ordering::Release);
        was_paused
    }

    /// Marks the first frame ready and starts the wall clock.
    fn start_clock(&self, generation: u64) {
        let mut clock = self.clock.lock().unwrap();
        if !self.current(generation) {
            return;
        }
        clock.anchor = Instant::now();
        self.started.store(true, Ordering::Release);
    }

    /// Audio ran out before the picture did: keep time by wall clock.
    fn fall_back_to_wall_clock(&self, generation: u64) {
        let mut clock = self.clock.lock().unwrap();
        if !self.current(generation) {
            return;
        }
        let position = self.clock_position(&clock);
        clock.audio = false;
        clock.anchor_position = position;
        clock.anchor = Instant::now();
    }

    fn kill_children(&self) {
        let mut children: Vec<Child> = self.children.lock().unwrap().drain(..).collect();
        // Shutdown can run during thread-local destruction on Windows, when
        // spawning a reaper thread is forbidden. Kill all pipes first, then
        // reap their processes here; Drop must never start another thread.
        for child in &mut children { let _ = child.kill(); }
        for mut child in children { let _ = child.wait(); }
    }

    /// Register under the same lock used to drain children. An obsolete
    /// worker owns only its new child; it must never kill newer decoders.
    fn register_child(&self, generation: u64, mut child: Child) -> bool {
        let mut children = self.children.lock().unwrap();
        if self.current(generation) {
            children.push(child);
            true
        } else {
            drop(children);
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }

    fn queue_restart(self: &Arc<Self>, position: f64) {
        // Opening will apply the latest config after probing. Invalidate a
        // decoder only once media info exists; otherwise an initial speed
        // setting would cancel the probe and the first playback generation.
        if self.info.lock().unwrap().is_none() {
            return;
        }
        // Assign the generation before spawning so request order wins over
        // thread scheduling. Waiting restarts skip work once superseded.
        let generation = {
            let mut pending = self.pending_seek.lock().unwrap();
            let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
            *pending = Some((generation, position));
            generation
        };
        let inner = self.clone();
        std::thread::spawn(move || inner.restart(generation, position));
    }

    fn input_args(&self, command: &mut Command) {
        let url = &self.request.url;
        if url.starts_with("http://") || url.starts_with("https://") {
            command.args([
                "-reconnect",
                "1",
                "-reconnect_streamed",
                "1",
                "-reconnect_delay_max",
                "5",
            ]);
            if !self.request.headers.is_empty() {
                let headers: String = self
                    .request
                    .headers
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}\r\n"))
                    .collect();
                command.args(["-headers", &headers]);
            }
        }
    }

    fn ffmpeg(&self, tool: &PathBuf) -> Command {
        let mut command = Command::new(tool);
        command
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console(&mut command);
        command
    }

    fn open(self: Arc<Self>) {
        let Some((_, ffprobe)) = tools() else {
            self.emit(Event::Error { generation: 0, message:
                "ffmpeg is missing from the Spoutpeak folder. Reinstall Spoutpeak or choose an external player in Settings.".into()
            });
            return;
        };
        self.clone().start_audio_output();

        let mut command = Command::new(ffprobe);
        command.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]);
        self.input_args(&mut command);
        command
            .arg(&self.request.url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        let output = match command.spawn() {
            Ok(mut child) => {
                let mut stdout = child.stdout.take();
                let mut stderr = child.stderr.take();
                if !self.register_child(0, child) {
                    return;
                }
                let mut json = Vec::new();
                if let Some(out) = stdout.as_mut() {
                    let _ = out.read_to_end(&mut json);
                }
                let mut errors = String::new();
                if let Some(err) = stderr.as_mut() {
                    let _ = err.read_to_string(&mut errors);
                }
                (json, errors)
            }
            Err(error) => {
                self.emit(Event::Error { generation: 0, message: format!("could not run ffprobe: {error}") });
                return;
            }
        };
        if !self.alive.load(Ordering::Acquire) {
            return;
        }
        let remote = self.request.url.starts_with("http://") || self.request.url.starts_with("https://");
        let Some(info) = parse_probe(&output.0, self.request.max_height, remote) else {
            let reason = output.1.lines().last().unwrap_or("").trim().to_owned();
            self.emit(Event::Error { generation: 0, message: if reason.is_empty() {
                "This stream could not be opened.".into()
            } else {
                format!("This stream could not be opened: {reason}")
            }});
            return;
        };
        {
            let preferred = &self.request.preferred_audio;
            let mut config = self.config.lock().unwrap();
            config.audio_track = info.preferred_audio_index(preferred);
        }
        *self.info.lock().unwrap() = Some(info.clone());
        self.emit(Event::Info(info));
        self.clone().start_ticker();
        let start = self.request.start;
        self.restart(0, start);
    }

    /// Owns the sound device for the engine's lifetime.
    fn start_audio_output(self: Arc<Self>) {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
        let inner = self.clone();
        let _ = std::thread::Builder::new()
            .name("player-audio-out".into())
            .spawn(move || {
                let Ok(mut output) = rodio::DeviceSinkBuilder::open_default_sink() else {
                    let _ = ready_tx.send(false);
                    return;
                };
                output.log_on_drop(false);
                let sink = rodio::Player::connect_new(output.mixer());
                sink.append(QueueSource {
                    inner: inner.clone(),
                    local: Vec::new(),
                    cursor: 0,
                    generation: 0,
                    frame_count: 0,
                });
                let _ = ready_tx.send(true);
                while inner.alive.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                sink.stop();
            });
        let available = ready_rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false);
        self.audio_device.store(available, Ordering::Release);
    }

    fn start_ticker(self: Arc<Self>) {
        let _ = std::thread::Builder::new()
            .name("player-ticker".into())
            .spawn(move || {
                let mut previous = -1.0;
                let mut still = 0;
                let mut previous_generation = 0;
                while self.alive.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(200));
                    let generation = self.generation.load(Ordering::Acquire);
                    let seconds = self.now();
                    let paused = self.paused.load(Ordering::Acquire);
                    if generation == previous_generation && !paused && (seconds - previous).abs() < 0.001 {
                        still += 1;
                    } else {
                        still = 0;
                    }
                    previous = seconds;
                    previous_generation = generation;
                    self.emit(Event::Position {
                        generation,
                        seconds,
                        stalled: still >= 3,
                    });
                }
            });
    }

    /// (Re)starts decoding at `position`. Used for open, seek, track and
    /// speed changes, and reconnects.
    fn restart(self: Arc<Self>, generation: u64, position: f64) {
        let _restart = self.restart_lock.lock().unwrap();
        let Some((ffmpeg, _)) = tools() else { return };
        let Some(info) = self.info.lock().unwrap().clone() else {
            return;
        };
        if !self.current(generation) {
            return;
        }
        self.kill_children();
        self.audio_queue.lock().unwrap().clear();
        *self.last_restart.lock().unwrap() = Instant::now();
        self.last_error.lock().unwrap().clear();
        let config = self.config.lock().unwrap().clone();
        let limit = if info.duration > 1.0 {
            info.duration - 0.5
        } else {
            f64::MAX
        };
        let live = info.live;
        let position = if live { 0.0 } else { position.clamp(0.0, limit) };
        *self.live_video_start.lock().unwrap() = None;
        *self.live_audio_start.lock().unwrap() = None;
        let use_audio = self.audio_device.load(Ordering::Acquire) && !info.audio.is_empty();
        {
            let mut clock = self.clock.lock().unwrap();
            self.consumed_frames.store(0, Ordering::Release);
            self.trim_frames.store(0, Ordering::Release);
            self.started.store(!info.has_video, Ordering::Release);
            *clock = ClockState {
                base: position,
                speed: config.speed,
                audio: use_audio,
                anchor: Instant::now(),
                anchor_position: position,
            };
        }
        {
            let mut pending = self.pending_seek.lock().unwrap();
            if pending.as_ref().is_some_and(|(request, _)| *request == generation) {
                pending.take();
            }
        }
        let seek = format!("{position:.3}");

        if info.has_video {
            let (width, height, fps) = output_geometry(&info, self.request.max_height);
            let mut command = self.ffmpeg(&ffmpeg);
            if self.request.hardware_decoding {
                command.args(["-hwaccel", "auto"]);
            }
            if live {
                // Timestamps are reported on stderr (showinfo) for A/V sync.
                command.args(["-loglevel", "info", "-copyts"]);
            } else if position > 0.0 {
                // No seek from the start: seeking an MKV first loads its
                // index (Cues), usually from the end of the file, which for a
                // torrent means waiting for pieces playback doesn't need yet.
                command.args(["-ss", &seek]);
            }
            self.input_args(&mut command);
            command
                .arg("-i")
                .arg(&self.request.url)
                .args(["-map", &format!("0:{}", info.video_index), "-an", "-sn", "-dn"])
                .args(["-vf", &video_filter(&info, width, height, fps, live)])
                .args(["-pix_fmt", "rgba", "-f", "rawvideo"]);
            if live {
                // With copied timestamps a constant-rate muxer would pad the
                // hours before the first frame with duplicates.
                command.args(["-fps_mode", "passthrough"]);
            }
            command.arg("pipe:1").stderr(Stdio::piped());
            match command.spawn() {
                Ok(mut child) => {
                    let stdout = child.stdout.take();
                    let stderr = child.stderr.take();
                    if !self.register_child(generation, child) {
                        return;
                    }
                    if let Some(stderr) = stderr {
                        let inner = self.clone();
                        std::thread::spawn(move || {
                            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                                if live {
                                    inner.note_live_start(generation, &line, true);
                                } else if !line.trim().is_empty() {
                                    let mut last_error = inner.last_error.lock().unwrap();
                                    if inner.current(generation) { *last_error = line; }
                                }
                            }
                        });
                    }
                    if let Some(stdout) = stdout {
                        let inner = self.clone();
                        std::thread::spawn(move || {
                            inner.video_loop(generation, stdout, width, height, fps, position)
                        });
                    }
                }
                Err(error) => {
                    self.emit(Event::Error { generation, message: format!("could not run ffmpeg: {error}") });
                    return;
                }
            }
        }

        if use_audio {
            // The clock counts played samples from `position`; decoding the
            // sound from `position - delay` makes it lag (or lead) the picture
            // by `delay`. Before the start of the file, silence fills in.
            let audio_from = position - config.audio_delay;
            let lead_silence = if live { 0.0 } else { (-audio_from).max(0.0) };
            let mut command = self.ffmpeg(&ffmpeg);
            if live {
                command.args(["-loglevel", "info", "-copyts"]).stderr(Stdio::piped());
            } else if audio_from > 0.0 {
                command.args(["-ss", &format!("{audio_from:.3}")]);
            }
            self.input_args(&mut command);
            let audio_map = info
                .audio
                .get(config.audio_track)
                .map_or_else(|| "0:a:0".to_owned(), |t| format!("0:{}", t.stream_index));
            command
                .arg("-i")
                .arg(&self.request.url)
                .args(["-map", &audio_map])
                .args(["-vn", "-sn", "-dn"]);
            if live {
                command.args(["-af", "ashowinfo"]);
            } else if (config.speed - 1.0).abs() > 0.01 {
                command.args(["-af", &format!("atempo={:.2}", config.speed)]);
            }
            command.args(["-ac", "2", "-ar", &SAMPLE_RATE.to_string()]);
            command.args(["-f", "s16le", "pipe:1"]);
            match command.spawn() {
                Ok(mut child) => {
                    let stdout = child.stdout.take();
                    if let Some(stderr) = child.stderr.take() {
                        let inner = self.clone();
                        std::thread::spawn(move || {
                            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                                inner.note_live_start(generation, &line, false);
                            }
                        });
                    }
                    if !self.register_child(generation, child) {
                        return;
                    }
                    if let Some(stdout) = stdout {
                        let inner = self.clone();
                        let has_video = info.has_video;
                        // Silence is in output time, so it scales with the tempo.
                        let frames = (lead_silence / config.speed * SAMPLE_RATE as f64) as usize;
                        std::thread::spawn(move || {
                            inner.audio_loop(generation, stdout, has_video, frames)
                        });
                    }
                }
                Err(_) => self.audio_unavailable(generation, info.has_video),
            }
        }

        if let Some(track) = config.subtitle_track.filter(|_| !live) {
            // Start a little early so a cue already on screen is included.
            let from = (position - 15.0).max(0.0);
            let mut command = self.ffmpeg(&ffmpeg);
            if from > 0.0 {
                command.args(["-ss", &format!("{from:.3}")]);
            }
            // Getting subtitle packets means reading the whole interleaved
            // file. Unthrottled, this reader races to the end of a torrent and
            // takes bandwidth from playback; at 1.1x it stays just ahead.
            // The burst covers the lead-in before `position`.
            let burst = (position - from + 10.0).max(10.0);
            command.args(["-readrate", "1.1", "-readrate_initial_burst", &format!("{burst:.0}")]);
            self.input_args(&mut command);
            command
                .arg("-i")
                .arg(&self.request.url)
                .args([
                    "-map",
                    &info
                        .subtitles
                        .get(track)
                        .map_or_else(|| format!("0:s:{track}"), |t| format!("0:{}", t.stream_index)),
                    "-vn",
                    "-an",
                    "-dn",
                ])
                .args(["-flush_packets", "1", "-f", "srt", "pipe:1"]);
            // Start once the picture is up, so the first frames get every
            // peer.
            let inner = self.clone();
            std::thread::spawn(move || {
                let waited = Instant::now();
                while inner.current(generation)
                    && !inner.started.load(Ordering::Acquire)
                    && waited.elapsed() < Duration::from_secs(60)
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                if !inner.current(generation) {
                    return;
                }
                if let Ok(mut child) = command.spawn() {
                    let stdout = child.stdout.take();
                    if !inner.register_child(generation, child) {
                        return;
                    }
                    if let Some(stdout) = stdout {
                        let keep = |inner: &Inner| inner.current(generation);
                        inner.subtitle_loop(&keep, stdout, from, false, config.subtitle_revision);
                    }
                }
            });
        }
    }

    /// Records the first `pts_time` a live decoder reports.
    fn note_live_start(&self, generation: u64, line: &str, video: bool) {
        if !self.current(generation) || !line.contains("showinfo") {
            return;
        }
        let Some(value) = line.split("pts_time:").nth(1) else {
            return;
        };
        let Ok(seconds) = value.split_whitespace().next().unwrap_or("").parse::<f64>() else {
            return;
        };
        let slot = if video { &self.live_video_start } else { &self.live_audio_start };
        let mut slot = slot.lock().unwrap();
        if self.current(generation) && slot.is_none() {
            *slot = Some(seconds);
        }
    }

    /// How far the live picture's first frame sits after the first audio
    /// sample, once both decoders have reported in.
    fn live_offset(&self, generation: u64) -> f64 {
        let waited = Instant::now();
        while self.current(generation) && waited.elapsed() < Duration::from_secs(4) {
            let video = *self.live_video_start.lock().unwrap();
            let audio = *self.live_audio_start.lock().unwrap();
            if let (Some(video), Some(audio)) = (video, audio) {
                let offset = video - audio;
                return if offset.abs() < 30.0 { offset } else { 0.0 };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        0.0
    }

    fn publish(&self, generation: u64, pixels: &[u8], width: u32, height: u32) {
        let Ok(handle) = ImageHandle::from_rgba8(width, height, Arc::<[u8]>::from(pixels)) else {
            return;
        };
        let mut frame = self.frame.lock().unwrap();
        if !self.current(generation) {
            return;
        }
        *frame = Some(handle);
        drop(frame);
        if !self.frame_pending.swap(true, Ordering::AcqRel) {
            self.emit(Event::Frame);
        }
    }

    fn video_loop(
        self: Arc<Self>,
        generation: u64,
        mut stdout: ChildStdout,
        width: u32,
        height: u32,
        fps: f64,
        base: f64,
    ) {
        let mut buffer = vec![0u8; width as usize * height as usize * 4];
        let mut index: u64 = 0;
        let live = self.is_live();
        let uses_audio = self.clock.lock().unwrap().audio;
        let mut base = base;
        loop {
            if stdout.read_exact(&mut buffer).is_err() {
                break;
            }
            if !self.current(generation) {
                return;
            }
            let pts = base + index as f64 / fps;
            index += 1;
            if index == 1 {
                // Always show the first frame, so seeking while paused updates
                // the picture, and release the audio.
                self.publish(generation, &buffer, width, height);
                if live && uses_audio {
                    base += self.live_offset(generation);
                }
                self.start_clock(generation);
                continue;
            }
            loop {
                if !self.current(generation) {
                    return;
                }
                let ahead = pts - self.now();
                if ahead <= 0.002 {
                    break;
                }
                std::thread::sleep(Duration::from_secs_f64(ahead.min(0.015)));
            }
            if self.now() - pts > 0.2 {
                continue; // late: drop to catch up
            }
            self.publish(generation, &buffer, width, height);
        }
        if self.current(generation) {
            self.finished(generation, base + index as f64 / fps);
        }
    }

    fn audio_loop(
        self: Arc<Self>,
        generation: u64,
        mut stdout: ChildStdout,
        has_video: bool,
        silence_frames: usize,
    ) {
        // Stereo: two samples per frame.
        let mut silence = silence_frames * 2;
        while silence > 0 {
            if !self.current(generation) {
                return;
            }
            {
                let mut queue = self.audio_queue.lock().unwrap();
                if !self.current(generation) {
                    return;
                }
                if queue.len() < AUDIO_QUEUE_TARGET {
                    let chunk = silence.min(4096);
                    queue.extend(std::iter::repeat_n(0i16, chunk));
                    silence -= chunk;
                    continue;
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        let mut bytes = vec![0u8; 16_384];
        let mut carry: Option<u8> = None;
        let mut decoded_any = false;
        loop {
            let read = match stdout.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            decoded_any = true;
            let mut samples = Vec::with_capacity(read / 2 + 1);
            let mut iter = bytes[..read].iter().copied();
            if let Some(low) = carry.take() {
                if let Some(high) = iter.next() {
                    samples.push(i16::from_le_bytes([low, high]));
                }
            }
            loop {
                match (iter.next(), iter.next()) {
                    (Some(low), Some(high)) => samples.push(i16::from_le_bytes([low, high])),
                    (Some(low), None) => {
                        carry = Some(low);
                        break;
                    }
                    _ => break,
                }
            }
            loop {
                if !self.current(generation) {
                    return;
                }
                {
                    let mut queue = self.audio_queue.lock().unwrap();
                    if !self.current(generation) {
                        return;
                    }
                    if queue.len() < AUDIO_QUEUE_TARGET {
                        queue.extend(samples.iter().copied());
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        }
        // Let the queue drain before deciding what the end of audio means.
        while self.current(generation) && !self.audio_queue.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !self.current(generation) {
            return;
        }
        if !decoded_any {
            self.audio_unavailable(generation, has_video);
        } else if has_video {
            self.fall_back_to_wall_clock(generation);
        } else {
            let position = self.now();
            self.finished(generation, position);
        }
    }

    fn audio_unavailable(&self, generation: u64, has_video: bool) {
        if !self.current(generation) { return; }
        if has_video {
            // An empty/failed audio decoder must not leave the picture
            // waiting forever on a sample clock that can never advance.
            self.fall_back_to_wall_clock(generation);
        } else {
            self.emit(Event::Error { generation, message: "The audio stream could not be decoded.".into() });
        }
    }

    /// Reads SRT from `stdout` into `Cues` events while `keep` holds;
    /// `report` also sends `SubtitleProgress` (the whole-file pass).
    fn subtitle_loop(
        &self,
        keep: &dyn Fn(&Inner) -> bool,
        stdout: ChildStdout,
        offset: f64,
        report: bool,
        revision: u64,
    ) {
        let mut block = String::new();
        let mut batch: Vec<Cue> = Vec::new();
        let mut last_flush = Instant::now();
        let mut until = 0.0f64;
        let flush = |batch: &mut Vec<Cue>, until: &mut f64| {
            if batch.is_empty() || !keep(self) {
                return;
            }
            *until = batch.iter().fold(*until, |m, c| m.max(c.end));
            self.emit(Event::Cues { revision, cues: std::mem::take(batch), reset: false });
            if report {
                self.emit(Event::SubtitleProgress { revision, until: *until, done: false });
            }
        };
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !keep(self) {
                return;
            }
            if line.trim().is_empty() {
                if let Some(cue) = parse_block(&block, offset) {
                    batch.push(cue);
                }
                block.clear();
                if !report || until == 0.0 || batch.len() >= 40 || last_flush.elapsed() > Duration::from_millis(400) {
                    flush(&mut batch, &mut until);
                    last_flush = Instant::now();
                }
            } else {
                block.push_str(&line);
                block.push('\n');
            }
        }
        if let Some(cue) = parse_block(&block, offset) {
            batch.push(cue);
        }
        if !keep(self) {
            return;
        }
        flush(&mut batch, &mut until);
        if report {
            self.emit(Event::SubtitleProgress { revision, until, done: true });
        }
    }

    fn subtitle_pass_current(&self, pass: u64) -> bool {
        self.alive.load(Ordering::Acquire) && self.sub_pass.load(Ordering::Acquire) == pass
    }

    fn register_subtitle_child(&self, pass: u64, mut child: Child) -> bool {
        let mut slot = self.sub_child.lock().unwrap();
        if !self.subtitle_pass_current(pass) {
            drop(slot);
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        let previous = slot.replace(child);
        drop(slot);
        if let Some(mut previous) = previous {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        true
    }

    fn queue_full_subtitles(self: &Arc<Self>, fast: bool) {
        let config = self.config.lock().unwrap();
        let (track, revision) = (config.subtitle_track, config.subtitle_revision);
        // Allocate in request order and detach only the old owned process.
        // A delayed worker can never take or replace a newer pass's child.
        let (pass, previous) = {
            let mut slot = self.sub_child.lock().unwrap();
            if fast && self.sub_fast.load(Ordering::Acquire)
                && self.sub_fast_revision.load(Ordering::Acquire) == revision {
                return;
            }
            self.sub_fast.store(fast, Ordering::Release);
            self.sub_fast_revision.store(revision, Ordering::Release);
            (self.sub_pass.fetch_add(1, Ordering::AcqRel) + 1, slot.take())
        };
        drop(config);
        let inner = self.clone();
        std::thread::spawn(move || {
            if let Some(mut previous) = previous {
                let _ = previous.kill();
                let _ = previous.wait();
            }
            if let Some(track) = track {
                inner.clone().full_subtitles(pass, track, revision, fast);
            }
            let _slot = inner.sub_child.lock().unwrap();
            if inner.subtitle_pass_current(pass) {
                inner.sub_fast.store(false, Ordering::Release);
            }
        });
    }

    /// Read the captured track from start to end, independently of seeks.
    fn full_subtitles(self: Arc<Self>, pass: u64, track: usize, revision: u64, fast: bool) {
        if !self.subtitle_pass_current(pass) {
            return;
        }
        let Some((ffmpeg, _)) = tools() else { return };
        let keep = move |inner: &Inner| inner.subtitle_pass_current(pass);
        // Wait for the media info and the first picture, so opening gets
        // every peer first.
        let waited = Instant::now();
        while keep(&self)
            && !(self.info.lock().unwrap().is_some() && self.started.load(Ordering::Acquire))
            && waited.elapsed() < Duration::from_secs(60)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        let Some(info) = self.info.lock().unwrap().clone() else { return };
        if !keep(&self) || info.live {
            return;
        }
        self.emit(Event::SubtitleProgress { revision, until: 0.0, done: false });
        let mut command = self.ffmpeg(&ffmpeg);
        let cached = crate::streaming::completed_stream_file(&self.request.url);
        let network = cached.is_none() && (self.request.url.starts_with("http://") || self.request.url.starts_with("https://"));
        // Disk reads need no playback-rate limit. Uncached network reads
        // stay throttled until the user opens Sync or asks to load all.
        if !fast && network {
            command.args(["-readrate", "4", "-readrate_initial_burst", "60"]);
        }
        if cached.is_none() {
            self.input_args(&mut command);
            if self.request.url.starts_with("http://127.0.0.1:") {
                let mut headers: String = self.request.headers.iter().map(|(key, value)| format!("{key}: {value}\r\n")).collect();
                headers.push_str("X-Spoutpeak-Subtitles: 1\r\n");
                command.args(["-headers", &headers]);
            }
        }
        command
            .arg("-i");
        if let Some(path) = cached { command.arg(path); } else { command.arg(&self.request.url); }
        command
            .args([
                "-map",
                &info
                    .subtitles
                    .get(track)
                    .map_or_else(|| format!("0:s:{track}"), |t| format!("0:{}", t.stream_index)),
                "-vn",
                "-an",
                "-dn",
            ])
            .args(["-flush_packets", "1", "-f", "srt", "pipe:1"]);
        let Ok(mut child) = command.spawn() else { return };
        let stdout = child.stdout.take();
        if !self.register_subtitle_child(pass, child) {
            return;
        }
        if let Some(stdout) = stdout {
            self.subtitle_loop(&keep, stdout, 0.0, true, revision);
        }
    }

    /// The decoder stopped producing output at `position`.
    fn finished(self: Arc<Self>, generation: u64, position: f64) {
        if !self.current(generation) {
            return;
        }
        let (duration, live) = self
            .info
            .lock()
            .unwrap()
            .as_ref()
            .map_or((0.0, false), |i| (i.duration, i.live));
        if !live && (duration <= 0.0 || position >= duration - 5.0) {
            self.emit(Event::Ended { generation });
            return;
        }
        // Dropped connection mid-stream: reconnect, unless it keeps failing.
        let recently = self.last_restart.lock().unwrap().elapsed() < Duration::from_secs(8);
        if recently {
            let reason = self.last_error.lock().unwrap().clone();
            self.emit(Event::Error { generation, message: if reason.is_empty() {
                "The stream stopped unexpectedly.".into()
            } else {
                format!("Playback failed: {reason}")
            }});
        } else {
            // A reconnect from an old decoder must not supersede a seek.
            if self.generation.compare_exchange(
                generation, generation + 1, Ordering::AcqRel, Ordering::Acquire,
            ).is_ok() {
                self.restart(generation + 1, position);
            }
        }
    }
}

/// Endless audio source fed from the engine's sample queue. Counts what it
/// hands to the sound card: that count is the playback clock.
struct QueueSource {
    inner: Arc<Inner>,
    local: Vec<i16>,
    cursor: usize,
    generation: u64,
    frame_count: u64,
}

/// One frame in this many is skipped or repeated while slewing: about 4 %
/// faster or slower, quiet enough not to notice.
const SLEW_EVERY: u64 = 25;

impl Iterator for QueueSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let inner = &self.inner;
        if !inner.alive.load(Ordering::Relaxed) {
            return None;
        }
        if inner.paused.load(Ordering::Relaxed) || !inner.started.load(Ordering::Relaxed) {
            return Some(0.0);
        }
        let generation = inner.generation.load(Ordering::Relaxed);
        if generation != self.generation {
            self.generation = generation;
            self.local.clear();
            self.cursor = 0;
        }
        if self.cursor >= self.local.len() {
            self.local.clear();
            self.cursor = 0;
            let mut queue = inner.audio_queue.lock().unwrap();
            // Whole stereo frames only, so channels never swap.
            let take = queue.len().min(2048) & !1;
            self.local.extend(queue.drain(..take));
            if self.local.is_empty() {
                return Some(0.0);
            }
        }
        // Drift correction: at the start of every SLEW_EVERY-th frame, skip
        // one frame (catch up) or play this one twice (wait).
        if self.cursor % 2 == 0 {
            self.frame_count = self.frame_count.wrapping_add(1);
            let trim = inner.trim_frames.load(Ordering::Relaxed);
            if trim != 0 && self.frame_count % SLEW_EVERY == 0 {
                if trim > 0 && self.cursor + 4 <= self.local.len() {
                    self.cursor += 2;
                    inner.consumed_frames.fetch_add(1, Ordering::Relaxed);
                    inner.trim_frames.fetch_sub(1, Ordering::Relaxed);
                } else if trim < 0 && self.cursor >= 2 {
                    self.cursor -= 2;
                    inner.consumed_frames.fetch_sub(1, Ordering::Relaxed);
                    inner.trim_frames.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let sample = self.local[self.cursor];
        self.cursor += 1;
        if self.cursor % 2 == 0 {
            inner.consumed_frames.fetch_add(1, Ordering::Relaxed);
        }
        let volume = f32::from_bits(inner.volume.load(Ordering::Relaxed));
        Some(audio_sample(sample, volume))
    }
}

/// Highest playback gain (200 %).
pub const MAX_VOLUME: f32 = 2.0;

/// FFmpeg emits signed 16-bit PCM; Rodio expects normalized float samples.
fn audio_sample(sample: i16, volume: f32) -> f32 {
    let scaled = sample as f32 / 32768.0 * volume;
    let out = if volume > 1.0 { soft_clip(scaled) } else { scaled };
    out.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod seek_tests {
    use super::*;

    fn engine(url: String) -> Engine {
        Engine { inner: Arc::new(Inner::new(OpenRequest {
            url, headers: Vec::new(), start: 0.0, max_height: 90,
            hardware_decoding: false, volume: 1.0, preferred_audio: String::new(),
        }, |_| {})) }
    }

    #[test]
    fn settings_before_probe_do_not_cancel_opening() {
        let engine = engine(String::new());
        engine.set_speed(1.5);
        engine.set_audio_track(1);
        engine.set_audio_delay(0.25);
        engine.seek(30.0);
        assert!(engine.inner.current(0), "initial probe must remain current");
        let config = engine.inner.config.lock().unwrap();
        assert_eq!(config.speed, 1.5);
        assert_eq!(config.audio_track, 1);
        assert_eq!(config.audio_delay, 0.25);
    }

    #[test]
    fn probe_recognizes_matroska_language_tags_regardless_of_case() {
        let json = serde_json::json!({
            "format": { "duration": "2577.597" },
            "streams": [
                { "index": 0, "codec_type": "video", "codec_name": "hevc", "width": 1920, "height": 1080, "avg_frame_rate": "24000/1001" },
                { "index": 1, "codec_type": "audio", "codec_name": "aac", "tags": { "LANGUAGE": "eng", "TITLE": "Surround" } },
                { "index": 2, "codec_type": "subtitle", "codec_name": "ass", "tags": { "LANGUAGE": "ara" } },
                { "index": 4, "codec_type": "subtitle", "codec_name": "ass", "tags": { "LANGUAGE": "eng" } },
                { "index": 5, "codec_type": "subtitle", "codec_name": "ass", "tags": { "Language": "eng", "Title": "Alternate" } }
            ]
        });
        let info = parse_probe(&serde_json::to_vec(&json).unwrap(), 1080, false).unwrap();
        assert_eq!(info.audio[0].language, "eng");
        assert_eq!(info.audio[0].title, "Surround");
        assert_eq!(info.subtitles.iter().map(|track| track.language.as_str()).collect::<Vec<_>>(), ["ara", "eng", "eng"]);
        assert_eq!(info.subtitles[2].title, "Alternate");
    }

    #[test]
    fn changing_settings_during_a_seek_keeps_the_requested_destination() {
        let engine = engine(String::new());
        *engine.inner.info.lock().unwrap() = Some(MediaInfo { duration: 120.0, ..Default::default() });
        let _restart = engine.inner.restart_lock.lock().unwrap();
        engine.seek(80.0);
        engine.set_speed(1.5);
        engine.set_audio_delay(0.2);
        engine.set_audio_track(1);
        assert_eq!(engine.inner.pending_seek.lock().unwrap().as_ref().map(|(_, position)| *position), Some(80.0));
        assert_eq!(engine.inner.config.lock().unwrap().speed, 1.5);
        engine.stop();
    }

    #[test]
    fn setting_an_already_playing_live_stream_to_play_does_not_restart_it() {
        let engine = engine(String::new());
        *engine.inner.info.lock().unwrap() = Some(MediaInfo { live: true, ..Default::default() });
        let _restart = engine.inner.restart_lock.lock().unwrap();
        engine.set_paused(false);
        engine.set_paused(false);
        assert_eq!(engine.inner.generation.load(Ordering::Acquire), 0);
        engine.set_paused(true);
        engine.set_paused(false);
        let generation = engine.inner.generation.load(Ordering::Acquire);
        assert_eq!(generation, 1);
        engine.set_paused(false);
        assert_eq!(engine.inner.generation.load(Ordering::Acquire), generation);
        engine.stop();
    }

    #[test]
    fn failed_audio_does_not_freeze_the_picture_or_mark_audio_only_as_finished() {
        let mut engine = engine(String::new());
        let (tx, rx) = std::sync::mpsc::channel();
        Arc::get_mut(&mut engine.inner).unwrap().emit = Box::new(move |event| { let _ = tx.send(event); });
        {
            let mut clock = engine.inner.clock.lock().unwrap();
            clock.audio = true;
            clock.base = 30.0;
        }
        engine.inner.consumed_frames.store(SAMPLE_RATE as u64, Ordering::Release);
        engine.inner.started.store(true, Ordering::Release);
        engine.inner.audio_unavailable(0, true);
        assert!(!engine.inner.clock.lock().unwrap().audio);
        assert!(engine.position() >= 31.0);
        assert!(rx.try_recv().is_err());
        engine.inner.audio_unavailable(0, false);
        assert!(matches!(rx.try_recv().unwrap(), Event::Error { generation: 0, .. }));
    }

    #[test]
    fn pause_and_resume_cannot_restore_a_position_from_before_a_seek() {
        let engine = engine(String::new());
        let inner = engine.inner.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer_barrier = barrier.clone();
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            for position in 1..20_000 {
                {
                    // This is the wall-clock reset performed by restart.
                    let mut clock = inner.clock.lock().unwrap();
                    clock.base = position as f64;
                    clock.anchor_position = position as f64;
                    clock.anchor = Instant::now();
                }
                std::thread::yield_now();
            }
        });
        barrier.wait();
        // No first frame yet: neither pause nor resume may advance the
        // clock or copy a position sampled before the latest reset.
        for turn in 0..20_000 {
            engine.set_paused(turn % 2 == 0);
            let clock = engine.inner.clock.lock().unwrap();
            assert_eq!(clock.anchor_position, clock.base, "pause restored a stale seek position");
        }
        writer.join().unwrap();
    }

    #[test]
    fn paused_wall_clock_keeps_its_position_when_resumed() {
        let engine = engine(String::new());
        let inner = &engine.inner;
        inner.started.store(true, Ordering::Release);
        {
            let mut clock = inner.clock.lock().unwrap();
            clock.anchor_position = 20.0;
            clock.anchor = Instant::now() - Duration::from_secs(5);
        }
        engine.set_paused(true);
        let paused_position = engine.position();
        inner.clock.lock().unwrap().anchor = Instant::now() - Duration::from_secs(60);
        assert_eq!(engine.position(), paused_position);
        engine.set_paused(false);
        assert_eq!(inner.clock.lock().unwrap().anchor_position, paused_position);
        assert!(engine.position() >= paused_position);
    }

    #[test]
    fn subtitle_requests_cancel_previous_passes_in_request_order() {
        let engine = engine(String::new());
        engine.set_subtitle_track(Some(0), 10);
        let first = engine.inner.sub_pass.load(Ordering::Acquire);
        engine.set_subtitle_track(Some(1), 11);
        let second = engine.inner.sub_pass.load(Ordering::Acquire);
        assert!(second > first);
        assert!(!engine.inner.subtitle_pass_current(first));
        engine.load_all_subtitles();
        let third = engine.inner.sub_pass.load(Ordering::Acquire);
        assert!(third > second);
        engine.load_all_subtitles();
        assert_eq!(engine.inner.sub_pass.load(Ordering::Acquire), third, "a repeated fast-load request restarted the scan");
        engine.set_subtitle_track(None, 12);
        assert!(!engine.inner.subtitle_pass_current(third));
        let config = engine.inner.config.lock().unwrap();
        assert_eq!(config.subtitle_track, None);
        assert_eq!(config.subtitle_revision, 12);
    }

    #[test]
    fn fast_subtitle_setting_applies_to_each_track_without_restarting_video() {
        let engine = engine(String::new());
        engine.set_subtitle_loading(true);
        assert!(engine.inner.config.lock().unwrap().fast_subtitles);
        assert_eq!(engine.inner.sub_pass.load(Ordering::Acquire), 0, "no track is selected yet");
        *engine.inner.info.lock().unwrap() = Some(MediaInfo { has_video: true, ..Default::default() });
        let _restart = engine.inner.restart_lock.lock().unwrap();
        engine.set_subtitle_track(Some(0), 10);
        let first = engine.inner.sub_pass.load(Ordering::Acquire);
        assert!(engine.inner.sub_fast.load(Ordering::Acquire));
        engine.set_subtitle_track(Some(1), 11);
        let second = engine.inner.sub_pass.load(Ordering::Acquire);
        assert!(second > first, "fast loading must not suppress a different track's request");
        assert_eq!(engine.inner.sub_fast_revision.load(Ordering::Acquire), 11);
        let generation = engine.inner.generation.load(Ordering::Acquire);
        engine.load_all_subtitles();
        assert_eq!(engine.inner.sub_pass.load(Ordering::Acquire), second, "same fast request should be reused");
        engine.set_subtitle_loading(false);
        assert!(!engine.inner.sub_fast.load(Ordering::Acquire));
        assert!(engine.inner.sub_pass.load(Ordering::Acquire) > second);
        assert_eq!(engine.inner.generation.load(Ordering::Acquire), generation, "loading preferences must not restart A/V");
        engine.set_subtitle_loading(true);
        assert!(engine.inner.sub_fast.load(Ordering::Acquire));
        assert_eq!(engine.inner.generation.load(Ordering::Acquire), generation);
        engine.stop();
    }

    #[test]
    #[ignore = "requires FFmpeg on PATH; exercises subtitle process ownership and shutdown"]
    fn cancelled_subtitle_spawn_cannot_replace_or_kill_the_new_reader() {
        let (ffmpeg, _) = tools().expect("FFmpeg and FFprobe on PATH");
        let spawn = || {
            let mut command = Command::new(&ffmpeg);
            command.args(["-nostdin", "-re", "-f", "lavfi", "-i", "color=s=16x16:d=20",
                "-f", "null", "-"]).stdout(Stdio::null()).stderr(Stdio::null());
            hide_console(&mut command);
            command.spawn().unwrap()
        };
        let engine = engine(String::new());
        let inner = &engine.inner;
        inner.sub_pass.store(1, Ordering::Release);
        assert!(inner.register_subtitle_child(1, spawn()));
        // Detach the old reader and cancel its pass before a delayed spawn
        // completes, then register the new reader ahead of the old worker.
        inner.queue_full_subtitles(false);
        let pass = inner.sub_pass.load(Ordering::Acquire);
        let latest = spawn();
        let latest_id = latest.id();
        assert!(inner.register_subtitle_child(pass, latest));
        assert!(!inner.register_subtitle_child(1, spawn()));
        {
            let mut slot = inner.sub_child.lock().unwrap();
            let child = slot.as_mut().expect("current reader was removed");
            assert_eq!(child.id(), latest_id);
            assert!(child.try_wait().unwrap().is_none(), "current reader was killed");
        }
        engine.stop();
        assert!(inner.sub_child.lock().unwrap().is_none());
        assert!(!inner.register_subtitle_child(pass, spawn()), "shutdown accepted a late reader");
        assert!(inner.sub_child.lock().unwrap().is_none());
    }

    #[test]
    #[ignore = "requires FFmpeg on PATH; extracts real tracks after rapid subtitle changes"]
    fn rapid_subtitle_changes_extract_the_latest_track_and_revision() {
        let (ffmpeg, _) = tools().expect("FFmpeg and FFprobe on PATH");
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/subtitle-switch-test");
        std::fs::create_dir_all(&directory).unwrap();
        for (name, text) in [("first", "First track"), ("second", "Second track")] {
            std::fs::write(directory.join(format!("{name}.srt")),
                format!("1\n00:00:00,000 --> 00:00:10,000\n{text}\n")).unwrap();
        }
        let media = directory.join("tracks.mks");
        let mut command = Command::new(ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
            .arg(directory.join("first.srt")).arg("-i").arg(directory.join("second.srt"))
            .args(["-map", "0", "-map", "1", "-c:s", "srt", "-f", "matroska"]).arg(&media);
        hide_console(&mut command);
        assert!(command.status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = engine(media.to_string_lossy().into_owned());
        Arc::get_mut(&mut engine.inner).unwrap().emit = Box::new(move |event| { let _ = tx.send(event); });
        *engine.inner.info.lock().unwrap() = Some(MediaInfo {
            duration: 10.0,
            subtitles: (0..2).map(|stream_index| Track {
                language: String::new(), title: String::new(), codec: "srt".into(), stream_index,
            }).collect(),
            ..Default::default()
        });
        engine.inner.started.store(true, Ordering::Release);
        for revision in 1..20 {
            engine.set_subtitle_track(Some(revision as usize % 2), revision);
        }
        engine.set_subtitle_track(Some(1), 1000);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut latest = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining).expect("latest subtitle reader did not finish") {
                Event::Cues { revision: 1000, cues, .. } => latest.extend(cues),
                Event::SubtitleProgress { revision: 1000, done: true, .. } => break,
                _ => {}
            }
        }
        // Near-playhead and whole-file readers overlap; the UI merges
        // identical cues, so both are allowed to publish the same line.
        latest.sort_by(|a, b| a.start.total_cmp(&b.start));
        latest.dedup_by(|a, b| a.start == b.start && a.text == b.text);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].text, "Second track");
        engine.stop();
    }

    #[test]
    #[ignore = "requires FFmpeg on PATH; checks local subtitle scan speed and pipe flushing"]
    fn local_subtitles_load_quickly_and_current_cues_flush_immediately() {
        let (ffmpeg, _) = tools().expect("FFmpeg and FFprobe on PATH");
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/subtitle-loading-test");
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("captions.srt");
        std::fs::write(&source,
            "1\n00:00:00,000 --> 00:00:01,000\nOpening line\n\n2\n00:30:00,000 --> 00:30:02,000\nLater line\n").unwrap();
        let media = directory.join("captions.mks");
        let mut mux = Command::new(&ffmpeg);
        mux.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]).arg(&source)
            .args(["-c:s", "srt", "-f", "matroska"]).arg(&media);
        hide_console(&mut mux);
        assert!(mux.status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = engine(media.to_string_lossy().into_owned());
        Arc::get_mut(&mut engine.inner).unwrap().emit = Box::new(move |event| { let _ = tx.send(event); });
        *engine.inner.info.lock().unwrap() = Some(MediaInfo {
            duration: 1802.0, subtitles: vec![Track {
                language: "eng".into(), title: String::new(), codec: "srt".into(), stream_index: 0,
            }], ..Default::default()
        });
        engine.inner.started.store(true, Ordering::Release);
        // This is the normal background request, without pressing Load all.
        // At the old 4x rate, a 30-minute timestamp would take minutes.
        {
            let mut config = engine.inner.config.lock().unwrap();
            config.subtitle_track = Some(0);
            config.subtitle_revision = 1;
        }
        engine.inner.queue_full_subtitles(false);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut lines = Vec::new();
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("local subtitle scan was rate limited") {
                Event::Cues { cues, .. } => lines.extend(cues),
                Event::SubtitleProgress { done: true, .. } => break,
                _ => {}
            }
        }
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].start, 1800.0);

        // The near-playhead reader must publish its first cue before the
        // next cue arrives, rather than holding it in a 40-line batch.
        while rx.try_recv().is_ok() {}
        let mut command = Command::new(&ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-readrate", "1", "-i"])
            .arg(&media).args(["-flush_packets", "1", "-f", "srt", "pipe:1"])
            .stdout(Stdio::piped()).stderr(Stdio::null());
        hide_console(&mut command);
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let generation = engine.inner.generation.load(Ordering::Acquire);
        assert!(engine.inner.register_child(generation, child));
        let inner = engine.inner.clone();
        let reader = std::thread::spawn(move || {
            let keep = |inner: &Inner| inner.current(generation);
            inner.subtitle_loop(&keep, stdout, 0.0, false, 2);
        });
        match rx.recv_timeout(Duration::from_secs(3)).expect("current subtitle was buffered awaiting another cue") {
            Event::Cues { revision: 2, cues, .. } => assert_eq!(cues[0].text, "Opening line"),
            _ => panic!("expected the first current subtitle cue"),
        }
        engine.stop();
        reader.join().unwrap();
    }

    #[test]
    fn obsolete_video_cannot_release_audio_or_replace_the_picture() {
        let engine = engine(String::new());
        let inner = &engine.inner;
        inner.generation.store(2, Ordering::Release);
        inner.publish(2, &[0, 0, 255, 255], 1, 1);
        inner.publish(1, &[255, 0, 0, 255], 1, 1);
        inner.start_clock(1);
        assert!(!inner.started.load(Ordering::Acquire));
        assert_eq!(engine.take_frame().unwrap().decoded().pixels().as_ref(), &[0, 0, 255, 255]);
        inner.start_clock(2);
        assert!(inner.started.load(Ordering::Acquire));
    }

    #[test]
    fn obsolete_audio_cannot_change_the_new_clock() {
        let engine = engine(String::new());
        let inner = &engine.inner;
        inner.generation.store(2, Ordering::Release);
        inner.clock.lock().unwrap().audio = true;
        inner.fall_back_to_wall_clock(1);
        assert!(inner.clock.lock().unwrap().audio);
    }

    #[test]
    #[ignore = "requires FFmpeg on PATH; exercises rapid seeks with video, audio and subtitles"]
    fn rapid_seeks_keep_video_advancing_with_audio() {
        let (ffmpeg, _) = tools().expect("FFmpeg and FFprobe on PATH");
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/rapid-seek-test");
        std::fs::create_dir_all(&directory).unwrap();
        let subtitles = directory.join("captions.srt");
        std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:19,000\nSeek regression\n").unwrap();
        let video = directory.join("video.mkv");
        let mut command = Command::new(&ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y",
            "-f", "lavfi", "-i", "testsrc2=s=160x90:r=20:d=20",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=20"])
            .arg("-i").arg(&subtitles)
            .args(["-c:v", "ffv1", "-c:a", "pcm_s16le", "-c:s", "srt"]).arg(&video);
        hide_console(&mut command);
        assert!(command.status().unwrap().success());

        let engine = engine(video.to_string_lossy().into_owned());
        let inner = &engine.inner;
        let track = |stream_index| Track {
            language: String::new(), title: String::new(), codec: String::new(), stream_index,
        };
        *inner.info.lock().unwrap() = Some(MediaInfo {
            duration: 20.0, width: 160, height: 90, fps: 20.0, has_video: true,
            audio: vec![track(1)], subtitles: vec![track(2)], ..Default::default()
        });
        inner.config.lock().unwrap().subtitle_track = Some(0);
        inner.audio_device.store(true, Ordering::Release);
        engine.set_paused(true);
        // Queue requests while startup is blocked to force superseded workers
        // to compete for the lock, then also interrupt active process starts.
        {
            let _restart = inner.restart_lock.lock().unwrap();
            for second in 1..20 { engine.seek(second as f64 / 2.0); }
            engine.seek(3.0);
        }
        for second in 0..30 {
            std::thread::sleep(Duration::from_millis(8));
            engine.seek((second % 12) as f64);
        }
        engine.seek(12.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !inner.started.load(Ordering::Acquire) || inner.clock.lock().unwrap().base != 12.0 {
            assert!(Instant::now() < deadline, "latest seek did not produce a frame");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(engine.position(), 12.0, "paused seek must not advance");
        assert!(engine.take_frame().is_some(), "paused seek must update the picture");

        // Reproduce a subtitle spawn finishing after its seek was cancelled.
        // Rejecting it must leave the current video and audio children alive.
        let generation = inner.generation.load(Ordering::Acquire);
        let mut stale = Command::new(&ffmpeg);
        stale.args(["-nostdin", "-re", "-f", "lavfi", "-i", "color=s=16x16:d=20",
            "-f", "null", "-"]).stdout(Stdio::null()).stderr(Stdio::null());
        hide_console(&mut stale);
        assert!(!inner.register_child(generation - 1, stale.spawn().unwrap()));
        assert!(inner.children.lock().unwrap().iter_mut().all(|child| child.try_wait().unwrap().is_none()));

        // Drive the real PCM queue without requiring a sound device in CI.
        let mut audio = QueueSource {
            inner: inner.clone(), local: Vec::new(), cursor: 0, generation: 0, frame_count: 0,
        };
        engine.set_paused(false);
        let mut pictures = std::collections::HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while engine.position() < 13.0 {
            assert!(Instant::now() < deadline, "audio stopped after rapid seeks");
            for _ in 0..960 { let _ = audio.next(); }
            if let Some(frame) = engine.take_frame() {
                pictures.insert(frame.decoded().pixels().to_vec());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pictures.len() >= 10, "video froze while audio advanced: {} distinct frames", pictures.len());
        engine.stop();
        assert!(inner.children.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod audio_tests {
    use super::*;

    #[test]
    fn pcm_conversion_preserves_silence_polarity_and_full_scale() {
        assert_eq!(audio_sample(i16::MIN, 1.0), -1.0);
        assert_eq!(audio_sample(0, 1.0), 0.0);
        assert_eq!(audio_sample(i16::MAX, 1.0), 32767.0 / 32768.0);
        assert_eq!(audio_sample(i16::MAX, 0.0), 0.0);
        assert_eq!(audio_sample(16384, 0.5), 0.25);
    }

    #[test]
    fn boosted_audio_stays_bounded_without_flattening_quiet_samples() {
        assert_eq!(audio_sample(4096, MAX_VOLUME), 0.25);
        let positive = audio_sample(30000, MAX_VOLUME);
        assert!(positive > 0.8 && positive <= 1.0);
        assert_eq!(audio_sample(-30000, MAX_VOLUME), -positive);
    }
}

/// Leaves quiet samples alone and bends loud ones smoothly towards full
/// scale, so boosted volume compresses peaks instead of clipping them.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        x
    } else {
        let room = 1.0 - KNEE;
        x.signum() * (KNEE + room * ((magnitude - KNEE) / room).tanh())
    }
}

impl rodio::Source for QueueSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(SAMPLE_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Tone-maps HDR (PQ or HLG, BT.2020) to SDR BT.709.
/// Without it the HDR signal is shown as if it were SDR and looks
/// grey and washed out. Runs after downscaling, so it works on the output
/// size rather than the source's (often 4K). `zscale` reads the source
/// transfer and primaries from the frames. `mobius` keeps colours in the SDR
/// range nearly as they are and compresses only highlights above it
/// (`hable` dims everything by about 40%). It ends in 8-bit RGB, so the
/// final `rgba` conversion is only a repack, with no YUV matrix to guess.
/// This is the fallback for machines where libplacebo can't run. `npl=203`
/// maps HDR reference white (BT.2408) to SDR white, as libplacebo does;
/// 100 showed HDR about 15% too bright.
const HDR_TO_SDR: &str = ",zscale=t=linear:npl=203,format=gbrpf32le,zscale=p=bt709,\
    tonemap=tonemap=mobius:desat=0,zscale=t=bt709:r=full,format=gbrp";

/// The decoder's filter chain: frame rate, size and the colour conversion the
/// source needs to look right as SDR RGBA.
pub(super) fn video_filter(info: &MediaInfo, width: u32, height: u32, fps: f64, live: bool) -> String {
    let probe = if live { "showinfo," } else { "" };
    let rate = format!("fps={fps:.3}");
    if info.dolby_vision && libplacebo_available() {
        // libplacebo applies the Dolby Vision reshaping (from the metadata
        // the HEVC decoder attaches to each frame), tone-maps to SDR and
        // scales on the GPU, like mpv's gpu-next renderer. Only here: with
        // CPU-decoded frames it copies each one to the GPU and back, which
        // managed 22 fps from 4K, too slow for HDR10 films at 24 fps.
        return format!(
            "{probe}{rate},libplacebo=w={width}:h={height}:upscaler=bilinear:downscaler=bilinear:\
             colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc:format=rgba"
        );
    }
    let scale = format!("scale={width}:{height}:flags=fast_bilinear");
    if info.hdr && !info.dolby_vision {
        return format!("{probe}{rate},{scale}{HDR_TO_SDR}");
    }
    // Untagged video: guess the matrix from the size, as mpv does (HD and up
    // is BT.709). ffmpeg's guess is always BT.601, which shifts HD colours.
    let matrix = if !info.untagged {
        ""
    } else if info.width >= 1280 || info.height > 576 {
        ":in_color_matrix=bt709"
    } else {
        ":in_color_matrix=bt601"
    };
    format!("{probe}{rate},{scale}{matrix}")
}

/// Whether the bundled ffmpeg can run `libplacebo` here (it needs a working
/// Vulkan GPU driver). Checked once, on first use.
fn libplacebo_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let Some((ffmpeg, _)) = tools() else { return false };
        let mut command = Command::new(ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(["-f", "lavfi", "-i", "color=c=black:s=64x64:d=0.1"])
            .args(["-vf", "libplacebo=w=32:h=32:format=rgba", "-frames:v", "1", "-f", "null", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_console(&mut command);
        command.status().is_ok_and(|status| status.success())
    })
}

fn output_geometry(info: &MediaInfo, max_height: u32) -> (u32, u32, f64) {
    let (mut width, mut height) = (info.width.max(2), info.height.max(2));
    if max_height > 0 && height > max_height {
        width = (width as u64 * max_height as u64 / height as u64) as u32;
        height = max_height;
    }
    // Even dimensions keep every scaler path happy.
    let (width, height) = ((width & !1).max(2), (height & !1).max(2));
    let fps = if info.fps.is_finite() && info.fps > 1.0 {
        info.fps.min(30.0)
    } else {
        25.0
    };
    (width, height, fps)
}

fn parse_rate(value: &str) -> f64 {
    match value.split_once('/') {
        Some((n, d)) => {
            let (n, d) = (n.parse::<f64>().unwrap_or(0.0), d.parse::<f64>().unwrap_or(0.0));
            if d > 0.0 { n / d } else { 0.0 }
        }
        None => value.parse().unwrap_or(0.0),
    }
}

fn parse_probe(json: &[u8], max_height: u32, remote: bool) -> Option<MediaInfo> {
    let value: Value = serde_json::from_slice(json).ok()?;
    let streams = value.get("streams")?.as_array()?;
    let mut info = MediaInfo {
        duration: value
            .pointer("/format/duration")
            .and_then(Value::as_str)
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
        ..MediaInfo::default()
    };
    for stream in streams {
        let text = |key: &str| {
            stream
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let tag = |key: &str| {
            stream
                .get("tags")
                .and_then(Value::as_object)
                .and_then(|tags| tags.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let stream_index = stream.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let track = || Track {
            language: tag("language"),
            title: tag("title"),
            codec: text("codec_name"),
            stream_index,
        };
        match text("codec_type").as_str() {
            "video" => {
                let cover = stream
                    .pointer("/disposition/attached_pic")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    == 1;
                let width = stream.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                let height = stream.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                if cover || width == 0 || height == 0 {
                    continue;
                }
                // Adaptive streams expose every quality: take the best one
                // that fits the height cap, else the smallest.
                let fits = |h: u32| max_height == 0 || h <= max_height;
                let better = !info.has_video
                    || match (fits(height), fits(info.height)) {
                        (true, true) => height > info.height,
                        (true, false) => true,
                        (false, true) => false,
                        (false, false) => height < info.height,
                    };
                if !better {
                    continue;
                }
                info.has_video = true;
                info.video_index = stream_index;
                info.width = width;
                info.height = height;
                info.hdr = matches!(text("color_transfer").as_str(), "smpte2084" | "arib-std-b67");
                info.untagged = matches!(text("color_space").as_str(), "" | "unknown");
                info.dolby_vision = stream
                    .get("side_data_list")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find(|d| {
                        d.get("side_data_type").and_then(Value::as_str)
                            == Some("DOVI configuration record")
                    })
                    .is_some_and(|d| {
                        let number = |key: &str| d.get(key).and_then(Value::as_i64);
                        number("dv_profile") == Some(5)
                            || number("dv_bl_signal_compatibility_id") == Some(0)
                    });
                info.fps = parse_rate(&text("avg_frame_rate"));
                if info.fps <= 1.0 {
                    info.fps = parse_rate(&text("r_frame_rate"));
                }
                if info.width == 0 || info.height == 0 {
                    info.has_video = false;
                }
            }
            "audio" => info.audio.push(track()),
            "subtitle" => info.subtitles.push(track()),
            _ => {}
        }
    }
    info.live = remote && info.duration <= 0.0;
    (info.has_video || !info.audio.is_empty()).then_some(info)
}
