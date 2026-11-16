//! Playback orchestration: turning an addon stream into something playing,
//! tracking progress into the library, subtitles, and next-episode binge.

pub mod engine;
pub mod subtitles;
pub mod source;
pub mod preview;

use crate::state::{App, Load, Route, Shared, StreamGroup, spawn, ui};
use crate::store::{PlayerKind, Settings, SubtitleStyle};
use crate::addons::{self, http};
use crate::streaming::{self, TorrentStats};
use crate::addons::types::*;
use crate::util::same_language;
use engine::{Engine, Event, MediaInfo, OpenRequest, Track};
use incular::prelude::*;
use std::cell::{Cell, RefCell};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Instant;
use subtitles::Cue;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Opening,
    Playing,
    Ended,
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    None,
    Audio,
    Subtitles,
    Speed,
    /// The subtitle line list used to fix timing by picking a line.
    Sync,
    /// Subtitle appearance (colours, opacity, edges).
    SubtitleStyle,
    /// Picture adjustments (brightness).
    Picture,
    /// Watch-together room: people and chat.
    Together,
    More,
    Peers,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubChoice {
    Off,
    Embedded(usize),
    Addon(usize),
}

pub struct PlayerState {
    pub title: String,
    pub subtitle: String,
    pub status: Status,
    /// What is happening while the stream opens.
    pub stage: String,
    pub duration: f64,
    pub live: bool,
    pub paused: bool,
    pub stalled: bool,
    pub has_video: bool,
    pub volume: f32,
    pub muted: bool,
    pub speed: f64,
    pub audio_tracks: Vec<Track>,
    pub audio_index: usize,
    /// Seconds the sound lags the picture (negative: plays earlier).
    pub audio_delay: f64,
    pub embedded_subs: Vec<Track>,
    pub addon_subs: Vec<Subtitle>,
    pub sub_choice: SubChoice,
    pub sub_delay: f64,
    /// How far into the video the selected subtitle's lines are known
    /// (seconds), while they are still loading.
    pub sub_loaded_until: f64,
    /// Every line of the selected subtitle is loaded.
    pub sub_loaded: bool,
    pub sub_failed: bool,
    pub sub_size: f32,
    pub sub_style: SubtitleStyle,
    pub brightness: f32,
    pub controls_visible: bool,
    pub menu: Menu,
    pub torrent: Option<TorrentStats>,
    pub next: Option<Video>,
    /// Whole seconds left while the "Up next" card is showing.
    pub up_next: Option<u32>,
    /// The card was dismissed: no card and no autoplay for this video.
    pub up_next_cancelled: bool,
}

struct Session {
    meta: Option<Meta>,
    video: Option<Video>,
    stream: Option<Stream>,
    torrent_url: Option<String>,
    preferred_sub: String,
    /// Which text track of the chosen language to keep across episodes.
    preferred_sub_ordinal: usize,
    auto_sub_done: bool,
    last_position: f64,
    unsaved_ms: f64,
    last_record: Instant,
    last_push: Instant,
    last_activity: Instant,
    last_stats: Instant,
}

#[derive(Clone)]
pub(crate) struct PendingEpisode {
    session_id: u64,
    meta: Meta,
    video: Video,
    group: Option<String>,
}

/// Prefer the same release across episodes; wait for providers before
/// falling back to another source that the built-in player can open.
fn next_stream(groups: &[StreamGroup], wanted: Option<&str>) -> Option<Stream> {
    let playable = groups.iter().filter_map(|group| match &group.items {
        Load::Ready(streams) => Some(streams),
        _ => None,
    }).flatten().filter(|stream| matches!(stream.source(), StreamSource::Url(_) | StreamSource::Torrent { .. }));
    if let Some(wanted) = wanted {
        if let Some(stream) = playable.clone().find(|stream| stream.behavior_hints.binge_group.as_deref() == Some(wanted)) {
            return Some(stream.clone());
        }
        if groups.iter().any(|group| matches!(group.items, Load::Loading)) {
            return None;
        }
    }
    playable.into_iter().next().cloned()
}

#[derive(Clone)]
/// Playback following is independent of scroll activity driven by the user.
pub struct SubtitleFollow {
    enabled: Rc<Cell<bool>>,
    pending: Rc<Cell<bool>>,
    pressed: Rc<Cell<bool>>,
    automatic: Rc<Cell<bool>>,
}

impl SubtitleFollow {
    fn new() -> Self {
        Self {
            enabled: Rc::new(Cell::new(true)),
            pending: Rc::new(Cell::new(false)),
            pressed: Rc::new(Cell::new(false)),
            automatic: Rc::new(Cell::new(false)),
        }
    }

    fn resume(&self) { self.enabled.set(true); }
    pub fn suspend(&self) { self.enabled.set(false); }
    pub fn set_pressed(&self, pressed: bool) { self.pressed.set(pressed); }

    pub fn on_scroll(&self, event: ScrollNotification) {
        if !self.automatic.get() && (event.kind == ScrollNotificationType::UserScroll
            || event.drag_position.is_some()
            || (self.pressed.get() && event.kind == ScrollNotificationType::Update && event.delta != 0.0))
        {
            self.suspend();
        }
    }
}

#[derive(Clone)]
pub struct PlayerUi {
    pub frame: Signal<Option<ImageHandle>>,
    pub preview: preview::SeekPreview,
    pub position: Signal<f64>,
    pub subtitle_follow: SubtitleFollow,
    pub cue: Signal<String>,
    /// Window mode is independent of playback updates and drives the app frame.
    pub fullscreen: Signal<bool>,
    fullscreen_restore_maximized: Rc<Cell<bool>>,
    /// Position being previewed while the seek bar is dragged.
    pub scrub: Rc<Cell<Option<f64>>>,
    pub state: Shared<PlayerState>,
    engine: Rc<RefCell<Option<Engine>>>,
    cues: Rc<RefCell<Vec<Cue>>>,
    session_id: Rc<Cell<u64>>,
    session: Rc<RefCell<Session>>,
    pub(crate) pending_binge: Rc<RefCell<Option<PendingEpisode>>>,
    subtitle_revision: Rc<Cell<u64>>,
    pub window: Rc<RefCell<Option<WindowHandle>>>,
}

impl PlayerUi {
    pub fn new(settings: &Settings) -> Self {
        Self {
            frame: Signal::new(None),
            preview: preview::SeekPreview::new(),
            position: Signal::new(0.0),
            subtitle_follow: SubtitleFollow::new(),
            cue: Signal::new(String::new()),
            fullscreen: Signal::new(false),
            fullscreen_restore_maximized: Rc::new(Cell::new(false)),
            scrub: Rc::new(Cell::new(None)),
            state: Shared::new(PlayerState {
                title: String::new(),
                subtitle: String::new(),
                status: Status::Idle,
                stage: String::new(),
                duration: 0.0,
                live: false,
                paused: false,
                stalled: false,
                has_video: true,
                volume: settings.volume,
                muted: false,
                speed: 1.0,
                audio_tracks: Vec::new(),
                audio_index: 0,
                audio_delay: 0.0,
                embedded_subs: Vec::new(),
                addon_subs: Vec::new(),
                sub_choice: SubChoice::Off,
                sub_delay: 0.0,
                sub_loaded_until: 0.0,
                sub_loaded: false,
                sub_failed: false,
                sub_size: settings.subtitle_size,
                sub_style: settings.subtitle_style.clone(),
                brightness: settings.brightness,
                controls_visible: true,
                menu: Menu::None,
                torrent: None,
                next: None,
                up_next: None,
                up_next_cancelled: false,
            }),
            engine: Rc::new(RefCell::new(None)),
            cues: Rc::new(RefCell::new(Vec::new())),
            session_id: Rc::new(Cell::new(0)),
            session: Rc::new(RefCell::new(Session {
                meta: None,
                video: None,
                stream: None,
                torrent_url: None,
                preferred_sub: String::new(),
                preferred_sub_ordinal: 0,
                auto_sub_done: false,
                last_position: 0.0,
                unsaved_ms: 0.0,
                last_record: Instant::now(),
                last_push: Instant::now(),
                last_activity: Instant::now(),
                last_stats: Instant::now(),
            })),
            pending_binge: Rc::new(RefCell::new(None)),
            subtitle_revision: Rc::new(Cell::new(0)),
            window: Rc::new(RefCell::new(None)),
        }
    }

    /// The loaded subtitle cues, in time order.
    pub fn cue_list(&self) -> Vec<Cue> {
        let mut cues = self.cues.borrow().clone();
        cues.sort_by(|a, b| a.start.total_cmp(&b.start));
        cues
    }

    /// The stream playing now, with its title and episode.
    pub fn current_media(&self) -> Option<(Stream, Option<Meta>, Option<Video>)> {
        let session = self.session.borrow();
        Some((session.stream.clone()?, session.meta.clone(), session.video.clone()))
    }

    /// How many subtitle lines are loaded.
    pub fn cue_count(&self) -> usize {
        self.cues.borrow().len()
    }

