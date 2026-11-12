//! Addon-protocol data model.
//!
//! Addons in the wild are sloppy (numbers as strings, nulls where arrays are
//! expected, missing fields), so every field is defaulted and the ambiguous
//! ones go through lenient deserializers. Unknown fields are preserved on the
//! types we save back to disk (manifests, library items).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) if !s.is_empty() => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn lenient_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
            .map(|v| v as u32),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn lenient_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    })
}

fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => s == "true" || s == "1",
        _ => false,
    })
}

fn lenient_strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => vec![s],
        Value::Array(items) => items
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// Deserializes a list, dropping elements that fail to parse instead of
/// failing the whole response.
fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        _ => Vec::new(),
    })
}

fn lenient_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(serde_json::from_value(Value::deserialize(d)?).unwrap_or_default())
}

// ---------------------------------------------------------------- manifest

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub types: Vec<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub resources: Vec<ManifestResource>,
    #[serde(
        default,
        rename = "idPrefixes",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_default"
    )]
    pub id_prefixes: Option<Vec<String>>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub catalogs: Vec<ManifestCatalog>,
    #[serde(
        default,
        rename = "addonCatalogs",
        deserialize_with = "lenient_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub addon_catalogs: Vec<ManifestCatalog>,
    #[serde(default, rename = "behaviorHints", deserialize_with = "lenient_default")]
    pub behavior_hints: ManifestHints,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ManifestHints {
    #[serde(default, deserialize_with = "lenient_bool")]
    pub adult: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub p2p: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub configurable: bool,
    #[serde(default, rename = "configurationRequired", deserialize_with = "lenient_bool")]
    pub configuration_required: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ManifestResource {
    Short(String),
    Full {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        types: Option<Vec<String>>,
        #[serde(default, rename = "idPrefixes", skip_serializing_if = "Option::is_none")]
        id_prefixes: Option<Vec<String>>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ManifestCatalog {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_vec", skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraProp>,
    #[serde(
        default,
        rename = "extraSupported",
        deserialize_with = "lenient_strings",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub extra_supported: Vec<String>,
    #[serde(
        default,
        rename = "extraRequired",
        deserialize_with = "lenient_strings",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub extra_required: Vec<String>,
    #[serde(default, deserialize_with = "lenient_strings", skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtraProp {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "isRequired", deserialize_with = "lenient_bool")]
    pub is_required: bool,
    #[serde(default, deserialize_with = "lenient_strings", skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl ManifestCatalog {
    /// Extra properties in the modern notation, upgrading the legacy
    /// `extraSupported` / `extraRequired` / `genres` form when needed.
    pub fn extras(&self) -> Vec<ExtraProp> {
        if !self.extra.is_empty() {
            return self.extra.clone();
        }
        self.extra_supported
            .iter()
            .map(|name| ExtraProp {
                name: name.clone(),
                is_required: self.extra_required.contains(name),
                options: if name == "genre" {
                    self.genres.clone()
                } else {
                    Vec::new()
                },
                other: Map::new(),
            })
            .collect()
    }

    pub fn supports_extra(&self, name: &str) -> bool {
        self.extras().iter().any(|e| e.name == name)
    }

    pub fn requires_extra(&self, name: &str) -> bool {
        self.extras().iter().any(|e| e.name == name && e.is_required)
    }

    /// True when the catalog can be requested without any extra argument,
    /// which is what makes it eligible for the Board.
    pub fn is_browsable(&self) -> bool {
        self.extras().iter().all(|e| !e.is_required)
    }

    pub fn genre_options(&self) -> Vec<String> {
        self.extras()
            .into_iter()
            .find(|e| e.name == "genre")
            .map(|e| e.options)
            .unwrap_or_default()
    }

    pub fn title(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
    }
}

impl Manifest {
    /// Whether this addon should be asked for `resource` of `kind`/`id`.
    pub fn supports(&self, resource: &str, kind: &str, id: &str) -> bool {
        for entry in &self.resources {
            let (name, types, prefixes) = match entry {
                ManifestResource::Short(name) => (name, None, None),
                ManifestResource::Full {
                    name,
                    types,
                    id_prefixes,
                } => (name, types.as_ref(), id_prefixes.as_ref()),
            };
            if name != resource {
                continue;
            }
            let types = types.unwrap_or(&self.types);
            if !types.iter().any(|t| t == kind) {
                continue;
            }
            let prefixes = prefixes.or(self.id_prefixes.as_ref());
            match prefixes {
                Some(prefixes) if !prefixes.is_empty() => {
                    if prefixes.iter().any(|p| id.starts_with(p.as_str())) {
                        return true;
                    }
                }
                _ => return true,
            }
        }
        false
    }

    pub fn has_resource(&self, resource: &str) -> bool {
        self.resources.iter().any(|entry| match entry {
            ManifestResource::Short(name) => name == resource,
            ManifestResource::Full { name, .. } => name == resource,
        })
    }
}

/// An installed addon: where it lives plus its manifest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Descriptor {
    #[serde(default, rename = "transportUrl")]
    pub transport_url: String,
    #[serde(default, rename = "transportName", skip_serializing_if = "Option::is_none")]
    pub transport_name: Option<String>,
    #[serde(default)]
    pub manifest: Manifest,
    #[serde(default)]
    pub flags: DescriptorFlags,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DescriptorFlags {
    #[serde(default, deserialize_with = "lenient_bool")]
    pub official: bool,
    #[serde(default, rename = "protected", deserialize_with = "lenient_bool")]
    pub protected: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

// -------------------------------------------------------------------- meta

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default, deserialize_with = "lenient_string")]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub poster: Option<String>,
    #[serde(default, rename = "posterShape", deserialize_with = "lenient_string")]
    pub poster_shape: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub background: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub logo: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub description: Option<String>,
    #[serde(default, rename = "releaseInfo", deserialize_with = "lenient_string")]
    pub release_info: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub year: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub released: Option<String>,
    #[serde(default, rename = "imdbRating", deserialize_with = "lenient_string")]
    pub imdb_rating: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub runtime: Option<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub genres: Vec<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub cast: Vec<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub director: Vec<String>,
    #[serde(default, deserialize_with = "lenient_strings")]
    pub writer: Vec<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub links: Vec<MetaLink>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub videos: Vec<Video>,
    #[serde(default, rename = "trailerStreams", deserialize_with = "lenient_vec")]
    pub trailer_streams: Vec<Stream>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub trailers: Vec<Trailer>,
    #[serde(default, rename = "behaviorHints", deserialize_with = "lenient_default")]
    pub behavior_hints: MetaHints,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetaHints {
    #[serde(default, rename = "defaultVideoId", deserialize_with = "lenient_string")]
    pub default_video_id: Option<String>,
    #[serde(default, rename = "hasScheduledVideos", deserialize_with = "lenient_bool")]
    pub has_scheduled_videos: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetaLink {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trailer {
    #[serde(default)]
    pub source: String,
    #[serde(default, rename = "type")]
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Video {
    #[serde(default, deserialize_with = "lenient_string")]
    pub id: Option<String>,
    // Addons use `title` or `name`, `episode` or `number`, `overview` or
    // `description` - often both at once, so each is its own field.
    #[serde(default, deserialize_with = "lenient_string")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "lenient_string", skip_serializing)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub released: Option<String>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub season: Option<u32>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub episode: Option<u32>,
    #[serde(default, deserialize_with = "lenient_u32", skip_serializing)]
    pub number: Option<u32>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub thumbnail: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub overview: Option<String>,
    #[serde(default, deserialize_with = "lenient_string", skip_serializing)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub streams: Vec<Stream>,
}

impl Video {
    /// Folds the alternative spellings into the canonical fields.
    fn normalized(mut self) -> Self {
        self.title = self.title.or(self.name.take());
        self.episode = self.episode.or(self.number.take());
        self.overview = self.overview.or(self.description.take());
        self
    }
}

impl Meta {
    /// Canonicalises fields that addons spell in more than one way.
    pub fn normalized(mut self) -> Self {
        self.videos = self.videos.into_iter().map(Video::normalized).collect();
        self
    }
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }
    pub fn title(&self) -> &str {
        self.name.as_deref().unwrap_or("Untitled")
    }
    /// Year / release info line shown under titles.
    pub fn release_line(&self) -> Option<String> {
        self.release_info
            .clone()
            .or_else(|| self.year.clone())
            .or_else(|| self.released.as_ref().map(|r| r.chars().take(4).collect()))
    }
    /// Season numbers in display order (specials, season 0, go last).
    pub fn seasons(&self) -> Vec<u32> {
        let mut seasons: Vec<u32> = self.videos.iter().filter_map(|v| v.season).collect();
        seasons.sort_unstable();
        seasons.dedup();
        if seasons.first() == Some(&0) {
            seasons.remove(0);
            seasons.push(0);
        }
        seasons
    }
    pub fn episodes(&self, season: Option<u32>) -> Vec<Video> {
        let mut videos: Vec<Video> = self
            .videos
            .iter()
            .filter(|v| season.is_none() || v.season == season)
            .cloned()
            .collect();
        videos.sort_by(|a, b| {
            (a.season, a.episode, &a.released).cmp(&(b.season, b.episode, &b.released))
        });
        videos
    }
    /// Videos in watch order across all seasons, used for "next episode".
    pub fn ordered_videos(&self) -> Vec<Video> {
        let mut videos = self.videos.clone();
        videos.sort_by_key(|v| {
            let season = v.season.unwrap_or(0);
            // Specials sort after regular seasons.
            (if season == 0 { u32::MAX } else { season }, v.episode.unwrap_or(0))
        });
        videos
    }
}

impl Video {
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }
    pub fn label(&self) -> String {
        let title = self.title.clone().unwrap_or_default();
        match (self.season, self.episode) {
            (Some(s), Some(e)) if title.is_empty() => format!("S{s} E{e}"),
            (Some(s), Some(e)) => format!("S{s}:E{e}  {title}"),
            (None, Some(e)) if !title.is_empty() => format!("{e}. {title}"),
            _ if title.is_empty() => "Video".to_owned(),
            _ => title,
        }
    }
}

// ------------------------------------------------------------------ stream

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    #[serde(default, deserialize_with = "lenient_string", skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(
        default,
        rename = "ytId",
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub yt_id: Option<String>,
    #[serde(
        default,
        rename = "infoHash",
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub info_hash: Option<String>,
    #[serde(
        default,
        rename = "fileIdx",
        deserialize_with = "lenient_u32",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_idx: Option<u32>,
    #[serde(
        default,
        rename = "externalUrl",
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub external_url: Option<String>,
    #[serde(default, deserialize_with = "lenient_string", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "lenient_string", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient_strings",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub sources: Vec<String>,
    #[serde(default, deserialize_with = "lenient_vec", skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<Subtitle>,
    #[serde(default, rename = "behaviorHints", deserialize_with = "lenient_default")]
    pub behavior_hints: StreamHints,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamHints {
    #[serde(default, rename = "notWebReady", deserialize_with = "lenient_bool")]
    pub not_web_ready: bool,
    #[serde(
        default,
        rename = "bingeGroup",
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub binge_group: Option<String>,
    #[serde(default, rename = "proxyHeaders", deserialize_with = "lenient_default")]
    pub proxy_headers: ProxyHeaders,
    #[serde(default, deserialize_with = "lenient_string", skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(
        default,
        rename = "videoSize",
        deserialize_with = "lenient_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub video_size: Option<u64>,
    #[serde(
        default,
        rename = "videoHash",
        deserialize_with = "lenient_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub video_hash: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProxyHeaders {
    #[serde(default, deserialize_with = "lenient_default")]
    pub request: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "lenient_default")]
    pub response: BTreeMap<String, String>,
}

/// What a stream actually points at, after validation.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamSource {
    Url(String),
    Torrent {
        info_hash: String,
        file_idx: Option<u32>,
        trackers: Vec<String>,
    },
    YouTube(String),
    External(String),
    Unsupported,
}

