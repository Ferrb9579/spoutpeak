//! Local, bounded diagnostics. Initialize before any native UI/graphics code.
//! Logs flush per event; panic reports bypass the logger's lock entirely.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use regex::Regex;
use tracing_subscriber::{EnvFilter, fmt::MakeWriter, prelude::*};

#[cfg(windows)]
mod native;

const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
const KEEP_SESSIONS: usize = 8;
const DEFAULT_FILTER: &str =
    "info,wgpu_hal=warn,naga=warn,reqwest=warn,hyper=warn,h2=warn,rustls=warn,librqbit=warn";
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static PANICKED: AtomicBool = AtomicBool::new(false);

pub fn directory() -> PathBuf {
    DIRECTORY.get().cloned().unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("spoutpeak/logs")
    })
}

pub struct Session {
    path: PathBuf,
    started: String,
}

impl Session {
    pub fn finish(self, success: bool) {
        let status = if PANICKED.load(Ordering::Relaxed) {
            "panic"
        } else if success {
            "clean_shutdown"
        } else {
            "startup_or_runtime_error"
        };
        tracing::info!(status, "Session finished");
        let _ = write_status(&self.path, &self.started, status);
    }
}

fn write_status(path: &Path, started: &str, status: &str) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "revision": option_env!("SPOUTPEAK_BUILD_REVISION").unwrap_or("unknown"),
        "pid": std::process::id(), "started_utc": started, "status": status,
        "updated_utc": chrono::Utc::now().to_rfc3339(),
    }))?;
    let mut file = File::create(path)?;
    file.write_all(&bytes)?;
    file.sync_data()
}

pub fn init() -> Option<Session> {
    let preferred = std::env::var_os("SPOUTPEAK_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(directory);
    let (dir, log) = [preferred, std::env::temp_dir().join("spoutpeak-logs")]
        .into_iter()
        .find_map(|dir| {
            fs::create_dir_all(&dir).ok()?;
            let base = format!(
                "session-{}-{}",
                chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f"),
                std::process::id()
            );
            let path = dir.join(format!("{base}.log"));
            FileLog::new(path, MAX_LOG_BYTES).ok().map(|log| (dir, log))
        })
        .or_else(|| {
            eprintln!("Spoutpeak could not create its diagnostic logs");
            None
        })?;
    let base = log.path.with_extension("");
    let _ = DIRECTORY.set(dir.clone());
    install_panic_hook(base.with_extension("panic.txt"));
    cleanup(&dir, &base, KEEP_SESSIONS);
    let requested = std::env::var("SPOUTPEAK_LOG").or_else(|_| std::env::var("RUST_LOG"));
    let filter = requested
        .as_deref()
        .ok()
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER));
    let writer = LogWriter(Arc::new(Mutex::new(log)));
    if let Err(error) = tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_thread_names(true)
                .with_thread_ids(true)
                .with_writer(writer),
        )
        .try_init()
    {
        let _ = fs::write(
            base.with_extension("logger.txt"),
            format!("Logger initialization failed: {error}"),
        );
    }
    let started = chrono::Utc::now().to_rfc3339();
    let status_path = base.with_extension("status.json");
    let _ = write_status(&status_path, &started, "running");
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        revision = option_env!("SPOUTPEAK_BUILD_REVISION").unwrap_or("unknown"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        debug = cfg!(debug_assertions),
        pid = std::process::id(),
        cpu_threads = std::thread::available_parallelism().map_or(0, usize::from),
        "Session started"
    );
    if let Ok(exe) = std::env::current_exe() {
        tracing::info!(executable = %exe.display(), "Executable location");
    }
    #[cfg(windows)]
    if let Err(error) = native::install(&base) {
        tracing::warn!(%error, "Native crash capture could not be installed");
    }
    collect_system_info(base.with_extension("system.json"));
    let _ = std::thread::Builder::new()
        .name("diagnostics-tools".into())
        .spawn(|| {
            if let Some((ffmpeg, ffprobe)) = crate::player::engine::tools() {
                for tool in [ffmpeg, ffprobe] {
                    let mut command = std::process::Command::new(&tool);
                    command
                        .arg("-version")
                        .stdin(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    crate::util::hide_console(&mut command);
                    if let Ok(output) = command.output() {
                        let version = String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .to_owned();
                        tracing::info!(path = %tool.display(), %version, "Media helper version");
                    }
                }
            } else {
                tracing::warn!("FFmpeg or FFprobe was not found or could not start");
            }
        });
    Some(Session {
        path: status_path,
        started,
    })
}

