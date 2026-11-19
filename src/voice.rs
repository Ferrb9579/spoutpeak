//! Voice chat for watch-together rooms.
//!
//! The microphone is captured with cpal, mixed down to mono, resampled to
//! 48 kHz, cut into 20 ms frames and Opus-encoded. Frames go out over the
//! room's WebSocket as binary messages; the server relays them to everyone
//! else in voice (see `watch-server`). Incoming frames are decoded per
//! speaker into a small jitter buffer, and all speakers are mixed into one
//! rodio output.
//!
//! Before encoding, each 10 ms of microphone audio is cleaned by WebRTC's
//! audio processing (see `voice_processing`): echo of the film and of the
//! other voices is cancelled, background noise suppressed and the level
//! evened out. A voice gate then sends nothing while it's quiet.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::voice_processing::{BLOCK, Processor, TapWriter, VoiceSettings};

/// Opus works at 48 kHz; one frame is 20 ms.
pub const RATE: u32 = 48_000;
const FRAME: usize = 960;
/// Speech bitrate: clear voice at a few KB/s.
const BITRATE: i32 = 24_000;
/// Keep sending this long after speech stops, so word endings aren't cut.
const HANGOVER: Duration = Duration::from_millis(350);
/// A speaker starts playing once this much is buffered (smooths jitter).
const PREBUFFER: usize = FRAME * 3;
/// Most audio buffered per speaker; older audio is dropped to stay live.
const MAX_BUFFER: usize = RATE as usize * 3 / 10;
/// Lost frames concealed by Opus before giving up on the gap.
const MAX_CONCEAL: u16 = 4;
/// A speaker counts as talking this long after their last frame.
const SPEAKING: Duration = Duration::from_millis(300);

/// Where encoded frames go: the room connection.
pub type Outbox = Box<dyn Fn(Vec<u8>) + Send + Sync>;

struct Speaker {
    decoder: opus::Decoder,
    queue: VecDeque<f32>,
    playing: bool,
    last_seq: Option<u16>,
    heard: Instant,
}

struct Shared {
    speakers: Mutex<HashMap<u32, Speaker>>,
    muted: AtomicBool,
    deafened: AtomicBool,
    running: AtomicBool,
    talking: AtomicBool,
    seq: AtomicU16,
    outbox: Outbox,
    /// The last microphone / speaker problem, for the UI.
    problem: Mutex<Option<String>>,
    /// Echo / noise / gain / gate preferences; read by the capture thread.
    settings: Mutex<VoiceSettings>,
}

/// A running voice session; dropping it (or [`Voice::stop`]) ends it.
pub struct Voice {
    shared: Arc<Shared>,
}

impl Voice {
    /// Opens the microphone and speakers on a thread of their own.
    pub fn start(outbox: Outbox, muted: bool, deafened: bool, settings: VoiceSettings) -> Self {
        let shared = Arc::new(Shared {
            speakers: Mutex::new(HashMap::new()),
            muted: AtomicBool::new(muted),
            deafened: AtomicBool::new(deafened),
            running: AtomicBool::new(true),
            talking: AtomicBool::new(false),
            seq: AtomicU16::new(0),
            outbox,
            problem: Mutex::new(None),
            settings: Mutex::new(settings),
        });
        let thread = shared.clone();
        let _ = std::thread::Builder::new().name("voice".into()).spawn(move || run(thread));
        Self { shared }
    }

    pub fn stop(&self) {
        self.shared.running.store(false, Ordering::Release);
    }

    /// New echo / noise / gain / gate preferences, applied from the next
    /// 10 ms of microphone audio.
    pub fn set_settings(&self, settings: VoiceSettings) {
        *self.shared.settings.lock().unwrap() = settings;
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
        if muted {
            self.shared.talking.store(false, Ordering::Relaxed);
        }
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.shared.deafened.store(deafened, Ordering::Relaxed);
        if deafened {
            self.shared.speakers.lock().unwrap().clear();
        }
    }

