use std::path::PathBuf;
use std::process::Command;

/// Keeps helper processes (ffmpeg, the streaming server) from flashing a
/// console window on Windows.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

pub fn data_dir() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spoutpeak");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn cache_dir() -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(data_dir)
        .join("spoutpeak");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `1:02:03` / `2:03`.
pub fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_speed(bytes_per_second: f64) -> String {
    if bytes_per_second >= 1_048_576.0 {
        format!("{:.1} MB/s", bytes_per_second / 1_048_576.0)
    } else {
        format!("{:.0} KB/s", bytes_per_second / 1024.0)
    }
}

/// `734 MB` / `1.42 GB`.
pub fn format_bytes(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1_073_741_824.0 {
        format!("{:.2} GB", bytes / 1_073_741_824.0)
    } else if bytes >= 1_048_576.0 {
        format!("{:.0} MB", bytes / 1_048_576.0)
    } else {
        format!("{:.0} KB", bytes / 1024.0)
    }
}

pub fn hash_hex(value: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Human name for an ISO 639 language code as used by subtitle addons.
pub fn language_name(code: &str) -> String {
    let name = match code.to_lowercase().as_str() {
        "eng" | "en" => "English",
        "spa" | "es" => "Spanish",
        "fre" | "fra" | "fr" => "French",
        "ger" | "deu" | "de" => "German",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "pob" | "pt-br" => "Portuguese (BR)",
        "rus" | "ru" => "Russian",
        "jpn" | "ja" => "Japanese",
        "kor" | "ko" => "Korean",
        "chi" | "zho" | "zh" => "Chinese",
        "ara" | "ar" => "Arabic",
        "hin" | "hi" => "Hindi",
        "tur" | "tr" => "Turkish",
        "pol" | "pl" => "Polish",
        "dut" | "nld" | "nl" => "Dutch",
        "swe" | "sv" => "Swedish",
        "nor" | "no" => "Norwegian",
        "dan" | "da" => "Danish",
        "fin" | "fi" => "Finnish",
        "gre" | "ell" | "el" => "Greek",
        "heb" | "he" => "Hebrew",
        "hun" | "hu" => "Hungarian",
        "cze" | "ces" | "cs" => "Czech",
        "rum" | "ron" | "ro" => "Romanian",
        "bul" | "bg" => "Bulgarian",
        "ukr" | "uk" => "Ukrainian",
        "vie" | "vi" => "Vietnamese",
        "tha" | "th" => "Thai",
        "ind" | "id" => "Indonesian",
        "hrv" | "hr" => "Croatian",
        "srp" | "sr" => "Serbian",
        "slv" | "sl" => "Slovenian",
        "per" | "fas" | "fa" => "Persian",
        "ben" | "bn" => "Bengali",
        "tam" | "ta" => "Tamil",
        "tel" | "te" => "Telugu",
        "und" | "" => "Unknown",
        other => return other.to_uppercase(),
    };
    name.to_owned()
}

/// Loose equality between language codes of different lengths (`en`/`eng`).
pub fn same_language(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && language_name(a) == language_name(b)
}
