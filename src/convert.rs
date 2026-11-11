//! Converting saved videos to MP4 (H.264 video, AAC audio) so they play on
//! phones, TVs and browsers, optionally scaled down to save space.
//!
//! Video is re-encoded on the GPU when the PC has an encoder ffmpeg can use
//! (NVIDIA NVENC, Intel Quick Sync, AMD AMF, Media Foundation), else with
//! OpenH264 on the CPU. Streams that are already compatible are copied
//! as they are, so an H.264/AAC MKV just changes container in seconds.

use crate::store::DownloadFormat;
use crate::util::hide_console;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// An H.264 encoder that works on this PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoder {
    pub name: &'static str,
    pub label: &'static str,
}

const ENCODERS: &[Encoder] = &[
    Encoder { name: "h264_nvenc", label: "NVIDIA GPU" },
    Encoder { name: "h264_qsv", label: "Intel GPU" },
    Encoder { name: "h264_amf", label: "AMD GPU" },
    Encoder { name: "h264_mf", label: "Windows Media Foundation" },
    Encoder { name: "libopenh264", label: "CPU" },
];

fn ffmpeg_tools() -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    crate::player::engine::tools()
        .ok_or_else(|| "ffmpeg is missing from the Spoutpeak folder".to_owned())
}

static FOUND: OnceLock<Option<Encoder>> = OnceLock::new();

/// The encoder search result if it has run, without blocking.
pub fn encoder_if_known() -> Option<Option<Encoder>> {
    FOUND.get().copied()
}

/// The first encoder that can encode a short test clip (cached; the test
/// takes about a second, so call it off the UI thread first).
pub fn encoder() -> Option<Encoder> {
    *FOUND.get_or_init(|| {
        let (ffmpeg, _) = ffmpeg_tools().ok()?;
        ENCODERS.iter().copied().find(|encoder| {
            let mut command = Command::new(&ffmpeg);
            command
                .args(["-hide_banner", "-v", "error", "-nostdin", "-f", "lavfi"])
                .args(["-i", "color=black:s=320x240:d=0.2", "-pix_fmt", "yuv420p"])
                .args(["-c:v", encoder.name, "-f", "null", "-"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            hide_console(&mut command);
            command.status().is_ok_and(|s| s.success())
        })
    })
}

struct Probe {
    duration: f64,
    /// (codec, pixel format, height) of the first real video stream.
    video: Option<(String, String, u32)>,
    /// (stream index, codec) per audio stream.
    audio: Vec<(u64, String)>,
    /// Stream indices of text subtitles (MP4 can't hold picture ones).
    text_subtitles: Vec<u64>,
}

fn probe(ffprobe: &Path, input: &Path) -> Result<Probe, String> {
    let mut command = Command::new(ffprobe);
    command
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(input)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let output = command.output().map_err(|e| format!("could not run ffprobe: {e}"))?;
    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "could not read the video file")?;
    let mut probe = Probe {
        duration: value
            .pointer("/format/duration")
            .and_then(Value::as_str)
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
        video: None,
        audio: Vec::new(),
        text_subtitles: Vec::new(),
    };
    for stream in value.get("streams").and_then(Value::as_array).into_iter().flatten() {
        let text = |key: &str| stream.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let index = stream.get("index").and_then(Value::as_u64).unwrap_or(0);
        match text("codec_type").as_str() {
            "video" if probe.video.is_none() => {
                let cover = stream.pointer("/disposition/attached_pic").and_then(Value::as_i64) == Some(1);
                let height = stream.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                if !cover && height > 0 {
                    probe.video = Some((text("codec_name"), text("pix_fmt"), height));
                }
            }
            "audio" => probe.audio.push((index, text("codec_name"))),
            "subtitle" => {
                let codec = text("codec_name");
                if matches!(codec.as_str(), "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text") {
                    probe.text_subtitles.push(index);
                }
            }
            _ => {}
        }
    }
    Ok(probe)
}

/// Target bitrate in kbit/s for an output height; generous enough that the
/// conversion is hard to tell from the source on a TV.
fn bitrate_kbps(height: u32) -> u32 {
    match height {
        0..=480 => 1_800,
        481..=720 => 3_500,
        721..=1080 => 7_000,
        _ => 16_000,
    }
}

/// Converts `input` into an MP4 at `output` according to `format`.
/// `progress` gets the finished share (0 to 1) about twice a second.
/// Returns the encoder used, or `None` when the video could be copied.
pub fn convert(
    input: &Path,
    output: &Path,
    format: DownloadFormat,
    cancel: &AtomicBool,
    progress: impl Fn(f32),
) -> Result<Option<Encoder>, String> {
    let (ffmpeg, ffprobe) = ffmpeg_tools()?;
    let info = probe(&ffprobe, input)?;
    let (codec, pix_fmt, height) = info.video.clone().ok_or("the file has no video")?;
    let target_height = format.max_height().map_or(height, |max| height.min(max));
    let scale = target_height < height;
    let compatible = codec == "h264" && matches!(pix_fmt.as_str(), "yuv420p" | "yuvj420p");

    let mut command = Command::new(&ffmpeg);
    command
        .args(["-hide_banner", "-nostdin", "-y", "-v", "error"])
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0"]);
    let encoder = if compatible && !scale {
        command.args(["-c:v", "copy"]);
        None
    } else {
        let encoder = encoder().ok_or("no H.264 encoder works on this PC")?;
        let rate = bitrate_kbps(target_height);
        if scale {
            command.args(["-vf", &format!("scale=-2:{target_height}")]);
        }
        command
            .args(["-c:v", encoder.name, "-pix_fmt", "yuv420p"])
            .args(["-b:v", &format!("{rate}k")])
            .args(["-maxrate", &format!("{}k", rate * 3 / 2)])
            .args(["-bufsize", &format!("{}k", rate * 2)]);
        Some(encoder)
    };
    for (n, (index, codec)) in info.audio.iter().enumerate() {
        command.args(["-map", &format!("0:{index}")]);
        if codec == "aac" {
            command.args([format!("-c:a:{n}"), "copy".to_owned()]);
        } else {
            command.args([format!("-c:a:{n}"), "aac".to_owned(), format!("-b:a:{n}"), "320k".to_owned()]);
        }
    }
    for index in &info.text_subtitles {
        command.args(["-map", &format!("0:{index}")]);
    }
    if !info.text_subtitles.is_empty() {
        command.args(["-c:s", "mov_text"]);
    }
    command
        .args(["-map_metadata", "0", "-movflags", "+faststart"])
        .args(["-progress", "pipe:1", "-nostats", "-f", "mp4"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);

    let mut child = command.spawn().map_err(|e| format!("could not run ffmpeg: {e}"))?;
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            if cancel.load(Ordering::Acquire) {
                let _ = child.kill();
                break;
            }
            let Ok(line) = line else { break };
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                if info.duration > 0.0 {
                    progress(((us / 1_000_000.0) / info.duration).clamp(0.0, 1.0) as f32);
                }
            }
        }
    }
    let status = child.wait().map_err(|e| format!("ffmpeg failed: {e}"))?;
    let errors = errors.join().unwrap_or_default();
    if cancel.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(output);
        return Err("cancelled".into());
    }
    if !status.success() {
        let _ = std::fs::remove_file(output);
        let last = errors.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        return Err(format!("Conversion failed: {}", last.trim()));
    }
    progress(1.0);
    Ok(encoder)
}
