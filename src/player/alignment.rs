//! Local, timestamped speech matching. Source cues remain immutable; accepted
//! audio anchors supply local corrections, including changes at episode cuts.
use super::{subtitles::Cue, PlayerUi, Status, SubChoice};
use crate::{
    ai::{self, Artifact, AudioSample},
    state::{spawn, App},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const MODEL_SIZE: u64 = 147_951_465;
const MODEL_HASH: &str = "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";
const RUNTIME_SIZE: u64 = 8_928_640;
pub const DOWNLOAD_SIZE: u64 = MODEL_SIZE + RUNTIME_SIZE;
const STEP: u64 = 45;
const WINDOW: f64 = 60.0;
const VERSION: &str = "whisper-base-b5454-dtw-v1";

pub fn model_dir() -> PathBuf {
    ai::model_dir().parent().unwrap().join("subtitle-timing")
}
pub fn installed() -> bool {
    let dir = model_dir();
    std::fs::read_to_string(dir.join("installation.txt")).is_ok_and(|v| v == VERSION)
        && std::fs::metadata(dir.join("ggml-base.bin")).is_ok_and(|m| m.len() == MODEL_SIZE)
        && dir.join("runtime/whisper-cli.exe").is_file()
}

pub async fn install(id: u64, cancel: CancellationToken) -> Result<(), String> {
    let dir = model_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let _ = tokio::fs::remove_file(dir.join("installation.txt")).await;
    let client = reqwest::Client::builder()
        .user_agent("Spoutpeak/0.1")
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let assets = [
        Artifact { name: "ggml-base.bin", size: MODEL_SIZE, hash: MODEL_HASH,
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin".into() },
        Artifact { name: "runtime.zip", size: RUNTIME_SIZE,
            hash: "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb",
            url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip".into() },
    ];
    let mut done = 0;
    for asset in assets {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("Cancelled".into()),
            r = ai::download(&client, &dir, &asset, &cancel, |bytes, stage| ai::progress(id, done + bytes, stage)) => r?,
        }
        done += asset.size;
    }
    let extract_dir = dir.clone();
    let extract_cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        ai::unpack_runtime(&extract_dir, "whisper-cli.exe", &extract_cancel)
    })
    .await
    .map_err(|e| e.to_string())??;
    if cancel.is_cancelled() {
        return Err("Cancelled".into());
    }
    tokio::fs::write(dir.join("installation.txt"), VERSION)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub probability: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlignedLine {
    pub original_start: f64,
    pub original_end: f64,
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f64,
}

#[derive(Clone, Default)]
pub struct TimingMap {
    pub lines: Vec<AlignedLine>,
    evidence: Vec<AlignedLine>,
}

impl TimingMap {
    pub(super) fn reconcile(&mut self, cues: &[Cue]) {
        if self.evidence.is_empty() {
            return;
        }
        let mut counts = HashMap::<Vec<String>, usize>::new();
        for cue in cues {
            *counts.entry(normalized(&cue.text)).or_default() += 1;
        }
        self.evidence
            .retain(|line| counts.get(&normalized(&line.text)) == Some(&1));
        self.lines = confirmed(self.evidence.clone());
    }
    pub fn merge(&mut self, lines: Vec<AlignedLine>) {
        for line in lines {
            if let Some(old) = self
                .evidence
                .iter_mut()
                .find(|v| v.original_start == line.original_start && v.text == line.text)
            {
                // Overlapping windows mustn't make subtitles oscillate.
                if line.confidence > old.confidence + 0.05 {
                    *old = line;
                }
            } else {
                self.evidence.push(line);
            }
        }
        self.lines = confirmed(self.evidence.clone());
    }

    pub fn retime(&self, cue: &Cue) -> Cue {
        if let Some(line) = self
            .lines
            .iter()
            .find(|v| v.original_start == cue.start && v.text == cue.text)
        {
            return Cue {
                start: line.start,
                end: line.end,
                text: cue.text.clone(),
            };
        }
        let before = self
            .lines
            .iter()
            .rev()
            .find(|v| v.original_start <= cue.start);
        let after = self.lines.iter().find(|v| v.original_start > cue.start);
        let offset = match (before, after) {
            (Some(a), Some(b)) if b.original_start - a.original_start <= 360.0 => {
                let da = a.start - a.original_start;
                let db = b.start - b.original_start;
                if (da - db).abs() <= 1.0 {
                    let f = (cue.start - a.original_start) / (b.original_start - a.original_start);
                    Some(da + f * (db - da))
                } else {
                    // A cut changes the offset in a step, not a gradual drift.
                    Some(
                        if cue.start - a.original_start < b.original_start - cue.start {
                            da
                        } else {
                            db
                        },
                    )
                }
            }
            (Some(a), _) if cue.start - a.original_start <= 180.0 => {
                Some(a.start - a.original_start)
            }
            (_, Some(b)) if b.original_start - cue.start <= 180.0 => {
                Some(b.start - b.original_start)
            }
            _ => None,
        };
        offset.map_or_else(
            || cue.clone(),
            |delta| Cue {
                start: (cue.start + delta).max(0.0),
                end: (cue.end + delta).max(0.0),
                text: cue.text.clone(),
            },
        )
    }
}

fn normalized(text: &str) -> Vec<String> {
    ai::words(text)
        .into_iter()
        .map(|w| w.replace('\'', ""))
        .collect()
}

/// Convert decoder subword tokens into actual timed words. DTW requires flash
/// attention OFF; missing DTW timestamps are rejected, never treated as zero.
fn timed_words(
    json: &serde_json::Value,
    offset: f64,
    duration: f64,
) -> Result<Vec<TimedWord>, String> {
    let mut result: Vec<TimedWord> = Vec::new();
    let mut raw = String::new();
    let mut current: Option<TimedWord> = None;
    let flush = |raw: &mut String, current: &mut Option<TimedWord>, result: &mut Vec<TimedWord>| {
        if let Some(word) = current.take() {
            for text in normalized(raw) {
                result.push(TimedWord {
                    text,
                    ..word.clone()
                });
            }
        }
        raw.clear();
    };
    let segments = json["transcription"]
        .as_array()
        .ok_or("Timing model returned no transcription")?;
    for segment in segments {
        let Some(tokens) = segment["tokens"].as_array() else {
            continue;
        };
        for token in tokens {
            let text = token["text"].as_str().unwrap_or("");
            if text.starts_with('[') || text.starts_with("<|") {
                continue;
            }
            if text.starts_with(char::is_whitespace) {
                flush(&mut raw, &mut current, &mut result);
            }
            if !text.chars().any(char::is_alphanumeric) {
                continue;
            }
            let center = token["t_dtw"].as_f64().unwrap_or(-1.0) / 100.0;
            if !center.is_finite() || center < 0.0 || center > duration {
                continue;
            }
            let from = token["offsets"]["from"].as_f64().unwrap_or(0.0) / 1000.0;
            let to = token["offsets"]["to"].as_f64().unwrap_or(from) / 1000.0;
            let half = ((to - from).max(0.02) / 2.0).min(0.25);
            let start = (center - half).max(0.0) + offset;
            let end = (center + half).min(duration) + offset;
            let p = token["p"].as_f64().unwrap_or(0.0);
            if let Some(word) = current.as_mut() {
                word.end = word.end.max(end);
                word.probability = word.probability.min(p);
            } else {
                current = Some(TimedWord {
                    text: String::new(),
                    start,
                    end,
                    probability: p,
                });
            }
            raw.push_str(text.trim());
        }
        flush(&mut raw, &mut current, &mut result);
    }
    result.retain(|w| w.start.is_finite() && w.end.is_finite() && w.end > w.start);
    if !result.is_empty() && result.windows(2).any(|w| w[1].start + 0.5 < w[0].start) {
        return Err("Timing model returned non-monotonic word timestamps".into());
    }
    Ok(result)
}

fn edit_distance(a: &[String], b: &[TimedWord]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, word) in a.iter().enumerate() {
        let mut row = vec![i + 1; b.len() + 1];
        for (j, heard) in b.iter().enumerate() {
            row[j + 1] = (previous[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous[j] + usize::from(*word != heard.text));
        }
        previous = row;
    }
    previous[b.len()]
}

#[cfg(test)]
fn match_lines(cues: &[Cue], words: &[TimedWord], from: f64, until: f64) -> Vec<AlignedLine> {
    confirmed(find_candidates(cues, words, from, until))
}

fn find_candidates(cues: &[Cue], words: &[TimedWord], from: f64, until: f64) -> Vec<AlignedLine> {
    let texts: Vec<_> = cues.iter().map(|c| normalized(&c.text)).collect();
    let mut candidates = Vec::new();
    for (index, cue) in cues.iter().enumerate() {
        let text = &texts[index];
        if text.len() < 4 || text.len() > 36 {
            continue;
        }
        // Repeated dialogue elsewhere in the episode cannot provide an anchor.
        if texts
            .iter()
            .enumerate()
            .any(|(other, t)| other != index && *t == *text)
        {
            continue;
        }
        let mut hits = Vec::new();
        for i in 0..words.len() {
            if words[i].text != text[0] || words[i].start < from + 1.0 {
                continue;
            }
            for count in text.len().saturating_sub(2).max(4)..=text.len() + 2 {
                let Some(heard) = words.get(i..i + count) else {
                    continue;
                };
                if heard.last().unwrap().text != *text.last().unwrap()
                    || heard.last().unwrap().end > until - 1.0
                {
                    continue;
                }
                let score = 1.0 - edit_distance(text, heard) as f64 / text.len().max(count) as f64;
                let probability = heard.iter().map(|w| w.probability).sum::<f64>() / count as f64;
                let duration = heard.last().unwrap().end - heard[0].start;
                if score >= 0.84
                    && probability >= 0.55
                    && duration >= 0.6
                    && duration < (cue.end - cue.start).max(1.0) * 2.5
                {
                    hits.push(AlignedLine {
                        original_start: cue.start,
                        original_end: cue.end,
                        text: cue.text.clone(),
                        start: heard[0].start,
                        end: heard.last().unwrap().end,
                        confidence: score * probability,
                    });
                }
            }
        }
        hits.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        if let Some(best) = hits.first() {
            if !hits
                .iter()
                .skip(1)
                .any(|h| (h.start - best.start).abs() > 0.5 && best.confidence - h.confidence < 0.1)
            {
                candidates.push(best.clone());
            }
        }
    }
    candidates
}

fn confirmed(mut candidates: Vec<AlignedLine>) -> Vec<AlignedLine> {
    // Three separate, temporally consistent lines must agree, including sparse
    // dialogue in neighboring windows. Evidence is deduplicated before here.
    // This rejects
    // hallucinated speech, isolated quotations, and unrelated subtitle releases.
    let snapshot = candidates.clone();
    candidates.retain(|a| {
        let delta = a.start - a.original_start;
        let nearby: Vec<_> = snapshot
            .iter()
            .filter(|b| {
                (b.start - b.original_start - delta).abs() <= 1.2
                    && (b.start - a.start).abs() <= 120.0
                    && ((b.start - a.start) * (b.original_start - a.original_start) >= 0.0)
            })
            .collect();
        nearby.len() >= 3
            && nearby
                .iter()
                .map(|b| b.start)
                .fold(f64::NEG_INFINITY, f64::max)
                - nearby.iter().map(|b| b.start).fold(f64::INFINITY, f64::min)
                >= 5.0
    });
    candidates.sort_by(|a, b| a.original_start.total_cmp(&b.original_start));
    candidates
}

struct WorkDir(PathBuf);
impl WorkDir {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = crate::util::cache_dir()
            .join("subtitle-alignment")
            .join(format!(
                "work-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub async fn recognize(
    sample: AudioSample,
    duration: f64,
    cancel: CancellationToken,
) -> Result<Vec<TimedWord>, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Cancelled".into()),
        r = tokio::time::timeout(Duration::from_secs(150), recognize_inner(&sample, duration)) =>
            r.map_err(|_| "Audio alignment timed out; this section will be retried.".to_owned())?,
    }
}

async fn recognize_inner(sample: &AudioSample, duration: f64) -> Result<Vec<TimedWord>, String> {
    if !installed() {
        return Err("Download the speech timing model in Settings → Local AI.".into());
    }
    let mut key = Sha256::new();
    let source_url = if std::fs::metadata(&sample.url).is_ok() {
        sample.url.replace('\\', "/")
    } else {
        sample.url.clone()
    };
    key.update(format!(
        "{VERSION}|{}|{}|{}|{}|{duration}",
        source_url, sample.stream_index, sample.position, sample.language
    ));
    for (k, v) in &sample.headers {
        key.update(k);
        key.update(v);
    }
    if let Ok(meta) = std::fs::metadata(&sample.url) {
        key.update(format!("{}|{:?}", meta.len(), meta.modified()));
    }
    let cache_dir = crate::util::cache_dir().join("subtitle-alignment");
    let cached = cache_dir.join(format!("{:x}.json", key.finalize()));
    if let Ok(data) = tokio::fs::read(&cached).await {
        if let Ok(words) = serde_json::from_slice::<Vec<TimedWord>>(&data) {
            return Ok(words);
        }
    }
    let work = WorkDir::new()?;
    let wav = work.0.join("audio.wav");
    let output_path = work.0.join("transcript");
    let (ffmpeg, _) =
        super::engine::tools().ok_or("FFmpeg is needed for automatic subtitle sync")?;
    let mut decode = ai::command(ffmpeg);
    decode.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "1",
    ]);
    if sample.url.starts_with("http://") || sample.url.starts_with("https://") {
        let mut headers: String = sample
            .headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        headers.push_str("X-Spoutpeak-Preview: 1\r\n");
        decode.args(["-rw_timeout", "15000000", "-headers", &headers]);
    }
    decode
        .args([
            "-ss",
            &sample.position.to_string(),
            "-i",
            &sample.url,
            "-map",
            &format!("0:{}", sample.stream_index),
            "-t",
            &duration.to_string(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-y",
        ])
        .arg(&wav);
    let decoded = decode.output().await.map_err(|e| e.to_string())?;
    if !decoded.status.success() {
        return Err("Audio is not buffered here yet; automatic sync will retry.".into());
    }
    let dir = model_dir();
    let mut infer = ai::command(dir.join("runtime/whisper-cli.exe"));
    infer
        .arg("-m")
        .arg(dir.join("ggml-base.bin"))
        .arg("-f")
        .arg(&wav)
        .args([
            "-l", "auto", "-dtw", "base", "-nfa", "-ojf", "-t", "4", "-ng", "-np",
        ])
        .arg("-of")
        .arg(&output_path);
    let output = infer.output().await.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("The speech timing model could not analyze this audio.".into());
    }
    let data = tokio::fs::read(output_path.with_extension("json"))
        .await
        .map_err(|e| e.to_string())?;
    let json = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let words = timed_words(&json, sample.position, duration)?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| e.to_string())?;
    let pending = work.0.join("cache.json");
    tokio::fs::write(
        &pending,
        serde_json::to_vec(&words).map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| e.to_string())?;
    let _ = tokio::fs::rename(pending, cached).await;
    Ok(words)
}