impl Stream {
    pub fn source(&self) -> StreamSource {
        if let Some(url) = &self.url {
            if url.starts_with("magnet:") {
                if let Some(hash) = magnet_info_hash(url) {
                    return StreamSource::Torrent {
                        info_hash: hash,
                        file_idx: self.file_idx,
                        trackers: magnet_trackers(url),
                    };
                }
            }
            return StreamSource::Url(url.clone());
        }
        if let Some(hash) = &self.info_hash {
            let trackers = self
                .sources
                .iter()
                .filter_map(|s| s.strip_prefix("tracker:").map(str::to_owned))
                .collect();
            return StreamSource::Torrent {
                info_hash: hash.to_lowercase(),
                file_idx: self.file_idx,
                trackers,
            };
        }
        if let Some(id) = &self.yt_id {
            return StreamSource::YouTube(id.clone());
        }
        if let Some(url) = &self.external_url {
            return StreamSource::External(url.clone());
        }
        StreamSource::Unsupported
    }

    /// First line: quality / addon tag. Second: the descriptive text.
    pub fn headline(&self) -> String {
        self.name.clone().unwrap_or_default()
    }
    pub fn details(&self) -> String {
        self.description
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_default()
    }
}

fn magnet_info_hash(magnet: &str) -> Option<String> {
    let query = magnet.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let value = pair.strip_prefix("xt=urn:btih:")?;
        Some(value.to_lowercase())
    })
}

