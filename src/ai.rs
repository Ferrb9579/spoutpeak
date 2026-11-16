//! Optional, on-demand local inference. No model downloads or processes at startup.
use crate::state::{self, App, Shared};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const REVISION: &str = "d508b88f08a5b4286f2d2805a6a8d69a0ff44c72";
const MODEL: &str = "LFM3-Omni-2B-Q4_K_M.gguf";
const PROJECTOR: &str = "mmproj-LFM3-Omni-2B-Q8_0.gguf";
const MODEL_SIZE: u64 = 1_099_630_080;
const PROJECTOR_SIZE: u64 = 506_842_528;
const RUNTIME_SIZE: u64 = 24_407_745;
pub const DOWNLOAD_SIZE: u64 = MODEL_SIZE + PROJECTOR_SIZE + RUNTIME_SIZE;
const INSTALL: &str = "installation-v1.json";

pub fn model_dir() -> PathBuf {
    dirs::data_local_dir()
        .map(|p| p.join("spoutpeak"))
        .unwrap_or_else(crate::util::data_dir)
        .join("models")
        .join("lfm3-omni-2b")
}

pub fn supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

fn installed_at(dir: &Path) -> bool {
    std::fs::read(dir.join(INSTALL))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|v| v["revision"] == REVISION)
        && [(MODEL, MODEL_SIZE), (PROJECTOR, PROJECTOR_SIZE)]
            .iter()
            .all(|(name, size)| std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size))
        && dir.join("runtime/llama-server.exe").is_file()
}

#[derive(Clone, Default)]
pub struct ModelState {
    pub installed: bool,
    pub busy: bool,
    pub downloading: bool,
    pub downloaded: u64,
    pub stage: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct LocalAi {
    pub state: Shared<ModelState>,
    generation: Rc<Cell<u64>>,
    cancel: Rc<RefCell<Option<CancellationToken>>>,
}

impl LocalAi {
    pub fn new() -> Self {
        Self {
            state: Shared::new(ModelState {
                installed: installed_at(&model_dir()),
                ..Default::default()
            }),
            generation: Rc::new(Cell::new(0)),
            cancel: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn begin(&self, stage: &str, downloading: bool) -> Option<(u64, CancellationToken)> {
        if self.state.peek(|s| s.busy) || state::tokio().is_none() {
            return None;
        }
        let id = self.generation.get() + 1;
        self.generation.set(id);
        let cancel = CancellationToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());
        self.state.write(|s| {
            s.busy = true;
            s.downloading = downloading;
            s.error = None;
            s.stage = stage.into();
            s.downloaded = 0;
        });
        Some((id, cancel))
    }

    pub fn cancel(&self) {
        if let Some(cancel) = self.cancel.borrow().as_ref() {
            cancel.cancel();
        }
    }

    pub(crate) fn finish(&self, id: u64, result: Result<(), String>) {
        if self.generation.get() != id {
            return;
        }
        self.cancel.borrow_mut().take();
        self.state.write(|s| {
            s.busy = false;
            s.downloading = false;
            s.stage.clear();
            s.installed = installed_at(&model_dir());
            s.error = result.err().filter(|e| e != "Cancelled");
        });
    }
}

fn progress(id: u64, downloaded: u64, stage: &str) {
    let stage = stage.to_owned();
    state::ui(move |app| {
        if app.ai.generation.get() == id && app.ai.state.peek(|s| s.busy) {
            app.ai.state.write(|s| {
                s.downloaded = downloaded;
                s.stage = stage;
            });
        }
    });
}

impl App {
    pub fn download_ai_model(&self) {
        if !supported() {
            self.notify("Local AI installation currently requires Windows x64.");
            return;
        }
        if !self.profile.peek(|p| p.settings.local_ai_enabled) {
            return;
        }
        let Some((id, cancel)) = self.ai.begin("Preparing download…", true) else {
            return;
        };
        state::spawn(install(id, cancel), move |app, result| {
            app.ai.finish(id, result)
        });
    }

