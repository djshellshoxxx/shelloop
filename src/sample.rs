//! WAV sample instruments (spec 04).
//!
//! Files are decoded, validated and converted to immutable stereo `f32` frames
//! on the control thread. The audio callback only sees [`CompiledSamplePlayback`]
//! (shared frame storage plus precomputed frame bounds) and a fixed set of
//! [`SampleVoice`]s, so playback never touches the filesystem, allocates or
//! indexes outside the decoded buffer.

use crate::EngineCommand;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default decoded-sample memory budget per project: 512 MiB.
pub const DEFAULT_SAMPLE_MEMORY_BUDGET: usize = 512 * 1024 * 1024;
/// Bytes of decoded storage per frame (stereo `f32`).
pub const BYTES_PER_FRAME: usize = std::mem::size_of::<StereoFrame>();
pub const MIN_SAMPLE_RATE: u32 = 1_000;
pub const MAX_SAMPLE_RATE: u32 = 384_000;
pub const DEFAULT_SAMPLE_VOICES: u16 = 16;
pub const MAX_SAMPLE_VOICES: u16 = 64;
pub const MAX_PITCH_SEMITONES: f32 = 48.0;
pub const MAX_FADE_SECS: f32 = 1.0;
/// Upper bound on the per-sample read increment. Extreme transpositions are
/// clamped here so rendering stays finite and bounded.
pub const MAX_PLAYBACK_INCREMENT: f64 = 64.0;
pub const MIN_PLAYBACK_INCREMENT: f64 = 1.0 / 1024.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StereoFrame {
    pub left: f32,
    pub right: f32,
}

impl StereoFrame {
    pub const SILENT: Self = Self {
        left: 0.0,
        right: 0.0,
    };

    pub fn new(left: f32, right: f32) -> Self {
        Self { left, right }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleAssetId(pub u32);

/// A decoded, immutable sample. `source_path` is for control/persistence only.
#[derive(Debug, Clone)]
pub struct SampleAsset {
    pub id: SampleAssetId,
    pub source_path: PathBuf,
    pub source_sample_rate: u32,
    pub channels: u16,
    pub frames: Arc<[StereoFrame]>,
}

impl SampleAsset {
    pub fn byte_size(&self) -> usize {
        self.frames.len() * BYTES_PER_FRAME
    }
}

/// Decoded audio before it is registered in a bank.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: Vec<StereoFrame>,
}

/// Decode a WAV stream into stereo `f32` frames. Mono is duplicated to both
/// channels. Supports 8/16/24/32-bit integer PCM and 32-bit float.
/// `max_bytes` is checked against the header before any sample data is
/// decoded, so an oversized file is refused without allocating for it.
pub fn decode_wav_reader<R: Read>(reader: R, max_bytes: usize) -> Result<DecodedWav, String> {
    let reader = hound::WavReader::new(reader).map_err(|error| format!("invalid WAV: {error}"))?;
    let spec = reader.spec();
    if !(1..=2).contains(&spec.channels) {
        return Err(format!(
            "unsupported channel count {}; only mono and stereo WAV files are supported",
            spec.channels
        ));
    }
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&spec.sample_rate) {
        return Err(format!(
            "unsupported sample rate {} Hz; expected {MIN_SAMPLE_RATE}-{MAX_SAMPLE_RATE} Hz",
            spec.sample_rate
        ));
    }
    let frame_count = reader.duration() as usize;
    if frame_count == 0 {
        return Err("WAV file contains no audio frames".into());
    }
    let requested = frame_count.saturating_mul(BYTES_PER_FRAME);
    if requested > max_bytes {
        return Err(format!(
            "decoded sample needs {requested} bytes but only {max_bytes} bytes of the sample memory budget remain"
        ));
    }