fn install_panic_hook(path: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        PANICKED.store(true, Ordering::Relaxed);
        let thread = std::thread::current();
        let report = redact(&format!(
            "Spoutpeak {} ({})\nUTC: {}\nThread: {} {:?}\n{info}\n\n{}\n",
            env!("CARGO_PKG_VERSION"),
            option_env!("SPOUTPEAK_BUILD_REVISION").unwrap_or("unknown"),
            chrono::Utc::now().to_rfc3339(),
            thread.name().unwrap_or("unnamed"),
            thread.id(),
            std::backtrace::Backtrace::force_capture(),
        ));
        // Separate file and no logger mutex: this also works if a panic
        // happened during formatting or while another thread logs.
        if fs::metadata(&path).map_or(true, |metadata| metadata.len() < MAX_LOG_BYTES)
            && let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path)
        {
            let _ = file.write_all(report.as_bytes());
            let _ = file.sync_data();
        }
        eprintln!("{report}");
    }));
}

pub(super) fn redact(input: &str) -> String {
    static URLS: OnceLock<Regex> = OnceLock::new();
    static SECRETS: OnceLock<Regex> = OnceLock::new();
    let urls = URLS.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:https?|wss?|socks5h?)://[^\s"'<>]+|magnet:\?[^\s"'<>]+"#).unwrap()
    });
    let secrets = SECRETS.get_or_init(|| Regex::new(
        r#"(?i)\b(authorization|proxy-authorization|cookie|set-cookie|api[_-]?key|token|access[_-]?token|refresh[_-]?token|password|secret)(["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\r\n,;]+)"#
    ).unwrap());
    let text = urls.replace_all(input, |captures: &regex::Captures<'_>| {
        let url = &captures[0];
        let Some((scheme, rest)) = url.split_once("://") else {
            return "magnet:[redacted]".to_owned();
        };
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('@')
            .next()
            .unwrap_or("");
        format!("{scheme}://{authority}/[redacted]")
    });
    let mut text = secrets
        .replace_all(&text, "${1}${2}[redacted]")
        .into_owned();
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        text = text
            .replace(home.as_ref(), "<USER>")
            .replace(&home.replace('\\', "/"), "<USER>");
    }
    text
}

struct FileLog {
    path: PathBuf,
    file: Option<File>,
    bytes: u64,
    limit: u64,
}
impl FileLog {
    fn new(path: PathBuf, limit: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
            bytes: 0,
            limit,
        })
    }
    fn append(&mut self, record: &[u8]) -> io::Result<()> {
        if self.bytes > 0 && self.bytes + record.len() as u64 > self.limit {
            self.file.take();
            let previous = self.path.with_extension("previous.log");
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&self.path, previous)?;
            self.file = Some(File::create(&self.path)?);
            self.bytes = 0;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log unavailable"))?;
        file.write_all(record)?;
        file.flush()?;
        self.bytes += record.len() as u64;
        Ok(())
    }
}

#[derive(Clone)]
struct LogWriter(Arc<Mutex<FileLog>>);
struct RecordWriter {
    log: Arc<Mutex<FileLog>>,
    bytes: Vec<u8>,
}
impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = RecordWriter;
    fn make_writer(&'a self) -> Self::Writer {
        RecordWriter {
            log: self.0.clone(),
            bytes: Vec::new(),
        }
    }
}
impl Write for RecordWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Bound a single event even if a dependency logs a huge response.
        let remaining = (64 * 1024usize).saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..remaining.min(bytes.len())]);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for RecordWriter {
    fn drop(&mut self) {
        let mut record = redact(&String::from_utf8_lossy(&self.bytes));
        if !record.ends_with('\n') {
            record.push('\n');
        }
        if let Ok(mut log) = self.log.lock() {
            let _ = log.append(record.as_bytes());
        }
    }
}

fn diagnostic_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("session-")
        && name[8..].chars().take(8).all(|c| c.is_ascii_digit())
        && [
            ".log",
            ".panic.txt",
            ".native.txt",
            ".logger.txt",
            ".system.json",
            ".status.json",
            ".dmp",
        ]
        .iter()
        .any(|ext| name.ends_with(ext))
}

fn files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| diagnostic_file(path))
        .collect()
}