    /// A relayed frame: `[sender id: u32 LE][seq: u16 LE][Opus packet]`.
    /// Called on the network thread.
    pub fn receive(&self, frame: &[u8]) {
        if frame.len() < 7 || self.shared.deafened.load(Ordering::Relaxed) {
            return;
        }
        let id = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let seq = u16::from_le_bytes([frame[4], frame[5]]);
        let packet = &frame[6..];
        let mut speakers = self.shared.speakers.lock().unwrap();
        let speaker = match speakers.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Ok(decoder) = opus::Decoder::new(RATE, opus::Channels::Mono) else { return };
                entry.insert(Speaker {
                    decoder,
                    queue: VecDeque::new(),
                    playing: false,
                    last_seq: None,
                    heard: Instant::now(),
                })
            }
        };
        let mut pcm = [0f32; FRAME * 6];
        if let Some(last) = speaker.last_seq {
            let step = seq.wrapping_sub(last);
            // A duplicate or a late, reordered frame: too late to play.
            if step == 0 || step > u16::MAX / 2 {
                return;
            }
            // Frames lost in between: let Opus fill a short gap (using the
            // redundancy in this packet for the last one).
            let lost = step - 1;
            if (1..=MAX_CONCEAL).contains(&lost) {
                for missing in 0..lost {
                    let fec = missing + 1 == lost;
                    let input: &[u8] = if fec { packet } else { &[] };
                    if let Ok(n) = speaker.decoder.decode_float(input, &mut pcm[..FRAME], fec) {
                        speaker.queue.extend(&pcm[..n]);
                    }
                }
            }
        }
        speaker.last_seq = Some(seq);
        speaker.heard = Instant::now();
        if let Ok(n) = speaker.decoder.decode_float(packet, &mut pcm, false) {
            speaker.queue.extend(&pcm[..n]);
        }
        let excess = speaker.queue.len().saturating_sub(MAX_BUFFER);
        speaker.queue.drain(..excess);
    }

    /// Room ids of the people talking right now.
    pub fn speaking(&self) -> Vec<u64> {
        self.shared
            .speakers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.heard.elapsed() < SPEAKING)
            .map(|(id, _)| u64::from(*id))
            .collect()
    }

    /// This app's microphone is sending speech.
    pub fn talking(&self) -> bool {
        self.shared.talking.load(Ordering::Relaxed)
    }

    pub fn problem(&self) -> Option<String> {
        self.shared.problem.lock().unwrap().clone()
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The voice thread: owns the audio devices until the session ends.
fn run(shared: Arc<Shared>) {
    let _taps = crate::voice_processing::TapsOn::new();
    let output = start_output(&shared);
    let input = start_input(&shared);
    if let Err(problem) = &output {
        tracing::warn!(%problem, "Voice output unavailable");
    }
    if let Err(problem) = &input {
        tracing::warn!(%problem, "Microphone unavailable");
    }
    let problem = match (&input, &output) {
        (Err(problem), _) => Some(format!("Microphone: {problem}")),
        (_, Err(problem)) => Some(format!("Speakers: {problem}")),
        _ => None,
    };
    *shared.problem.lock().unwrap() = problem;
    while shared.running.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(100));
    }
    // The streams stop as they drop here.
}

fn start_output(shared: &Arc<Shared>) -> Result<rodio::MixerDeviceSink, String> {
    let mut output = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
    output.log_on_drop(false);
    output.mixer().add(Mix::new(shared.clone()));
    Ok(output)
}

/// All speakers mixed, as one 48 kHz mono source.
struct Mix {
    shared: Arc<Shared>,
    buffer: Vec<f32>,
    cursor: usize,
    /// What is played goes to the echo canceller.
    tap: TapWriter,
}

impl Mix {
    fn new(shared: Arc<Shared>) -> Self {
        Self { shared, buffer: Vec::new(), cursor: 0, tap: TapWriter::new(&crate::voice_processing::VOICES, 1) }
    }