    pub fn remove_ai_model(&self) {
        let Some((id, _)) = self.ai.begin("Removing model…", false) else {
            return;
        };
        self.update_settings(|s| s.local_ai_enabled = false);
        state::spawn(
            async {
                let dir = model_dir();
                // Only the fixed application-owned model folder, never a supplied path.
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
}

struct Artifact {
    name: &'static str,
    size: u64,
    hash: &'static str,
    url: String,
}

fn artifacts() -> Vec<Artifact> {
    let hf = |name| {
        format!("https://huggingface.co/LiquidAI/LFM3-Omni-2B-GGUF/resolve/{REVISION}/{name}")
    };
    vec![
        Artifact { name: MODEL, size: MODEL_SIZE, hash: "9f8d30e9d4fd7a9349d586e077988229b3ca0a0e4ad730897563d1eccc9893e2", url: hf(MODEL) },
        Artifact { name: PROJECTOR, size: PROJECTOR_SIZE, hash: "8a25f9d1931244429de404fccacfb7fb933970a36aa5688d2f5069f29c6b27b1", url: hf(PROJECTOR) },
        Artifact { name: "runtime.zip", size: RUNTIME_SIZE, hash: "73f962f82850d244db47eaacb9d913637027b82399124324298576d58c5f4287", url: "https://github.com/ggml-org/llama.cpp/releases/download/b11960/llama-b11960-bin-win-cpu-x64.zip".into() },
    ]
}

async fn install(id: u64, cancel: CancellationToken) -> Result<(), String> {
    let dir = model_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    // A previous installation must not stay marked ready after a failed repair.
    let _ = tokio::fs::remove_file(dir.join(INSTALL)).await;
    let client = reqwest::Client::builder()
        .user_agent("Spoutpeak/0.1")
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut completed = 0;
    for artifact in artifacts() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("Cancelled".into()),
            result = download(&client, &dir, &artifact, &cancel, |bytes, stage| {
                progress(id, completed + bytes, stage);
            }) => result?,
        }
        completed += artifact.size;
    }
    progress(id, completed, "Installing local runtime…");
    let extract_dir = dir.clone();
    let extract_cancel = cancel.clone();
    tokio::task::spawn_blocking(move || extract_runtime(&extract_dir, &extract_cancel))
        .await
        .map_err(|e| e.to_string())??;
    if cancel.is_cancelled() {
        return Err("Cancelled".into());
    }
    let manifest = serde_json::json!({"revision": REVISION, "runtime": "b11960", "quantization": "Q4_K_M", "projector": "Q8_0"});
    tokio::fs::write(dir.join(INSTALL), manifest.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Resumes partial downloads only when the server confirms the exact byte range.
/// Hashes resumed bytes too; only verified artifacts get their final filename.
async fn download(
    client: &reqwest::Client,
    dir: &Path,
    artifact: &Artifact,
    cancel: &CancellationToken,
    report: impl Fn(u64, &str),
) -> Result<(), String> {
    let final_path = dir.join(artifact.name);
    let partial = dir.join(format!("{}.part", artifact.name));
    if final_path.exists() {
        // Revalidate finished files on an explicit retry/repair, off the UI thread.
        if verify(&final_path, artifact, cancel).await? {
            report(artifact.size, artifact.name);
            return Ok(());
        }
        tokio::fs::remove_file(&final_path)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut offset = tokio::fs::metadata(&partial).await.map_or(0, |m| m.len());
    if offset > artifact.size {
        tokio::fs::remove_file(&partial)
            .await
            .map_err(|e| e.to_string())?;
        offset = 0;
    }
    if offset < artifact.size {
        let mut request = client
            .get(&artifact.url)
            .header("Accept-Encoding", "identity");
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let resume = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if resume {
            let expected = format!("bytes {offset}-{}/{}", artifact.size - 1, artifact.size);
            if response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                != Some(expected.as_str())
            {
                return Err(
                    "The download server returned an unexpected byte range. Retry the download."
                        .into(),
                );
            }
        } else {
            offset = 0;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resume)
            .truncate(!resume)
            .open(&partial)
            .await
            .map_err(|e| e.to_string())?;
        let mut last = Instant::now() - Duration::from_secs(1);
        while let Some(bytes) = response.chunk().await.map_err(|e| e.to_string())? {
            if cancel.is_cancelled() {
                return Err("Cancelled".into());
            }
            offset += bytes.len() as u64;
            if offset > artifact.size {
                return Err("Model download exceeded its expected size.".into());
            }
            file.write_all(&bytes).await.map_err(|e| e.to_string())?;
            if last.elapsed() >= Duration::from_millis(200) {
                report(offset, artifact.name);
                last = Instant::now();
            }
        }
        file.flush().await.map_err(|e| e.to_string())?;
    }
    report(offset, "Verifying download…");
    if !verify(&partial, artifact, cancel).await? {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(format!(
            "{} failed verification. Retry to download it again.",
            artifact.name
        ));
    }
    tokio::fs::rename(&partial, &final_path)
        .await
        .map_err(|e| e.to_string())?;
    report(artifact.size, artifact.name);
    Ok(())
}

async fn verify(
    path: &Path,
    artifact: &Artifact,
    cancel: &CancellationToken,
) -> Result<bool, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?;
    if file.metadata().await.map_err(|e| e.to_string())?.len() != artifact.size {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1024 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        let len = file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if len == 0 {
            break;
        }
        hash.update(&buf[..len]);
    }
    Ok(format!("{:x}", hash.finalize()) == artifact.hash)
}

fn extract_runtime(dir: &Path, cancel: &CancellationToken) -> Result<(), String> {
    let file = std::fs::File::open(dir.join("runtime.zip")).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let target = dir.join("runtime");
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = entry.enclosed_name().ok_or("Unsafe runtime archive path")?;
        // Official Windows archive is flat. Retain runtime licenses alongside DLLs.
        if entry.is_dir() {
            continue;
        }
        let name = path.file_name().ok_or("Missing runtime filename")?;
        let mut output = std::fs::File::create(target.join(name)).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut output).map_err(|e| e.to_string())?;
    }
    if !target.join("llama-server.exe").is_file() {
        return Err("The runtime archive is missing llama-server.exe.".into());
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct AudioSample {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub stream_index: usize,
    pub position: f64,
    pub language: String,
}

#[derive(Clone, PartialEq)]
pub struct SubtitleMatch {
    pub transcript: String,
    pub cue: Option<crate::player::subtitles::Cue>,
    pub position: f64,
}

fn command(path: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(path);
    c.kill_on_drop(true).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

pub(crate) async fn transcribe(
    sample: AudioSample,
    cancel: CancellationToken,
) -> Result<String, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Cancelled".into()),
        result = tokio::time::timeout(Duration::from_secs(240), transcribe_inner(sample)) =>
            result.map_err(|_| "Local AI timed out. Try again at a short, clear line of dialogue.".to_owned())?,
    }
}

async fn transcribe_inner(sample: AudioSample) -> Result<String, String> {
    let dir = model_dir();
    if !installed_at(&dir) {
        return Err("Download LFM3-Omni-2B in Settings first.".into());
    }
    let (ffmpeg, _) = crate::player::engine::tools().ok_or("FFmpeg is needed to sample audio.")?;
    let mut extract = command(ffmpeg);
    extract.args([
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
        extract.args(["-rw_timeout", "15000000", "-headers", &headers]);
    }
    extract.args([
        "-ss",
        &sample.position.max(0.0).to_string(),
        "-i",
        &sample.url,
        "-map",
        &format!("0:{}", sample.stream_index),
        "-t",
        "8",
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-c:a",
        "pcm_s16le",
        "-f",
        "wav",
        "pipe:1",
    ]);
    let audio = extract.output().await.map_err(|e| e.to_string())?;
    if !audio.status.success() || audio.stdout.len() < 1000 {
        return Err(
            "Could not read audio at this position. Let this section buffer and retry.".into(),
        );
    }
    // Bind locally and authenticate requests so a port conflict cannot send media
    // to another service. The model is unloaded when this task finishes/cancels.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| e.to_string())?
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    let key = format!(
        "spoutpeak-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let threads =
        std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 4));
    let mut server = command(dir.join("runtime/llama-server.exe"));
    server
        .arg("-m")
        .arg(dir.join(MODEL))
        .arg("--mmproj")
        .arg(dir.join(PROJECTOR))
        .args([
            "--alias",
            "lfm3-omni",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--api-key",
            &key,
            "--reasoning",
            "off",
            "--temp",
            "0",
            "-c",
            "4096",
            "--parallel",
            "1",
            "-ngl",
            "0",
            "-t",
            &threads.to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = server
        .spawn()
        .map_err(|e| format!("Could not start local AI: {e}"))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    let base = format!("http://127.0.0.1:{port}");
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "Local AI exited ({status}). Repair the model download or check available memory."
            ));
        }
        if client
            .get(format!("{base}/health"))
            .bearer_auth(&key)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let models = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(&key)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?;
    if !models["data"][0]["architecture"]["input_modalities"]
        .as_array()
        .is_some_and(|modes| modes.iter().any(|mode| mode == "audio"))
    {
        return Err(
            "The local runtime did not expose audio input. Verify / repair the model in Settings."
                .into(),
        );
    }
    let language = crate::util::language_name(&sample.language);
    let system = if sample.language.is_empty() || sample.language == "und" {
        "Transcribe the speech exactly in its original language. Respond in text.".to_owned()
    } else {
        format!("Perform ASR in {language}.")
    };
    let result = client.post(format!("{base}/v1/chat/completions")).bearer_auth(&key)
        .json(&serde_json::json!({
            "model": "lfm3-omni", "temperature": 0, "max_tokens": 256,
            "messages": [
                {"role":"system", "content":system},
                {"role":"user", "content":[{"type":"input_audio", "input_audio":{"data": STANDARD.encode(&audio.stdout), "format":"wav"}}]}
            ]
        })).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?
        .json::<serde_json::Value>().await.map_err(|e| e.to_string())?;
    child.kill().await.ok();
    let text = result["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Err(
            "The model did not recognize speech here. Try a clear line of dialogue.".into(),
        );
    }
    Ok(text.to_owned())
}

fn words(text: &str) -> Vec<String> {
    let mut clean = String::new();
    let mut hidden = false;
    for ch in text.chars() {
        if matches!(ch, '<' | '[' | '(') {
            hidden = true;
        } else if matches!(ch, '>' | ']' | ')') {
            hidden = false;
            clean.push(' ');
        } else if !hidden {
            clean.push(ch);
        }
    }
    let mut spoken = String::new();
    for line in clean.lines() {
        let line = line.trim_start_matches(|c: char| c.is_whitespace() || c == '-');
        let dialogue = line
            .split_once(':')
            .and_then(|(label, dialogue)| {
                let names: Vec<_> = label.split_whitespace().collect();
                // Subtitle speaker labels aren't spoken. Keep ordinary dialogue
                // containing a colon, e.g. "I have a question: ...".
                (!names.is_empty()
                    && names.len() <= 3
                    && label.chars().count() <= 24
                    && names.iter().all(|name| {
                        name.chars().next().is_some_and(char::is_uppercase)
                            && name.chars().all(char::is_alphabetic)
                    }))
                .then_some(dialogue)
            })
            .unwrap_or(line);
        spoken.extend(dialogue.chars().flat_map(char::to_lowercase).map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        }));
        spoken.push(' ');
    }
    spoken.split_whitespace().map(str::to_owned).collect()
}

