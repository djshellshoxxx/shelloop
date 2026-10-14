//! Per-sample-track shaping: multimode filter, drive, ADSR amplitude
//! envelope and a zero-lookahead transient shaper.
//!
//! Signal order inside a voice: source -> transient shaper -> drive ->
//! filter -> (envelope and gain, applied by the sample voice). Every stage
//! is bypassed when its parameters are neutral, so a default
//! [`SampleShaping`] leaves playback bit-identical to the legacy path.

use crate::envelope::AdsrParams;
use crate::filter::{clamp_cutoff, FilterMode, FilterParams, StateVariableFilter};
use crate::mixer::SmoothedParam;
use serde::{Deserialize, Serialize};

pub const SHAPING_VERSION: u32 = 1;
pub const MAX_FILTER_RESONANCE: f32 = 0.95;
pub const MAX_DRIVE_DB: f32 = 24.0;
/// Parameter smoothing time for live changes.
pub const SHAPING_SMOOTH_SECONDS: f32 = 0.005;
const FAST_ATTACK_SECONDS: f32 = 0.001;
const FAST_RELEASE_SECONDS: f32 = 0.03;
const SLOW_ATTACK_SECONDS: f32 = 0.03;
const SLOW_RELEASE_SECONDS: f32 = 0.12;
const MIN_TRANSIENT_GAIN: f32 = 0.25;
const MAX_TRANSIENT_GAIN: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFilterMode {
    #[default]
    Off,
    LowPass,
    HighPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleEnvelope {
    pub attack_secs: f32,
    pub decay_secs: f32,
    pub sustain: f32,
    pub release_secs: f32,
}

impl SampleEnvelope {
    fn params(&self) -> AdsrParams {
        AdsrParams {
            attack_secs: self.attack_secs,
            decay_secs: self.decay_secs,
            sustain: self.sustain,
            release_secs: self.release_secs,
        }
    }
}

fn default_version() -> u32 {
    SHAPING_VERSION
}
fn default_cutoff() -> f32 {
    20_000.0
}
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
fn is_default_cutoff(value: &f32) -> bool {
    *value == default_cutoff()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleShaping {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub filter_mode: SampleFilterMode,
    #[serde(default = "default_cutoff", skip_serializing_if = "is_default_cutoff")]
    pub filter_cutoff: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub filter_resonance: f32,
    /// 0 dB bypasses the drive stage.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub drive_db: f32,
    /// `None` keeps the legacy `attack_secs`/`release_secs` fades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<SampleEnvelope>,
    /// -1.0 (soften attacks) ..= 1.0 (emphasise attacks); 0 = bypass.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub transient_attack: f32,
    /// -1.0 (shorten tails) ..= 1.0 (lengthen tails); 0 = bypass.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub transient_sustain: f32,
}

impl Default for SampleShaping {
    fn default() -> Self {
        Self {
            version: SHAPING_VERSION,
            filter_mode: SampleFilterMode::Off,
            filter_cutoff: default_cutoff(),
            filter_resonance: 0.0,
            drive_db: 0.0,
            envelope: None,
            transient_attack: 0.0,
            transient_sustain: 0.0,
        }
    }
}

impl SampleShaping {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != SHAPING_VERSION {
            return Err(format!(
                "unsupported sample_shaping version {} (expected {SHAPING_VERSION})",
                self.version
            ));
        }
        if !self.filter_cutoff.is_finite() || !(20.0..=20_000.0).contains(&self.filter_cutoff) {
            return Err("sample filter cutoff must be finite and between 20 and 20000 Hz".into());
        }
        if !self.filter_resonance.is_finite()
            || !(0.0..=MAX_FILTER_RESONANCE).contains(&self.filter_resonance)
        {
            return Err(format!(
                "sample filter resonance must be between 0.0 and {MAX_FILTER_RESONANCE}"
            ));
        }
        if !self.drive_db.is_finite() || !(0.0..=MAX_DRIVE_DB).contains(&self.drive_db) {
            return Err(format!(
                "sample drive must be between 0 and {MAX_DRIVE_DB} dB"
            ));
        }
        for (name, value) in [
            ("transient attack", self.transient_attack),
            ("transient sustain", self.transient_sustain),
        ] {
            if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
                return Err(format!("sample {name} must be between -1.0 and 1.0"));
            }
        }
        if let Some(envelope) = &self.envelope {
            envelope.params().validate()?;
        }
        Ok(())
    }
}