struct Attempt {
    cues: usize,
    retry: Instant,
}
struct Job {
    window: u64,
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct AutoSync {
    pub map: TimingMap,
    pub message: String,
    revision: u64,
    attempts: HashMap<u64, Attempt>,
    job: Option<Job>,
}

impl AutoSync {
    fn next_window(&self, position: f64, duration: f64, cues: usize, local: bool) -> Option<u64> {
        let current = position.max(0.0) as u64 / STEP;
        let count = (duration.max(1.0) / STEP as f64).ceil() as u64;
        let usable = |window: &u64| {
            *window < count
                && self
                    .attempts
                    .get(window)
                    .is_none_or(|a| a.cues < cues || Instant::now() >= a.retry)
        };
        (0..=3)
            .flat_map(|distance| [current + distance, current.saturating_sub(distance)])
            .chain(if local { 0..count } else { 0..0 })
            .find(usable)
    }
}

impl PlayerUi {
    pub(crate) fn clear_alignment(&self) {
        self.alignment.write(|s| {
            if let Some(job) = s.job.take() {
                job.cancel.cancel();
            }
            let revision = s.revision + 1;
            *s = AutoSync {
                revision,
                ..Default::default()
            };
        });
    }
}

impl App {
    pub(crate) fn auto_sync_preferences_changed(&self) {
        self.player.clear_alignment();
        if self
            .profile
            .peek(|p| p.settings.local_ai_enabled && p.settings.automatic_subtitle_sync)
        {
            // The former manual correction must not be applied a second time
            // on top of the new, audio-derived timing map.
            self.player.state.write(|s| s.sub_delay = 0.0);
        }
        self.maybe_auto_sync();
    }