    let channels = usize::from(spec.channels);
    let mut interleaved = Vec::with_capacity(frame_count * channels);
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => {
            for sample in reader.into_samples::<f32>() {
                let sample = sample.map_err(|error| format!("corrupt WAV data: {error}"))?;
                interleaved.push(if sample.is_finite() { sample } else { 0.0 });
            }
        }
        (hound::SampleFormat::Int, bits @ (8 | 16 | 24 | 32)) => {
            let scale = 1.0 / (1_u64 << (bits - 1)) as f64;
            for sample in reader.into_samples::<i32>() {
                let sample = sample.map_err(|error| format!("corrupt WAV data: {error}"))?;
                interleaved.push((f64::from(sample) * scale) as f32);
            }
        }
        (format, bits) => {
            return Err(format!(
                "unsupported WAV encoding: {bits}-bit {}",
                match format {
                    hound::SampleFormat::Float => "float",
                    hound::SampleFormat::Int => "integer PCM",
                }
            ))
        }
    }
    if interleaved.len() != frame_count * channels {
        return Err(format!(
            "corrupt WAV data: header promises {frame_count} frames but the file ends early"
        ));
    }

    let frames = if channels == 1 {
        interleaved
            .iter()
            .map(|&sample| StereoFrame::new(sample, sample))
            .collect()
    } else {
        interleaved
            .chunks_exact(2)
            .map(|pair| StereoFrame::new(pair[0], pair[1]))
            .collect()
    };
    Ok(DecodedWav {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        frames,
    })
}

pub fn decode_wav_file(path: impl AsRef<Path>, max_bytes: usize) -> Result<DecodedWav, String> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot open sample {}: {error}", path.display()))?;
    decode_wav_reader(std::io::BufReader::new(file), max_bytes)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Immutable set of decoded samples with a bounded memory budget.
#[derive(Debug, Clone)]
pub struct SampleAssetBank {
    assets: Vec<SampleAsset>,
    by_path: HashMap<PathBuf, SampleAssetId>,
    budget_bytes: usize,
    used_bytes: usize,
}

impl SampleAssetBank {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            assets: Vec::new(),
            by_path: HashMap::new(),
            budget_bytes,
            used_bytes: 0,
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn len(&self) -> usize {
        self.assets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    pub fn get(&self, id: SampleAssetId) -> Option<&SampleAsset> {
        self.assets.get(id.0 as usize)
    }

    pub fn id_for_path(&self, path: impl AsRef<Path>) -> Option<SampleAssetId> {
        self.by_path.get(path.as_ref()).copied()
    }

    /// Register already-decoded audio, enforcing the memory budget.
    pub fn insert_decoded(
        &mut self,
        source_path: impl Into<PathBuf>,
        decoded: DecodedWav,
    ) -> Result<SampleAssetId, String> {
        let source_path = source_path.into();
        if let Some(id) = self.by_path.get(&source_path) {
            return Ok(*id);
        }
        let requested = decoded.frames.len() * BYTES_PER_FRAME;
        if requested > self.remaining_bytes() {
            return Err(format!(
                "sample memory budget exceeded loading {}: requested {requested} bytes, {} of {} bytes already in use",
                source_path.display(),
                self.used_bytes,
                self.budget_bytes
            ));
        }
        if decoded.frames.is_empty() {
            return Err(format!("{}: sample has no frames", source_path.display()));
        }
        let id = SampleAssetId(self.assets.len() as u32);
        self.used_bytes += requested;
        self.by_path.insert(source_path.clone(), id);
        self.assets.push(SampleAsset {
            id,
            source_path,
            source_sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            frames: decoded.frames.into(),
        });
        Ok(id)
    }

    /// Decode and register a WAV file. The same path is only decoded once.
    pub fn load_file(&mut self, path: impl AsRef<Path>) -> Result<SampleAssetId, String> {
        let path = path.as_ref();
        if let Some(id) = self.by_path.get(path) {
            return Ok(*id);
        }
        let remaining = self.remaining_bytes();
        let decoded = decode_wav_file(path, remaining).map_err(|error| {
            if error.contains("sample memory budget") {
                format!(
                    "{error} (budget {} bytes, {} in use)",
                    self.budget_bytes, self.used_bytes
                )
            } else {
                error
            }
        })?;
        self.insert_decoded(path, decoded)
    }

    /// Build a new bank holding every sample referenced by `settings`, resolving
    /// relative paths against `base_dir`. Every failure is collected so the
    /// user sees all missing or invalid files at once. Nothing is returned on
    /// failure, so whatever bank is currently active stays untouched.
    pub fn load_all<'a>(
        settings: impl IntoIterator<Item = &'a SampleSettings>,
        base_dir: &Path,
        budget_bytes: usize,
    ) -> Result<Self, String> {
        let mut bank = Self::new(budget_bytes);
        let mut errors = Vec::new();
        for settings in settings {
            if let Err(error) = bank.load_file(settings.resolve_path(base_dir)) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(bank)
        } else {
            Err(format!(
                "{} sample file(s) could not be loaded:\n  {}",
                errors.len(),
                errors.join("\n  ")
            ))
        }
    }

