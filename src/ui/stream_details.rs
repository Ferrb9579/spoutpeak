//! Keep provider statistics separate from release names when laying out a tile.

pub(super) struct StreamText<'a> {
    pub description: &'a str,
    pub metadata: &'a str,
}

pub(super) fn split(details: &str) -> StreamText<'_> {
    // Torrentio and similar addons put seeders, file size and source after
    // these markers, sometimes on the same line as a very long release name.
    let symbols = ["👤", "👥", "🌱", "💾", "📦", "⚙", "🔗", "📡"];
    let symbol_start = symbols
        .iter()
        .filter_map(|symbol| details.find(symbol))
        .min();
    let lower = details.to_ascii_lowercase();
    let labels = [
        "seeders:", "seeds:", "peers:", "size:", "hoster:", "source:", "tracker:",
    ];
    let label_start = labels
        .iter()
        .flat_map(|label| lower.match_indices(label).map(|(at, _)| at))
        .filter(|&at| {
            details[..at]
                .chars()
                .next_back()
                .is_none_or(|ch| ch.is_whitespace() || matches!(ch, '|' | '·' | '•'))
        })
        .min();
    let at = symbol_start.into_iter().chain(label_start).min();
    match at {
        Some(at) => StreamText {
            description: details[..at].trim(),
            metadata: details[at..].trim(),
        },
        None => StreamText {
            description: details.trim(),
            metadata: "",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn long_collection_names_do_not_consume_statistics() {
        let name = "Marvel.Cinematic.Universe.MCU.Collection.2008-2018.1080p.IMAX.Dual.Audio.Hindi.640kbps.English.5.1.ESUBS-KartiKing".repeat(6);
        let details =
            format!("{name}\nThor Ragnarok (2017)\n👤 12 💾 3.1 GB ⚙️ 1337x\n🇮🇳 Hindi / English");
        let parts = split(&details);
        assert_eq!(parts.description, format!("{name}\nThor Ragnarok (2017)"));
        assert_eq!(
            parts.metadata,
            "👤 12 💾 3.1 GB ⚙️ 1337x\n🇮🇳 Hindi / English"
        );
    }

    #[test]
    fn inline_statistics_and_windows_newlines_are_preserved() {
        let parts = split("Thor.Ragnarok.1080p.WEBRip 👥 3 💾 5.9 GB\r\n⚙️ TorrentGalaxy");
        assert_eq!(parts.description, "Thor.Ragnarok.1080p.WEBRip");
        assert_eq!(parts.metadata, "👥 3 💾 5.9 GB\r\n⚙️ TorrentGalaxy");
    }

    #[test]
    fn text_labels_support_providers_without_emoji() {
        let parts =
            split("A long release name\nSEEDERS: 42 | Peers: 8\nSize: 4 GB\nHoster: Example");
        assert_eq!(parts.description, "A long release name");
        assert_eq!(
            parts.metadata,
            "SEEDERS: 42 | Peers: 8\nSize: 4 GB\nHoster: Example"
        );
    }

    #[test]
    fn ordinary_filenames_are_not_mistaken_for_statistics() {
        let parts = split("Source.Code.2011.1080p.WEBRip\nDual Audio");
        assert_eq!(
            parts.description,
            "Source.Code.2011.1080p.WEBRip\nDual Audio"
        );
        assert!(parts.metadata.is_empty());
    }
}
