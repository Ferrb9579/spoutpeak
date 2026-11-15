//! Shareable original source links, independent of the local playback proxy.
use crate::addons::types::{Stream, StreamSource};

pub fn magnet_url(stream: &Stream, title: &str) -> Option<String> {
    if let Some(url) = stream.url.as_ref().filter(|url| url.starts_with("magnet:")) {
        return Some(url.clone());
    }
    let StreamSource::Torrent { info_hash, file_idx, trackers } = stream.source() else { return None; };
    if info_hash.is_empty() { return None; }
    let mut url = format!("magnet:?xt=urn:btih:{info_hash}");
    let name = stream.behavior_hints.filename.as_deref().unwrap_or(title);
    if !name.is_empty() { url.push_str(&format!("&dn={}", urlencoding::encode(name))); }
    if let Some(index) = file_idx { url.push_str(&format!("&so={index}")); }
    let mut seen = std::collections::HashSet::new();
    for tracker in trackers {
        if !tracker.is_empty() && seen.insert(tracker.clone()) {
            url.push_str(&format!("&tr={}", urlencoding::encode(&tracker)));
        }
    }
    Some(url)
}

pub fn source_copy(stream: &Stream) -> Option<(&'static str, String)> {
    match stream.source() {
        StreamSource::Url(url) if std::path::Path::new(&url).is_absolute() => Some(("Copy file path", url)),
        StreamSource::Url(url) if url.starts_with("file:") => {
            let path = reqwest::Url::parse(&url).ok()?.to_file_path().ok()?;
            Some(("Copy file path", path.to_string_lossy().into_owned()))
        }
        StreamSource::Url(url) | StreamSource::External(url) => Some(("Copy stream URL", url)),
        StreamSource::YouTube(id) => Some(("Copy video URL", format!("https://www.youtube.com/watch?v={}", urlencoding::encode(&id)))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_magnets_preserve_every_parameter() {
        let original = "magnet:?xt=urn:btih:ABC123&dn=Movie&tr=udp%3A%2F%2Ftracker&so=4";
        let stream = Stream { url: Some(original.into()), ..Stream::default() };
        assert_eq!(magnet_url(&stream, "Other"), Some(original.into()));
    }

    #[test]
    fn generated_magnet_includes_the_selected_file_and_deduplicated_trackers() {
        let stream = Stream {
            info_hash: Some("ABCDEF".into()), file_idx: Some(0),
            sources: vec!["tracker:udp://tracker.example:80/announce".into(),
                          "tracker:udp://tracker.example:80/announce".into()],
            ..Stream::default()
        };
        let url = magnet_url(&stream, "Movie & episode").unwrap();
        assert!(url.starts_with("magnet:?xt=urn:btih:abcdef&dn=Movie%20%26%20episode&so=0"));
        assert_eq!(url.matches("&tr=").count(), 1);
        assert!(url.contains("udp%3A%2F%2Ftracker.example%3A80%2Fannounce"));
    }

    #[test]
    fn direct_urls_are_not_converted_into_magnets() {
        let stream = Stream { url: Some("https://example.test/video.mp4?token=test".into()), ..Stream::default() };
        assert!(magnet_url(&stream, "Video").is_none());
        assert_eq!(source_copy(&stream), Some(("Copy stream URL", stream.url.unwrap())));
    }
}