    /// Mixes the next 10 ms under one lock.
    fn refill(&mut self) {
        const CHUNK: usize = FRAME / 2;
        self.buffer.clear();
        self.buffer.resize(CHUNK, 0.0);
        self.cursor = 0;
        if self.shared.deafened.load(Ordering::Relaxed) {
            return;
        }
        let mut speakers = self.shared.speakers.lock().unwrap();
        speakers.retain(|_, s| s.heard.elapsed() < Duration::from_secs(30));
        for speaker in speakers.values_mut() {
            if !speaker.playing && speaker.queue.len() >= PREBUFFER {
                speaker.playing = true;
            }
            if !speaker.playing {
                continue;
            }
            for slot in self.buffer.iter_mut() {
                match speaker.queue.pop_front() {
                    Some(sample) => *slot += sample,
                    None => {
                        // Ran dry: buffer up again before playing on.
                        speaker.playing = false;
                        break;
                    }
                }
            }
        }
        for sample in &mut self.buffer {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

impl Iterator for Mix {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.shared.running.load(Ordering::Relaxed) {
            return None;
        }
        if self.cursor >= self.buffer.len() {
            self.refill();
        }
        self.cursor += 1;
        let sample = self.buffer[self.cursor - 1];
        self.tap.sample(sample);
        Some(sample)
    }
}

impl rodio::Source for Mix {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(1).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Turns the device's audio into 20 ms mono frames at 48 kHz and sends the
/// spoken ones.
struct Capture {
    shared: Arc<Shared>,
    encoder: opus::Encoder,
    channels: usize,
    /// Input samples per output sample (device rate / 48 kHz).
    step: f64,
    /// Position of the next output sample, in input samples from `last`.
    phase: f64,
    last: f32,
    frame: Vec<f32>,
    loud_until: Option<Instant>,
    packet: Vec<u8>,
    /// Echo cancellation, noise suppression and gain control.
    processor: Processor,
}

impl Capture {
    fn push(&mut self, interleaved: impl Iterator<Item = f32>) {
        let mut channel = 0;
        let mut sum = 0.0;
        for sample in interleaved {
            sum += sample;
            channel += 1;
            if channel < self.channels {
                continue;
            }
            let mono = sum / self.channels as f32;
            (channel, sum) = (0, 0.0);
            // Linear resampling between the previous and this sample.
            while self.phase <= 1.0 {
                let at = self.phase as f32;
                self.frame.push(self.last + (mono - self.last) * at);
                self.phase += self.step;
                if self.frame.len() == FRAME {
                    self.send_frame();
                }
            }
            self.phase -= 1.0;
            self.last = mono;
        }
    }

    fn send_frame(&mut self) {
        let shared = &self.shared;
        let settings = *shared.settings.lock().unwrap();
        self.processor.apply(settings);
        // Cleaned before the gate, so noise and echo don't open it.
        let (blocks, _) = self.frame.as_chunks_mut::<BLOCK>();
        for block in blocks {
            self.processor.process(block);
        }
        let level = (self.frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
        let now = Instant::now();
        if level >= settings.gate() {
            self.loud_until = Some(now + HANGOVER);
        }
        let open = self.loud_until.is_some_and(|until| now < until);
        let silent = shared.muted.load(Ordering::Relaxed) || shared.deafened.load(Ordering::Relaxed);
        shared.talking.store(open && !silent, Ordering::Relaxed);
        if open && !silent {
            let seq = shared.seq.fetch_add(1, Ordering::Relaxed);
            self.packet.resize(2 + 1500, 0);
            self.packet[..2].copy_from_slice(&seq.to_le_bytes());
            if let Ok(n) = self.encoder.encode_float(&self.frame, &mut self.packet[2..]) {
                self.packet.truncate(2 + n);
                (shared.outbox)(self.packet.clone());
            }
        }
        self.frame.clear();
    }
}

fn start_input(shared: &Arc<Shared>) -> Result<cpal::Stream, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host().default_input_device().ok_or("no microphone found")?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    let processor = Processor::new(*shared.settings.lock().unwrap());
    let mut encoder =
        opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).map_err(|e| e.to_string())?;
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(BITRATE));
    let _ = encoder.set_inband_fec(true);
    let _ = encoder.set_packet_loss_perc(5);
    let capture = Arc::new(Mutex::new(Capture {
        shared: shared.clone(),
        encoder,
        channels: usize::from(config.channels.max(1)),
        step: f64::from(config.sample_rate) / f64::from(RATE),
        phase: 0.0,
        last: 0.0,
        frame: Vec::with_capacity(FRAME),
        loud_until: None,
        packet: Vec::new(),
        processor,
    }));
    let problem = shared.clone();
    let on_error = move |error: cpal::Error| {
        *problem.problem.lock().unwrap() = Some(format!("Microphone: {error}"));
    };
    macro_rules! stream {
        ($t:ty, $to_f32:expr) => {{
            let capture = capture.clone();
            device.build_input_stream::<$t, _, _>(
                config,
                move |data: &[$t], _| capture.lock().unwrap().push(data.iter().map($to_f32)),
                on_error,
                None,
            )
        }};
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32, |s: &f32| *s),
        cpal::SampleFormat::I16 => stream!(i16, |s: &i16| f32::from(*s) / 32768.0),
        cpal::SampleFormat::U16 => stream!(u16, |s: &u16| (f32::from(*s) - 32768.0) / 32768.0),
        cpal::SampleFormat::I32 => stream!(i32, |s: &i32| *s as f32 / 2_147_483_648.0),
        other => return Err(format!("unsupported microphone format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silent_shared(out: Arc<Mutex<Vec<Vec<u8>>>>) -> Arc<Shared> {
        Arc::new(Shared {
            speakers: Mutex::new(HashMap::new()),
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            running: AtomicBool::new(true),
            talking: AtomicBool::new(false),
            seq: AtomicU16::new(0),
            outbox: Box::new(move |packet| out.lock().unwrap().push(packet)),
            problem: Mutex::new(None),
            // Raw signal: these tests check framing, not cleaning.
            settings: Mutex::new(VoiceSettings {
                echo_cancellation: false,
                noise_suppression: crate::voice_processing::NoiseLevel::Off,
                auto_gain: false,
                gate_db: -38.0,
            }),
        })
    }

    fn capture(shared: &Arc<Shared>, device_rate: u32, channels: usize) -> Capture {
        Capture {
            shared: shared.clone(),
            encoder: opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).unwrap(),
            channels,
            step: f64::from(device_rate) / f64::from(RATE),
            phase: 0.0,
            last: 0.0,
            frame: Vec::new(),
            loud_until: None,
            packet: Vec::new(),
            processor: Processor::new(*shared.settings.lock().unwrap()),
        }
    }

    fn tone(rate: u32, seconds: f32, channels: usize) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3;
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    #[test]
    fn speech_is_sent_and_silence_is_not() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let shared = silent_shared(out.clone());
        let mut capture = capture(&shared, RATE, 1);
        capture.push(vec![0.0; RATE as usize].into_iter());
        assert!(out.lock().unwrap().is_empty(), "silence sent");
        capture.push(tone(RATE, 0.2, 1).into_iter());
        let sent = out.lock().unwrap().len();
        assert!((9..=10).contains(&sent), "{sent} frames for 200 ms");
    }

