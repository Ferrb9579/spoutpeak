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
    /// Start a full embedded subtitle scan without a playback-rate cap.
    pub fast_subtitles: bool,
    /// Local AI is opt-in; enabling does not download or start the model.
    pub local_ai_enabled: bool,
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
    pub download_format: DownloadFormat,
    pub volume: f32,
    pub hardware_decoding: bool,
    /// Most recent first.
    pub recent_searches: Vec<String>,
    /// The name others in a watch-together room see.
    pub watch_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            torrent: TorrentSettings::default(),
            player: PlayerKind::Builtin,
            custom_player_path: String::new(),
            subtitle_lang: "eng".to_owned(),
            fast_subtitles: false,
            local_ai_enabled: false,
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
            download_format: DownloadFormat::Original,
            volume: 1.0,
            hardware_decoding: true,
            recent_searches: Vec::new(),
            watch_name: String::new(),
        }
    }
}

#[cfg(test)]
mod subtitle_loading_tests {
    use super::*;

    #[test]
    fn subtitle_loading_setting_preserves_old_profiles_and_round_trips() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(!settings.fast_subtitles);
        assert!(!settings.local_ai_enabled);
        settings.fast_subtitles = true;
        settings.local_ai_enabled = true;
        let saved = serde_json::to_vec(&settings).unwrap();
        let restored: Settings = serde_json::from_slice(&saved).unwrap();
        assert!(restored.fast_subtitles);
        assert!(restored.local_ai_enabled);
    }
}

/// What downloads are saved as (see `crate::convert`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadFormat {
    /// The file exactly as the source has it.
    #[default]
    Original,
    /// MP4 with H.264 video and AAC audio, same resolution.
    Mp4,
    Mp4At1080,
    Mp4At720,
}

impl DownloadFormat {
    pub const ALL: [DownloadFormat; 4] = [Self::Original, Self::Mp4, Self::Mp4At1080, Self::Mp4At720];

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Original file",
            Self::Mp4 => "MP4",
            Self::Mp4At1080 => "MP4 1080p",
            Self::Mp4At720 => "MP4 720p",
        }
    }

    pub fn max_height(self) -> Option<u32> {
        match self {
            Self::Original | Self::Mp4 => None,
            Self::Mp4At1080 => Some(1080),
            Self::Mp4At720 => Some(720),
        }
    }
}

/// How far past the playback position torrent streaming fetches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadAhead {
    /// About 2.5% of the file (at least 64 MB, at most 512 MB).
    #[default]
    Auto,
    Megabytes(u32),
    /// This much play time, from the file's average bitrate (Auto until the
    /// player knows the duration).
    Seconds(u32),
    /// Keep going to the end of the file.
    WholeFile,
}

/// The built-in torrent engine (see `crate::streaming`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TorrentSettings {
    /// Bytes of torrent data kept on disk for replays; `None` = unlimited.
    pub cache_size: Option<u64>,
    /// Days a cached torrent is kept after it was last streamed; `Some(0)`
    /// = only while it is open, `None` = until the size limit needs room.
    pub cache_max_age_days: Option<u32>,
    /// How much of the file past the playback position is fetched.
    pub read_ahead: ReadAhead,
    /// Peer connections per torrent.
    pub max_connections: u32,
    /// Download cap in bytes per second; `None` = unlimited.
    pub speed_limit: Option<u64>,
}

impl Default for TorrentSettings {
    fn default() -> Self {
        Self {
            cache_size: Some(2 * 1024 * 1024 * 1024),
            cache_max_age_days: Some(7),
            read_ahead: ReadAhead::Auto,
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
    /// The stream last played per title / episode, most recent first.
    pub last_streams: Vec<LastStream>,
}

/// The stream picked for one title or episode.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LastStream {
    /// `meta id` or `meta id/video id`.
    pub key: String,
    pub stream: crate::addons::types::Stream,
}

/// How many titles / episodes remember their stream.
pub const LAST_STREAMS_KEPT: usize = 500;

pub fn last_stream_key(meta_id: &str, video_id: Option<&str>) -> String {
    match video_id {
        Some(video) if !video.is_empty() => format!("{meta_id}/{video}"),
        _ => meta_id.to_owned(),
    }
}

fn profile_path() -> PathBuf {
    data_dir().join("profile.json")
}

fn library_path() -> PathBuf {
    data_dir().join("library.json")
}

pub fn load_profile() -> Profile {
    let bytes = std::fs::read(profile_path()).ok();
    let profile: Profile = bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok())
        .unwrap_or_default();
    // Older versions stored an online-account login token; rewrite the file
    // without it now that accounts are gone.
    let has_old_login = bytes
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value.get("auth").is_some());
    if has_old_login {
        save_profile(&profile);
    }
    profile
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
