//! On-disk profile: installed addons, settings and the library.

use crate::addons::types::{Descriptor, LibraryItem};
use crate::util::data_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerKind {
    /// The in-app player (decodes through ffmpeg).
    #[default]
    Builtin,
    Vlc,
    Mpv,
    Custom,
}

impl PlayerKind {
    pub const ALL: [PlayerKind; 4] = [Self::Builtin, Self::Vlc, Self::Mpv, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Builtin => "Built-in",
            Self::Vlc => "VLC",
            Self::Mpv => "mpv",
            Self::Custom => "Custom",
        }
    }
}

/// Outline drawn around subtitle glyphs (YouTube's "character edge style").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtitleEdge {
    #[default]
    None,
    DropShadow,
    Outline,
    Raised,
    Depressed,
}

impl SubtitleEdge {
    pub const ALL: [SubtitleEdge; 5] =
        [Self::None, Self::DropShadow, Self::Outline, Self::Raised, Self::Depressed];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::DropShadow => "Shadow",
            Self::Outline => "Outline",
            Self::Raised => "Raised",
            Self::Depressed => "Depressed",
        }
    }
}

/// How subtitles look, modelled on YouTube's caption options: text, a box
/// behind each line ("background") and a box behind the whole cue
/// ("window"), each with its own colour and opacity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    pub text_color: [u8; 3],
    pub text_opacity: f32,
    pub background_color: [u8; 3],
    pub background_opacity: f32,
    pub window_color: [u8; 3],
    pub window_opacity: f32,
    pub edge: SubtitleEdge,
    pub bold: bool,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            text_color: [255, 255, 255],
            text_opacity: 1.0,
            background_color: [0, 0, 0],
            background_opacity: 0.75,
            window_color: [0, 0, 0],
            window_opacity: 0.0,
            edge: SubtitleEdge::None,
            bold: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub torrent: TorrentSettings,
    pub player: PlayerKind,
    pub custom_player_path: String,
    pub subtitle_lang: String,
    pub audio_lang: String,
    pub subtitle_size: f32,
    pub subtitle_style: SubtitleStyle,
    /// Picture brightness multiplier in the built-in player (1 = unchanged).
    pub brightness: f32,
    /// Upper bound on the decoded picture height of the built-in player.
    pub max_video_height: u32,
    pub autoplay_next: bool,
    /// Seconds before the end at which the "Up next" card appears (0: never).
    pub up_next_seconds: u32,
    /// Start where the last viewing stopped.
    pub resume_playback: bool,
    /// Arrow-key seek step, and with Shift held.
    pub seek_step: u32,
    pub seek_step_shift: u32,
    /// Volume step for the Up/Down keys and the mouse wheel, in percent.
    pub volume_step: u32,
    /// Seconds of inactivity before the player hides its controls.
    pub controls_hide_seconds: u32,
    /// Playback speed every new video starts at.
    pub default_speed: f64,
    /// Where downloads go; empty means `<Downloads>/Spoutpeak`.
    pub download_dir: String,
    pub volume: f32,
    pub hardware_decoding: bool,
    /// Most recent first.
    pub recent_searches: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            torrent: TorrentSettings::default(),
            player: PlayerKind::Builtin,
            custom_player_path: String::new(),
            subtitle_lang: "eng".to_owned(),
            audio_lang: "eng".to_owned(),
            subtitle_size: 28.0,
            subtitle_style: SubtitleStyle::default(),
            brightness: 1.0,
            max_video_height: 720,
            autoplay_next: true,
            up_next_seconds: 30,
            resume_playback: true,
            seek_step: 10,
            seek_step_shift: 60,
            volume_step: 5,
            controls_hide_seconds: 3,
            default_speed: 1.0,
            download_dir: String::new(),
            volume: 1.0,
            hardware_decoding: true,
            recent_searches: Vec::new(),
        }
    }
}

/// The built-in torrent engine (see `crate::streaming`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TorrentSettings {
    /// Bytes of torrent data kept on disk for replays; `None` = unlimited.
    pub cache_size: Option<u64>,
    /// Peer connections per torrent.
    pub max_connections: u32,
    /// Download cap in bytes per second; `None` = unlimited.
    pub speed_limit: Option<u64>,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        Self {
            cache_size: Some(2 * 1024 * 1024 * 1024),
            max_connections: 200,
            speed_limit: Some(39_321_600),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub addons: Vec<Descriptor>,
    pub settings: Settings,
}

fn profile_path() -> PathBuf {
    data_dir().join("profile.json")
}

fn library_path() -> PathBuf {
    data_dir().join("library.json")
}

pub fn load_profile() -> Profile {
    std::fs::read(profile_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn load_library() -> HashMap<String, LibraryItem> {
    std::fs::read(library_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<LibraryItem>>(&bytes).ok())
        .map(|items| items.into_iter().map(|i| (i.id.clone(), i)).collect())
        .unwrap_or_default()
}

fn write_atomic(path: PathBuf, bytes: Vec<u8>) {
    std::thread::spawn(move || {
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    });
}

pub fn save_profile(profile: &Profile) {
    if let Ok(bytes) = serde_json::to_vec_pretty(profile) {
        write_atomic(profile_path(), bytes);
    }
}

pub fn save_library(library: &HashMap<String, LibraryItem>) {
    let items: Vec<&LibraryItem> = library.values().collect();
    if let Ok(bytes) = serde_json::to_vec(&items) {
        write_atomic(library_path(), bytes);
    }
}
