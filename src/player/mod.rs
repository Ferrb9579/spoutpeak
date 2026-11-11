//! Playback orchestration: turning a Media stream into something playing,
//! tracking progress into the library, subtitles, and next-episode binge.

pub mod engine;
pub mod subtitles;

use crate::state::{App, Route, Shared, spawn, ui};
use crate::store::{PlayerKind, Settings, SubtitleStyle};
use crate::media::addon::{self, http};
use crate::media::server::{self, TorrentStats};
use crate::media::types::*;
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
    pub sub_size: f32,
    pub sub_style: SubtitleStyle,
    pub brightness: f32,
    pub controls_visible: bool,
    pub menu: Menu,
    pub fullscreen: bool,
    pub torrent: Option<TorrentStats>,
    pub next: Option<Video>,
}

struct Session {
    meta: Option<Meta>,
    video: Option<Video>,
    stream: Option<Stream>,
    torrent_url: Option<String>,
    preferred_sub: String,
    auto_sub_done: bool,
    last_position: f64,
    unsaved_ms: f64,
    last_record: Instant,
    last_push: Instant,
    last_activity: Instant,
    last_stats: Instant,
}

#[derive(Clone)]
pub struct PlayerUi {
    pub frame: Signal<Option<ImageHandle>>,
    pub position: Signal<f64>,
    pub cue: Signal<String>,
    /// Position being previewed while the seek bar is dragged.
    pub scrub: Rc<Cell<Option<f64>>>,
    pub state: Shared<PlayerState>,
    engine: Rc<RefCell<Option<Engine>>>,
    cues: Rc<RefCell<Vec<Cue>>>,
    session_id: Rc<Cell<u64>>,
    session: Rc<RefCell<Session>>,
    pub pending_binge: Rc<RefCell<Option<String>>>,
    pub window: Rc<RefCell<Option<WindowHandle>>>,
}