    fn remaining_bytes(&self) -> usize {
        self.budget_bytes.saturating_sub(self.used_bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleMode {
    /// Plays to the end of the region; note-off is ignored.
    OneShot,
    /// Plays while the note is held, then fades out over `release_secs`.
    Gate,
    /// Loops between `loop_start` and `loop_end` while held, then fades out.
    Loop,
}

fn default_end() -> f64 {
    1.0
}
fn default_gain() -> f32 {
    1.0
}
fn default_release() -> f32 {
    0.01
}
fn default_voices() -> u16 {
    DEFAULT_SAMPLE_VOICES
}
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Persistent per-track sample settings. Positions are normalized (0.0-1.0) so
/// a project stays valid if the file is replaced by one of a different length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleSettings {
    /// WAV path, relative to the project file's folder unless absolute.
    pub path: String,
    pub mode: SampleMode,
    #[serde(default, skip_serializing_if = "is_default")]
    pub start: f64,
    #[serde(default = "default_end")]
    pub end: f64,
    #[serde(default, skip_serializing_if = "is_default")]
    pub loop_start: f64,
    #[serde(default = "default_end")]
    pub loop_end: f64,
    #[serde(default = "default_gain")]
    pub gain: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pan: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pitch_semitones: f32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub fine_cents: f32,
    /// When set, pattern notes transpose the sample chromatically relative to
    /// this MIDI note. When absent, every note plays at the sample's own pitch,
    /// which is what drum one-shots want.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_note: Option<u8>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub reverse: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub attack_secs: f32,
    #[serde(default = "default_release")]
    pub release_secs: f32,
    #[serde(default = "default_voices")]
    pub voices: u16,
}

impl SampleSettings {
    pub fn new(path: impl Into<String>, mode: SampleMode) -> Self {
        Self {
            path: path.into(),
            mode,
            start: 0.0,
            end: 1.0,
            loop_start: 0.0,
            loop_end: 1.0,
            gain: 1.0,
            pan: 0.0,
            pitch_semitones: 0.0,
            fine_cents: 0.0,
            root_note: None,
            reverse: false,
            attack_secs: 0.0,
            release_secs: default_release(),
            voices: DEFAULT_SAMPLE_VOICES,
        }
    }

    pub fn resolve_path(&self, base_dir: &Path) -> PathBuf {
        let path = Path::new(&self.path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.path.trim().is_empty() {
            return Err("sample path may not be empty".into());
        }
        let unit = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
        if !unit(self.start) || !unit(self.end) || self.start >= self.end {
            return Err("sample start and end must satisfy 0.0 <= start < end <= 1.0".into());
        }
        if self.mode == SampleMode::Loop
            && (!unit(self.loop_start)
                || !unit(self.loop_end)
                || self.loop_start < self.start
                || self.loop_end > self.end
                || self.loop_start >= self.loop_end)
        {
            return Err("sample loop must satisfy start <= loop_start < loop_end <= end".into());
        }
        if !self.gain.is_finite() || !(0.0..=2.0).contains(&self.gain) {
            return Err("sample gain must be finite and between 0.0 and 2.0".into());
        }
        if !self.pan.is_finite() || !(-1.0..=1.0).contains(&self.pan) {
            return Err("sample pan must be finite and between -1.0 and 1.0".into());
        }
        if !self.pitch_semitones.is_finite()
            || !(-MAX_PITCH_SEMITONES..=MAX_PITCH_SEMITONES).contains(&self.pitch_semitones)
        {
            return Err(format!(
                "sample pitch_semitones must be between -{MAX_PITCH_SEMITONES} and {MAX_PITCH_SEMITONES}"
            ));
        }
        if !self.fine_cents.is_finite() || !(-100.0..=100.0).contains(&self.fine_cents) {
            return Err("sample fine_cents must be between -100 and 100".into());
        }
        if self.root_note.is_some_and(|note| note > 127) {
            return Err("sample root_note must be a MIDI note 0..=127".into());
        }
        for (name, value) in [
            ("attack_secs", self.attack_secs),
            ("release_secs", self.release_secs),
        ] {
            if !value.is_finite() || !(0.0..=MAX_FADE_SECS).contains(&value) {
                return Err(format!(
                    "sample {name} must be between 0.0 and {MAX_FADE_SECS} seconds"
                ));
            }
        }
        if !(1..=MAX_SAMPLE_VOICES).contains(&self.voices) {
            return Err(format!(
                "sample voices must be between 1 and {MAX_SAMPLE_VOICES}"
            ));
        }
        Ok(())
    }
}

/// Interpolation used to read between source frames. Isolated here so a
/// higher-quality interpolator can be added without changing project data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
}

impl Interpolation {
    /// Read at fractional `position`. `next` is the index used as the right
    /// neighbour, already wrapped or clamped by the caller.
    #[inline]
    fn read(self, frames: &[StereoFrame], index: usize, next: usize, fraction: f32) -> StereoFrame {
        match self {
            Self::Linear => {
                let a = frames[index];
                let b = frames[next];
                StereoFrame::new(
                    a.left + (b.left - a.left) * fraction,
                    a.right + (b.right - a.right) * fraction,
                )
            }
        }
    }
}

/// Everything the audio thread needs to play one track's sample, with all
/// frame bounds checked against the decoded buffer up front.
#[derive(Debug, Clone)]
pub struct CompiledSamplePlayback {
    frames: Arc<[StereoFrame]>,
    mode: SampleMode,
    start_frame: usize,
    end_frame: usize,
    loop_start: usize,
    loop_end: usize,
    base_increment: f64,
    root_note: Option<u8>,
    reverse: bool,
    gain: f32,
    left_gain: f32,
    right_gain: f32,
    attack_frames: u32,
    release_frames: u32,
    voices: u16,
    interpolation: Interpolation,
}

impl CompiledSamplePlayback {
    pub fn new(
        asset: &SampleAsset,
        settings: &SampleSettings,
        output_sample_rate: u32,
    ) -> Result<Self, String> {
        settings.validate()?;
        if output_sample_rate == 0 {
            return Err("output sample rate must be greater than zero".into());
        }
        let len = asset.frames.len();
        if len == 0 {
            return Err("sample has no frames".into());
        }
        let to_frame = |position: f64| ((position * len as f64).round() as usize).min(len);
        let start_frame = to_frame(settings.start);
        let end_frame = to_frame(settings.end).max(start_frame + 1).min(len);
        if start_frame >= end_frame {
            return Err("sample region is empty after conversion to frames".into());
        }
        let (loop_start, loop_end) = if settings.mode == SampleMode::Loop {
            let loop_start = to_frame(settings.loop_start).clamp(start_frame, end_frame - 1);
            let loop_end = to_frame(settings.loop_end).clamp(loop_start + 1, end_frame);
            (loop_start, loop_end)
        } else {
            (start_frame, end_frame)
        };

        let semitones =
            f64::from(settings.pitch_semitones) + f64::from(settings.fine_cents) / 100.0;
        let base_increment = f64::from(asset.source_sample_rate) / f64::from(output_sample_rate)
            * 2.0_f64.powf(semitones / 12.0);
        let seconds_to_frames =
            |secs: f32| (f64::from(secs) * f64::from(output_sample_rate)).round() as u32;
        let balance = settings.pan.clamp(-1.0, 1.0);
        Ok(Self {
            frames: Arc::clone(&asset.frames),
            mode: settings.mode,
            start_frame,
            end_frame,
            loop_start,
            loop_end,
            base_increment,
            root_note: settings.root_note,
            reverse: settings.reverse,
            gain: settings.gain,
            left_gain: (1.0 - balance).min(1.0),
            right_gain: (1.0 + balance).min(1.0),
            attack_frames: seconds_to_frames(settings.attack_secs),
            release_frames: seconds_to_frames(settings.release_secs),
            voices: settings.voices,
            interpolation: Interpolation::Linear,
        })
    }

    pub fn mode(&self) -> SampleMode {
        self.mode
    }

    pub fn region(&self) -> (usize, usize) {
        (self.start_frame, self.end_frame)
    }

    pub fn loop_region(&self) -> (usize, usize) {
        (self.loop_start, self.loop_end)
    }

    pub fn voices(&self) -> usize {
        usize::from(self.voices)
    }

    /// Read increment for a note, clamped to keep extreme transpositions finite.
    pub fn increment_for_note(&self, note: u8) -> f64 {
        let transpose = self
            .root_note
            .map(|root| f64::from(i16::from(note) - i16::from(root)))
            .unwrap_or(0.0);
        (self.base_increment * 2.0_f64.powf(transpose / 12.0))
            .clamp(MIN_PLAYBACK_INCREMENT, MAX_PLAYBACK_INCREMENT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoicePhase {
    Idle,
    Playing,
    Releasing,
}

/// One playing instance of a sample. Positions are `f64` source frames.
#[derive(Debug, Clone)]
pub struct SampleVoice {
    phase: VoicePhase,
    position: f64,
    increment: f64,
    amplitude: f32,
    level: f32,
    level_step: f32,
    attack_remaining: u32,
    channel: u8,
    note: u8,
    key_held: bool,
    started_order: u64,
}

impl Default for SampleVoice {
    fn default() -> Self {
        Self {
            phase: VoicePhase::Idle,
            position: 0.0,
            increment: 1.0,
            amplitude: 0.0,
            level: 0.0,
            level_step: 0.0,
            attack_remaining: 0,
            channel: 0,
            note: 0,
            key_held: false,
            started_order: 0,
        }
    }
}

impl SampleVoice {
    pub fn is_active(&self) -> bool {
        self.phase != VoicePhase::Idle
    }

    /// Current read position in source frames, for tests and diagnostics.
    pub fn position(&self) -> f64 {
        self.position
    }

    fn start(
        &mut self,
        playback: &CompiledSamplePlayback,
        channel: u8,
        note: u8,
        velocity: f32,
        order: u64,
    ) {
        self.phase = VoicePhase::Playing;
        self.increment = playback.increment_for_note(note);
        self.position = if playback.reverse {
            (playback.end_frame - 1) as f64
        } else {
            playback.start_frame as f64
        };
        self.amplitude = velocity.clamp(0.0, 1.0) * playback.gain;
        self.channel = channel;
        self.note = note;
        self.key_held = true;
        self.started_order = order;
        if playback.attack_frames == 0 {
            self.level = 1.0;
            self.level_step = 0.0;
            self.attack_remaining = 0;
        } else {
            self.level = 0.0;
            self.level_step = 1.0 / playback.attack_frames as f32;
            self.attack_remaining = playback.attack_frames;
        }
    }

    fn release(&mut self, playback: &CompiledSamplePlayback) {
        self.key_held = false;
        if self.phase != VoicePhase::Playing || playback.mode == SampleMode::OneShot {
            return;
        }
        if playback.release_frames == 0 {
            self.stop();
            return;
        }
        self.phase = VoicePhase::Releasing;
        self.attack_remaining = 0;
        self.level_step = -self.level / playback.release_frames as f32;
    }

    fn stop(&mut self) {
        self.phase = VoicePhase::Idle;
        self.key_held = false;
        self.level = 0.0;
        self.level_step = 0.0;
    }

    fn looping(&self, playback: &CompiledSamplePlayback) -> bool {
        playback.mode == SampleMode::Loop
    }

    fn next_frame(&mut self, playback: &CompiledSamplePlayback) -> StereoFrame {
        if self.phase == VoicePhase::Idle {
            return StereoFrame::SILENT;
        }
        let looping = self.looping(playback);

        // Bounds check before every read: the position is always inside
        // [start_frame, end_frame) here, and the neighbour is wrapped or clamped.
        if self.position < playback.start_frame as f64
            || self.position >= playback.end_frame as f64
            || !self.position.is_finite()
        {
            self.stop();
            return StereoFrame::SILENT;
        }
        let index = self.position as usize;
        let fraction = (self.position - index as f64) as f32;
        let next = if playback.reverse {
            // Reading backwards, the next frame in time is the one below, but
            // interpolation is always between `index` and `index + 1`.
            (index + 1).min(playback.end_frame - 1)
        } else if looping && index + 1 >= playback.loop_end {
            playback.loop_start
        } else {
            (index + 1).min(playback.end_frame - 1)
        };
        let frame = playback
            .interpolation
            .read(&playback.frames, index, next, fraction);

        let gain = self.amplitude * self.level;
        let out = StereoFrame::new(
            frame.left * gain * playback.left_gain,
            frame.right * gain * playback.right_gain,
        );

        self.advance_level();
        self.advance_position(playback, looping);
        out
    }

    fn advance_level(&mut self) {
        match self.phase {
            VoicePhase::Playing if self.attack_remaining > 0 => {
                self.attack_remaining -= 1;
                self.level = if self.attack_remaining == 0 {
                    1.0
                } else {
                    (self.level + self.level_step).min(1.0)
                };
            }
            VoicePhase::Releasing => {
                self.level += self.level_step;
                if self.level <= 0.0 {
                    self.stop();
                }
            }
            _ => {}
        }
    }

    fn advance_position(&mut self, playback: &CompiledSamplePlayback, looping: bool) {
        if self.phase == VoicePhase::Idle {
            return;
        }
        if playback.reverse {
            self.position -= self.increment;
            if looping {
                let loop_start = playback.loop_start as f64;
                let loop_len = (playback.loop_end - playback.loop_start) as f64;
                if self.position < loop_start {
                    self.position =
                        playback.loop_end as f64 - (loop_start - self.position) % loop_len;
                    if self.position >= playback.loop_end as f64 {
                        self.position = loop_start;
                    }
                }
            }
        } else {
            self.position += self.increment;
            if looping {
                let loop_start = playback.loop_start as f64;
                let loop_end = playback.loop_end as f64;
                if self.position >= loop_end {
                    let loop_len = loop_end - loop_start;
                    self.position = loop_start + (self.position - loop_start) % loop_len;
                }
            }
        }
        // Retire the voice as soon as it leaves the region, so the voice count
        // drops on the frame that played the last sample rather than one later.
        if self.position < playback.start_frame as f64 || self.position >= playback.end_frame as f64
        {
            self.stop();
        }
    }
}

/// Fixed-capacity sample instrument driven by the same [`EngineCommand`]s as
/// the synth. Voices are allocated up front; nothing grows during playback.
#[derive(Debug, Clone)]
pub struct RealtimeSampler {
    playback: CompiledSamplePlayback,
    voices: Vec<SampleVoice>,
    next_order: u64,
}

impl RealtimeSampler {
    pub fn new(playback: CompiledSamplePlayback) -> Self {
        let voices = vec![SampleVoice::default(); playback.voices()];
        Self {
            playback,
            voices,
            next_order: 1,
        }
    }

    pub fn playback(&self) -> &CompiledSamplePlayback {
        &self.playback
    }

    pub fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|voice| voice.is_active()).count()
    }

    pub fn voices(&self) -> &[SampleVoice] {
        &self.voices
    }

    pub fn handle(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if !velocity.is_finite() || velocity <= 0.0 || note > 127 {
                    return;
                }
                let slot = self.choose_slot();
                let order = self.next_order;
                self.next_order = self.next_order.wrapping_add(1).max(1);
                self.voices[slot].start(&self.playback, channel, note, velocity, order);
            }
            EngineCommand::NoteOff { channel, note } => {
                // Release one voice per note-off, oldest first, matching the synth.
                if let Some(voice) = self
                    .voices
                    .iter_mut()
                    .filter(|voice| {
                        voice.is_active()
                            && voice.key_held
                            && voice.channel == channel
                            && voice.note == note
                    })
                    .min_by_key(|voice| voice.started_order)
                {
                    voice.release(&self.playback);
                }
            }
            EngineCommand::Sustain { .. } => {}
            EngineCommand::Panic => {
                for voice in &mut self.voices {
                    voice.stop();
                }
            }
        }
    }

    /// Deterministic stealing: a free voice first, then the oldest voice that
    /// is no longer held (released or a finished-gate one-shot), then the
    /// oldest voice of all.
    fn choose_slot(&self) -> usize {
        if let Some(index) = self.voices.iter().position(|voice| !voice.is_active()) {
            return index;
        }
        let oldest = |held: Option<bool>| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| held.is_none_or(|held| voice.key_held == held))
                .min_by_key(|(_, voice)| voice.started_order)
                .map(|(index, _)| index)
        };
        oldest(Some(false)).or_else(|| oldest(None)).unwrap_or(0)
    }

    pub fn next_stereo_frame(&mut self) -> (f32, f32) {
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for voice in &mut self.voices {
            if voice.is_active() {
                let frame = voice.next_frame(&self.playback);
                left += frame.left;
                right += frame.right;
            }
        }
        // Saturation happens once, in the channel strip and master bus, so the
        // sample itself is passed through unshaped. Only non-finite values are
        // removed here.
        (finite_or_zero(left), finite_or_zero(right))
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}
