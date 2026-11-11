//! Network images: fetched on Tokio, cached on disk, decoded and downscaled
//! off the UI thread, then published through a per-image signal so only the
//! widget showing that image rebuilds.

use crate::state::spawn;
use crate::media::addon::http;
use crate::util::{cache_dir, hash_hex};
use incular::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

#[derive(Clone, Debug, PartialEq)]
pub enum Picture {
    Loading,
    Ready(ImageHandle),
    Failed,
}

const MAX_ENTRIES: usize = 700;

thread_local! {
    static CACHE: RefCell<HashMap<String, Signal<Picture>>> = RefCell::new(HashMap::new());
}

fn limiter() -> Arc<Semaphore> {
    static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMIT.get_or_init(|| Arc::new(Semaphore::new(8))).clone()
}

/// Returns the signal for `url` scaled down to at most `max_width` pixels,
/// starting the load on first use. UI thread only.
pub fn picture(url: &str, max_width: u32) -> Signal<Picture> {
    shaped(url, max_width, None)
}

/// How an image is cropped and rounded before upload: the box it will be
/// shown in (logical units) and that box's corner radius.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

/// Like [`picture`], but cover-cropped to `shape` with rounded corners baked
/// into the alpha channel (Incular 0.1.1's `ClipRRect` clips to the bounding
/// rectangle only).
pub fn shaped(url: &str, max_width: u32, shape: Option<Shape>) -> Signal<Picture> {
    let key = match shape {
        Some(s) => format!("{max_width}|{}x{}r{}|{url}", s.width, s.height, s.radius),
        None => format!("{max_width}|{url}"),
    };
    if let Some(existing) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return existing;
    }
    let signal = Signal::new(Picture::Loading);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        // Bounded: live widgets keep their own signal clones, and anything
        // dropped here reloads from the disk cache if shown again.
        if cache.len() >= MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(key.clone(), signal.clone());
    });
    let url = url.to_owned();
    spawn(
        async move {
            let handle = load(&url, max_width, shape).await;
            if handle.is_none() && std::env::var_os("SPOUTPEAK_DEBUG").is_some() {
                eprintln!("[image] failed: {url}");
            }
            handle
        },
        move |_, handle| {
            let signal = CACHE.with(|cache| cache.borrow().get(&key).cloned());
            if let Some(signal) = signal {
                signal.set(match handle {
                    Some(handle) => Picture::Ready(handle),
                    None => Picture::Failed,
                });
            }
        },
    );
    signal
}

/// Hosts that keep timing out are skipped for a while, so one dead image
/// CDN cannot hold every download slot and starve the others.
fn host_health() -> &'static Mutex<HashMap<String, (u32, Instant)>> {
    static HEALTH: OnceLock<Mutex<HashMap<String, (u32, Instant)>>> = OnceLock::new();
    HEALTH.get_or_init(|| Mutex::new(HashMap::new()))
}

fn host_of(url: &str) -> String {
    url.split('/').nth(2).unwrap_or("").to_owned()
}

fn host_is_down(host: &str) -> bool {
    host_health()
        .lock()
        .unwrap()
        .get(host)
        .is_some_and(|(failures, since)| *failures >= 3 && since.elapsed() < Duration::from_secs(90))
}

fn record_host(host: &str, reachable: bool) {
    let mut health = host_health().lock().unwrap();
    if reachable {
        health.remove(host);
    } else {
        let entry = health.entry(host.to_owned()).or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
    }
}

async fn load(url: &str, max_width: u32, shape: Option<Shape>) -> Option<ImageHandle> {
    let dir = cache_dir().join("images");
    let path = dir.join(hash_hex(url));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => {
            let host = host_of(url);
            if host_is_down(&host) {
                return None;
            }
            let _permit = limiter().acquire_owned().await.ok()?;
            if host_is_down(&host) {
                return None;
            }
            let response = match http().get(url).timeout(Duration::from_secs(8)).send().await {
                Ok(response) => {
                    record_host(&host, true);
                    response
                }
                Err(_) => {
                    record_host(&host, false);
                    return None;
                }
            };
            if !response.status().is_success() {
                return None;
            }
            let bytes = response.bytes().await.ok()?.to_vec();
            let _ = tokio::fs::create_dir_all(&dir).await;
            let _ = tokio::fs::write(&path, &bytes).await;
            bytes
        }
    };
    tokio::task::spawn_blocking(move || decode(&bytes, max_width, shape))
        .await
        .ok()?
}

fn decode(bytes: &[u8], max_width: u32, shape: Option<Shape>) -> Option<ImageHandle> {
    let mut decoded = image::load_from_memory(bytes).ok()?;
    if let Some(shape) = shape.filter(|s| s.width > 0.0 && s.height > 0.0) {
        // Cover-crop to the display box's aspect ratio, centred.
        let (w, h) = (decoded.width() as f32, decoded.height() as f32);
        let aspect = shape.width / shape.height;
        let (cw, ch) = if w / h > aspect { (h * aspect, h) } else { (w, w / aspect) };
        let (cw, ch) = ((cw.round() as u32).max(1), (ch.round() as u32).max(1));
        decoded = decoded.crop_imm(
            (decoded.width() - cw.min(decoded.width())) / 2,
            (decoded.height() - ch.min(decoded.height())) / 2,
            cw,
            ch,
        );
    }
    if decoded.width() > max_width {
        let height = (decoded.height() as u64 * max_width as u64 / decoded.width() as u64) as u32;
        decoded = decoded.resize_exact(
            max_width,
            height.max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    let mut rgba = decoded.into_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    if let Some(shape) = shape.filter(|s| s.radius > 0.0 && s.width > 0.0) {
        round_corners(&mut rgba, shape.radius * width as f32 / shape.width);
    }
    ImageHandle::from_rgba8(width, height, rgba.into_raw()).ok()
}

/// Fades alpha outside a rounded rectangle of `radius` pixels, antialiased.
fn round_corners(image: &mut image::RgbaImage, radius: f32) {
    let (width, height) = (image.width(), image.height());
    let radius = radius.min(width as f32 / 2.0).min(height as f32 / 2.0);
    let reach = radius.ceil() as u32;
    for y in 0..height {
        let near_y = y < reach || y >= height.saturating_sub(reach);
        if !near_y {
            continue;
        }
        for x in (0..reach.min(width)).chain(width.saturating_sub(reach)..width) {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let cx = px.clamp(radius, width as f32 - radius);
            let cy = py.clamp(radius, height as f32 - radius);
            let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            if coverage < 1.0 {
                let pixel = image.get_pixel_mut(x, y);
                pixel.0[3] = (pixel.0[3] as f32 * coverage) as u8;
            }
        }
    }
}