impl PlayerUi {
    pub fn new(settings: &Settings) -> Self {
        Self {
            frame: Signal::new(None),
            position: Signal::new(0.0),
            cue: Signal::new(String::new()),
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
                sub_size: settings.subtitle_size,
                sub_style: settings.subtitle_style.clone(),
                brightness: settings.brightness,
                controls_visible: true,
                menu: Menu::None,
                fullscreen: false,
                torrent: None,
                next: None,
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
                auto_sub_done: false,
                last_position: 0.0,
                unsaved_ms: 0.0,
                last_record: Instant::now(),
                last_push: Instant::now(),
                last_activity: Instant::now(),
                last_stats: Instant::now(),
            })),
            pending_binge: Rc::new(RefCell::new(None)),
            window: Rc::new(RefCell::new(None)),
        }
    }

    /// The loaded subtitle cues, in time order.
    pub fn cue_list(&self) -> Vec<Cue> {
        let mut cues = self.cues.borrow().clone();
        cues.sort_by(|a, b| a.start.total_cmp(&b.start));
        cues
    }

    fn with_engine<R>(&self, call: impl FnOnce(&Engine) -> R) -> Option<R> {
        self.engine.borrow().as_ref().map(call)
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
    pub fn play_stream(&self, stream: &Stream) {
        let (meta, video) = self.detail.peek(|d| (d.best_meta(), d.video.clone()));
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
                    self.stop_playback();
                    let (title, subtitle) = titles(stream, meta.as_ref(), video.as_ref());
                    self.player.frame.set(None);
                    self.player.state.write(|s| {
                        s.title = title;
                        s.subtitle = subtitle;
                        s.status = Status::Opening;
                        s.stage = "Asking the streaming server to join the swarm…".into();
                        s.live = false;
                        s.duration = 0.0;
                        s.torrent = None;
                        s.next = None;
                        s.controls_visible = true;
                        s.menu = Menu::None;
                    });
                    self.push(Route::Player);
                    Some(self.player.session_id.get())
                } else {
                    self.notify("Preparing torrent…");
                    None
                };
                let base = self.server_url();
                let stream = stream.clone();
                spawn(
                    async move {
                        server::torrent_stream_url(&base, &info_hash, file_idx, &trackers).await
                    },
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

        self.stop_playback();
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
            preferred_sub: settings.subtitle_lang.clone(),
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
            s.speed = 1.0;
            s.audio_tracks.clear();
            s.audio_index = 0;
            s.audio_delay = 0.0;
            s.embedded_subs.clear();
            s.addon_subs = stream.subtitles.clone();
            s.sub_choice = SubChoice::Off;
            s.sub_delay = 0.0;
            s.sub_size = settings.subtitle_size;
            s.controls_visible = true;
            s.menu = Menu::None;
            s.torrent = None;
            s.next = next;
        });
        let (volume, muted) = self.player.state.peek(|s| (s.volume, s.muted));
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
    fn poll_opening_stats(&self, session_id: u64, url: String) {
        spawn(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                let stats = server::torrent_stats(&url).await;
                (url, stats)
            },
            move |app, (url, stats)| {
                let opening = app.player.session_id.get() == session_id
                    && app.player.state.peek(|s| s.status == Status::Opening);
                if !opening {
                    return;
                }
                if stats.is_some() {
                    app.player.state.write(|s| s.torrent = stats);
                }
                app.poll_opening_stats(session_id, url);
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
                async move { addon::subtitles(&descriptor, &kind, &video_id, &extra).await },
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
        let (done, preferred) = {
            let session = self.player.session.borrow();
            (session.auto_sub_done, session.preferred_sub.clone())
        };
        if done || preferred.is_empty() || preferred == "off" {
            return;
        }
        let choice = self.player.state.peek(|s| {
            if s.sub_choice != SubChoice::Off {
                return None;
            }
            s.embedded_subs
                .iter()
                .position(|t| t.is_text_subtitle() && same_language(&t.language, &preferred))
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
        match event {
            Event::Info(info) => self.player_info(info),
            Event::Frame => {
                let frame = self.player.with_engine(Engine::take_frame).flatten();
                if let Some(frame) = frame {
                    self.player.frame.set(Some(frame));
                }
            }
            Event::Position { seconds, stalled } => self.player_tick(seconds, stalled),
            Event::Cues { cues, reset } => {
                if !matches!(
                    self.player.state.peek(|s| s.sub_choice.clone()),
                    SubChoice::Embedded(_)
                ) {
                    return;
                }
                let mut all = self.player.cues.borrow_mut();
                if reset {
                    all.clear();
                }
                all.extend(cues);
            }
            Event::Ended => self.player_ended(),
            Event::Error(message) => {
                self.player.state.write(|s| {
                    s.status = Status::Error(message);
                    s.controls_visible = true;
                });
            }
        }
    }

    fn player_info(&self, info: MediaInfo) {
        let audio_index = {
            let preferred = self.profile.peek(|p| p.settings.audio_lang.clone());
            info.audio
                .iter()
                .position(|t| same_language(&t.language, &preferred))
                .unwrap_or(0)
        };
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
    }

    /// Clicking a control moves or clears keyboard focus; take it back so
    /// the player's shortcuts keep working.
    fn keep_player_focus(&self) {
        let focus = &self.controllers.player_focus;
        if !focus.has_focus() {
            focus.request_focus();
        }
    }

    fn player_tick(&self, seconds: f64, stalled: bool) {
        self.keep_player_focus();
        if self.player.scrub.get().is_none() {
            self.player.position.set((seconds * 10.0).round() / 10.0);
        }

        let delay = self.player.state.peek(|s| s.sub_delay);
        let text = subtitles::active_text(&self.player.cues.borrow(), seconds - delay);
        if text != self.player.cue.get_untracked() {
            self.player.cue.set(text);
        }

        let (was_stalled, visible, paused, menu, status) = self.player.state.peek(|s| {
            (
                s.stalled,
                s.controls_visible,
                s.paused,
                s.menu,
                s.status.clone(),
            )
        });
        if status != Status::Playing {
            return;
        }
        let idle = self.player.session.borrow().last_activity.elapsed().as_secs_f64() > 3.0;
        let hide = visible && idle && !paused && menu == Menu::None;
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
            let session_id = self.player.session_id.get();
            spawn(
                async move { server::torrent_stats(&url).await },
                move |app, stats| {
                    if app.player.session_id.get() == session_id && stats.is_some() {
                        app.player.state.write(|s| s.torrent = stats);
                    }
                },
            );
        }
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
        let next = self.player.state.peek(|s| s.next.clone());
        let autoplay = self.profile.peek(|p| p.settings.autoplay_next);
        self.player.state.write(|s| {
            s.status = Status::Ended;
            s.controls_visible = true;
        });
        if let (Some(next), true) = (next, autoplay) {
            self.play_next(next);
        }
    }

    /// Goes back to the detail view on the next episode and, when the
    /// current stream has a binge group, auto-plays the matching stream.
    pub fn play_next(&self, next: Video) {
        let group = self
            .player
            .session
            .borrow()
            .stream
            .as_ref()
            .and_then(|s| s.behavior_hints.binge_group.clone());
        self.back();
        if let Some(season) = next.season {
            self.select_season(season);
        }
        *self.player.pending_binge.borrow_mut() = group;
        self.select_video(next);
    }

    /// Called as stream lists arrive for the selected video.
    pub fn try_binge(&self, streams: &[Stream]) {
        let wanted = self.player.pending_binge.borrow().clone();
        let Some(wanted) = wanted else { return };
        if let Some(stream) = streams
            .iter()
            .find(|s| s.behavior_hints.binge_group.as_deref() == Some(wanted.as_str()))
        {
            self.player.pending_binge.borrow_mut().take();
            self.play_stream(stream);
        }
    }

    pub fn stop_playback(&self) {
        let engine = self.player.engine.borrow_mut().take();
        let Some(engine) = engine else {
            // Nothing decoding yet, but a torrent may still be preparing:
            // invalidate it.
            self.player.session_id.set(self.player.session_id.get() + 1);
            if self.player.state.peek(|s| s.status != Status::Idle) {
                self.player.state.write(|s| s.status = Status::Idle);
            }
            return;
        };
        let ended = self.player.state.peek(|s| s.status == Status::Ended);
        if !ended {
            self.record_player_progress(engine.position(), false, true);
        }
        engine.stop();
        self.player.session_id.set(self.player.session_id.get() + 1);
        self.player.frame.set(None);
        self.player.cue.set(String::new());
        self.player.cues.borrow_mut().clear();
        let (volume, fullscreen) = self.player.state.write(|s| {
            s.status = Status::Idle;
            s.menu = Menu::None;
            let fullscreen = std::mem::replace(&mut s.fullscreen, false);
            (s.volume, fullscreen)
        });
        if fullscreen {
            self.apply_fullscreen(false);
        }
        self.update_settings(|s| s.volume = volume);
    }

    // ------------------------------------------------------------ controls

    pub fn player_activity(&self) {
        self.player.session.borrow_mut().last_activity = Instant::now();
        if !self.player.state.peek(|s| s.controls_visible) {
            self.player.state.write(|s| s.controls_visible = true);
        }
    }

    pub fn toggle_pause(&self) {
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
    }

    pub fn seek_to(&self, seconds: f64) {
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
            self.player.state.write(|s| s.status = Status::Playing);
        }
        self.player_activity();
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
        if self.player.state.peek(|s| s.live) {
            self.player.state.write(|s| s.menu = Menu::None);
            return;
        }
        self.player.with_engine(|e| e.set_speed(speed));
        self.player.state.write(|s| {
            s.speed = speed;
            s.menu = Menu::None;
        });
    }

    pub fn select_audio(&self, index: usize) {
        self.player.with_engine(|e| e.set_audio_track(index));
        self.player.state.write(|s| {
            s.audio_index = index;
            s.menu = Menu::None;
        });
    }

    pub fn select_subtitle(&self, choice: SubChoice) {
        let previous = self.player.state.peek(|s| s.sub_choice.clone());
        if previous == choice {
            self.player.state.write(|s| s.menu = Menu::None);
            return;
        }
        self.player.session.borrow_mut().auto_sub_done = true;
        self.player.cues.borrow_mut().clear();
        self.player.cue.set(String::new());
        self.player.state.write(|s| {
            s.sub_choice = choice.clone();
            s.menu = Menu::None;
        });
        match &choice {
            SubChoice::Embedded(index) => {
                let index = *index;
                self.player.with_engine(|e| e.set_subtitle_track(Some(index)));
            }
            SubChoice::Off | SubChoice::Addon(_) => {
                if matches!(previous, SubChoice::Embedded(_)) {
                    self.player.with_engine(|e| e.set_subtitle_track(None));
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
                    let bytes = http().get(&url).send().await.ok()?.bytes().await.ok()?;
                    Some(subtitles::parse(&String::from_utf8_lossy(&bytes)))
                },
                move |app, cues: Option<Vec<Cue>>| {
                    let still_selected = app.player.session_id.get() == session_id
                        && app.player.state.peek(|s| s.sub_choice == SubChoice::Addon(index));
                    if !still_selected {
                        return;
                    }
                    match cues {
                        Some(cues) if !cues.is_empty() => *app.player.cues.borrow_mut() = cues,
                        _ => app.notify("Could not load that subtitle"),
                    }
                },
            );
        }
    }

    /// Height of one row in the subtitle sync list.
    pub const SYNC_ROW: f32 = 46.0;

    /// Opens the line list, scrolled to where the subtitles currently are.
    pub fn open_subtitle_sync(&self) {
        let position = self.player.position.get_untracked();
        let delay = self.player.state.peek(|s| s.sub_delay);
        let cues = self.player.cue_list();
        let index = cues
            .iter()
            .position(|cue| cue.end > position - delay)
            .unwrap_or(cues.len().saturating_sub(1));
        self.player.state.write(|s| s.menu = Menu::Sync);
        self.player_activity();
        let offset = (index.saturating_sub(2)) as f32 * Self::SYNC_ROW;
        // After the list has been built with its new contents.
        ui(move |app| {
            app.controllers.sync_list.jump_to(offset);
        });
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
        if opened != Menu::None && opened != Menu::Sync {
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
        let brightness = (brightness.clamp(0.3, 1.7) * 20.0).round() / 20.0;
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
        let fullscreen = self.player.state.write(|s| {
            s.fullscreen = !s.fullscreen;
            s.fullscreen
        });
        self.apply_fullscreen(fullscreen);
    }

    fn apply_fullscreen(&self, fullscreen: bool) {
        let mode = fullscreen.then_some(Fullscreen::Borderless);
        if let Some(window) = self.player.window.borrow().as_ref() {
            let _ = window.set_fullscreen(mode);
            return;
        }
        // The handle is not available while the app is still being built,
        // so it is looked up the first time it is needed.
        crate::state::with_window(move |app, window| {
            if let Some(window) = window {
                let _ = window.set_fullscreen(mode);
                *app.player.window.borrow_mut() = Some(window);
            } else {
                app.notify("Fullscreen is not available");
            }
        });
    }
}