    fn with_engine<R>(&self, call: impl FnOnce(&Engine) -> R) -> Option<R> {
        self.engine.borrow().as_ref().map(call)
    }

    /// The engine's own clock, fresher than the position signal.
    pub(crate) fn precise_position(&self) -> Option<f64> {
        self.with_engine(|e| e.position())
    }

    pub(crate) fn nudge(&self, seconds: f64) {
        self.with_engine(|e| e.nudge(seconds));
    }
}

/// Title and (for series) episode line shown by the player.
fn titles(stream: &Stream, meta: Option<&Meta>, video: Option<&Video>) -> (String, String) {
    let title = meta
        .map(|m| m.title().to_owned())
        .or_else(|| stream.name.clone())
        .unwrap_or_else(|| "Video".to_owned());
    let is_series = meta.is_some_and(|m| m.videos.len() > 1);
    let subtitle = match (video, is_series) {
        (Some(video), true) => video.label(),
        _ => String::new(),
    };
    (title, subtitle)
}

fn open_externally(target: &str) -> Result<(), String> {
    open::that_detached(target).map_err(|e| format!("could not open {target}: {e}"))
}

fn find_program(names: &[&str], extra_dirs: &[&str]) -> Option<std::path::PathBuf> {
    for dir in extra_dirs {
        for name in names {
            let candidate = std::path::Path::new(dir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

fn launch_external(
    kind: PlayerKind,
    custom_path: &str,
    url: &str,
    title: &str,
    start: f64,
    headers: &[(String, String)],
) -> Result<(), String> {
    let mut command = match kind {
        PlayerKind::Vlc => {
            let program = find_program(
                &["vlc.exe", "vlc"],
                &[
                    r"C:\Program Files\VideoLAN\VLC",
                    r"C:\Program Files (x86)\VideoLAN\VLC",
                    "/Applications/VLC.app/Contents/MacOS",
                ],
            )
            .ok_or("VLC was not found")?;
            let mut command = Command::new(program);
            command.arg(url).arg(format!("--meta-title={title}"));
            if start > 1.0 {
                command.arg(format!("--start-time={start:.0}"));
            }
            command
        }
        PlayerKind::Mpv => {
            let program = find_program(
                &["mpv.exe", "mpv"],
                &[r"C:\Program Files\mpv", "/Applications/mpv.app/Contents/MacOS"],
            )
            .ok_or("mpv was not found")?;
            let mut command = Command::new(program);
            command.arg(url).arg(format!("--force-media-title={title}"));
            if start > 1.0 {
                command.arg(format!("--start={start:.0}"));
            }
            if !headers.is_empty() {
                let fields = headers
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                command.arg(format!("--http-header-fields={fields}"));
            }
            command
        }
        PlayerKind::Custom => {
            if custom_path.trim().is_empty() {
                return Err("Set the external player path in Settings first".into());
            }
            let mut command = Command::new(custom_path.trim());
            command.arg(url);
            command
        }
        PlayerKind::Builtin => return Err("not an external player".into()),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start the player: {e}"))
}

impl App {
    // ------------------------------------------------------------ starting

    /// Plays `stream` for the item currently open in the detail view.
    fn remember_stream(&self, meta_id: &str, video_id: Option<&str>, stream: &Stream) {
        if meta_id.is_empty() {
            return;
        }
        let key = crate::store::last_stream_key(meta_id, video_id);
        self.profile.write(|p| {
            p.last_streams.retain(|entry| entry.key != key);
            p.last_streams.insert(0, crate::store::LastStream { key, stream: stream.clone() });
            p.last_streams.truncate(crate::store::LAST_STREAMS_KEPT);
        });
        self.save_profile();
    }

    /// The stream last played for this title / episode.
    pub fn last_stream(&self, meta_id: &str, video_id: Option<&str>) -> Option<Stream> {
        let key = crate::store::last_stream_key(meta_id, video_id);
        self.profile.peek(|p| p.last_streams.iter().find(|e| e.key == key).map(|e| e.stream.clone()))
    }

    pub fn play_stream(&self, stream: &Stream) {
        let (meta, video) = self.detail.peek(|d| (d.best_meta(), d.video.clone()));
        self.play_with(stream, meta, video);
    }

    /// Plays `stream` as part of `meta` / `video` (also used for a stream
    /// picked by a watch-together room).
    pub fn play_with(&self, stream: &Stream, meta: Option<Meta>, video: Option<Video>) {
        if matches!(stream.source(), StreamSource::Url(_) | StreamSource::Torrent { .. }) {
            if self.together_blocks_control() {
                return;
            }
            self.together_local_media(stream, meta.as_ref(), video.as_ref());
        }
        if let Some(meta) = &meta {
            self.remember_stream(meta.id(), video.as_ref().map(|v| v.id()), stream);
        }
        match stream.source() {
            StreamSource::External(url) => {
                if let Err(error) = open_externally(&url) {
                    self.notify(error);
                }
            }
            StreamSource::YouTube(id) => {
                let url = format!("https://www.youtube.com/watch?v={id}");
                if let Err(error) = open_externally(&url) {
                    self.notify(error);
                }
            }
            StreamSource::Unsupported => self.notify("This stream type is not supported"),
            StreamSource::Url(url) => self.begin_playback(url, stream.clone(), meta, video, false),
            StreamSource::Torrent {
                info_hash,
                file_idx,
                trackers,
            } => {
                let builtin = self.profile.peek(|p| p.settings.player == PlayerKind::Builtin);
                let ticket = if builtin {
                    // Show the player straight away, with progress.
                    let (preferred_sub, preferred_sub_ordinal) = self.opening_subtitle_preference(meta.as_ref(), video.as_ref());
                    self.finish_playback(true);
                    let (title, subtitle) = titles(stream, meta.as_ref(), video.as_ref());
                    self.player.frame.set(None);
                    self.player.state.write(|s| {
                        s.title = title;
                        s.subtitle = subtitle;
                        s.status = Status::Opening;
                        s.stage = "Joining the swarm…".into();
                        s.live = false;
                        s.duration = 0.0;
                        s.torrent = None;
                        s.next = None;
                        s.controls_visible = true;
                        s.menu = Menu::None;
                    });
                    // Source actions are available while the swarm prepares,
                    // including when the source cannot start playing yet.
                    {
                        let mut session = self.player.session.borrow_mut();
                        session.stream = Some(stream.clone());
                        session.meta = meta.clone();
                        session.video = video.clone();
                        session.preferred_sub = preferred_sub;
                        session.preferred_sub_ordinal = preferred_sub_ordinal;
                        session.auto_sub_done = false;
                    }
                    self.push(Route::Player);
                    Some(self.player.session_id.get())
                } else {
                    self.notify("Preparing torrent…");
                    None
                };
                if let Some(ticket) = ticket {
                    self.poll_opening_stats(ticket, info_hash.clone());
                }
                let stream = stream.clone();
                spawn(
                    async move { streaming::stream_url(&info_hash, file_idx, &trackers).await },
                    move |app, result| {
                        if let Some(ticket) = ticket {
                            let still_waiting = app.player.session_id.get() == ticket
                                && app.route.get_untracked().last() == Some(&Route::Player);
                            if !still_waiting {
                                return;
                            }
                        }
                        match result {
                            Ok(url) => app.begin_playback(url, stream, meta, video, true),
                            Err(error) if ticket.is_some() => app.player.state.write(|s| {
                                s.status = Status::Error(error);
                            }),
                            Err(error) => app.notify(error),
                        }
                    },
                );
            }
        }
    }

    /// Plays a local file or arbitrary URL with no library bookkeeping.
    pub fn play_url(&self, url: String) {
        let title = url
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&url)
            .to_owned();
        let stream = Stream {
            url: Some(url.clone()),
            name: Some(title),
            ..Stream::default()
        };
        self.begin_playback(url, stream, None, None, false);
    }

    fn begin_playback(
        &self,
        url: String,
        stream: Stream,
        meta: Option<Meta>,
        video: Option<Video>,
        torrent: bool,
    ) {
        let settings = self.profile.peek(|p| p.settings.clone());
        let headers: Vec<(String, String)> = stream
            .behavior_hints
            .proxy_headers
            .request
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let (title, subtitle) = titles(&stream, meta.as_ref(), video.as_ref());
        let video_id = video.as_ref().map(|v| v.id().to_owned());
        let start = meta
            .as_ref()
            .filter(|_| settings.resume_playback)
            .and_then(|m| {
                self.library.peek(|l| {
                    l.get(m.id()).and_then(|item| {
                        let state = &item.state;
                        let same = state.video_id == video_id;
                        let unfinished = state.duration <= 0.0
                            || state.time_offset < state.duration * 0.95;
                        (same && unfinished && state.time_offset > 5000.0)
                            .then_some(state.time_offset / 1000.0)
                    })
                })
            })
            .unwrap_or(0.0);

        if settings.player != PlayerKind::Builtin {
            let full_title = if subtitle.is_empty() {
                title
            } else {
                format!("{title} - {subtitle}")
            };
            match launch_external(
                settings.player,
                &settings.custom_player_path,
                &url,
                &full_title,
                start,
                &headers,
            ) {
                Ok(()) => self.notify(format!("Opened in {}", settings.player.label())),
                Err(error) => self.notify(error),
            }
            return;
        }

        let (preferred_sub, preferred_sub_ordinal) = self.opening_subtitle_preference(meta.as_ref(), video.as_ref());
        self.finish_playback(true);
        let session_id = self.player.session_id.get() + 1;
        self.player.session_id.set(session_id);
        let next = match (&meta, &video_id) {
            (Some(meta), Some(id)) => {
                let ordered = meta.ordered_videos();
                ordered
                    .iter()
                    .position(|v| v.id() == id)
                    .and_then(|i| ordered.get(i + 1).cloned())
            }
            _ => None,
        };
        *self.player.session.borrow_mut() = Session {
            meta: meta.clone(),
            video: video.clone(),
            stream: Some(stream.clone()),
            torrent_url: torrent.then(|| url.clone()),
            preferred_sub,
            preferred_sub_ordinal,
            auto_sub_done: false,
            last_position: start,
            unsaved_ms: 0.0,
            last_record: Instant::now(),
            last_push: Instant::now(),
            last_activity: Instant::now(),
            last_stats: Instant::now(),
        };
        self.player.cues.borrow_mut().clear();
        self.player.cue.set(String::new());
        self.player.frame.set(None);
        self.player.position.set(start);
        self.player.state.write(|s| {
            s.title = title;
            s.subtitle = subtitle;
            s.status = Status::Opening;
            s.stage = if torrent {
                "Buffering the first pieces…".into()
            } else {
                "Reading media information…".into()
            };
            s.duration = 0.0;
            s.live = false;
            s.paused = false;
            s.stalled = false;
            s.has_video = true;
            s.speed = settings.default_speed;
            s.audio_tracks.clear();
            s.audio_index = 0;
            s.audio_delay = 0.0;
            s.embedded_subs.clear();
            s.addon_subs = stream.subtitles.clone();
            s.sub_choice = SubChoice::Off;
            s.sub_delay = 0.0;
            s.sub_loaded_until = 0.0;
            s.sub_loaded = false;
            s.sub_failed = false;
            s.sub_size = settings.subtitle_size;
            s.controls_visible = true;
            s.menu = Menu::None;
            s.torrent = None;
            s.next = next;
            s.up_next = None;
            s.up_next_cancelled = false;
        });
        let (volume, muted) = self.player.state.peek(|s| (s.volume, s.muted));
        self.player.preview.configure(url.clone(), headers.clone(), torrent);
        let engine = Engine::open(
            OpenRequest {
                url,
                headers,
                start,
                max_height: settings.max_video_height,
                hardware_decoding: settings.hardware_decoding,
                volume: if muted { 0.0 } else { volume },
                preferred_audio: settings.audio_lang.clone(),
            },
            move |event| ui(move |app| app.player_event(session_id, event)),
        );
        if (settings.default_speed - 1.0).abs() > 0.01 {
            engine.set_speed(settings.default_speed);
        }
        *self.player.engine.borrow_mut() = Some(engine);
        self.push(Route::Player);
        if torrent {
            if let Some(url) = self.player.session.borrow().torrent_url.clone() {
                self.poll_opening_stats(session_id, url);
            }
        }
        self.fetch_addon_subtitles(session_id, &stream, meta.as_ref(), video.as_ref());
    }

    /// Keeps swarm statistics fresh while a torrent is still opening (the
    /// regular tick only starts once the media has been read).
    fn poll_opening_stats(&self, session_id: u64, source: String) {
        spawn(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                let stats = streaming::stats(&source);
                (source, stats)
            },
            move |app, (source, stats)| {
                let opening = app.player.session_id.get() == session_id
                    && app.player.state.peek(|s| s.status == Status::Opening);
                if !opening {
                    return;
                }
                if stats.is_some() {
                    app.player.state.write(|s| s.torrent = stats);
                }
                app.poll_opening_stats(session_id, source);
            },
        );
    }

    fn fetch_addon_subtitles(
        &self,
        session_id: u64,
        stream: &Stream,
        meta: Option<&Meta>,
        video: Option<&Video>,
    ) {
        let (Some(meta), Some(video)) = (meta, video) else {
            return;
        };
        let kind = meta.kind.clone();
        let video_id = video.id().to_owned();
        let mut extra = Vec::new();
        let hints = &stream.behavior_hints;
        if let Some(hash) = &hints.video_hash {
            extra.push(("videoHash".to_owned(), hash.clone()));
        }
        if let Some(size) = hints.video_size {
            extra.push(("videoSize".to_owned(), size.to_string()));
        }
        if let Some(name) = &hints.filename {
            extra.push(("filename".to_owned(), name.clone()));
        }
        let addons: Vec<Descriptor> = self.profile.peek(|p| {
            p.addons
                .iter()
                .filter(|a| a.manifest.supports("subtitles", &kind, &video_id))
                .cloned()
                .collect()
        });
        for descriptor in addons {
            let (kind, video_id, extra) = (kind.clone(), video_id.clone(), extra.clone());
            spawn(
                async move { addons::subtitles(&descriptor, &kind, &video_id, &extra).await },
                move |app, result| {
                    if app.player.session_id.get() != session_id {
                        return;
                    }
                    let Ok(found) = result else { return };
                    app.player.state.write(|s| {
                        for subtitle in found {
                            if !s.addon_subs.iter().any(|x| x.url == subtitle.url) {
                                s.addon_subs.push(subtitle);
                            }
                        }
                    });
                    app.auto_select_subtitle();
                },
            );
        }
    }

    /// Turns on the preferred-language subtitle once, when one shows up.
    fn auto_select_subtitle(&self) {
        let (done, preferred, ordinal) = {
            let session = self.player.session.borrow();
            (session.auto_sub_done, session.preferred_sub.clone(), session.preferred_sub_ordinal)
        };
        if done || preferred.is_empty() || preferred == "off" {
            return;
        }
        let choice = self.player.state.peek(|s| {
            // Addons may answer before FFprobe. Give embedded tracks the
            // first choice once their language metadata is available.
            if s.status == Status::Opening || s.sub_choice != SubChoice::Off {
                return None;
            }
            let matching: Vec<usize> = s.embedded_subs
                .iter()
                .enumerate()
                .filter(|(_, t)| t.is_text_subtitle() && same_language(&t.language, &preferred))
                .map(|(index, _)| index).collect();
            matching.get(ordinal).or_else(|| matching.first()).copied()
                .map(SubChoice::Embedded)
                .or_else(|| {
                    s.addon_subs
                        .iter()
                        .position(|t| {
                            same_language(t.lang.as_deref().unwrap_or(""), &preferred)
                        })
                        .map(SubChoice::Addon)
                })
        });
        if let Some(choice) = choice {
            self.player.session.borrow_mut().auto_sub_done = true;
            self.select_subtitle(choice);
        }
    }

    // -------------------------------------------------------------- events

    fn player_event(&self, session_id: u64, event: Event) {
        if self.player.session_id.get() != session_id {
            return;
        }
        let generation = match &event {
            Event::Position { generation, .. } | Event::Ended { generation } | Event::Error { generation, .. } => Some(*generation),
            _ => None,
        };
        if let Some(generation) = generation {
            if !self.player.with_engine(|engine| engine.is_current_generation(generation)).unwrap_or(false) {
                return;
            }
        }
        match event {
            Event::Info(info) => self.player_info(info),
            Event::Frame => {
                let frame = self.player.with_engine(Engine::take_frame).flatten();
                if let Some(frame) = frame {
                    self.player.frame.set(Some(frame));
                }
            }
            Event::Position { seconds, stalled, .. } => self.player_tick(seconds, stalled),
            Event::Cues { revision, cues, reset } => {
                if revision != self.player.subtitle_revision.get() || !matches!(
                    self.player.state.peek(|s| s.sub_choice.clone()),
                    SubChoice::Embedded(_)
                ) {
                    return;
                }
                let mut all = self.player.cues.borrow_mut();
                if reset {
                    all.clear();
                }
                // The near-playhead reader and the whole-file reader overlap.
                for cue in cues {
                    let known = all.iter().rev().any(|c| {
                        (c.start - cue.start).abs() < 0.25 && c.text == cue.text
                    });
                    if !known {
                        all.push(cue);
                    }
                }
            }
            Event::SubtitleProgress { revision, until, done } => {
                if revision != self.player.subtitle_revision.get() || !matches!(
                    self.player.state.peek(|s| s.sub_choice.clone()),
                    SubChoice::Embedded(_)
                ) {
                    return;
                }
                self.player.state.write(|s| {
                    s.sub_loaded_until = until;
                    s.sub_loaded = done;
                });
            }
            Event::Ended { .. } => self.player_ended(),
            Event::Error { message, .. } => {
                self.player.with_engine(|engine| engine.set_paused(true));
                self.player.state.write(|s| {
                    s.status = Status::Error(message);
                    s.paused = true;
                    s.stalled = false;
                    s.controls_visible = true;
                });
            }
        }
    }

    fn player_info(&self, info: MediaInfo) {
        self.player.preview.set_info(info.clone());
        let audio_index = {
            let preferred = self.profile.peek(|p| p.settings.audio_lang.clone());
            info.preferred_audio_index(&preferred)
        };
        // Lets a read-ahead set in seconds work out how many bytes that is.
        if let Some(url) = self.player.session.borrow().torrent_url.as_deref() {
            streaming::set_duration(url, info.duration);
        }
        self.player.state.write(|s| {
            s.status = Status::Playing;
            s.duration = info.duration;
            s.live = info.live;
            s.has_video = info.has_video;
            s.audio_tracks = info.audio;
            s.audio_index = audio_index;
            s.embedded_subs = info.subtitles;
        });
        self.auto_select_subtitle();
        self.together_player_ready();
    }

    /// Clicking a control moves or clears keyboard focus; take it back so
    /// the player's shortcuts keep working.
    fn player_tick(&self, seconds: f64, stalled: bool) {
        if self.player.scrub.get().is_none() {
            self.player.position.set((seconds * 10.0).round() / 10.0);
        }

        let delay = self.player.state.peek(|s| s.sub_delay);
        if self.player.state.peek(|s| s.menu == Menu::Sync && !s.paused)
            && self.player.subtitle_follow.enabled.get()
        {
            self.queue_subtitle_follow();
        }
        let text = subtitles::active_text(&self.player.cues.borrow(), seconds - delay);
        if text != self.player.cue.get_untracked() {
            self.player.cue.set(text);
        }

        let (was_stalled, visible, menu, status) = self.player.state.peek(|s| {
            (
                s.stalled,
                s.controls_visible,
                s.menu,
                s.status.clone(),
            )
        });
        if status != Status::Playing {
            return;
        }
        self.update_up_next(seconds);
        let hide_after = self.profile.peek(|p| p.settings.controls_hide_seconds).max(1);
        let idle = self.player.session.borrow().last_activity.elapsed().as_secs_f64()
            > f64::from(hide_after);
        // The engine's wall-clock ticker continues while paused, so the same
        // idle timeout applies in either state. Menus and active scrubs stay up.
        let hide = visible && idle && menu == Menu::None && self.player.scrub.get().is_none();
        if hide {
            crate::ui::hide_player_tooltips();
            self.player.preview.cancel();
        }
        if was_stalled != stalled || hide {
            self.player.state.write(|s| {
                s.stalled = stalled;
                if hide {
                    s.controls_visible = false;
                }
            });
        }

        // Progress bookkeeping.
        let (record, push, stats_url) = {
            let mut session = self.player.session.borrow_mut();
            let delta = seconds - session.last_position;
            if delta > 0.0 && delta < 2.0 {
                session.unsaved_ms += delta * 1000.0;
            }
            session.last_position = seconds;
            let record = session.last_record.elapsed().as_secs() >= 5;
            let push = session.last_push.elapsed().as_secs() >= 45;
            if record {
                session.last_record = Instant::now();
            }
            if push {
                session.last_push = Instant::now();
            }
            let stats = (session.last_stats.elapsed().as_secs() >= 2)
                .then(|| session.torrent_url.clone())
                .flatten();
            if stats.is_some() {
                session.last_stats = Instant::now();
            }
            (record || push, push, stats)
        };
        if record {
            self.record_player_progress(seconds, false, push);
        }
        if let Some(url) = stats_url {
            if let Some(stats) = streaming::stats(&url) {
                self.player.state.write(|s| s.torrent = Some(stats));
            }
        }
        self.together_tick(stalled);
    }

    fn record_player_progress(&self, seconds: f64, ended: bool, push: bool) {
        let (meta, video, watched) = {
            let mut session = self.player.session.borrow_mut();
            let watched = std::mem::take(&mut session.unsaved_ms);
            (session.meta.clone(), session.video.clone(), watched)
        };
        let (Some(meta), Some(video)) = (meta, video) else {
            return;
        };
        let (duration, next) = self.player.state.peek(|s| (s.duration, s.next.clone()));
        if duration <= 0.0 && !ended {
            return;
        }
        self.record_progress(
            &meta,
            video.id(),
            seconds * 1000.0,
            duration * 1000.0,
            watched,
            ended,
            next.as_ref(),
            push,
        );
    }

    fn player_ended(&self) {
        let position = self.player.position.get_untracked();
        self.record_player_progress(position, true, true);
        let (next, cancelled) = self.player.state.peek(|s| (s.next.clone(), s.up_next_cancelled));
        let autoplay = self.profile.peek(|p| p.settings.autoplay_next) && !cancelled;
        self.player.state.write(|s| {
            s.status = Status::Ended;
            s.controls_visible = true;
            s.up_next = None;
        });
        if let (Some(next), true) = (next, autoplay) {
            self.play_next(next);
        }
    }

    /// Resolve the next episode inside the player, retaining window mode.
    pub fn play_next(&self, next: Video) {
        if self.together_blocks_control() || self.player.pending_binge.borrow().is_some() {
            return;
        }
        let (meta, group, preferred_sub, preferred_sub_ordinal) = {
            let session = self.player.session.borrow();
            if session.video.as_ref().map(Video::id) == Some(next.id()) {
                return;
            }
            (session.meta.clone(), session.stream.as_ref().and_then(|s| s.behavior_hints.binge_group.clone()),
                session.preferred_sub.clone(), session.preferred_sub_ordinal)
        };
        let Some(meta) = meta else { return };
        self.finish_playback(true);
        let (title, subtitle) = titles(&Stream::default(), Some(&meta), Some(&next));
        {
            let mut session = self.player.session.borrow_mut();
            session.meta = Some(meta.clone());
            session.video = Some(next.clone());
            session.stream = None;
            session.torrent_url = None;
            session.preferred_sub = preferred_sub;
            session.preferred_sub_ordinal = preferred_sub_ordinal;
            session.auto_sub_done = false;
            session.last_position = 0.0;
        }
        self.player.state.write(|s| {
            s.status = Status::Opening;
            s.stage = "Finding streams for the next episode…".into();
            s.title = title;
            s.subtitle = subtitle;
        });
        // Keep the detail selection current for Back without navigating to it.
        self.detail.write(|d| {
            d.kind = meta.kind.clone();
            d.id = meta.id().to_owned();
            d.preview = Some(meta.clone());
            d.meta = Load::Ready(meta.clone());
        });
        if let Some(season) = next.season {
            self.select_season(season);
        }
        *self.player.pending_binge.borrow_mut() = Some(PendingEpisode {
            session_id: self.player.session_id.get(), meta, video: next.clone(), group,
        });
        self.select_video(next);
    }

    /// Called as stream lists arrive for the selected video.
    pub fn try_binge(&self) {
        let pending = self.player.pending_binge.borrow().clone();
        let Some(pending) = pending else { return };
        if self.player.session_id.get() != pending.session_id || self.current_route() != Route::Player {
            self.player.pending_binge.borrow_mut().take();
            return;
        }
        let (stream, loading) = self.detail.peek(|d| {
            if d.video.as_ref().map(Video::id) != Some(pending.video.id()) {
                return (None, true);
            }
            (next_stream(&d.streams, pending.group.as_deref()),
                d.streams.iter().any(|group| matches!(group.items, Load::Loading)))
        });
        if let Some(stream) = stream {
            self.player.pending_binge.borrow_mut().take();
            self.play_with(&stream, Some(pending.meta), Some(pending.video));
        } else if !loading {
            self.player.pending_binge.borrow_mut().take();
            self.player.state.write(|s| s.status = Status::Error(
                "No playable streams were found for this episode. Go back to choose another source or episode.".into()
            ));
        }
    }

    pub fn stop_playback(&self) {
        self.finish_playback(false);
    }

    fn opening_subtitle_preference(&self, meta: Option<&Meta>, video: Option<&Video>) -> (String, usize) {
        let session = self.player.session.borrow();
        if self.player.state.peek(|s| s.status == Status::Opening)
            && session.meta.as_ref().map(Meta::id) == meta.map(Meta::id)
            && session.video.as_ref().map(Video::id) == video.map(Video::id)
        {
            (session.preferred_sub.clone(), session.preferred_sub_ordinal)
        } else {
            self.profile.peek(|p| (p.settings.subtitle_lang.clone(), 0))
        }
    }

    fn finish_playback(&self, replacing: bool) {
        self.player.subtitle_follow.suspend();
        self.player.subtitle_follow.set_pressed(false);
        crate::ui::hide_player_tooltips();
        self.player.preview.clear();
        // Also restore the window if playback is still preparing without an engine.
        if !replacing && self.player.fullscreen.get_untracked() {
            self.apply_fullscreen(false);
        }
        let engine = self.player.engine.borrow_mut().take();
        let had_engine = engine.is_some();
        if let Some(engine) = engine {
            if self.player.state.peek(|s| s.status != Status::Ended) {
                self.record_player_progress(engine.position(), false, true);
            }
            engine.stop();
        }
        self.player.session_id.set(self.player.session_id.get() + 1);
        self.player.subtitle_revision.set(self.player.subtitle_revision.get() + 1);
        self.player.pending_binge.borrow_mut().take();
        self.player.scrub.set(None);
        self.player.frame.set(None);
        self.player.position.set(0.0);
        self.player.cue.set(String::new());
        self.player.cues.borrow_mut().clear();
        {
            let mut session = self.player.session.borrow_mut();
            session.stream = None;
            session.meta = None;
            session.video = None;
            session.torrent_url = None;
            session.unsaved_ms = 0.0;
            session.last_position = 0.0;
        }
        let volume = self.player.state.write(|s| {
            s.status = Status::Idle;
            s.menu = Menu::None;
            s.duration = 0.0;
            s.live = false;
            s.paused = false;
            s.stalled = false;
            s.controls_visible = true;
            s.audio_tracks.clear();
            s.audio_index = 0;
            s.audio_delay = 0.0;
            s.embedded_subs.clear();
            s.addon_subs.clear();
            s.sub_choice = SubChoice::Off;
            s.sub_delay = 0.0;
            s.sub_loaded_until = 0.0;
            s.sub_loaded = false;
            s.sub_failed = false;
            s.next = None;
            s.up_next = None;
            s.up_next_cancelled = false;
            s.torrent = None;
            s.volume
        });
        if had_engine {
            self.update_settings(|s| s.volume = volume);
        }
    }

    // ------------------------------------------------------------ controls

    pub fn player_activity(&self) {
        self.player.session.borrow_mut().last_activity = Instant::now();
        if !self.player.state.peek(|s| s.controls_visible) {
            self.player.state.write(|s| s.controls_visible = true);
        }
    }

    pub fn toggle_pause(&self) {
        if self.together_blocks_control() {
            return;
        }
        if !self.player.state.peek(|s| matches!(s.status, Status::Playing | Status::Ended)) {
            return;
        }
        if self.player.state.peek(|s| s.status == Status::Ended) {
            self.seek_to(0.0);
            return;
        }
        let paused = !self.player.state.peek(|s| s.paused);
        self.player.with_engine(|e| e.set_paused(paused));
        self.player.state.write(|s| s.paused = paused);
        self.player_activity();
        if paused {
            let position = self.player.position.get_untracked();
            self.record_player_progress(position, false, true);
        }
        self.together_local_change();
    }

    pub fn seek_to(&self, seconds: f64) {
        if !seconds.is_finite() || !self.player.state.peek(|s| matches!(s.status, Status::Playing | Status::Ended)) {
            return;
        }
        if self.together_blocks_control() {
            return;
        }
        if self.player.state.peek(|s| s.live) {
            return;
        }
        let duration = self.player.state.peek(|s| s.duration);
        let seconds = if duration > 0.0 {
            seconds.clamp(0.0, (duration - 1.0).max(0.0))
        } else {
            seconds.max(0.0)
        };
        self.player.with_engine(|e| e.seek(seconds));
        self.player.session.borrow_mut().last_position = seconds;
        self.player.position.set(seconds);
        if self.player.state.peek(|s| s.status == Status::Ended) {
            self.player.with_engine(|e| e.set_paused(false));
            self.player.state.write(|s| {
                s.status = Status::Playing;
                s.paused = false;
                s.up_next_cancelled = false;
            });
        }
        self.player_activity();
        self.together_local_change();
    }

    pub fn seek_by(&self, delta: f64) {
        let position = self.player.position.get_untracked();
        self.seek_to(position + delta);
    }

    pub fn set_volume(&self, volume: f32) {
        // Snap to whole percents so keyboard steps land on 100 % exactly.
        let volume = (volume.clamp(0.0, engine::MAX_VOLUME) * 100.0).round() / 100.0;
        self.player.with_engine(|e| e.set_volume(volume));
        self.player.state.write(|s| {
            s.volume = volume;
            s.muted = false;
        });
        self.player_activity();
    }

    pub fn toggle_mute(&self) {
        let (muted, volume) = self.player.state.write(|s| {
            s.muted = !s.muted;
            (s.muted, s.volume)
        });
        self.player
            .with_engine(|e| e.set_volume(if muted { 0.0 } else { volume }));
    }

    pub fn set_speed(&self, speed: f64) {
        if self.together_blocks_control() {
            return;
        }
        if self.player.state.peek(|s| s.live) {
            self.player.state.write(|s| s.menu = Menu::None);
            return;
        }
        self.player.with_engine(|e| e.set_speed(speed));
        self.player.state.write(|s| {
            s.speed = speed;
            s.menu = Menu::None;
        });
        self.together_local_change();
    }

    pub fn select_audio(&self, index: usize) {
        if !self.player.state.peek(|s| index < s.audio_tracks.len() && s.audio_index != index) {
            return;
        }
        self.player.with_engine(|e| e.set_audio_track(index));
        self.player.state.write(|s| {
            s.audio_index = index;
            s.menu = Menu::None;
        });
    }

    pub fn select_subtitle(&self, choice: SubChoice) {
        let valid = self.player.state.peek(|s| match &choice {
            SubChoice::Off => true,
            SubChoice::Embedded(index) => s.embedded_subs.get(*index).is_some_and(Track::is_text_subtitle),
            SubChoice::Addon(index) => *index < s.addon_subs.len(),
        });
        if !valid { return; }
        let previous = self.player.state.peek(|s| s.sub_choice.clone());
        if previous == choice && !self.player.state.peek(|s| s.sub_failed) {
            if choice == SubChoice::Off {
                // Off may already be showing while providers are still
                // loading. An explicit choice must prevent later auto-select.
                let mut session = self.player.session.borrow_mut();
                session.auto_sub_done = true;
                session.preferred_sub = "off".into();
                session.preferred_sub_ordinal = 0;
            }
            self.player.state.write(|s| s.menu = Menu::None);
            return;
        }
        self.player.session.borrow_mut().auto_sub_done = true;
        let language = self.player.state.peek(|s| match &choice {
            SubChoice::Off => Some("off".to_owned()),
            SubChoice::Embedded(index) => s.embedded_subs.get(*index).map(|t| t.language.clone()),
            SubChoice::Addon(index) => s.addon_subs.get(*index).and_then(|t| t.lang.clone()),
        });
        if let Some(language) = language.filter(|language| !language.is_empty()) {
            let ordinal = self.player.state.peek(|s| match &choice {
                SubChoice::Embedded(index) => s.embedded_subs.iter().take(index + 1)
                    .filter(|track| track.is_text_subtitle() && same_language(&track.language, &language)).count().saturating_sub(1),
                _ => 0,
            });
            let mut session = self.player.session.borrow_mut();
            session.preferred_sub = language;
            session.preferred_sub_ordinal = ordinal;
        }
        let revision = self.player.subtitle_revision.get() + 1;
        self.player.subtitle_revision.set(revision);
        self.player.cues.borrow_mut().clear();
        self.player.cue.set(String::new());
        self.player.state.write(|s| {
            s.sub_choice = choice.clone();
            s.menu = Menu::None;
            s.sub_loaded_until = 0.0;
            s.sub_loaded = choice == SubChoice::Off;
            s.sub_failed = false;
        });
        match &choice {
            SubChoice::Embedded(index) => {
                let index = *index;
                self.player.with_engine(|e| e.set_subtitle_track(Some(index), revision));
            }
            SubChoice::Off | SubChoice::Addon(_) => {
                if matches!(previous, SubChoice::Embedded(_)) {
                    self.player.with_engine(|e| e.set_subtitle_track(None, revision));
                }
            }
        }
        if let SubChoice::Addon(index) = choice {
            let Some(url) = self
                .player
                .state
                .peek(|s| s.addon_subs.get(index).map(|t| t.url.clone()))
            else {
                return;
            };
            let session_id = self.player.session_id.get();
            spawn(
                async move {
                    let bytes = http().get(&url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
                    Some(subtitles::parse(&String::from_utf8_lossy(&bytes)))
                },
                move |app, cues: Option<Vec<Cue>>| {
                    app.addon_subtitle_loaded(session_id, revision, index, cues);
                },
            );
        }
    }

    fn addon_subtitle_loaded(&self, session_id: u64, revision: u64, index: usize, cues: Option<Vec<Cue>>) {
        if self.player.session_id.get() != session_id || self.player.subtitle_revision.get() != revision
            || !self.player.state.peek(|s| s.sub_choice == SubChoice::Addon(index)) {
            return;
        }
        match cues {
            Some(cues) if !cues.is_empty() => {
                let until = cues.iter().fold(0.0f64, |m, c| m.max(c.end));
                *self.player.cues.borrow_mut() = cues;
                self.player.state.write(|s| {
                    s.sub_loaded_until = until;
                    s.sub_loaded = true;
                    s.sub_failed = false;
                });
            }
            _ => {
                self.player.state.write(|s| s.sub_failed = true);
                self.notify("Could not load that subtitle. Select it again to retry.");
            }
        }
    }

    /// Reads the rest of the embedded subtitle track without the read-rate
    /// cap (uses more bandwidth on torrents).
    pub fn load_all_subtitles(&self) {
        self.player.with_engine(Engine::load_all_subtitles);
        self.player.state.write(|s| s.sub_loaded = false);
    }

    /// Height of one row in the subtitle sync list.
    pub const SYNC_ROW: f32 = 46.0;

    /// Opens the line list, scrolled to where the subtitles currently are.
    pub fn open_subtitle_sync(&self) {
        self.player.state.write(|s| s.menu = Menu::Sync);
        self.scroll_subtitle_sync_to_current();
    }

    /// Returns to the playing line or marker and resumes following playback.
    pub fn scroll_subtitle_sync_to_current(&self) {
        self.player.subtitle_follow.resume();
        self.player_activity();
        self.queue_subtitle_follow();
    }

    fn queue_subtitle_follow(&self) {
        if self.player.subtitle_follow.pending.replace(true) { return; }
        let app = self.clone();
        // Wait for the marker and list extents to reflect this playback tick.
        add_post_frame_callback(move |_| {
            let follow = &app.player.subtitle_follow;
            follow.pending.set(false);
            if !follow.enabled.get() || app.player.state.peek(|s| s.menu != Menu::Sync) { return; }
            let position = app.player.position.get_untracked();
            let delay = app.player.state.peek(|s| s.sub_delay);
            let cues = app.player.cue_list();
            let index = subtitles::position_at(&cues, position - delay).index;
            let controller = &app.controllers.sync_list;
            let offset = (index.saturating_sub(2)) as f32 * Self::SYNC_ROW;
            follow.automatic.set(true);
            controller.jump_to(offset.clamp(controller.min_offset(), controller.max_offset()));
            follow.automatic.set(false);
        });
        schedule_frame();
    }

    /// Re-times the subtitles so the line starting at `cue_start` is the one
    /// being spoken right now.
    pub fn sync_subtitle_to(&self, cue_start: f64) {
        let position = self.player.position.get_untracked();
        let delay = ((position - cue_start) * 20.0).round() / 20.0;
        self.player.state.write(|s| s.sub_delay = delay);
        self.player_activity();
        self.notify(format!("Subtitles shifted {delay:+.2}s"));
    }

    pub fn reset_subtitle_delay(&self) {
        self.player.state.write(|s| s.sub_delay = 0.0);
    }

    pub fn adjust_subtitle_delay(&self, delta: f64) {
        self.player
            .state
            .write(|s| s.sub_delay = ((s.sub_delay + delta) * 4.0).round() / 4.0);
    }

    /// Shows the "Up next" countdown near the end of an episode.
    fn update_up_next(&self, seconds: f64) {
        let lead = self.profile.peek(|p| p.settings.up_next_seconds);
        let (duration, has_next, cancelled, live, shown) = self.player.state.peek(|s| {
            (s.duration, s.next.is_some(), s.up_next_cancelled, s.live, s.up_next)
        });
        let remaining = duration - seconds;
        let wanted = (lead > 0 && has_next && !cancelled && !live && duration > 60.0
            && remaining <= f64::from(lead)
            && remaining > 0.0)
            .then(|| remaining.ceil() as u32);
        if wanted != shown {
            self.player.state.write(|s| s.up_next = wanted);
        }
    }

    pub fn cancel_up_next(&self) {
        self.player.state.write(|s| {
            s.up_next = None;
            s.up_next_cancelled = true;
        });
    }

    /// `C`: off -> first available track -> off.
    pub fn cycle_subtitles(&self) {
        let next = self.player.state.peek(|s| {
            if s.sub_choice != SubChoice::Off {
                return Some(SubChoice::Off);
            }
            s.embedded_subs
                .iter()
                .position(|t| t.is_text_subtitle())
                .map(SubChoice::Embedded)
                .or_else(|| (!s.addon_subs.is_empty()).then_some(SubChoice::Addon(0)))
        });
        match next {
            Some(choice) => {
                let off = choice == SubChoice::Off;
                self.select_subtitle(choice);
                self.notify(if off { "Subtitles off" } else { "Subtitles on" });
            }
            None => self.notify("No subtitles for this video"),
        }
    }

    pub fn adjust_audio_delay(&self, delta: f64) {
        let delay = self.player.state.write(|s| {
            s.audio_delay = ((s.audio_delay + delta).clamp(-10.0, 10.0) * 20.0).round() / 20.0;
            s.audio_delay
        });
        self.player.with_engine(|e| e.set_audio_delay(delay));
        self.player_activity();
    }

    pub fn reset_audio_delay(&self) {
        self.player.state.write(|s| s.audio_delay = 0.0);
        self.player.with_engine(|e| e.set_audio_delay(0.0));
        self.player_activity();
    }

    pub fn adjust_subtitle_size(&self, delta: f32) {
        let size = self.player.state.write(|s| {
            s.sub_size = (s.sub_size + delta).clamp(14.0, 64.0);
            s.sub_size
        });
        self.update_settings(|s| s.subtitle_size = size);
    }

    pub fn set_player_menu(&self, menu: Menu) {
        let opened = self.player.state.write(|s| {
            s.menu = if s.menu == menu { Menu::None } else { menu };
            s.menu
        });
        if opened == Menu::Together {
            self.together_mark_read();
        } else {
            // Back from the room panel's chat field: keys go to the player.
            self.controllers.player_focus.request_focus();
        }
        if opened != Menu::None && opened != Menu::Sync && opened != Menu::Together {
            // A freshly opened panel starts at its top; rebuilds while it
            // stays open keep the scroll position (shared controller).
            ui(|app| {
                app.controllers.player_menu.jump_to(0.0);
            });
        }
        self.player_activity();
    }

    /// Brightness multiplier applied to the picture (0.3 – 1.7).
    pub fn set_brightness(&self, brightness: f32) {
        let brightness = (brightness.clamp(0.3, 1.7) * 100.0).round() / 100.0;
        self.player.state.write(|s| s.brightness = brightness);
        self.update_settings(|s| s.brightness = brightness);
        self.player_activity();
    }

    pub fn adjust_brightness(&self, delta: f32) {
        let current = self.player.state.peek(|s| s.brightness);
        self.set_brightness(current + delta);
    }

    /// Changes the subtitle appearance and remembers it.
    pub fn update_subtitle_style(&self, change: impl FnOnce(&mut SubtitleStyle)) {
        let style = self.player.state.write(|s| {
            change(&mut s.sub_style);
            s.sub_style.clone()
        });
        self.update_settings(|s| s.subtitle_style = style);
        self.player_activity();
    }

    pub fn reset_subtitle_style(&self) {
        self.update_subtitle_style(|style| *style = SubtitleStyle::default());
        let size = Settings::default().subtitle_size;
        self.player.state.write(|s| s.sub_size = size);
        self.update_settings(|s| s.subtitle_size = size);
    }

    pub fn toggle_fullscreen(&self) {
        self.apply_fullscreen(!self.player.fullscreen.get_untracked());
    }

    fn apply_fullscreen(&self, fullscreen: bool) {
        let mode = fullscreen.then_some(Fullscreen::Borderless);
        let window = self.player.window.borrow().clone();
        if let Some(window) = window {
            let was_maximized = fullscreen && window.observed_state().maximized.unwrap_or(false);
            // An undecorated maximized window can keep a work-area-sized client
            // surface in borderless mode. Restore it before requesting fullscreen.
            let request = if was_maximized {
                window.set_maximized(false).and_then(|()| window.set_fullscreen(mode))
            } else {
                window.set_fullscreen(mode)
            };
            match request {
                Ok(()) => {
                    if fullscreen {
                        self.player.fullscreen_restore_maximized.set(was_maximized);
                    } else if self.player.fullscreen_restore_maximized.replace(false) {
                        if let Err(error) = window.set_maximized(true) {
                            self.notify(format!("Could not restore window size: {error}"));
                        }
                    }
                    self.player.fullscreen.set(fullscreen);
                }
                Err(error) => self.notify(format!("Could not change fullscreen: {error}")),
            }
            return;
        }
        // The handle is not available while the app is still being built,
        // so it is looked up the first time it is needed.
        crate::state::with_window(move |app, window| {
            if let Some(window) = window {
                *app.player.window.borrow_mut() = Some(window);
                app.apply_fullscreen(fullscreen);
            } else {
                app.notify("Fullscreen is not available");
            }
        });
    }
}

#[cfg(test)]
mod episode_transition_tests {
    use super::*;

    fn stream(url: &str, group: Option<&str>) -> Stream {
        Stream {
            url: Some(url.into()),
            behavior_hints: StreamHints { binge_group: group.map(str::to_owned), ..Default::default() },
            ..Default::default()
        }
    }

    fn group(items: Load<Vec<Stream>>) -> StreamGroup {
        StreamGroup { addon_name: "Provider".into(), items }
    }

    fn episode(number: u32) -> Video {
        Video { id: Some(format!("series:1:{number}")), season: Some(1), episode: Some(number), ..Default::default() }
    }

    fn app() -> App {
        App::with_profile(crate::store::Profile::default())
    }

    #[test]
    fn external_subtitles_wait_for_embedded_language_metadata_before_auto_selection() {
        let app = app();
        app.player.session.borrow_mut().preferred_sub = "eng".into();
        app.player.state.write(|s| {
            s.status = Status::Opening;
            s.addon_subs.push(Subtitle { url: "https://example.test/hdtv.srt".into(), lang: Some("eng".into()), ..Default::default() });
        });
        app.auto_select_subtitle();
        assert_eq!(app.player.state.peek(|s| s.sub_choice.clone()), SubChoice::Off);
        assert!(!app.player.session.borrow().auto_sub_done);
        app.player_info(MediaInfo {
            has_video: true, subtitles: vec![Track {
                language: "eng".into(), title: String::new(), codec: "ass".into(), stream_index: 4,
            }], ..Default::default()
        });
        assert_eq!(app.player.state.peek(|s| s.sub_choice.clone()), SubChoice::Embedded(0));
    }

    #[test]
    fn alternate_english_track_choice_survives_the_next_episode_transition() {
        let app = app();
        app.profile.write(|p| p.addons.clear());
        app.player.session.borrow_mut().meta = Some(Meta { id: Some("series".into()), kind: "series".into(), ..Default::default() });
        app.player.session.borrow_mut().video = Some(episode(1));
        app.route.set(vec![Route::Board, Route::Detail, Route::Player]);
        let tracks = vec![
            Track { language: "ara".into(), title: String::new(), codec: "ass".into(), stream_index: 2 },
            Track { language: "eng".into(), title: String::new(), codec: "ass".into(), stream_index: 4 },
            Track { language: "eng".into(), title: String::new(), codec: "ass".into(), stream_index: 5 },
        ];
        app.player.state.write(|s| s.embedded_subs = tracks.clone());
        app.select_subtitle(SubChoice::Embedded(2));
        assert_eq!(app.player.session.borrow().preferred_sub_ordinal, 1);
        app.play_next(episode(2));
        app.player_info(MediaInfo { has_video: true, subtitles: tracks, ..Default::default() });
        assert_eq!(app.player.state.peek(|s| s.sub_choice.clone()), SubChoice::Embedded(2));
    }

    #[test]
    fn subtitle_release_and_frame_rate_metadata_is_preserved() {
        let subtitle: Subtitle = serde_json::from_value(serde_json::json!({
            "id": "6081377", "url": "https://example.test/english.srt", "lang": "eng",
            "movieReleaseName": "Show.S05E14.HDTV.x264-KILLERS", "releaseFormat": "HDTV",
            "releaseGroup": "KILLERS", "fpsMilli": 23976, "m": "i"
        })).unwrap();
        assert_eq!(subtitle.release_name(), Some("Show.S05E14.HDTV.x264-KILLERS"));
        assert_eq!(subtitle.release_summary(), "HDTV · KILLERS · 23.976 fps");
        assert_eq!(subtitle.other["m"], "i");
    }

    #[test]
    fn old_playback_events_cannot_end_or_rewind_a_new_seek() {
        let app = app();
        *app.player.engine.borrow_mut() = Some(Engine::test_stub(2));
        app.player.state.write(|s| s.status = Status::Playing);
        app.player.position.set(50.0);
        let session = app.player.session_id.get();
        for event in [Event::Ended { generation: 1 },
            Event::Error { generation: 1, message: "Obsolete error".into() },
            Event::Position { generation: 1, seconds: 10.0, stalled: true }] {
            app.player_event(session, event);
        }
        assert_eq!(app.player.state.peek(|s| s.status.clone()), Status::Playing);
        assert_eq!(app.player.position.get_untracked(), 50.0);
        app.player_event(session, Event::Position { generation: 2, seconds: 51.0, stalled: false });
        assert_eq!(app.player.position.get_untracked(), 51.0);
        app.player_event(session, Event::Error { generation: 2, message: "Current error".into() });
        app.player.state.peek(|s| {
            assert_eq!(s.status, Status::Error("Current error".into()));
            assert!(s.paused);
        });
    }

    #[test]
    fn controls_cannot_fake_playback_while_media_is_opening_or_failed() {
        let app = app();
        for status in [Status::Idle, Status::Opening, Status::Error("Unavailable".into())] {
            app.player.state.write(|s| { s.status = status.clone(); s.paused = false; });
            app.player.position.set(0.0);
            app.toggle_pause();
            app.seek_to(30.0);
            assert_eq!(app.player.position.get_untracked(), 0.0);
            assert!(!app.player.state.peek(|s| s.paused));
            assert_eq!(app.player.state.peek(|s| s.status.clone()), status);
        }
        app.player.state.write(|s| s.status = Status::Playing);
        app.seek_to(f64::NAN);
        assert_eq!(app.player.position.get_untracked(), 0.0);
    }

    #[test]
    fn replay_resumes_playback_and_resets_the_cancelled_next_episode() {
        let app = app();
        *app.player.engine.borrow_mut() = Some(Engine::test_stub(0));
        app.player.state.write(|s| {
            s.status = Status::Ended;
            s.paused = true;
            s.up_next_cancelled = true;
        });
        app.toggle_pause();
        app.player.state.peek(|s| {
            assert_eq!(s.status, Status::Playing);
            assert!(!s.paused && !s.up_next_cancelled);
        });
    }

    #[test]
    fn default_audio_menu_matches_the_adaptive_video_variant() {
        let app = app();
        app.profile.write(|p| p.settings.audio_lang = "fra".into());
        app.player_info(MediaInfo {
            has_video: true, video_index: 2,
            audio: [0, 3].into_iter().map(|stream_index| Track {
                language: "eng".into(), title: String::new(), codec: "aac".into(), stream_index,
            }).collect(), ..Default::default()
        });
        assert_eq!(app.player.state.peek(|s| s.audio_index), 1);
    }

    #[test]
    fn failed_subtitle_download_is_retryable_and_old_completions_are_ignored() {
        let app = app();
        app.player.state.write(|s| {
            s.sub_choice = SubChoice::Addon(0);
            s.addon_subs.push(Subtitle { id: None, url: "http://127.0.0.1:0/subtitle.srt".into(), lang: Some("eng".into()), ..Default::default() });
        });
        let session = app.player.session_id.get();
        let first = app.player.subtitle_revision.get();
        app.addon_subtitle_loaded(session, first, 0, None);
        assert!(app.player.state.peek(|s| s.sub_failed));
        app.select_subtitle(SubChoice::Addon(0));
        let retry = app.player.subtitle_revision.get();
        assert!(retry > first);
        assert!(!app.player.state.peek(|s| s.sub_failed));
        app.addon_subtitle_loaded(session, first, 0, None);
        assert!(!app.player.state.peek(|s| s.sub_failed));
        app.addon_subtitle_loaded(session, retry, 0, Some(vec![Cue { start: 0.0, end: 5.0, text: "Retried subtitle".into() }]));
        assert_eq!(app.player.cue_list()[0].text, "Retried subtitle");
        assert!(app.player.state.peek(|s| s.sub_loaded && !s.sub_failed));
    }

    #[test]
    fn choosing_off_before_subtitles_arrive_prevents_later_auto_selection() {
        let app = app();
        app.player.session.borrow_mut().preferred_sub = "eng".into();
        app.select_subtitle(SubChoice::Off);
        app.player.state.write(|s| s.embedded_subs.push(Track {
            language: "eng".into(), title: String::new(), codec: "srt".into(), stream_index: 2,
        }));
        app.auto_select_subtitle();
        assert_eq!(app.player.state.peek(|s| s.sub_choice.clone()), SubChoice::Off);
        assert_eq!(app.player.session.borrow().preferred_sub, "off");
    }

    #[test]
    fn stopping_preparation_clears_the_old_media_and_subtitle_errors() {
        let app = app();
        app.player.session.borrow_mut().stream = Some(stream("https://example.test/old.mkv", None));
        app.player.state.write(|s| s.sub_failed = true);
        app.stop_playback();
        assert!(app.player.current_media().is_none());
        assert!(!app.player.state.peek(|s| s.sub_failed));
    }

    #[test]
    fn next_source_waits_for_the_matching_release_then_falls_back() {
        let fallback = stream("https://example.test/next.mkv", Some("other"));
        let matching = stream("https://example.test/matching.mkv", Some("release"));
        let mut groups = vec![group(Load::Ready(vec![fallback.clone()])), group(Load::Loading)];
        assert!(next_stream(&groups, Some("release")).is_none());
        groups[1].items = Load::Ready(vec![matching.clone()]);
        assert_eq!(next_stream(&groups, Some("release")), Some(matching));
        groups[1].items = Load::Failed("Unavailable".into());
        assert_eq!(next_stream(&groups, Some("release")), Some(fallback.clone()));
        assert_eq!(next_stream(&groups, None), Some(fallback));
    }

    #[test]
    fn next_source_accepts_included_streams_without_a_binge_group() {
        let included = stream("https://example.test/episode2.mkv", None);
        let groups = vec![group(Load::Ready(vec![Stream::default(), included.clone()]))];
        assert_eq!(next_stream(&groups, None), Some(included));
        assert!(next_stream(&[group(Load::Ready(vec![Stream {
            external_url: Some("https://example.test/browser".into()), ..Default::default()
        }]))], None).is_none());
    }

    #[test]
    fn next_episode_preserves_the_player_and_discards_old_subtitles() {
        let app = app();
        app.profile.write(|p| p.addons.clear());
        let next = episode(2);
        let meta = Meta { id: Some("series".into()), kind: "series".into(), name: Some("Show".into()),
            videos: vec![episode(1), next.clone()], ..Default::default() };
        {
            let mut session = app.player.session.borrow_mut();
            session.meta = Some(meta);
            session.video = Some(episode(1));
            session.preferred_sub = "eng".into();
            session.auto_sub_done = true;
        }
        let routes = vec![Route::Board, Route::Detail, Route::Player];
        app.route.set(routes.clone());
        app.player.fullscreen.set(true);
        app.player.cue.set("Previous episode".into());
        app.player.cues.borrow_mut().push(Cue { start: 0.0, end: 99.0, text: "Previous episode".into() });
        app.player.scrub.set(Some(99.0));
        app.player.state.write(|s| {
            s.status = Status::Playing;
            s.sub_choice = SubChoice::Addon(0);
            s.addon_subs.push(Subtitle { id: None, url: "https://example.test/old.srt".into(), lang: Some("eng".into()), ..Default::default() });
            s.sub_loaded_until = 99.0;
            s.sub_loaded = true;
            s.up_next = Some(5);
            s.next = Some(next.clone());
        });
        let previous_session = app.player.session_id.get();
        app.play_next(next.clone());
        assert_eq!(app.route.get_untracked(), routes);
        assert!(app.player.fullscreen.get_untracked());
        assert_eq!(app.player.session.borrow().video.as_ref().map(Video::id), Some(next.id()));
        assert_eq!(app.detail.peek(|d| d.video.as_ref().map(|v| v.id().to_owned())), Some(next.id().to_owned()));
        assert!(app.player.cue.get_untracked().is_empty());
        assert!(app.player.cue_list().is_empty());
        assert_eq!(app.player.scrub.get(), None);
        app.player.state.peek(|s| {
            assert_eq!(s.sub_choice, SubChoice::Off);
            assert!(s.addon_subs.is_empty() && s.embedded_subs.is_empty());
            assert_eq!(s.sub_loaded_until, 0.0);
            assert!(!s.sub_loaded);
            assert!(s.up_next.is_none());
            assert!(matches!(s.status, Status::Error(_)), "no sources must produce an in-player error");
        });
        assert!(!app.player.session.borrow().auto_sub_done);
        assert_eq!(app.player.session.borrow().preferred_sub, "eng");
        // The new media info should select a fresh track using the carried
        // language, rather than retaining the old episode's addon index.
        app.player.state.write(|s| {
            s.status = Status::Opening;
            s.embedded_subs.push(Track {
                language: "eng".into(), title: "New episode English".into(), codec: "srt".into(), stream_index: 3,
            });
        });
        let meta = app.player.session.borrow().meta.clone();
        assert_eq!(app.opening_subtitle_preference(meta.as_ref(), Some(&next)), ("eng".into(), 0));
        app.player.state.write(|s| s.status = Status::Playing);
        app.auto_select_subtitle();
        assert_eq!(app.player.state.peek(|s| s.sub_choice.clone()), SubChoice::Embedded(0));
        app.player_event(previous_session, Event::Cues {
            revision: app.player.subtitle_revision.get(),
            cues: vec![Cue { start: 0.0, end: 10.0, text: "Late old subtitle".into() }], reset: false,
        });
        assert!(app.player.cue_list().is_empty());
        app.player_event(app.player.session_id.get(), Event::Cues {
            revision: app.player.subtitle_revision.get(),
            cues: vec![Cue { start: 0.0, end: 10.0, text: "New episode subtitle".into() }], reset: false,
        });
        assert_eq!(app.player.cue_list()[0].text, "New episode subtitle");
    }

    #[test]
    fn leaving_while_the_next_episode_loads_cancels_autoplay() {
        let app = app();
        app.route.set(vec![Route::Board, Route::Detail, Route::Player]);
        *app.player.pending_binge.borrow_mut() = Some(PendingEpisode {
            session_id: app.player.session_id.get(), meta: Meta::default(), video: episode(2), group: None,
        });
        app.back();
        app.try_binge();
        assert_eq!(app.current_route(), Route::Detail);
        assert!(app.player.pending_binge.borrow().is_none());
        assert_eq!(app.player.state.peek(|s| s.status.clone()), Status::Idle);
        assert!(app.player.engine.borrow().is_none());
    }

    #[test]
    fn subtitle_language_and_off_choice_follow_the_next_episode() {
        let app = app();
        app.player.state.write(|s| s.embedded_subs.push(Track {
            language: "eng".into(), title: "English".into(), codec: "srt".into(), stream_index: 2,
        }));
        app.select_subtitle(SubChoice::Embedded(0));
        assert_eq!(app.player.session.borrow().preferred_sub, "eng");
        let revision = app.player.subtitle_revision.get();
        app.select_subtitle(SubChoice::Off);
        assert_eq!(app.player.session.borrow().preferred_sub, "off");
        assert!(app.player.subtitle_revision.get() > revision);
    }

    #[test]
    fn rapid_subtitle_switches_reject_queued_results_even_after_switching_back() {
        let app = app();
        app.player.state.write(|s| {
            for language in ["eng", "spa"] {
                s.embedded_subs.push(Track {
                    language: language.into(), title: language.into(), codec: "srt".into(), stream_index: 2,
                });
            }
        });
        app.select_subtitle(SubChoice::Embedded(0));
        let first = app.player.subtitle_revision.get();
        app.select_subtitle(SubChoice::Embedded(1));
        app.select_subtitle(SubChoice::Embedded(0));
        let latest = app.player.subtitle_revision.get();
        let session = app.player.session_id.get();
        app.player_event(session, Event::Cues {
            revision: first, cues: vec![Cue { start: 0.0, end: 10.0, text: "Obsolete".into() }], reset: false,
        });
        app.player_event(session, Event::SubtitleProgress { revision: first, until: 999.0, done: true });
        assert!(app.player.cue_list().is_empty());
        app.player.state.peek(|s| {
            assert_eq!(s.sub_loaded_until, 0.0);
            assert!(!s.sub_loaded);
        });
        app.player_event(session, Event::Cues {
            revision: latest, cues: vec![Cue { start: 0.0, end: 10.0, text: "Current".into() }], reset: false,
        });
        app.player_event(session, Event::SubtitleProgress { revision: latest, until: 10.0, done: true });
        assert_eq!(app.player.cue_list()[0].text, "Current");
        assert!(app.player.state.peek(|s| s.sub_loaded));
        app.select_subtitle(SubChoice::Off);
        app.player_event(session, Event::Cues {
            revision: latest, cues: vec![Cue { start: 0.0, end: 10.0, text: "Late after Off".into() }], reset: false,
        });
        assert!(app.player.cue_list().is_empty());
    }
}

#[cfg(test)]
mod subtitle_follow_tests {
    use super::*;

    fn scroll(kind: ScrollNotificationType) -> ScrollNotification {
        ScrollNotification {
            kind, metrics: ScrollMetrics::default(), delta: 20.0,
            overscroll: 0.0, velocity: 0.0, drag_position: None, depth: 0,
        }
    }

    #[test]
    fn automatic_scrolls_and_layout_updates_keep_following() {
        let follow = SubtitleFollow::new();
        follow.on_scroll(scroll(ScrollNotificationType::Metrics));
        follow.on_scroll(scroll(ScrollNotificationType::Update));
        assert!(follow.enabled.get());
        follow.set_pressed(true);
        follow.automatic.set(true);
        follow.on_scroll(scroll(ScrollNotificationType::Update));
        follow.automatic.set(false);
        assert!(follow.enabled.get());
    }

    #[test]
    fn manual_scroll_stays_suspended_until_current_position_resumes_it() {
        let follow = SubtitleFollow::new();
        follow.set_pressed(true);
        follow.on_scroll(scroll(ScrollNotificationType::Update));
        follow.set_pressed(false);
        assert!(!follow.enabled.get());
        follow.on_scroll(scroll(ScrollNotificationType::End));
        assert!(!follow.enabled.get());
        follow.resume();
        assert!(follow.enabled.get());
        follow.on_scroll(scroll(ScrollNotificationType::UserScroll));
        assert!(!follow.enabled.get());
        follow.resume();
        let mut drag = scroll(ScrollNotificationType::Update);
        drag.drag_position = Some(Offset::new(10.0, 20.0));
        follow.on_scroll(drag);
        assert!(!follow.enabled.get());
    }
}