/// Validated, `Copy` shaping data handed to the audio thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledShaping {
    pub sample_rate: f32,
    pub filter_mode: SampleFilterMode,
    pub filter_cutoff: f32,
    pub filter_resonance: f32,
    pub drive_gain: f32,
    pub envelope: Option<AdsrParams>,
    pub transient_attack: f32,
    pub transient_sustain: f32,
}

impl CompiledShaping {
    pub fn compile(shaping: &SampleShaping, sample_rate: u32) -> Result<Self, String> {
        shaping.validate()?;
        if sample_rate == 0 {
            return Err("shaping sample rate must be greater than zero".into());
        }
        Ok(Self {
            sample_rate: sample_rate as f32,
            filter_mode: shaping.filter_mode,
            filter_cutoff: shaping.filter_cutoff,
            filter_resonance: shaping.filter_resonance,
            drive_gain: 10f32.powf(shaping.drive_db / 20.0),
            envelope: shaping.envelope.map(|e| e.params()),
            transient_attack: shaping.transient_attack,
            transient_sustain: shaping.transient_sustain,
        })
    }

    pub fn inactive(sample_rate: u32) -> Self {
        Self::compile(&SampleShaping::default(), sample_rate.max(1))
            .expect("default shaping is valid")
    }

    pub fn smoothing_frames(&self) -> u32 {
        (self.sample_rate * SHAPING_SMOOTH_SECONDS).round().max(1.0) as u32
    }

    /// Whether any DSP stage (not the envelope) must run.
    pub fn has_dsp(&self) -> bool {
        self.filter_mode != SampleFilterMode::Off
            || self.drive_gain != 1.0
            || self.transient_attack != 0.0
            || self.transient_sustain != 0.0
    }

    pub fn uses_envelope(&self) -> bool {
        self.envelope.is_some()
    }
}

fn coeff(seconds: f32, sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (seconds * sample_rate)).exp()
}

/// Per-voice DSP state. Fixed size; never allocates after construction.
#[derive(Debug, Clone)]
pub struct VoiceShaping {
    filters: [StateVariableFilter; 2],
    mode: SampleFilterMode,
    cutoff: SmoothedParam,
    resonance: SmoothedParam,
    drive: SmoothedParam,
    tr_attack: SmoothedParam,
    tr_sustain: SmoothedParam,
    fast: f32,
    slow: f32,
    fast_coeffs: (f32, f32),
    slow_coeffs: (f32, f32),
    dsp_active: bool,
    cutoff_locked: bool,
}

