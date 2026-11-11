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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
    pub has_video: bool,
    /// Container index of the video stream chosen for playback.
    pub video_index: usize,
    /// No known duration over the network: play from the live edge, no seeking.
    pub live: bool,
    pub audio: Vec<Track>,
    pub subtitles: Vec<Track>,
}

pub enum Event {
    Info(MediaInfo),
    /// A new frame is waiting in [`Engine::take_frame`].
    Frame,
    Position { seconds: f64, stalled: bool },
    /// Cues from the embedded subtitle track; `reset` clears earlier ones.
    Cues { cues: Vec<Cue>, reset: bool },
    Ended,
    Error(String),
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
    paused: AtomicBool,
    /// Set once the first frame after a (re)start is ready, so sound and
    /// picture begin together.
    started: AtomicBool,
    consumed_frames: AtomicU64,
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
    pub fn open(request: OpenRequest, emit: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let inner = Arc::new(Inner {
            volume: AtomicU32::new(request.volume.to_bits()),
            request,
            emit: Box::new(emit),
            alive: AtomicBool::new(true),
            generation: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            started: AtomicBool::new(false),
            consumed_frames: AtomicU64::new(0),
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
                speed: 1.0,
                audio_delay: 0.0,
            }),
            last_error: Mutex::new(String::new()),
            last_restart: Mutex::new(Instant::now()),
            live_video_start: Mutex::new(None),
            live_audio_start: Mutex::new(None),
        });
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

    pub fn set_paused(&self, paused: bool) {
        self.inner.set_paused(paused);
        // A paused live stream falls out of the broadcast window: resuming
        // rejoins at the live edge.
        if !paused && self.inner.is_live() {
            let inner = self.inner.clone();
            std::thread::spawn(move || inner.restart(0.0));
        }
    }

    pub fn seek(&self, seconds: f64) {
        let inner = self.inner.clone();
        std::thread::spawn(move || inner.restart(seconds));
    }

    pub fn set_volume(&self, volume: f32) {
        self.inner
            .volume
            .store(volume.clamp(0.0, MAX_VOLUME).to_bits(), Ordering::Release);
    }

    pub fn set_speed(&self, speed: f64) {
        self.inner.config.lock().unwrap().speed = speed;
        self.seek(self.position());
    }

    pub fn set_audio_track(&self, index: usize) {
        self.inner.config.lock().unwrap().audio_track = index;
        self.seek(self.position());
    }

    /// Shifts the sound against the picture; restarts decoding in place.
    pub fn set_audio_delay(&self, seconds: f64) {
        self.inner.config.lock().unwrap().audio_delay = seconds;
        if !self.inner.is_live() {
            self.seek(self.position());
        }
    }

    /// Selects an embedded subtitle track to extract, or none.
    pub fn set_subtitle_track(&self, index: Option<usize>) {
        self.inner.config.lock().unwrap().subtitle_track = index;
        self.seek(self.position());
    }

    pub fn stop(&self) {
        self.inner.alive.store(false, Ordering::Release);
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        self.inner.kill_children();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Inner {
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

    fn set_paused(&self, paused: bool) {
        let position = self.now();
        let mut clock = self.clock.lock().unwrap();
        if !clock.audio {
            clock.anchor_position = position;
            clock.anchor = Instant::now();
        }
        self.paused.store(paused, Ordering::Release);
    }

    /// Marks the first frame ready and starts the wall clock.
    fn start_clock(&self) {
        let mut clock = self.clock.lock().unwrap();
        clock.anchor = Instant::now();
        self.started.store(true, Ordering::Release);
    }

    /// Audio ran out before the picture did: keep time by wall clock.
    fn fall_back_to_wall_clock(&self) {
        let position = self.now();
        let mut clock = self.clock.lock().unwrap();
        clock.audio = false;
        clock.anchor_position = position;
        clock.anchor = Instant::now();
    }

    fn kill_children(&self) {
        let children: Vec<Child> = self.children.lock().unwrap().drain(..).collect();
        if children.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for mut child in children {
                let _ = child.kill();
                let _ = child.wait();
            }
        });
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
            self.emit(Event::Error(
                "ffmpeg is missing from the Spoutpeak folder. Reinstall Spoutpeak or choose an external player in Settings."
                    .into(),
            ));
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
                self.children.lock().unwrap().push(child);
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
                self.emit(Event::Error(format!("could not run ffprobe: {error}")));
                return;
            }
        };
        if !self.alive.load(Ordering::Acquire) {
            return;
        }
        let remote = self.request.url.starts_with("http://") || self.request.url.starts_with("https://");
        let Some(info) = parse_probe(&output.0, self.request.max_height, remote) else {
            let reason = output.1.lines().last().unwrap_or("").trim().to_owned();
            self.emit(Event::Error(if reason.is_empty() {
                "This stream could not be opened.".into()
            } else {
                format!("This stream could not be opened: {reason}")
            }));
            return;
        };
        {
            let preferred = &self.request.preferred_audio;
            let mut config = self.config.lock().unwrap();
            config.audio_track = info
                .audio
                .iter()
                .position(|t| crate::util::same_language(&t.language, preferred))
                // Adaptive streams list one audio per variant: take the one
                // that travels with the chosen video.
                .or_else(|| {
                    info.audio
                        .iter()
                        .position(|t| info.has_video && t.stream_index > info.video_index)
                })
                .unwrap_or(0);
        }
        *self.info.lock().unwrap() = Some(info.clone());
        self.emit(Event::Info(info));
        self.clone().start_ticker();
        let start = self.request.start;
        self.restart(start);
    }

    /// Owns the sound device for the engine's lifetime.
    fn start_audio_output(self: Arc<Self>) {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
        let inner = self.clone();
        let _ = std::thread::Builder::new()
            .name("player-audio-out".into())
            .spawn(move || {
                let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
                    let _ = ready_tx.send(false);
                    return;
                };
                let Ok(sink) = rodio::Sink::try_new(&handle) else {
                    let _ = ready_tx.send(false);
                    return;
                };
                sink.append(QueueSource {
                    inner: inner.clone(),
                    local: Vec::new(),
                    cursor: 0,
                    generation: 0,
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
                while self.alive.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(200));
                    let seconds = self.now();
                    let paused = self.paused.load(Ordering::Acquire);
                    if !paused && (seconds - previous).abs() < 0.001 {
                        still += 1;
                    } else {
                        still = 0;
                    }
                    previous = seconds;
                    self.emit(Event::Position {
                        seconds,
                        stalled: still >= 3,
                    });
                }
            });
    }

    /// (Re)starts decoding at `position`. Used for open, seek, track and
    /// speed changes, and reconnects.
    fn restart(self: Arc<Self>, position: f64) {
        let Some((ffmpeg, _)) = tools() else { return };
        let Some(info) = self.info.lock().unwrap().clone() else {
            return;
        };
        if !self.alive.load(Ordering::Acquire) {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.kill_children();
        self.audio_queue.lock().unwrap().clear();
        *self.last_restart.lock().unwrap() = Instant::now();
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
            self.started.store(!info.has_video, Ordering::Release);
            *clock = ClockState {
                base: position,
                speed: config.speed,
                audio: use_audio,
                anchor: Instant::now(),
                anchor_position: position,
            };
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
            } else {
                command.args(["-ss", &seek]);
            }
            self.input_args(&mut command);
            command
                .arg("-i")
                .arg(&self.request.url)
                .args(["-map", &format!("0:{}", info.video_index), "-an", "-sn", "-dn"])
                .args([
                    "-vf",
                    &format!(
                        "{}fps={fps:.3},scale={width}:{height}:flags=fast_bilinear",
                        if live { "showinfo," } else { "" }
                    ),
                ])
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
                    self.children.lock().unwrap().push(child);
                    if let Some(stderr) = stderr {
                        let inner = self.clone();
                        std::thread::spawn(move || {
                            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                                if live {
                                    inner.note_live_start(generation, &line, true);
                                } else if !line.trim().is_empty() {
                                    *inner.last_error.lock().unwrap() = line;
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
                Err(error) => self.emit(Event::Error(format!("could not run ffmpeg: {error}"))),
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
            } else {
                command.args(["-ss", &format!("{:.3}", audio_from.max(0.0))]);
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
            if let Ok(mut child) = command.spawn() {
                let stdout = child.stdout.take();
                if let Some(stderr) = child.stderr.take() {
                    let inner = self.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                            inner.note_live_start(generation, &line, false);
                        }
                    });
                }
                self.children.lock().unwrap().push(child);
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
        }

        if let Some(track) = config.subtitle_track.filter(|_| !live) {
            self.emit(Event::Cues {
                cues: Vec::new(),
                reset: true,
            });
            // Start a little early so a cue already on screen is included.
            let from = (position - 15.0).max(0.0);
            let mut command = self.ffmpeg(&ffmpeg);
            command.args(["-ss", &format!("{from:.3}")]);
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
                .args(["-f", "srt", "pipe:1"]);
            if let Ok(mut child) = command.spawn() {
                let stdout = child.stdout.take();
                self.children.lock().unwrap().push(child);
                if let Some(stdout) = stdout {
                    let inner = self.clone();
                    std::thread::spawn(move || inner.subtitle_loop(generation, stdout, from));
                }
            }
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
        if slot.is_none() {
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

    fn publish(&self, pixels: &[u8], width: u32, height: u32) {
        let Ok(handle) = ImageHandle::from_rgba8(width, height, Arc::<[u8]>::from(pixels)) else {
            return;
        };
        *self.frame.lock().unwrap() = Some(handle);
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
                self.publish(&buffer, width, height);
                if live && uses_audio {
                    base += self.live_offset(generation);
                }
                self.start_clock();
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
            self.publish(&buffer, width, height);
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
        if has_video || !decoded_any {
            self.fall_back_to_wall_clock();
        } else {
            let position = self.now();
            self.finished(generation, position);
        }
    }

    fn subtitle_loop(self: Arc<Self>, generation: u64, stdout: ChildStdout, offset: f64) {
        let mut block = String::new();
        let mut batch = Vec::new();
        let mut last_flush = Instant::now();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !self.current(generation) {
                return;
            }
            if line.trim().is_empty() {
                if let Some(cue) = parse_block(&block, offset) {
                    batch.push(cue);
                }
                block.clear();
                if batch.len() >= 40 || last_flush.elapsed() > Duration::from_millis(400) {
                    if !batch.is_empty() {
                        self.emit(Event::Cues {
                            cues: std::mem::take(&mut batch),
                            reset: false,
                        });
                    }
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
        if !batch.is_empty() && self.current(generation) {
            self.emit(Event::Cues {
                cues: batch,
                reset: false,
            });
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
            self.emit(Event::Ended);
            return;
        }
        // Dropped connection mid-stream: reconnect, unless it keeps failing.
        let recently = self.last_restart.lock().unwrap().elapsed() < Duration::from_secs(8);
        if recently {
            let reason = self.last_error.lock().unwrap().clone();
            self.emit(Event::Error(if reason.is_empty() {
                "The stream stopped unexpectedly.".into()
            } else {
                format!("Playback failed: {reason}")
            }));
        } else {
            self.restart(position);
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
}

impl Iterator for QueueSource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let inner = &self.inner;
        if !inner.alive.load(Ordering::Relaxed) {
            return None;
        }
        if inner.paused.load(Ordering::Relaxed) || !inner.started.load(Ordering::Relaxed) {
            return Some(0);
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
                return Some(0);
            }
        }
        let sample = self.local[self.cursor];
        self.cursor += 1;
        if self.cursor % 2 == 0 {
            inner.consumed_frames.fetch_add(1, Ordering::Relaxed);
        }
        let volume = f32::from_bits(inner.volume.load(Ordering::Relaxed));
        let scaled = sample as f32 / 32768.0 * volume;
        let out = if volume > 1.0 { soft_clip(scaled) } else { scaled };
        Some((out * 32767.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
    }
}

/// Highest playback gain (200 %).
pub const MAX_VOLUME: f32 = 2.0;

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
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
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
                .pointer(&format!("/tags/{key}"))
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