fn magnet_trackers(magnet: &str) -> Vec<String> {
    let Some((_, query)) = magnet.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter_map(|pair| pair.strip_prefix("tr="))
        .filter_map(|v| urlencoding::decode(v).ok().map(|s| s.into_owned()))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Subtitle {
    #[serde(default, deserialize_with = "lenient_string")]
    pub id: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub lang: Option<String>,
}

// --------------------------------------------------------------- responses

#[derive(Debug, Default, Deserialize)]
pub struct CatalogResponse {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub metas: Vec<Meta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MetaResponse {
    #[serde(default)]
    pub meta: Option<Meta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamsResponse {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub streams: Vec<Stream>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SubtitlesResponse {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub subtitles: Vec<Subtitle>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AddonCatalogResponse {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub addons: Vec<Descriptor>,
}

// ----------------------------------------------------------------- library

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryItem {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub poster: Option<String>,
    #[serde(default, rename = "posterShape", deserialize_with = "lenient_string")]
    pub poster_shape: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub removed: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub temp: bool,
    #[serde(default, rename = "_ctime", deserialize_with = "lenient_string")]
    pub ctime: Option<String>,
    #[serde(default, rename = "_mtime", deserialize_with = "lenient_string")]
    pub mtime: Option<String>,
    #[serde(default, deserialize_with = "lenient_default")]
    pub state: LibraryState,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryState {
    #[serde(default, rename = "lastWatched", deserialize_with = "lenient_string")]
    pub last_watched: Option<String>,
    /// Milliseconds watched of the current video.
    #[serde(default, rename = "timeWatched", deserialize_with = "lenient_f64")]
    pub time_watched: f64,
    /// Playback position of the current video, milliseconds.
    #[serde(default, rename = "timeOffset", deserialize_with = "lenient_f64")]
    pub time_offset: f64,
    #[serde(default, rename = "overallTimeWatched", deserialize_with = "lenient_f64")]
    pub overall_time_watched: f64,
    #[serde(default, rename = "timesWatched", deserialize_with = "lenient_f64")]
    pub times_watched: f64,
    #[serde(default, rename = "flaggedWatched", deserialize_with = "lenient_f64")]
    pub flagged_watched: f64,
    /// Duration of the current video, milliseconds.
    #[serde(default, deserialize_with = "lenient_f64")]
    pub duration: f64,
    #[serde(default, deserialize_with = "lenient_string")]
    pub video_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub watched: Option<String>,
    #[serde(default, rename = "noNotif", deserialize_with = "lenient_bool")]
    pub no_notif: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl LibraryItem {
    /// Fraction watched of the current video, 0..=1.
    pub fn progress(&self) -> f32 {
        if self.state.duration > 0.0 {
            (self.state.time_offset / self.state.duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
    /// Which items belong in the Continue Watching row.
    pub fn is_in_continue_watching(&self) -> bool {
        self.kind != "other" && (!self.removed || self.temp) && self.state.time_offset > 0.0
    }
    pub fn as_meta(&self) -> Meta {
        Meta {
            id: Some(self.id.clone()),
            kind: self.kind.clone(),
            name: Some(self.name.clone()),
            poster: self.poster.clone(),
            poster_shape: self.poster_shape.clone(),
            ..Meta::default()
        }
    }
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Stream {
    /// The same video source, whatever the addon's title says today
    /// (titles carry seeder counts that change between lookups).
    pub fn same_source(&self, other: &Stream) -> bool {
        match (&self.info_hash, &other.info_hash) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) && self.file_idx == other.file_idx,
            (None, None) => {
                (self.url.is_some() && self.url == other.url)
                    || (self.yt_id.is_some() && self.yt_id == other.yt_id)
                    || (self.external_url.is_some() && self.external_url == other.external_url)
            }
            _ => false,
        }
    }
}