    fn accepts_alignment(
        &self,
        session: u64,
        subtitle: u64,
        revision: u64,
        cancel: &CancellationToken,
    ) -> bool {
        !cancel.is_cancelled()
            && self.player.session_id.get() == session
            && self.player.subtitle_revision.get() == subtitle
            && self.player.alignment.peek(|s| s.revision == revision)
            && self
                .profile
                .peek(|p| p.settings.local_ai_enabled && p.settings.automatic_subtitle_sync)
    }
    pub fn download_timing_model(&self) {
        if !ai::supported() || !self.profile.peek(|p| p.settings.local_ai_enabled) {
            return;
        }
        let Some((id, cancel)) = self.ai.begin("Downloading speech timing model…", true) else {
            return;
        };
        self.ai.state.write(|s| s.total = DOWNLOAD_SIZE);
        spawn(install(id, cancel), move |app, result| {
            app.ai.finish(id, result);
            app.maybe_auto_sync();
        });
    }

    pub fn remove_timing_model(&self) {
        let Some((id, _)) = self.ai.begin("Removing speech timing model…", false) else {
            return;
        };
        self.update_settings(|s| s.automatic_subtitle_sync = false);
        spawn(
            async {
                let dir = model_dir();
                if dir.exists() {
                    tokio::fs::remove_dir_all(dir)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            },
            move |app, result| app.ai.finish(id, result),
        );
    }

    pub(crate) fn maybe_auto_sync(&self) {
        if !self
            .profile
            .peek(|p| p.settings.local_ai_enabled && p.settings.automatic_subtitle_sync)
            || !self.ai.state.peek(|s| s.timing_installed)
            || !self
                .player
                .state
                .peek(|s| s.status == Status::Playing && !s.live && s.sub_choice != SubChoice::Off)
        {
            return;
        }
        let position = self.player.position.get_untracked();
        if self.player.alignment.peek(|s| {
            s.job.as_ref().is_some_and(|j| {
                if (j.window as f64 * STEP as f64 - position).abs() > 240.0 {
                    j.cancel.cancel();
                }
                true
            })
        }) || self.ai.state.peek(|s| s.busy)
        {
            return;
        }
        let Some(mut sample) = self
            .player
            .engine
            .borrow()
            .as_ref()
            .and_then(|e| e.audio_sample(position))
        else {
            return;
        };
        if let Some(file) = crate::streaming::completed_stream_file(&sample.url) {
            sample.url = file.to_string_lossy().into_owned();
            sample.headers.clear();
        }
        let mut cues = self.player.cues.borrow().clone();
        if cues.len() < 4 {
            return;
        }
        cues.sort_by(|a, b| a.start.total_cmp(&b.start));
        let (duration, audio_delay) = self.player.state.peek(|s| (s.duration, s.audio_delay));
        let local = !sample.url.starts_with("http://") && !sample.url.starts_with("https://");
        let Some(window) = self
            .player
            .alignment
            .peek(|s| s.next_window(position, duration, cues.len(), local))
        else {
            return;
        };
        let Some((id, cancel)) = self
            .ai
            .begin("Automatically syncing subtitles to audio…", false)
        else {
            return;
        };
        let session = self.player.session_id.get();
        let subtitle = self.player.subtitle_revision.get();
        let revision = self.player.alignment.write(|s| {
            s.job = Some(Job {
                window,
                cancel: cancel.clone(),
            });
            s.message = "Analyzing audio near playback…".into();
            s.revision
        });
        let start = window as f64 * STEP as f64;
        sample.position = (start - audio_delay).max(0.0);
        let audio_start = sample.position;
        let length = WINDOW.min(duration - start).max(0.1);
        let cue_count = cues.len();
        let finished_cancel = cancel.clone();
        spawn(
            async move {
                let mut heard = recognize(sample, length, cancel).await?;
                for word in &mut heard {
                    word.start += audio_delay;
                    word.end += audio_delay;
                }
                Ok::<_, String>(find_candidates(
                    &cues,
                    &heard,
                    audio_start + audio_delay,
                    audio_start + audio_delay + length,
                ))
            },
            move |app, result| {
                let valid = app.accepts_alignment(session, subtitle, revision, &finished_cancel);
                if app.player.alignment.peek(|s| s.revision == revision) {
                    app.player.alignment.write(|s| {
                    s.job = None;
                    if valid {
                        let (retry, message) = match result {
                            Ok(lines) => {
                                let empty = lines.is_empty(); s.map.merge(lines);
                                s.map.reconcile(&app.player.cues.borrow());
                                (Duration::from_secs(86400), if empty { "No confident dialogue matches here; original timing retained.".to_owned() }
                                    else { format!("Automatic sync · {} lines aligned", s.map.lines.len()) })
                            }
                            Err(error) => (Duration::from_secs(30), error),
                        };
                        s.attempts.insert(window, Attempt { cues: cue_count, retry: Instant::now() + retry });
                        s.message = message;
                    }
                });
                }
                app.ai.finish(id, Ok(()));
                app.maybe_auto_sync();
            },
        );
    }

    pub fn cancel_ai_task(&self) {
        if self.player.alignment.peek(|s| s.job.is_some()) {
            self.update_settings(|s| s.automatic_subtitle_sync = false);
        } else {
            self.ai.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, text: &str) -> Cue {
        Cue {
            start,
            end: start + 3.0,
            text: text.into(),
        }
    }
    fn line(source: f64, video: f64, text: &str) -> AlignedLine {
        AlignedLine {
            original_start: source,
            original_end: source + 3.0,
            text: text.into(),
            start: video,
            end: video + 3.0,
            confidence: 0.95,
        }
    }

    #[test]
    fn local_map_handles_different_offsets_and_does_not_extrapolate_to_unknown_sections() {
        let mut map = TimingMap::default();
        let anchors = vec![
            line(100.0, 73.0, "first"),
            line(120.0, 93.0, "second"),
            line(160.0, 136.0, "third"),
            line(180.0, 156.0, "fourth"),
        ];
        map.lines = anchors.clone();
        map.evidence = anchors;
        assert_eq!(map.retime(&cue(100.0, "first")).start, 73.0);
        assert_eq!(map.retime(&cue(110.0, "unmatched")).start, 83.0);
        assert_eq!(map.retime(&cue(135.0, "before cut")).start, 108.0);
        assert_eq!(map.retime(&cue(150.0, "after cut")).start, 126.0);
        assert_eq!(map.retime(&cue(500.0, "unknown")).start, 500.0);
        // Repeated analysis of an overlap must preserve an equally good anchor.
        let mut stable = TimingMap::default();
        stable.merge(vec![
            line(100.0, 73.0, "first"),
            line(110.0, 83.0, "middle"),
            line(120.0, 93.0, "second"),
        ]);
        stable.merge(vec![line(100.0, 74.0, "first")]);
        assert_eq!(stable.retime(&cue(100.0, "first")).start, 73.0);
    }

    fn speech(start: f64, text: &str) -> Vec<TimedWord> {
        normalized(text)
            .into_iter()
            .enumerate()
            .map(|(i, text)| TimedWord {
                text,
                start: start + i as f64 * 0.3,
                end: start + i as f64 * 0.3 + 0.25,
                probability: 0.98,
            })
            .collect()
    }

    #[test]
    fn anchors_require_unique_dialogue_and_agreement_across_multiple_lines() {
        let cues = vec![
            cue(100.0, "We should take the first road home"),
            cue(110.0, "There are people waiting at the gate"),
            cue(120.0, "I will speak with them after dinner"),
        ];
        let mut words = speech(70.0, &cues[0].text);
        assert!(match_lines(&cues, &words, 60.0, 120.0).is_empty());
        words.extend(speech(80.0, &cues[1].text));
        words.extend(speech(90.0, &cues[2].text));
        assert_eq!(match_lines(&cues, &words, 60.0, 120.0).len(), 3);
        let mut repeated = cues.clone();
        repeated.push(cue(200.0, &cues[0].text));
        assert!(match_lines(&repeated, &words, 60.0, 120.0).is_empty());
        words.iter_mut().for_each(|w| w.probability = 0.1);
        assert!(match_lines(&cues, &words, 60.0, 120.0).is_empty());
    }

    #[test]
    fn token_times_need_dtw_and_are_independent_of_sample_start() {
        let json = serde_json::json!({"transcription":[{"tokens":[
            {"text":" Hello", "p":0.99, "t_dtw":200, "offsets":{"from":1500,"to":1900}},
            {"text":" world", "p":0.99, "t_dtw":250, "offsets":{"from":1900,"to":2300}},
            {"text":"[_TT_200]", "p":0.9, "t_dtw":-1}
        ]}]});
        let a = timed_words(&json, 100.0, 10.0).unwrap();
        assert_eq!(a.len(), 2);
        assert!((a[0].start - 101.8).abs() < 1e-6);
        let mut missing = json.clone();
        for token in missing["transcription"][0]["tokens"]
            .as_array_mut()
            .unwrap()
        {
            token["t_dtw"] = (-1).into();
        }
        assert!(timed_words(&missing, 100.0, 10.0).unwrap().is_empty());
    }

    #[test]
    fn scheduling_prioritizes_seek_destination_and_limits_network_reads() {
        let mut sync = AutoSync::default();
        assert_eq!(sync.next_window(900.0, 2500.0, 50, false), Some(20));
        for window in 17..=23 {
            sync.attempts.insert(
                window,
                Attempt {
                    cues: 50,
                    retry: Instant::now() + Duration::from_secs(60),
                },
            );
        }
        assert_eq!(sync.next_window(900.0, 2500.0, 50, false), None);
        assert_eq!(sync.next_window(900.0, 2500.0, 50, true), Some(0));
        assert_eq!(sync.next_window(900.0, 2500.0, 51, false), Some(20));
        assert_eq!(sync.next_window(1800.0, 2500.0, 50, false), Some(40));
    }

    #[test]
    fn sparse_dialogue_accumulates_unique_evidence_across_windows() {
        let mut map = TimingMap::default();
        map.merge(vec![line(100.0, 73.0, "one")]);
        map.merge(vec![line(100.0, 73.1, "one")]);
        assert!(map.lines.is_empty());
        map.merge(vec![line(150.0, 123.0, "two")]);
        assert!(map.lines.is_empty());
        map.merge(vec![line(200.0, 173.0, "three")]);
        assert_eq!(map.lines.len(), 3);
        map.merge(vec![line(1000.0, 973.0, "far away")]);
        assert_eq!(map.lines.len(), 3);
    }

    #[test]
    fn enabling_automatic_sync_clears_old_manual_delay_and_cancels_previous_jobs() {
        let app = App::with_profile(crate::store::Profile::default());
        let cancel = CancellationToken::new();
        app.player.alignment.write(|s| {
            s.job = Some(Job {
                window: 0,
                cancel: cancel.clone(),
            })
        });
        app.player.state.write(|s| s.sub_delay = -26.65);
        app.profile.write(|p| {
            p.settings.local_ai_enabled = true;
            p.settings.automatic_subtitle_sync = true;
        });
        app.auto_sync_preferences_changed();
        assert!(cancel.is_cancelled());
        assert_eq!(app.player.state.peek(|s| s.sub_delay), 0.0);
        assert!(app.player.alignment.peek(|s| s.map.lines.is_empty()));
    }

    #[test]
    fn obsolete_alignment_cannot_cross_sessions_tracks_disable_or_cancel() {
        let app = App::with_profile(crate::store::Profile::default());
        app.profile.write(|p| {
            p.settings.local_ai_enabled = true;
            p.settings.automatic_subtitle_sync = true;
        });
        let session = app.player.session_id.get();
        let subtitle = app.player.subtitle_revision.get();
        let revision = app.player.alignment.peek(|s| s.revision);
        let cancel = CancellationToken::new();
        assert!(app.accepts_alignment(session, subtitle, revision, &cancel));
        assert!(!app.accepts_alignment(session + 1, subtitle, revision, &cancel));
        assert!(!app.accepts_alignment(session, subtitle + 1, revision, &cancel));
        app.player.clear_alignment();
        assert!(!app.accepts_alignment(session, subtitle, revision, &cancel));
        let revision = app.player.alignment.peek(|s| s.revision);
        app.profile
            .write(|p| p.settings.automatic_subtitle_sync = false);
        assert!(!app.accepts_alignment(session, subtitle, revision, &cancel));
        app.profile
            .write(|p| p.settings.automatic_subtitle_sync = true);
        cancel.cancel();
        assert!(!app.accepts_alignment(session, subtitle, revision, &cancel));
    }

    #[test]
    fn later_subtitle_batches_invalidate_previously_unique_repeated_dialogue() {
        let mut map = TimingMap::default();
        let cues = vec![
            cue(100.0, "we should go home now"),
            cue(110.0, "take the first road back"),
            cue(120.0, "there is nobody waiting outside"),
        ];
        map.merge(
            cues.iter()
                .map(|c| line(c.start, c.start - 27.0, &c.text))
                .collect(),
        );
        assert_eq!(map.lines.len(), 3);
        let mut later = cues;
        later.push(cue(900.0, "we should go home now"));
        map.reconcile(&later);
        assert!(map.lines.is_empty());
    }

    #[tokio::test]
    #[ignore = "downloads optional 157 MB speech timing model; run explicitly"]
    async fn install_timing_model() {
        install(0, CancellationToken::new()).await.unwrap();
        assert!(installed());
    }

    #[tokio::test]
    #[ignore = "requires installed timing model and SPOUTPEAK_AI_CASES episode fixtures"]
    async fn real_automatic_alignment() {
        let input = std::env::var("SPOUTPEAK_AI_CASES").unwrap();
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
        let cues = super::super::subtitles::parse(
            &std::fs::read_to_string(fixture["subtitles"].as_str().unwrap()).unwrap(),
        );
        let mut results = Vec::new();
        let mut map = TimingMap::default();
        for window in fixture["alignment_windows"].as_array().unwrap() {
            let start = window.as_f64().unwrap();
            let timer = Instant::now();
            let words = recognize(
                AudioSample {
                    url: fixture["media"].as_str().unwrap().into(),
                    headers: vec![],
                    stream_index: 1,
                    position: start,
                    language: "eng".into(),
                },
                60.0,
                CancellationToken::new(),
            )
            .await
            .unwrap();
            let matches = find_candidates(&cues, &words, start, start + 60.0);
            map.merge(matches.clone());
            let row = serde_json::json!({"window":start,"seconds":timer.elapsed().as_secs_f64(),"word_count":words.len(),"matches":matches});
            println!(
                "window {start}: {} words, {} lines, {:.2}s",
                words.len(),
                matches.len(),
                timer.elapsed().as_secs_f64()
            );
            results.push(row);
            std::fs::write(
                format!("{input}.alignment.json"),
                serde_json::to_vec_pretty(&results).unwrap(),
            )
            .unwrap();
        }
        assert!(results
            .iter()
            .any(|r| r["matches"].as_array().unwrap().len() >= 3));
        std::fs::write(
            format!("{input}.timing-map.json"),
            serde_json::to_vec_pretty(&map.lines).unwrap(),
        )
        .unwrap();
        let retimed: Vec<_> = cues.iter().map(|cue| {
            let timed = map.retime(cue);
            serde_json::json!({"original_start":cue.start,"start":timed.start,"end":timed.end,"text":timed.text})
        }).collect();
        std::fs::write(
            format!("{input}.retimed.json"),
            serde_json::to_vec_pretty(&retimed).unwrap(),
        )
        .unwrap();
    }
}