fn cleanup(dir: &Path, current: &Path, keep: usize) {
    let mut sessions: Vec<_> = files(dir)
        .iter()
        .filter_map(|path| Some(path.file_name()?.to_str()?.split('.').next()?.to_owned()))
        .collect();
    sessions.sort();
    sessions.dedup();
    sessions.reverse();
    let current = current
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    for path in files(dir) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let session = name.split('.').next().unwrap_or("");
        if session != current && !sessions.iter().take(keep).any(|s| s == session) {
            let _ = fs::remove_file(path);
        }
    }
    let mut dumps: Vec<_> = files(dir)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "dmp"))
        .collect();
    dumps.sort();
    dumps.reverse();
    for path in dumps.into_iter().skip(3) {
        let _ = fs::remove_file(path);
    }
}

pub fn export_report() -> io::Result<PathBuf> {
    let output = dirs::download_dir()
        .filter(|path| path.is_dir())
        .unwrap_or_else(directory);
    export_to(&directory(), &output)
}

fn export_to(dir: &Path, output: &Path) -> io::Result<PathBuf> {
    fs::read_dir(dir)?;
    if files(dir).is_empty() {
        return Err(io::Error::other("No diagnostic logs were found"));
    }
    fs::create_dir_all(output)?;
    let path = output.join(format!(
        "Spoutpeak-diagnostics-{}-{}.zip",
        chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f"),
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let result = (|| {
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("README.txt", options)?;
        zip.write_all(format!("Spoutpeak {} diagnostics\nRevision: {}\nGenerated UTC: {}\n\nLogs and basic OS/GPU details from recent sessions.\nA status of 'running' without a later shutdown can mean a crash, forced termination, or a still-open instance.\nURLs and common credentials are redacted. Review before sharing.\nProfiles, addon configurations, media, and binary crash dumps are not included.\nNative crash dumps remain in the log folder for optional separate analysis.\n",
            env!("CARGO_PKG_VERSION"), option_env!("SPOUTPEAK_BUILD_REVISION").unwrap_or("unknown"), chrono::Utc::now().to_rfc3339()).as_bytes())?;
        let mut paths = files(dir);
        paths.sort();
        for file in paths {
            if file.extension().is_some_and(|ext| ext == "dmp") {
                continue;
            }
            let text = match fs::read(&file) {
                Ok(text) => text,
                Err(_) => continue,
            };
            zip.start_file(file.file_name().unwrap().to_string_lossy(), options)?;
            zip.write_all(redact(&String::from_utf8_lossy(&text)).as_bytes())?;
        }
        zip.finish()?.sync_all()?;
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    tracing::info!("Diagnostic report exported");
    Ok(path)
}

fn collect_system_info(path: PathBuf) {
    #[cfg(windows)]
    let _ = std::thread::Builder::new().name("diagnostics-system".into()).spawn(move || {
        // Only hardware/OS fields; no account, machine name or serial numbers.
        let script = "$ErrorActionPreference='Stop'; $os=Get-CimInstance Win32_OperatingSystem; $gpu=@(Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion); [pscustomobject]@{OS=$os.Caption;Version=$os.Version;Build=$os.BuildNumber;MemoryKB=$os.TotalVisibleMemorySize;CPU=$env:PROCESSOR_IDENTIFIER;GPU=$gpu} | ConvertTo-Json -Depth 4";
        let Ok(output) = File::create(&path) else { return };
        let mut command = std::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdin(std::process::Stdio::null()).stdout(output).stderr(std::process::Stdio::null());
        crate::util::hide_console(&mut command);
        if let Ok(mut child) = command.spawn() {
            for _ in 0..100 {
                if child.try_wait().ok().flatten().is_some() { return; }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = child.kill(); let _ = child.wait();
        }
    });
    #[cfg(not(windows))]
    let _ = fs::write(
        path,
        format!(
            "OS: {}\nArchitecture: {}\n",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
}

pub fn helper_line(component: &str, generation: u64, line: &str) {
    // FFmpeg's live timestamp stream has a line for every frame/sample.
    // Those are clock data, not diagnostic errors.
    if line.trim().is_empty()
        || line.contains("showinfo")
        || line.starts_with("frame=")
        || line.starts_with("size=")
    {
        return;
    }
    tracing::warn!(component, generation, message = %line, "Media helper output");
}

pub fn source_kind(source: &str) -> &'static str {
    if source.starts_with("magnet:") {
        "torrent"
    } else if source.starts_with("http://127.0.0.1:") {
        "local_stream"
    } else if source.starts_with("http://") || source.starts_with("https://") {
        "http_stream"
    } else {
        "local_file"
    }
}

pub fn capture_stderr(stderr: std::process::ChildStderr, component: &'static str, generation: u64) {
    let _ = std::thread::Builder::new()
        .name(format!("{component}-stderr"))
        .spawn(move || {
            use std::io::{BufRead, BufReader};
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            // Always drain the pipe, including after a very long error line,
            // so diagnostic capture can never block the decoder on stderr.
            while let Ok(bytes) = reader.fill_buf() {
                if bytes.is_empty() {
                    break;
                }
                let end = bytes.iter().position(|byte| *byte == b'\n');
                let take = end.map_or(bytes.len(), |i| i + 1);
                let keep = take.min(8192usize.saturating_sub(line.len()));
                line.extend_from_slice(&bytes[..keep]);
                reader.consume(take);
                if end.is_some() {
                    helper_line(component, generation, String::from_utf8_lossy(&line).trim());
                    line.clear();
                }
            }
            if !line.is_empty() {
                helper_line(component, generation, &String::from_utf8_lossy(&line));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "spoutpeak-diagnostics-test-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_micros(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sensitive_urls_and_credentials_are_redacted() {
        let text = redact(concat!(
            "https://alice:pw@example.com/private-addon-key/stream?token=query-key#fragment\n",
            "magnet:?xt=urn:btih:private-hash\n",
            "Authorization: Bearer auth-key\nCookie: session=cookie-key\n",
            "password='password-value'; api_key=api-value\n",
            "{\"access_token\":\"json-token\"}\n",
            "socks5://user:proxy-password@127.0.0.1:1080\n",
        ));
        for secret in [
            "alice",
            "pw@",
            "private-addon-key",
            "query-key",
            "fragment",
            "private-hash",
            "auth-key",
            "cookie-key",
            "password-value",
            "api-value",
            "json-token",
            "proxy-password",
        ] {
            assert!(
                !text.contains(secret),
                "credential survived redaction: {secret}"
            );
        }
        assert!(text.contains("https://example.com/[redacted]"));
        assert!(text.contains("127.0.0.1:1080"));
        assert_eq!(
            redact("audio device initialization failed"),
            "audio device initialization failed"
        );
    }

    #[test]
    fn events_are_flushed_and_redacted_before_reaching_disk() {
        let dir = TestDir::new();
        let path = dir.0.join("session-20261116-000000-001-1.log");
        let writer = LogWriter(Arc::new(Mutex::new(
            FileLog::new(path.clone(), 1024).unwrap(),
        )));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("Failed loading https://user:secret@example.com/key/path?token=value");
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.contains("Failed loading"));
            assert!(!text.contains("secret"));
            assert!(!text.contains("key/path"));
            assert!(text.contains("example.com"));
        });
    }

    #[test]
    fn rotation_bounds_disk_usage_and_preserves_recent_records() {
        let dir = TestDir::new();
        let path = dir.0.join("session-20261116-000000-001-1.log");
        let mut log = FileLog::new(path.clone(), 20).unwrap();
        for text in [
            b"000000000\n",
            b"111111111\n",
            b"222222222\n",
            b"333333333\n",
            b"444444444\n",
        ] {
            log.append(text).unwrap();
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "444444444\n");
        assert_eq!(
            fs::read_to_string(path.with_extension("previous.log")).unwrap(),
            "222222222\n333333333\n"
        );
        assert_eq!(files(&dir.0).len(), 2);
    }

    #[test]
    fn retention_keeps_recent_sessions_and_does_not_touch_unrelated_files() {
        let dir = TestDir::new();
        fs::write(dir.0.join("user-notes.txt"), "keep me").unwrap();
        for day in 1..=10 {
            let base = dir.0.join(format!("session-202611{day:02}-000000-000-1"));
            fs::write(base.with_extension("log"), "log").unwrap();
            fs::write(base.with_extension("dmp"), "dump").unwrap();
        }
        let current = dir.0.join("session-20261110-000000-000-1");
        cleanup(&dir.0, &current, 4);
        assert_eq!(
            files(&dir.0)
                .iter()
                .filter(|path| path.extension().unwrap() == "log")
                .count(),
            4
        );
        assert_eq!(
            files(&dir.0)
                .iter()
                .filter(|path| path.extension().unwrap() == "dmp")
                .count(),
            3
        );
        assert_eq!(
            fs::read_to_string(dir.0.join("user-notes.txt")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn export_contains_only_diagnostics_and_excludes_crash_memory_and_profiles() {
        let dir = TestDir::new();
        let prefix = "session-20261116-000000-001-1";
        fs::write(
            dir.0.join(format!("{prefix}.log")),
            "https://user:secret@example.com/private?key=token",
        )
        .unwrap();
        fs::write(
            dir.0.join(format!("{prefix}.native.txt")),
            "Exception code: 0xC0000005",
        )
        .unwrap();
        fs::write(dir.0.join(format!("{prefix}.dmp")), "private memory").unwrap();
        fs::write(dir.0.join("profile.json"), "private profile").unwrap();
        let path = export_to(&dir.0, &dir.0.join("reports")).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert_eq!(zip.len(), 3);
        for index in 0..zip.len() {
            use std::io::Read;
            let mut entry = zip.by_index(index).unwrap();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            assert!(!entry.name().ends_with(".dmp"));
            assert!(!text.contains("private memory"));
            assert!(!text.contains("private profile"));
            assert!(!text.contains("secret"));
            assert!(!text.contains("?key=token"));
        }
    }

    #[test]
    #[ignore = "isolated child process entry point"]
    fn subprocess_probe() {
        let Some(mode) = std::env::var_os("SPOUTPEAK_DIAGNOSTICS_TEST") else {
            return;
        };
        let session = init().expect("diagnostics should initialize");
        tracing::error!(
            "Child diagnostic marker https://user:fixture-password@example.com/private-path"
        );
        if mode == "panic" {
            panic!("worker panic marker https://example.com/private-panic-token");
        }
        #[cfg(windows)]
        if mode == "native" {
            unsafe {
                windows::Win32::System::Diagnostics::Debug::RaiseException(0xE0000001, 1, None);
            }
            panic!("native exception was not fatal");
        }
        session.finish(true);
    }

    fn child(mode: &str, dir: &Path) -> std::process::Output {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "diagnostics::tests::subprocess_probe",
                "--ignored",
                "--nocapture",
            ])
            .env("SPOUTPEAK_DIAGNOSTICS_TEST", mode)
            .env("SPOUTPEAK_LOG_DIR", dir);
        crate::util::hide_console(&mut command);
        command.output().unwrap()
    }

    #[test]
    fn isolated_session_flushes_before_exit_and_records_a_clean_shutdown() {
        let dir = TestDir::new();
        assert!(child("clean", &dir.0).status.success());
        let paths = files(&dir.0);
        let status = paths
            .iter()
            .find(|p| p.to_string_lossy().ends_with("status.json"))
            .unwrap();
        assert!(
            fs::read_to_string(status)
                .unwrap()
                .contains("clean_shutdown")
        );
        let log = paths
            .iter()
            .find(|p| p.extension().is_some_and(|ext| ext == "log"))
            .unwrap();
        let text = fs::read_to_string(log).unwrap();
        assert!(text.contains("Child diagnostic marker"));
        assert!(!text.contains("fixture-password"));
        assert!(!text.contains("private-path"));
    }

    #[test]
    fn isolated_panic_preserves_message_and_backtrace() {
        let dir = TestDir::new();
        assert!(!child("panic", &dir.0).status.success());
        let paths = files(&dir.0);
        let report = paths
            .iter()
            .find(|p| p.to_string_lossy().ends_with("panic.txt"))
            .unwrap();
        let text = fs::read_to_string(report).unwrap();
        assert!(text.contains("worker panic marker"));
        assert!(text.contains("diagnostics::tests::subprocess_probe"));
        assert!(!text.contains("private-panic-token"));
        let status = paths
            .iter()
            .find(|p| p.to_string_lossy().ends_with("status.json"))
            .unwrap();
        assert!(fs::read_to_string(status).unwrap().contains("running"));
    }

    #[cfg(windows)]
    #[test]
    fn isolated_native_exception_writes_a_report_and_valid_minidump() {
        let dir = TestDir::new();
        assert!(!child("native", &dir.0).status.success());
        let paths = files(&dir.0);
        let report = paths
            .iter()
            .find(|p| p.to_string_lossy().ends_with("native.txt"))
            .unwrap();
        let text = fs::read_to_string(report).unwrap();
        assert!(text.contains("0xE0000001"));
        assert!(text.contains("Crash dump result: Ok"), "{text}");
        let dump = paths
            .iter()
            .find(|p| p.extension().is_some_and(|ext| ext == "dmp"))
            .unwrap();
        let bytes = fs::read(dump).unwrap();
        assert!(bytes.starts_with(b"MDMP"));
    }
}