impl VoiceShaping {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let build = || {
            StateVariableFilter::new(
                rate,
                FilterParams {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 1_000.0,
                    resonance: 0.0,
                    key_tracking: 0.0,
                },
            )
            .expect("valid filter defaults")
        };
        Self {
            filters: [build(), build()],
            mode: SampleFilterMode::Off,
            cutoff: SmoothedParam::new(20_000.0),
            resonance: SmoothedParam::new(0.0),
            drive: SmoothedParam::new(1.0),
            tr_attack: SmoothedParam::new(0.0),
            tr_sustain: SmoothedParam::new(0.0),
            fast: 0.0,
            slow: 0.0,
            fast_coeffs: (
                coeff(FAST_ATTACK_SECONDS, rate),
                coeff(FAST_RELEASE_SECONDS, rate),
            ),
            slow_coeffs: (
                coeff(SLOW_ATTACK_SECONDS, rate),
                coeff(SLOW_RELEASE_SECONDS, rate),
            ),
            dsp_active: false,
            cutoff_locked: false,
        }
    }

    /// Applies new settings. `smooth_frames == 0` snaps (used at note-on);
    /// otherwise values glide, so live edits never click.
    pub fn retarget(&mut self, compiled: &CompiledShaping, smooth_frames: u32) {
        self.mode = compiled.filter_mode;
        if !self.cutoff_locked {
            self.cutoff
                .set_target(compiled.filter_cutoff, smooth_frames);
        }
        self.resonance
            .set_target(compiled.filter_resonance, smooth_frames);
        self.drive.set_target(compiled.drive_gain, smooth_frames);
        self.tr_attack
            .set_target(compiled.transient_attack, smooth_frames);
        self.tr_sustain
            .set_target(compiled.transient_sustain, smooth_frames);
        self.dsp_active = compiled.has_dsp();
    }

    /// Resets filter/follower memory and applies `compiled` immediately.
    /// `cutoff_override` is a step-lock cutoff that also enables the filter
    /// (as low-pass when the configured mode is off).
    pub fn start(&mut self, compiled: &CompiledShaping, cutoff_override: Option<f32>) {
        self.cutoff_locked = false;
        self.retarget(compiled, 0);
        if let Some(cutoff) = cutoff_override {
            self.cutoff.set_target(cutoff.clamp(20.0, 20_000.0), 0);
            self.cutoff_locked = true;
            if self.mode == SampleFilterMode::Off {
                self.mode = SampleFilterMode::LowPass;
            }
            self.dsp_active = true;
        }
        for filter in &mut self.filters {
            filter.reset();
        }
        self.fast = 0.0;
        self.slow = 0.0;
    }

    pub fn is_active(&self) -> bool {
        self.dsp_active
    }

    pub fn process(&mut self, sample_rate: f32, left: f32, right: f32) -> (f32, f32) {
        let (mut l, mut r) = (left, right);

        let attack = self.tr_attack.next_value();
        let sustain = self.tr_sustain.next_value();
        if attack != 0.0 || sustain != 0.0 {
            let level = l.abs().max(r.abs());
            let (fa, fr) = self.fast_coeffs;
            let (sa, sr) = self.slow_coeffs;
            self.fast += if level > self.fast { fa } else { fr } * (level - self.fast);
            self.slow += if level > self.slow { sa } else { sr } * (level - self.slow);
            let difference = (self.fast - self.slow) / (self.fast + self.slow + 1e-6);
            let amount = if difference > 0.0 {
                attack * difference
            } else {
                sustain * difference.abs()
            };
            let gain = (1.0 + 2.0 * amount).clamp(MIN_TRANSIENT_GAIN, MAX_TRANSIENT_GAIN);
            l *= gain;
            r *= gain;
        }

        let drive = self.drive.next_value();
        if drive != 1.0 {
            let norm = drive.tanh().max(1e-6);
            l = (l * drive).tanh() / norm;
            r = (r * drive).tanh() / norm;
        }

        let cutoff = self.cutoff.next_value();
        let resonance = self.resonance.next_value().clamp(0.0, MAX_FILTER_RESONANCE);
        let mode = match self.mode {
            SampleFilterMode::Off => None,
            SampleFilterMode::LowPass => Some(FilterMode::LowPass),
            SampleFilterMode::HighPass => Some(FilterMode::HighPass),
        };
        if let Some(mode) = mode {
            let cutoff = clamp_cutoff(cutoff, sample_rate);
            for (filter, sample) in self.filters.iter_mut().zip([&mut l, &mut r]) {
                filter.set_mode_and_resonance(mode, resonance);
                filter.set_cutoff_hz(cutoff);
                *sample = filter.process(*sample);
            }
        }

        (
            if l.is_finite() { l } else { 0.0 },
            if r.is_finite() { r } else { 0.0 },
        )
    }
}