/// Assistive matching only: no model-generated timestamps are trusted. Require
/// a unique dialogue prefix; ambiguous/translated/partial speech stays manual.
pub(crate) fn matching_cue(
    transcript: &str,
    cues: &[crate::player::subtitles::Cue],
) -> Option<crate::player::subtitles::Cue> {
    let heard = words(transcript);
    let mut matches = Vec::new();
    for cue in cues {
        let line = words(&cue.text);
        let count = line.len().min(heard.len()).min(10);
        if count < 4 {
            continue;
        }
        let equal = line
            .iter()
            .zip(&heard)
            .take(count)
            .filter(|(a, b)| a == b)
            .count();
        let score = equal as f64 / count as f64;
        if score >= 0.85 {
            matches.push((score, cue));
        }
    }
    matches.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (score, best) = matches.first()?;
    if matches.get(1).is_some_and(|(other, _)| score - other < 0.1) {
        return None;
    }
    Some((*best).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::subtitles::Cue;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "spoutpeak-ai-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cue(start: f64, text: &str) -> Cue {
        Cue {
            start,
            end: start + 3.0,
            text: text.into(),
        }
    }

    #[test]
    fn dialogue_match_requires_unique_prefix_and_enough_words() {
        let a = cue(
            10.0,
            "<i>You ever heard about the broken window theory?</i>",
        );
        let b = cue(20.0, "Boils down to this, you keep the windows intact.");
        let result = matching_cue(
            "You ever heard about the broken window theory? Boils down to this.",
            &[a.clone(), b.clone()],
        );
        assert_eq!(result, Some(a.clone()));
        assert!(matching_cue("Yeah, yeah.", &[a.clone()]).is_none());
        assert!(matching_cue("heard about the broken window theory", &[a.clone()]).is_none());
        assert!(matching_cue(&a.text, &[a.clone(), cue(50.0, &a.text)]).is_none());
        assert!(matching_cue("La fenêtre est cassée", &[a]).is_none());
    }

    #[test]
    fn installation_marker_alone_does_not_mean_ready() {
        let dir = TempDir::new();
        std::fs::write(
            dir.0.join(INSTALL),
            serde_json::json!({"revision":REVISION}).to_string(),
        )
        .unwrap();
        assert!(!installed_at(&dir.0));
        std::fs::write(dir.0.join(MODEL), "incomplete").unwrap();
        assert!(!installed_at(&dir.0));
    }

    #[test]
    fn dialogue_matching_ignores_speaker_labels_but_preserves_spoken_colons() {
        let line = cue(
            1502.785,
            "<i>Eugene: I take no responsibility for this.</i>",
        );
        assert_eq!(
            matching_cue(
                "I take no responsibility for this. I told you what I was.",
                &[line.clone()]
            ),
            Some(line)
        );
        assert_eq!(
            words("- (door opens)\n- Glenn: Help! I need help!"),
            words("Help! I need help!")
        );
        assert_eq!(
            words("I have a question: why are we here?"),
            words("I have a question why are we here?")
        );
    }

    async fn server(
        status: &str,
        headers: &str,
        body: &[u8],
    ) -> (String, Arc<Mutex<String>>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            body.len()
        );
        let body = body.to_vec();
        let request = Arc::new(Mutex::new(String::new()));
        let received = request.clone();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0u8; 1024];
                let count = stream.read(&mut buf).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..count]);
                if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            *received.lock().unwrap() = String::from_utf8_lossy(&bytes).to_string();
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        (format!("http://{address}/model"), request, handle)
    }

    fn artifact(url: String) -> Artifact {
        Artifact {
            name: "test.gguf",
            size: 6,
            hash: "bef57ec7f53a6d40beb640a780a639c83bc29ac8a9816f1fc6c5c6dcd93c4721",
            url,
        }
    }

    #[tokio::test]
    async fn download_resumes_verified_ranges_and_reuses_verified_files() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("test.gguf.part"), b"abc").unwrap();
        let (url, request, server) = server(
            "206 Partial Content",
            "Content-Range: bytes 3-5/6\r\n",
            b"def",
        )
        .await;
        let artifact = artifact(url);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        download(
            &client,
            &dir.0,
            &artifact,
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert!(request
            .lock()
            .unwrap()
            .to_lowercase()
            .contains("range: bytes=3-"));
        assert_eq!(std::fs::read(dir.0.join("test.gguf")).unwrap(), b"abcdef");
        assert!(!dir.0.join("test.gguf.part").exists());
        // The HTTP listener is gone; this must succeed without a new request.
        download(
            &client,
            &dir.0,
            &artifact,
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn download_restarts_if_range_ignored_and_rejects_corruption() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("test.gguf.part"), b"abc").unwrap();
        let (url, _, server) = server("200 OK", "", b"abcdef").await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        download(
            &client,
            &dir.0,
            &artifact(url),
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .unwrap();
        server.await.unwrap();
        std::fs::remove_file(dir.0.join("test.gguf")).unwrap();
        let (url, _, server) = self::server("200 OK", "", b"broken").await;
        assert!(download(
            &client,
            &dir.0,
            &artifact(url),
            &CancellationToken::new(),
            |_, _| {}
        )
        .await
        .is_err());
        server.await.unwrap();
        assert!(!dir.0.join("test.gguf").exists());
        assert!(!dir.0.join("test.gguf.part").exists());
    }

    #[tokio::test]
    async fn invalid_resume_range_never_appends_or_publishes() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("test.gguf.part"), b"abc").unwrap();
        let (url, _, server) = server(
            "206 Partial Content",
            "Content-Range: bytes 0-2/6\r\n",
            b"def",
        )
        .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(download(
            &client,
            &dir.0,
            &artifact(url),
            &CancellationToken::new(),
            |_, _| {}
        )
        .await
        .is_err());
        server.await.unwrap();
        assert_eq!(std::fs::read(dir.0.join("test.gguf.part")).unwrap(), b"abc");
        assert!(!dir.0.join("test.gguf").exists());
    }

    #[tokio::test]
    async fn cancelled_verification_does_not_publish_partial_artifact() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("test.gguf.part"), b"abcdef").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            download(
                &reqwest::Client::new(),
                &dir.0,
                &artifact(String::new()),
                &cancel,
                |_, _| {}
            )
            .await
            .unwrap_err(),
            "Cancelled"
        );
        assert!(!dir.0.join("test.gguf").exists());
        assert!(dir.0.join("test.gguf.part").exists());
    }

    #[test]
    #[ignore = "requires downloaded official runtime.zip in target/ai-diagnostics"]
    fn extracts_official_runtime_archive() {
        let dir = TempDir::new();
        std::fs::copy(
            "target/ai-diagnostics/runtime.zip",
            dir.0.join("runtime.zip"),
        )
        .unwrap();
        extract_runtime(&dir.0, &CancellationToken::new()).unwrap();
        assert!(dir.0.join("runtime/llama-server.exe").is_file());
        assert!(dir.0.join("runtime/llama-server-impl.dll").is_file());
    }

    #[tokio::test]
    #[ignore = "downloads and installs the optional 1.63 GB model; run explicitly"]
    async fn install_official_model() {
        assert!(supported());
        install(0, CancellationToken::new()).await.unwrap();
        assert!(installed_at(&model_dir()));
        println!("Verified model installed at {}", model_dir().display());
    }

    #[tokio::test]
    #[ignore = "requires installed model and SPOUTPEAK_AI_CASES JSON audio/subtitle fixtures"]
    async fn real_audio_subtitle_evaluation() {
        let input = std::env::var("SPOUTPEAK_AI_CASES").expect("fixture JSON path");
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
        let subtitle_text =
            std::fs::read_to_string(fixture["subtitles"].as_str().unwrap()).unwrap();
        let cues = crate::player::subtitles::parse(&subtitle_text);
        let mut results = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            let position = case["position"].as_f64().unwrap();
            let started = Instant::now();
            let transcript = transcribe(
                AudioSample {
                    url: fixture["media"].as_str().unwrap().into(),
                    headers: vec![],
                    stream_index: 1,
                    position,
                    language: "eng".into(),
                },
                CancellationToken::new(),
            )
            .await;
            let result = match transcript {
                Ok(transcript) => {
                    let found = matching_cue(&transcript, &cues);
                    serde_json::json!({"case":case, "elapsed_seconds":started.elapsed().as_secs_f64(),
                        "transcript":transcript, "matched_start":found.as_ref().map(|c|c.start),
                        "matched_text":found.as_ref().map(|c|&c.text), "proposed_delay":found.map(|c|position-c.start)})
                }
                Err(error) => {
                    serde_json::json!({"case":case,"elapsed_seconds":started.elapsed().as_secs_f64(),"error":error})
                }
            };
            println!("{result}");
            results.push(result);
            std::fs::write(
                format!("{input}.results.json"),
                serde_json::to_vec_pretty(&results).unwrap(),
            )
            .unwrap();
        }
        assert!(
            results.iter().all(|r| r.get("error").is_none()),
            "See the saved evaluation results"
        );
    }
}
