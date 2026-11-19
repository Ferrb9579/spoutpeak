//! Cleaning up the microphone for voice chat: echo cancellation, noise
//! suppression and automatic gain, from WebRTC's audio processing (the
//! pure-Rust port in `sonora`).
//!
//! Echo cancellation needs to know what the speakers are playing (the "far
//! end" or render signal) to take it back out of the microphone. Two things
//! play here: the film (the player's audio output) and the other people in
//! voice (the voice mix). Each copies what it plays into a [`Tap`]; the
//! capture side sums both taps into the reference, 10 ms at a time. The
//! taps only fill while a voice session runs.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// WebRTC processes 10 ms blocks; at 48 kHz that's 480 samples.
pub const BLOCK: usize = 480;
/// Reference audio kept waiting at most (250 ms): more and the echo would
/// fall outside what the canceller searches, so the oldest is dropped.
const MAX_BACKLOG: usize = BLOCK * 25;
/// Rough speaker-to-microphone delay of a desktop (output and input
/// buffering), a starting hint; the canceller measures the real delay.
const DELAY_HINT_MS: i32 = 60;

/// How hard noise is removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseLevel {
    Off,
    Low,
    Moderate,
    #[default]
    High,
    VeryHigh,
}

impl NoiseLevel {
    pub const ALL: [NoiseLevel; 5] = [Self::Off, Self::Low, Self::Moderate, Self::High, Self::VeryHigh];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Moderate => "Moderate",
            Self::High => "High",
            Self::VeryHigh => "Very high",
        }
    }
}

/// Voice chat preferences, kept with the other settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Remove the film and other voices from the microphone.
    pub echo_cancellation: bool,
    pub noise_suppression: NoiseLevel,
    /// Even out quiet and loud speakers.
    pub auto_gain: bool,
    /// How loud (dBFS, after cleaning) counts as speech; quieter isn't sent.
    pub gate_db: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { echo_cancellation: true, noise_suppression: NoiseLevel::High, auto_gain: true, gate_db: -45.0 }
    }
}

impl VoiceSettings {
    /// The gate as a linear RMS level.
    pub fn gate(&self) -> f32 {
        10f32.powf(self.gate_db.clamp(-70.0, -10.0) / 20.0)
    }

    fn config(&self) -> sonora::Config {
        use sonora::config::{EchoCanceller, GainController2, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel};
        let level = match self.noise_suppression {
            NoiseLevel::Off => None,
            NoiseLevel::Low => Some(NoiseSuppressionLevel::Low),
            NoiseLevel::Moderate => Some(NoiseSuppressionLevel::Moderate),
            NoiseLevel::High => Some(NoiseSuppressionLevel::High),
            NoiseLevel::VeryHigh => Some(NoiseSuppressionLevel::VeryHigh),
        };
        sonora::Config {
            // Rumble and desk thumps go first; the canceller wants it too.
            high_pass_filter: Some(HighPassFilter::default()),
            echo_canceller: self.echo_cancellation.then(EchoCanceller::default),
            noise_suppression: level.map(|level| NoiseSuppression { level, ..NoiseSuppression::default() }),
            gain_controller2: self.auto_gain.then(|| GainController2 {
                // Digital gain only: leave the system's microphone volume be.
                input_volume_controller: false,
                ..GainController2::default()
            }),
            ..sonora::Config::default()
        }
    }
}

/// Audio on its way to the speakers, kept for the echo canceller.
pub struct Tap {
    queue: Mutex<VecDeque<f32>>,
}

impl Tap {
    const fn new() -> Self {
        Self { queue: Mutex::new(VecDeque::new()) }
    }

    /// Adds played mono audio (48 kHz).
    pub fn push(&self, samples: &[f32]) {
        if !ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        let mut queue = self.queue.lock().unwrap();
        queue.extend(samples);
        let excess = queue.len().saturating_sub(MAX_BACKLOG);
        queue.drain(..excess);
    }

    /// Adds the next `BLOCK` samples into `out` (silence where it ran dry).
    fn take_into(&self, out: &mut [f32]) {
        let mut queue = self.queue.lock().unwrap();
        for slot in out.iter_mut() {
            match queue.pop_front() {
                Some(sample) => *slot += sample,
                None => break,
            }
        }
    }

    fn clear(&self) {
        self.queue.lock().unwrap().clear();
    }
}

/// The film's sound, as the player plays it.
pub static FILM: Tap = Tap::new();
/// The other people in voice, as mixed for the speakers.
pub static VOICES: Tap = Tap::new();
/// A voice session wants the taps filled.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Batches one producer's samples into 10 ms pushes, mixing stereo down,
/// so the audio thread takes the tap's lock 100 times a second, not 96 000.
pub struct TapWriter {
    tap: &'static Tap,
    channels: usize,
    pending: Vec<f32>,
    sum: f32,
    count: usize,
}

impl TapWriter {
    pub fn new(tap: &'static Tap, channels: usize) -> Self {
        Self { tap, channels: channels.max(1), pending: Vec::with_capacity(BLOCK), sum: 0.0, count: 0 }
    }

    /// One played sample (interleaved, when stereo).
    #[inline]
    pub fn sample(&mut self, sample: f32) {
        if !ACTIVE.load(Ordering::Relaxed) {
            if !self.pending.is_empty() {
                self.pending.clear();
                (self.sum, self.count) = (0.0, 0);
            }
            return;
        }
        self.sum += sample;
        self.count += 1;
        if self.count < self.channels {
            return;
        }
        self.pending.push(self.sum / self.channels as f32);
        (self.sum, self.count) = (0.0, 0);
        if self.pending.len() == BLOCK {
            self.tap.push(&self.pending);
            self.pending.clear();
        }
    }
}