    #[test]
    fn resamples_and_mixes_down_device_audio() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let shared = silent_shared(out.clone());
        // One second of 44.1 kHz stereo is 50 frames of 48 kHz mono.
        let mut capture = capture(&shared, 44_100, 2);
        capture.push(tone(44_100, 1.0, 2).into_iter());
        let sent = out.lock().unwrap().len();
        assert!((49..=50).contains(&sent), "{sent}");
    }

    #[test]
    fn muted_microphone_sends_nothing() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let shared = silent_shared(out.clone());
        shared.muted.store(true, Ordering::Relaxed);
        let mut capture = capture(&shared, RATE, 1);
        capture.push(tone(RATE, 0.5, 1).into_iter());
        assert!(out.lock().unwrap().is_empty());
    }

    #[test]
    fn received_frames_decode_into_the_mix() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let shared = silent_shared(out.clone());
        let mut capture = capture(&shared, RATE, 1);
        capture.push(tone(RATE, 0.2, 1).into_iter());
        let voice = Voice { shared: silent_shared(Arc::new(Mutex::new(Vec::new()))) };
        for packet in out.lock().unwrap().iter() {
            let mut frame = 9u32.to_le_bytes().to_vec();
            frame.extend_from_slice(packet);
            voice.receive(&frame);
        }
        assert_eq!(voice.speaking(), vec![9]);
        let mut mix = Mix::new(voice.shared.clone());
        let loud = (0..FRAME * 6).filter_map(|_| mix.next()).filter(|s| s.abs() > 0.05).count();
        assert!(loud > FRAME, "decoded audio plays ({loud} loud samples)");
        voice.set_deafened(true);
        assert!(voice.speaking().is_empty());
        voice.stop();
    }
}
