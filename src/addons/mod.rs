//! Addons: the HTTP addon protocol that every content source speaks.
//!
//! `{base}/{resource}/{type}/{id}.json`, or with extra arguments
//! `{base}/{resource}/{type}/{id}/{k=v&k2=v2}.json`.

pub mod types;

use serde::de::DeserializeOwned;
use types::*;
use std::sync::OnceLock;
use std::time::Duration;

pub const CINEMETA: &str = "https://v3-cinemeta.strem.io/manifest.json";
pub const OPENSUBTITLES: &str = "https://opensubtitles-v3.strem.io/manifest.json";
pub const WATCHHUB: &str = "https://watchhub.strem.io/manifest.json";
pub const COMMUNITY_CATALOG: &str = "https://api.strem.io/addonscollection.json";

/// Addons a new profile starts with.
pub const DEFAULT_ADDONS: &[&str] = &[CINEMETA, OPENSUBTITLES, WATCHHUB];

pub fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Spoutpeak/0.1")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client")
    })
}

/// Accepts what users paste: an addon install link (`media://…`, the scheme addon sites use), a bare host, a URL with or
/// without `manifest.json`.
pub fn normalize_transport_url(input: &str) -> Option<String> {
    let mut url = input.trim().to_owned();
    if url.is_empty() {
        return None;
    }
    if let Some((_, rest)) = url.split_once("://").filter(|(scheme, _)| !matches!(*scheme, "http" | "https")) {
        url = format!("https://{rest}");
    } else if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{url}");
    }
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
        None => (url, None),
    };
    let mut path = path.trim_end_matches('/').to_owned();
    if !path.ends_with("manifest.json") {
        path.push_str("/manifest.json");
    }
    Some(match query {
        Some(q) => format!("{path}?{q}"),
        None => path,
    })
}

pub fn base_url(transport_url: &str) -> String {
    let path = transport_url.split('?').next().unwrap_or(transport_url);
    path.strip_suffix("/manifest.json")
        .unwrap_or(path)
        .trim_end_matches('/')
        .to_owned()
}

/// Where the addon's own configuration page lives.
pub fn configure_url(transport_url: &str) -> String {
    format!("{}/configure", base_url(transport_url))
}

pub fn resource_url(
    transport_url: &str,
    resource: &str,
    kind: &str,
    id: &str,
    extra: &[(String, String)],
) -> String {
    let base = base_url(transport_url);
    let query = transport_url
        .split_once('?')
        .map(|(_, q)| format!("?{q}"))
        .unwrap_or_default();
    let kind = urlencoding::encode(kind);
    let id = urlencoding::encode(id);
    if extra.is_empty() {
        format!("{base}/{resource}/{kind}/{id}.json{query}")
    } else {
        let extra = extra
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{base}/{resource}/{kind}/{id}/{extra}.json{query}")
    }
}

pub async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    let response = http()
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", short_error(&e)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("read failed: {}", short_error(&e)))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid response: {e}"))
}

fn short_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "timed out".into()
    } else if error.is_connect() {
        "could not connect".into()
    } else {
        error.to_string()
    }
}

pub async fn fetch_manifest(transport_url: &str) -> Result<Descriptor, String> {
    let manifest: Manifest = get_json(transport_url).await?;
    if manifest.id.is_empty() {
        return Err("not an addon manifest".into());
    }
    Ok(Descriptor {
        transport_url: transport_url.to_owned(),
        transport_name: None,
        manifest,
        flags: DescriptorFlags::default(),
    })
}

pub async fn catalog(
    addon: &Descriptor,
    kind: &str,
    id: &str,
    extra: &[(String, String)],
) -> Result<Vec<Meta>, String> {
    let url = resource_url(&addon.transport_url, "catalog", kind, id, extra);
    let response: CatalogResponse = get_json(&url).await?;
    Ok(response
        .metas
        .into_iter()
        .filter(|m| m.id.is_some())
        .map(|mut m| {
            if m.kind.is_empty() {
                m.kind = kind.to_owned();
            }
            m
        })
        .collect())
}

/// Asks each capable addon in install order and returns the first meta.
pub async fn meta(addons: &[Descriptor], kind: &str, id: &str) -> Result<Meta, String> {
    let mut last_error = "no installed addon provides details for this item".to_owned();
    for addon in addons.iter().filter(|a| a.manifest.supports("meta", kind, id)) {
        let url = resource_url(&addon.transport_url, "meta", kind, id, &[]);
        match get_json::<MetaResponse>(&url).await {
            Ok(MetaResponse { meta: Some(mut meta) }) => {
                if meta.id.is_none() {
                    meta.id = Some(id.to_owned());
                }
                if meta.kind.is_empty() {
                    meta.kind = kind.to_owned();
                }
                return Ok(meta.normalized());
            }
            Ok(_) => last_error = format!("{} has no details for this item", addon.manifest.name),
            Err(error) => last_error = format!("{}: {error}", addon.manifest.name),
        }
    }
    Err(last_error)
}

pub async fn streams(addon: &Descriptor, kind: &str, video_id: &str) -> Result<Vec<Stream>, String> {
    let url = resource_url(&addon.transport_url, "stream", kind, video_id, &[]);
    Ok(get_json::<StreamsResponse>(&url).await?.streams)
}

pub async fn subtitles(
    addon: &Descriptor,
    kind: &str,
    video_id: &str,
    extra: &[(String, String)],
) -> Result<Vec<Subtitle>, String> {
    let url = resource_url(&addon.transport_url, "subtitles", kind, video_id, extra);
    Ok(get_json::<SubtitlesResponse>(&url).await?.subtitles)
}

/// The public community addon list shown under "Community addons".
pub async fn community_addons() -> Result<Vec<Descriptor>, String> {
    get_json::<Vec<Descriptor>>(COMMUNITY_CATALOG).await
}

pub async fn default_addons() -> Vec<Descriptor> {
    let mut addons = Vec::new();
    for url in DEFAULT_ADDONS {
        if let Ok(mut addon) = fetch_manifest(url).await {
            addon.flags.official = true;
            addon.flags.protected = *url == CINEMETA;
            addons.push(addon);
        }
    }
    addons
}

/// A catalog request an installed addon can answer.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogRef {
    pub addon: Descriptor,
    pub catalog: ManifestCatalog,
}

impl CatalogRef {
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.addon.transport_url, self.catalog.kind, self.catalog.id
        )
    }
    pub fn title(&self) -> String {
        let kind = &self.catalog.kind;
        let mut kind_label = kind.clone();
        if let Some(first) = kind_label.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{} - {}", self.catalog.title(), kind_label)
    }
}

pub fn all_catalogs(addons: &[Descriptor]) -> Vec<CatalogRef> {
    addons
        .iter()
        .flat_map(|addon| {
            addon.manifest.catalogs.iter().map(move |catalog| CatalogRef {
                addon: addon.clone(),
                catalog: catalog.clone(),
            })
        })
        .collect()
}