/// Turns the taps on for a voice session, and off again when dropped.
pub struct TapsOn;

impl TapsOn {
    pub fn new() -> Self {
        FILM.clear();
        VOICES.clear();
        ACTIVE.store(true, Ordering::Relaxed);
        Self
    }
}

impl Drop for TapsOn {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Relaxed);
        FILM.clear();
        VOICES.clear();
    }
}

/// Cleans 10 ms microphone blocks.
pub struct Processor {
    apm: sonora::AudioProcessing,
    settings: VoiceSettings,
    reference: Vec<f32>,
    scratch: Vec<f32>,
}

impl Processor {
    pub fn new(settings: VoiceSettings) -> Self {
        let stream = sonora::StreamConfig::new(48_000, 1);
        let mut apm = sonora::AudioProcessing::builder()
            .config(settings.config())
            .capture_config(stream)
            .render_config(stream)
            .build();
        let _ = apm.set_stream_delay_ms(DELAY_HINT_MS);
        Self { apm, settings, reference: vec![0.0; BLOCK], scratch: vec![0.0; BLOCK] }
    }

    /// New preferences take effect from the next block.
    pub fn apply(&mut self, settings: VoiceSettings) {
        if settings != self.settings {
            self.apm.apply_config(settings.config());
            self.settings = settings;
        }
    }

    /// Cleans one block in place. The reference is what the speakers played
    /// meanwhile (film plus other voices).
    pub fn process(&mut self, block: &mut [f32]) {
        debug_assert_eq!(block.len(), BLOCK);
        if self.settings.echo_cancellation {
            self.reference.iter_mut().for_each(|s| *s = 0.0);
            FILM.take_into(&mut self.reference);
            VOICES.take_into(&mut self.reference);
            for sample in &mut self.reference {
                *sample = sample.clamp(-1.0, 1.0);
            }
            let _ = self.apm.set_stream_delay_ms(DELAY_HINT_MS);
            let _ = self.apm.process_render_f32(&[&self.reference], &mut [&mut self.scratch]);
        }
        self.scratch.copy_from_slice(block);
        if self.apm.process_capture_f32(&[&self.scratch], &mut [block]).is_err() {
            block.copy_from_slice(&self.scratch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The taps are global: tests that use them take turns.
    static TAPS: Mutex<()> = Mutex::new(());

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// Deterministic noise in -1..1.
    fn noise(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    #[test]
    fn old_profiles_get_the_defaults() {
        let settings: VoiceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, VoiceSettings::default());
        assert!(settings.echo_cancellation && settings.auto_gain);
        assert!((settings.gate() - 0.0056).abs() < 0.0005);
    }

    #[test]
    fn steady_noise_is_turned_down() {
        let mut processor = Processor::new(VoiceSettings {
            echo_cancellation: false,
            auto_gain: false,
            ..VoiceSettings::default()
        });
        let mut seed = 7;
        let mut last = 0.0;
        // Two seconds of hiss: the suppressor learns it, then removes it.
        for _ in 0..200 {
            let mut block: Vec<f32> = (0..BLOCK).map(|_| noise(&mut seed) * 0.05).collect();
            processor.process(&mut block);
            last = rms(&block);
        }
        let input = 0.05 / 3f32.sqrt();
        assert!(last < input * 0.5, "noise {last} vs input {input}");
    }

    #[test]
    fn film_sound_picked_up_by_the_microphone_is_cancelled() {
        let _turn = TAPS.lock().unwrap_or_else(|e| e.into_inner());
        let _taps = TapsOn::new();
        let mut processor = Processor::new(VoiceSettings {
            noise_suppression: NoiseLevel::Off,
            auto_gain: false,
            ..VoiceSettings::default()
        });
        let mut seed = 11;
        let mut film_writer = TapWriter::new(&FILM, 2);
        // The room: the microphone hears the film 30 ms late and quieter.
        let delay = BLOCK * 3;
        let mut air: VecDeque<f32> = std::iter::repeat_n(0.0, delay).collect();
        let (mut before, mut after) = (0.0, 0.0);
        for round in 0..600 {
            let film: Vec<f32> = (0..BLOCK).map(|_| noise(&mut seed) * 0.3).collect();
            for &s in &film {
                // Stereo, as the player writes it.
                film_writer.sample(s);
                film_writer.sample(s);
            }
            air.extend(film.iter().map(|s| s * 0.5));
            let mut mic: Vec<f32> = air.drain(..BLOCK).collect();
            if round >= 500 {
                before += rms(&mic);
            }
            processor.process(&mut mic);
            if round >= 500 {
                after += rms(&mic);
            }
        }
        let reduction = 20.0 * (after / before).log10();
        assert!(reduction < -10.0, "echo only {reduction:.1} dB lower");
    }

    #[test]
    fn taps_stay_empty_without_a_voice_session() {
        let _turn = TAPS.lock().unwrap_or_else(|e| e.into_inner());
        let mut writer = TapWriter::new(&VOICES, 1);
        for _ in 0..BLOCK * 2 {
            writer.sample(0.5);
        }
        let mut out = vec![0.0; BLOCK];
        VOICES.take_into(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
