//! Search suggestions, matched on this PC (as Stremio does): Cinemeta
//! publishes a list of searchable titles (`feed.json`), fetched once a day
//! and cached. What is typed never leaves the device.

use serde::Deserialize;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const FEED_URL: &str = "https://cinemeta-catalogs.strem.io/feed.json";
/// Refetch the list when the cached copy is older than this.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub name: String,
    /// "movie" / "series".
    pub kind: String,
    pub year: Option<String>,
}

/// One title of the feed, with its name prepared for matching.
pub struct Entry {
    suggestion: Suggestion,
    /// Lower-case words of the name, punctuation dropped.
    words: Vec<String>,
    /// `words` joined by single spaces.
    joined: String,
    popularity: u64,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default, rename = "releaseInfo")]
    release: Option<String>,
    #[serde(default)]
    popularity: Option<u64>,
}

fn cache_path() -> PathBuf {
    crate::util::cache_dir().join("search-feed.json")
}

/// Lower-case words, with anything but letters and digits as separators.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn parse(bytes: &[u8]) -> Option<Vec<Entry>> {
    let raw: Vec<Raw> = serde_json::from_slice(bytes).ok()?;
    let entries: Vec<Entry> = raw
        .into_iter()
        .filter_map(|r| {
            let name = r.name?.trim().to_owned();
            let words = words(&name);
            if words.is_empty() {
                return None;
            }
            Some(Entry {
                joined: words.join(" "),
                words,
                popularity: r.popularity.unwrap_or(0),
                suggestion: Suggestion { name, kind: r.kind, year: r.release.filter(|y| !y.is_empty()) },
            })
        })
        .collect();
    (!entries.is_empty()).then_some(entries)
}

/// The titles, from the cache while it is fresh, else from Cinemeta (a
/// stale cache still serves when Cinemeta can't be reached).
pub async fn load() -> Option<Vec<Entry>> {
    let path = cache_path();
    let cached = tokio::fs::read(&path).await.ok();
    let fresh = tokio::fs::metadata(&path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < MAX_AGE);
    if fresh && let Some(entries) = cached.as_deref().and_then(parse) {
        return Some(entries);
    }
    let fetched = async {
        let response = crate::addons::http().get(FEED_URL).send().await.ok()?;
        let bytes = response.error_for_status().ok()?.bytes().await.ok()?;
        let entries = parse(&bytes)?;
        let temp = path.with_extension("tmp");
        if tokio::fs::write(&temp, &bytes).await.is_ok() {
            let _ = tokio::fs::rename(&temp, &path).await;
        }
        Some(entries)
    }
    .await;
    fetched.or_else(|| cached.as_deref().and_then(parse))
}

/// Up to `limit` titles matching `query`: every typed word must start a
/// word of the name. The whole name, then names holding the typed words
/// as whole words, then names starting with them rank first; ties go to
/// the most popular.
pub fn suggest(entries: &[Entry], query: &str, limit: usize) -> Vec<Suggestion> {
    let typed = words(query);
    if typed.is_empty() {
        return Vec::new();
    }
    let phrase = typed.join(" ");
    let mut found: Vec<(u8, u64, &Entry)> = entries
        .iter()
        .filter(|e| typed.iter().all(|t| e.words.iter().any(|w| w.starts_with(t.as_str()))))
        .map(|e| {
            let whole_words = typed.iter().all(|t| e.words.iter().any(|w| w == t));
            let rank = if e.joined == phrase {
                3
            } else if whole_words {
                2
            } else if e.joined.starts_with(&phrase) {
                1
            } else {
                0
            };
            (rank, e.popularity, e)
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|(_, _, e)| seen.insert(e.joined.clone()))
        .take(limit)
        .map(|(_, _, e)| e.suggestion.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed() -> Vec<Entry> {
        parse(
            br#"[
            {"name":"Vampire Hunter D: Bloodlust","type":"movie","releaseInfo":"2000","popularity":50},
            {"name":"Initial D","type":"series","releaseInfo":"1998-2014","popularity":80},
            {"name":"Mr. D","type":"series","popularity":10},
            {"name":"Dune","type":"movie","popularity":99},
            {"name":"The Walking Dead","type":"series","popularity":95},
            {"name":"","type":"movie"},
            {"type":"movie"}
        ]"#,
        )
        .unwrap()
    }

    fn names(found: Vec<Suggestion>) -> Vec<String> {
        found.into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn nameless_titles_are_dropped() {
        assert_eq!(feed().len(), 5);
    }

    #[test]
    fn whole_words_then_leading_matches_rank_before_popularity() {
        assert_eq!(
            names(suggest(&feed(), "d", 10)),
            ["Initial D", "Vampire Hunter D: Bloodlust", "Mr. D", "Dune", "The Walking Dead"]
        );
        assert_eq!(names(suggest(&feed(), "dune", 1)), ["Dune"]);
        assert_eq!(names(suggest(&feed(), "d", 2)), ["Initial D", "Vampire Hunter D: Bloodlust"]);
    }

    #[test]
    fn every_word_must_start_a_word_of_the_name() {
        assert_eq!(names(suggest(&feed(), "walk dea", 5)), ["The Walking Dead"]);
        assert!(suggest(&feed(), "alking", 5).is_empty());
        assert_eq!(names(suggest(&feed(), "mr.  d", 5)), ["Mr. D"]);
    }

    #[test]
    fn empty_query_suggests_nothing() {
        assert!(suggest(&feed(), "  ", 5).is_empty());
    }
}
