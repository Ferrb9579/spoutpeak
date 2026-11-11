//! On-disk profile: account, installed addons, settings and the library.

use crate::media::server::DEFAULT_URL;
use crate::media::types::{Auth, Descriptor, LibraryItem};
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server_url: String,
    pub autostart_server: bool,
    pub player: PlayerKind,
    pub custom_player_path: String,
    pub subtitle_lang: String,
    pub audio_lang: String,
    pub subtitle_size: f32,
    /// Upper bound on the decoded picture height of the built-in player.
    pub max_video_height: u32,
    pub autoplay_next: bool,
    pub volume: f32,
    pub hardware_decoding: bool,
    /// Most recent first.
    pub recent_searches: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_URL.to_owned(),
            autostart_server: true,
            player: PlayerKind::Builtin,
            custom_player_path: String::new(),
            subtitle_lang: "eng".to_owned(),
            audio_lang: "eng".to_owned(),
            subtitle_size: 28.0,
            max_video_height: 720,
            autoplay_next: true,
            volume: 1.0,
            hardware_decoding: true,
            recent_searches: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub auth: Option<Auth>,
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
