//! Spec 07 — bounded stereo effects.
//!
//! Effects use enum dispatch and allocate every buffer in [`Effect::new`];
//! `process`, `set_param`, `set_bypassed`, `set_tempo` and `reset` never
//! allocate, lock or panic, and always produce finite output.

use crate::mixer::SmoothedParam;
use crate::params::{EffectParamId, EffectSlotId, ParamCurve, ParamDescriptor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f32::consts::TAU;

pub const MAX_TRACK_INSERTS: usize = 4;
pub const MAX_SEND_BUSES: usize = 2;
pub const MAX_SEND_EFFECTS: usize = 2;
pub const MAX_MASTER_INSERTS: usize = 4;
pub const MAX_DELAY_SECONDS: f32 = 4.0;
pub const DEFAULT_EFFECT_MEMORY_BUDGET: usize = 64 * 1024 * 1024;

const MAX_EFFECT_PARAMS: usize = 6;
const PARAM_SMOOTH_SECONDS: f32 = 0.02;
const BYPASS_CROSSFADE_SECONDS: f32 = 0.01;
const DELAY_GLIDE_SECONDS: f32 = 0.1;
const MAX_PREDELAY_MS: f32 = 250.0;
/// Inputs beyond +80 dBFS are clamped so feedback paths can never overflow.
const INPUT_LIMIT: f32 = 1.0e4;
const DEFAULT_BPM: f64 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Saturation,
    Delay,
    Reverb,
}

impl EffectKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Saturation => "saturation",
            Self::Delay => "delay",
            Self::Reverb => "reverb",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "saturation" => Some(Self::Saturation),
            "delay" => Some(Self::Delay),
            "reverb" => Some(Self::Reverb),
            _ => None,
        }
    }
}

const fn param(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    curve: ParamCurve,
) -> ParamDescriptor {
    ParamDescriptor {
        name,
        min,
        max,
        default,
        curve,
        lockable: !matches!(curve, ParamCurve::Discrete),
    }
}

mod sat {
    pub const MODE: usize = 0;
    pub const DRIVE: usize = 1;
    pub const TONE: usize = 2;
    pub const MIX: usize = 3;
    pub const OUTPUT: usize = 4;
}

mod dly {
    pub const TIME_MS: usize = 0;
    pub const SYNC: usize = 1;
    pub const FEEDBACK: usize = 2;
    pub const MIX: usize = 3;
    pub const PING_PONG: usize = 4;
    pub const DAMPING: usize = 5;
}

mod rev {
    pub const SIZE: usize = 0;
    pub const DECAY: usize = 1;
    pub const DAMPING: usize = 2;
    pub const PREDELAY_MS: usize = 3;
    pub const MIX: usize = 4;
    pub const WIDTH: usize = 5;
}

static SATURATION_PARAMS: [ParamDescriptor; 5] = [
    param("mode", 0.0, 2.0, 2.0, ParamCurve::Discrete),
    param("drive", 0.0, 36.0, 6.0, ParamCurve::Linear),
    param("tone", 200.0, 20_000.0, 20_000.0, ParamCurve::Logarithmic),
    param("mix", 0.0, 1.0, 1.0, ParamCurve::Linear),
    param("output", 0.0, 2.0, 1.0, ParamCurve::Linear),
];

static DELAY_PARAMS: [ParamDescriptor; 6] = [
    param("time_ms", 1.0, 4_000.0, 375.0, ParamCurve::Linear),
    param("sync", 0.0, 6.0, 0.0, ParamCurve::Discrete),
    param("feedback", 0.0, 0.95, 0.35, ParamCurve::Linear),
    param("mix", 0.0, 1.0, 0.3, ParamCurve::Linear),
    param("ping_pong", 0.0, 1.0, 0.0, ParamCurve::Discrete),
    param("damping", 200.0, 20_000.0, 8_000.0, ParamCurve::Logarithmic),
];

static REVERB_PARAMS: [ParamDescriptor; 6] = [
    param("size", 0.0, 1.0, 0.5, ParamCurve::Linear),
    param("decay", 0.0, 1.0, 0.5, ParamCurve::Linear),
    param("damping", 0.0, 1.0, 0.5, ParamCurve::Linear),
    param("predelay_ms", 0.0, MAX_PREDELAY_MS, 0.0, ParamCurve::Linear),
    param("mix", 0.0, 1.0, 0.25, ParamCurve::Linear),
    param("width", 0.0, 1.0, 1.0, ParamCurve::Linear),
];

/// Static descriptor table; index in the slice == `EffectParamId.0`.
/// Descriptor `name` is the persisted snake_case key.
pub fn effect_params(kind: EffectKind) -> &'static [ParamDescriptor] {
    match kind {
        EffectKind::Saturation => &SATURATION_PARAMS,
        EffectKind::Delay => &DELAY_PARAMS,
        EffectKind::Reverb => &REVERB_PARAMS,
    }
}

pub fn find_effect_param(kind: EffectKind, name: &str) -> Option<EffectParamId> {
    effect_params(kind)
        .iter()
        .position(|descriptor| descriptor.name == name)
        .map(|index| EffectParamId(index as u8))
}

fn descriptor(kind: EffectKind, id: EffectParamId) -> Option<&'static ParamDescriptor> {
    effect_params(kind).get(usize::from(id.0))
}

fn check_value(kind: EffectKind, descriptor: &ParamDescriptor, value: f32) -> Result<(), String> {
    if !value.is_finite() || value < descriptor.min || value > descriptor.max {
        return Err(format!(
            "{} parameter `{}` must be within {}..={}, got {value}",
            kind.name(),
            descriptor.name,
            descriptor.min,
            descriptor.max
        ));
    }
    if descriptor.curve == ParamCurve::Discrete && value.fract() != 0.0 {
        return Err(format!(
            "{} parameter `{}` must be a whole number, got {value}",
            kind.name(),
            descriptor.name
        ));
    }
    Ok(())
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectConfig {
    pub kind: EffectKind,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bypassed: bool,
    /// Sparse overrides keyed by descriptor name; missing keys use defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f32>,
}

impl EffectConfig {
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            bypassed: false,
            params: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in &self.params {
            let id = find_effect_param(self.kind, name)
                .ok_or_else(|| format!("unknown {} parameter `{name}`", self.kind.name()))?;
            let descriptor = &effect_params(self.kind)[usize::from(id.0)];
            check_value(self.kind, descriptor, *value)?;
        }
        Ok(())
    }

    /// Configured value, or the descriptor default. Returns 0 for unknown ids.
    pub fn param_value(&self, id: EffectParamId) -> f32 {
        descriptor(self.kind, id).map_or(0.0, |descriptor| {
            self.params
                .get(descriptor.name)
                .copied()
                .unwrap_or(descriptor.default)
        })
    }

    pub fn set_param_value(&mut self, id: EffectParamId, value: f32) -> Result<(), String> {
        let descriptor = descriptor(self.kind, id)
            .ok_or_else(|| format!("{} has no parameter with id {}", self.kind.name(), id.0))?;
        check_value(self.kind, descriptor, value)?;
        self.params.insert(descriptor.name.to_owned(), value);
        Ok(())
    }

    pub fn required_memory_bytes(&self, sample_rate: u32) -> usize {
        let samples = match self.kind {
            EffectKind::Saturation => 0,
            EffectKind::Delay => 2 * delay_buffer_len(sample_rate),
            EffectKind::Reverb => ReverbLayout::new(sample_rate).total_samples(),
        };
        samples * std::mem::size_of::<f32>()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendBusConfig {
    #[serde(default)]
    pub effects: Vec<EffectConfig>,
    /// Linear return gain, 0..=2.
    #[serde(default = "one")]
    pub return_gain: f32,
}

impl Default for SendBusConfig {
    fn default() -> Self {
        Self {
            effects: Vec::new(),
            return_gain: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectsConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub send_buses: Vec<SendBusConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub master: Vec<EffectConfig>,
}

impl EffectsConfig {
    pub fn is_empty(&self) -> bool {
        self.send_buses.is_empty() && self.master.is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.send_buses.len() > MAX_SEND_BUSES {
            return Err(format!(
                "at most {MAX_SEND_BUSES} send buses are supported, got {}",
                self.send_buses.len()
            ));
        }
        for (index, bus) in self.send_buses.iter().enumerate() {
            if bus.effects.len() > MAX_SEND_EFFECTS {
                return Err(format!(
                    "send bus {index} has {} effects; at most {MAX_SEND_EFFECTS} are supported",
                    bus.effects.len()
                ));
            }
            if !bus.return_gain.is_finite() || !(0.0..=2.0).contains(&bus.return_gain) {
                return Err(format!(
                    "send bus {index} return_gain must be within 0..=2, got {}",
                    bus.return_gain
                ));
            }
            for effect in &bus.effects {
                effect
                    .validate()
                    .map_err(|error| format!("send bus {index}: {error}"))?;
            }
        }
        if self.master.len() > MAX_MASTER_INSERTS {
            return Err(format!(
                "at most {MAX_MASTER_INSERTS} master inserts are supported, got {}",
                self.master.len()
            ));
        }
        for effect in &self.master {
            effect
                .validate()
                .map_err(|error| format!("master: {error}"))?;
        }
        Ok(())
    }

    pub fn required_memory_bytes(&self, sample_rate: u32) -> usize {
        self.send_buses
            .iter()
            .flat_map(|bus| bus.effects.iter())
            .chain(self.master.iter())
            .map(|effect| effect.required_memory_bytes(sample_rate))
            .sum()
    }
}

pub fn validate_track_inserts(inserts: &[EffectConfig]) -> Result<(), String> {
    if inserts.len() > MAX_TRACK_INSERTS {
        return Err(format!(
            "at most {MAX_TRACK_INSERTS} track inserts are supported, got {}",
            inserts.len()
        ));
    }
    inserts.iter().try_for_each(EffectConfig::validate)
}

pub fn check_effect_memory(total_bytes: usize, budget: usize) -> Result<(), String> {
    if total_bytes > budget {
        return Err(format!(
            "effects need {total_bytes} bytes, exceeding the {budget} byte budget"
        ));
    }
    Ok(())
}

fn frames(seconds: f32, sample_rate: f32) -> u32 {
    (seconds * sample_rate).round().max(1.0) as u32
}

fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-INPUT_LIMIT, INPUT_LIMIT)
    } else {
        0.0
    }
}

fn finite_or_zero(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

/// One-pole lowpass coefficient; 1.0 (fully open) at the descriptor maximum.
fn one_pole_coeff(hz: f32, open_hz: f32, sample_rate: f32) -> f32 {
    if hz >= open_hz {
        return 1.0;
    }
    let hz = hz.clamp(1.0, sample_rate * 0.49);
    1.0 - (-TAU * hz / sample_rate).exp()
}

fn sanitize_bpm(bpm: f64) -> Option<f64> {
    (bpm.is_finite() && bpm > 0.0).then(|| bpm.clamp(20.0, 999.0))
}

/// Runtime effect: enum dispatch, all buffers allocated in `new`.
#[derive(Debug, Clone)]
pub struct Effect {
    kind: EffectKind,
    sample_rate: f32,
    bpm: f64,
    values: [f32; MAX_EFFECT_PARAMS],
    bypassed: bool,
    /// 1 = effect fully in circuit, 0 = dry.
    bypass_gain: SmoothedParam,
    bypass_frames: u32,
    smooth_frames: u32,
    state: EffectState,
}

#[derive(Debug, Clone)]
enum EffectState {
    Saturation(SaturationState),
    Delay(DelayState),
    Reverb(Box<ReverbState>),
}

impl Effect {
    pub fn new(config: &EffectConfig, sample_rate: u32, bpm: f64) -> Result<Self, String> {
        config.validate()?;
        if sample_rate == 0 {
            return Err("effect sample rate must be positive".to_owned());
        }
        let rate = sample_rate as f32;
        let mut values = [0.0; MAX_EFFECT_PARAMS];
        let count = effect_params(config.kind).len();
        for (index, value) in values.iter_mut().enumerate().take(count) {
            *value = config.param_value(EffectParamId(index as u8));
        }
        let state = match config.kind {
            EffectKind::Saturation => EffectState::Saturation(SaturationState::new(rate)),
            EffectKind::Delay => EffectState::Delay(DelayState::new(sample_rate)),
            EffectKind::Reverb => EffectState::Reverb(Box::new(ReverbState::new(sample_rate))),
        };
        let mut effect = Self {
            kind: config.kind,
            sample_rate: rate,
            bpm: sanitize_bpm(bpm).unwrap_or(DEFAULT_BPM),
            values,
            bypassed: config.bypassed,
            bypass_gain: SmoothedParam::new(if config.bypassed { 0.0 } else { 1.0 }),
            bypass_frames: frames(BYPASS_CROSSFADE_SECONDS, rate),
            smooth_frames: frames(PARAM_SMOOTH_SECONDS, rate),
            state,
        };
        effect.apply_all(true);
        Ok(effect)
    }

    pub fn kind(&self) -> EffectKind {
        self.kind
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (left, right) = (sanitize(left), sanitize(right));
        let gain = self.bypass_gain.next_value();
        if self.bypassed && gain <= 0.0 {
            return (left, right);
        }
        let (wet_left, wet_right) = match &mut self.state {
            EffectState::Saturation(state) => state.process(left, right),
            EffectState::Delay(state) => state.process(left, right),
            EffectState::Reverb(state) => state.process(left, right),
        };
        (
            finite_or_zero(left * (1.0 - gain) + finite_or_zero(wet_left) * gain),
            finite_or_zero(right * (1.0 - gain) + finite_or_zero(wet_right) * gain),
        )
    }

    /// Sets a parameter in descriptor units, clamped to its range (discrete
    /// parameters round to whole steps). Continuous parameters are smoothed.
    pub fn set_param(&mut self, id: EffectParamId, value: f32) -> bool {
        let Some(descriptor) = descriptor(self.kind, id) else {
            return false;
        };
        if !value.is_finite() {
            return false;
        }
        let mut value = value.clamp(descriptor.min, descriptor.max);
        if descriptor.curve == ParamCurve::Discrete {
            value = value.round();
        }
        let index = usize::from(id.0);
        self.values[index] = value;
        self.apply(index, false);
        true
    }

    pub fn param(&self, id: EffectParamId) -> Option<f32> {
        descriptor(self.kind, id).map(|_| self.values[usize::from(id.0)])
    }

    pub fn set_bypassed(&mut self, bypassed: bool) {
        if bypassed != self.bypassed {
            self.bypassed = bypassed;
            let target = if bypassed { 0.0 } else { 1.0 };
            self.bypass_gain.set_target(target, self.bypass_frames);
        }
    }

    pub fn is_bypassed(&self) -> bool {
        self.bypassed
    }

    /// Re-derives the tempo-synced delay time; ignored for invalid tempos.
    pub fn set_tempo(&mut self, bpm: f64) {
        let Some(bpm) = sanitize_bpm(bpm) else {
            return;
        };
        self.bpm = bpm;
        if self.kind == EffectKind::Delay {
            self.apply(dly::TIME_MS, false);
        }
    }

    /// Clears delay lines and filter state and snaps smoothed values to targets.
    pub fn reset(&mut self) {
        match &mut self.state {
            EffectState::Saturation(state) => state.lowpass = [0.0; 2],
            EffectState::Delay(state) => state.clear(),
            EffectState::Reverb(state) => state.clear(),
        }
        self.bypass_gain
            .set_target(if self.bypassed { 0.0 } else { 1.0 }, 0);
        self.apply_all(true);
    }

    fn apply_all(&mut self, snap: bool) {
        for index in 0..effect_params(self.kind).len() {
            self.apply(index, snap);
        }
    }

    /// Pushes `values[index]` into the DSP state.
    fn apply(&mut self, index: usize, snap: bool) {
        let smooth = if snap { 0 } else { self.smooth_frames };
        let value = self.values[index];
        match &mut self.state {
            EffectState::Saturation(state) => match index {
                sat::MODE => state.mode = value as u8,
                sat::DRIVE => state.drive.set_target(10f32.powf(value / 20.0), smooth),
                sat::TONE => state.tone_hz.set_target(value, smooth),
                sat::MIX => state.mix.set_target(value, smooth),
                sat::OUTPUT => state.output.set_target(value, smooth),
                _ => {}
            },
            EffectState::Delay(state) => match index {
                dly::TIME_MS | dly::SYNC => {
                    let samples = delay_samples(
                        self.values[dly::TIME_MS],
                        self.values[dly::SYNC],
                        self.bpm,
                        self.sample_rate,
                        state.max_delay,
                    );
                    let glide = if snap { 0 } else { state.glide_frames };
                    state.delay.set_target(samples, glide);
                }
                dly::FEEDBACK => state.feedback.set_target(value, smooth),
                dly::MIX => state.mix.set_target(value, smooth),
                dly::PING_PONG => state.ping_pong = value >= 0.5,
                dly::DAMPING => state.damping_hz.set_target(value, smooth),
                _ => {}
            },
            EffectState::Reverb(state) => match index {
                rev::SIZE => state.size.set_target(value, smooth),
                rev::DECAY => state.feedback.set_target(0.7 + 0.27 * value, smooth),
                rev::DAMPING => state.damping.set_target(0.4 * value, smooth),
                rev::PREDELAY_MS => {
                    let samples = (value * self.sample_rate / 1000.0).round();
                    state
                        .predelay
                        .set_target(samples.min(state.max_predelay), smooth);
                }
                rev::MIX => state.mix.set_target(value, smooth),
                rev::WIDTH => state.width.set_target(value, smooth),
                _ => {}
            },
        }
    }
}

#[derive(Debug, Clone)]
struct SaturationState {
    /// 0 soft clip, 1 hard clip, 2 tanh.
    mode: u8,
    /// Linear drive gain.
    drive: SmoothedParam,
    tone_hz: SmoothedParam,
    tone_coeff: f32,
    tone_coeff_hz: f32,
    lowpass: [f32; 2],
    mix: SmoothedParam,
    output: SmoothedParam,
    sample_rate: f32,
}

impl SaturationState {
    fn new(sample_rate: f32) -> Self {
        Self {
            mode: 2,
            drive: SmoothedParam::new(1.0),
            tone_hz: SmoothedParam::new(SATURATION_PARAMS[sat::TONE].max),
            tone_coeff: 1.0,
            tone_coeff_hz: f32::NAN,
            lowpass: [0.0; 2],
            mix: SmoothedParam::new(1.0),
            output: SmoothedParam::new(1.0),
            sample_rate,
        }
    }

    fn shape(&self, x: f32) -> f32 {
        match self.mode {
            0 => {
                // Cubic soft clip normalized to a unit ceiling.
                let x = x.clamp(-1.0, 1.0);
                1.5 * (x - x * x * x / 3.0)
            }
            1 => x.clamp(-1.0, 1.0),
            _ => x.tanh(),
        }
    }

    fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let drive = self.drive.next_value();
        let tone = self.tone_hz.next_value();
        if tone != self.tone_coeff_hz {
            self.tone_coeff_hz = tone;
            self.tone_coeff =
                one_pole_coeff(tone, SATURATION_PARAMS[sat::TONE].max, self.sample_rate);
        }
        let mix = self.mix.next_value();
        let output = self.output.next_value();
        let mut out = [left, right];
        for (channel, sample) in out.iter_mut().enumerate() {
            let shaped = self.shape(*sample * drive);
            let state = &mut self.lowpass[channel];
            *state = if self.tone_coeff >= 1.0 {
                shaped
            } else {
                *state + self.tone_coeff * (shaped - *state)
            };
            *sample = (*sample * (1.0 - mix) + *state * mix) * output;
        }
        (out[0], out[1])
    }
}

fn delay_max_samples(sample_rate: u32) -> usize {
    (MAX_DELAY_SECONDS * sample_rate as f32).round() as usize
}

/// Two guard samples: one for the interpolation neighbour, one so the
/// maximum delay never reads the slot being written.
fn delay_buffer_len(sample_rate: u32) -> usize {
    delay_max_samples(sample_rate) + 2
}

fn sync_beats(sync: f32) -> Option<f64> {
    match sync.round() as i32 {
        1 => Some(1.0),
        2 => Some(0.5),
        3 => Some(0.75),
        4 => Some(0.25),
        5 => Some(1.0 / 3.0),
        6 => Some(2.0),
        _ => None,
    }
}

/// Target delay in whole samples: `round(seconds * sample_rate)`.
fn delay_samples(time_ms: f32, sync: f32, bpm: f64, sample_rate: f32, max: f32) -> f32 {
    let seconds = match sync_beats(sync) {
        Some(beats) => beats * 60.0 / bpm,
        None => f64::from(time_ms) / 1000.0,
    };
    ((seconds * f64::from(sample_rate)).round() as f32).clamp(1.0, max)
}

/// Stereo delay. The steady-state delay is always a whole number of samples
/// (read without interpolation, so integer delays are bit-exact); linear
/// interpolation is only used while the read position glides to a new time.
#[derive(Debug, Clone)]
struct DelayState {
    buffers: [Vec<f32>; 2],
    write: usize,
    max_delay: f32,
    delay: SmoothedParam,
    glide_frames: u32,
    feedback: SmoothedParam,
    mix: SmoothedParam,
    ping_pong: bool,
    damping_hz: SmoothedParam,
    damping_coeff: f32,
    damping_coeff_hz: f32,
    lowpass: [f32; 2],
    sample_rate: f32,
}

impl DelayState {
    fn new(sample_rate: u32) -> Self {
        let len = delay_buffer_len(sample_rate);
        let rate = sample_rate as f32;
        Self {
            buffers: [vec![0.0; len], vec![0.0; len]],
            write: 0,
            max_delay: delay_max_samples(sample_rate).max(1) as f32,
            delay: SmoothedParam::new(1.0),
            glide_frames: frames(DELAY_GLIDE_SECONDS, rate),
            feedback: SmoothedParam::new(0.0),
            mix: SmoothedParam::new(0.0),
            ping_pong: false,
            damping_hz: SmoothedParam::new(DELAY_PARAMS[dly::DAMPING].max),
            damping_coeff: 1.0,
            damping_coeff_hz: f32::NAN,
            lowpass: [0.0; 2],
            sample_rate: rate,
        }
    }

    fn clear(&mut self) {
        for buffer in &mut self.buffers {
            buffer.fill(0.0);
        }
        self.lowpass = [0.0; 2];
        self.write = 0;
    }

    fn read(&self, channel: usize, whole: usize, frac: f32) -> f32 {
        let buffer = &self.buffers[channel];
        let len = buffer.len();
        let first = buffer[(self.write + len - whole) % len];
        if frac == 0.0 {
            return first;
        }
        let second = buffer[(self.write + len - whole - 1) % len];
        first + (second - first) * frac
    }

    fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let delay = self.delay.next_value().clamp(1.0, self.max_delay);
        let whole = delay.floor();
        let frac = delay - whole;
        let whole = whole as usize;
        let feedback = self.feedback.next_value();
        let mix = self.mix.next_value();
        let damping = self.damping_hz.next_value();
        if damping != self.damping_coeff_hz {
            self.damping_coeff_hz = damping;
            self.damping_coeff =
                one_pole_coeff(damping, DELAY_PARAMS[dly::DAMPING].max, self.sample_rate);
        }

        let wet = [self.read(0, whole, frac), self.read(1, whole, frac)];
        let mut damped = [0.0; 2];
        for (channel, value) in damped.iter_mut().enumerate() {
            let state = &mut self.lowpass[channel];
            *state = if self.damping_coeff >= 1.0 {
                wet[channel]
            } else {
                *state + self.damping_coeff * (wet[channel] - *state)
            };
            *state = finite_or_zero(*state);
            *value = *state * feedback;
        }
        let (write_left, write_right) = if self.ping_pong {
            ((left + right) * 0.5 + damped[1], damped[0])
        } else {
            (left + damped[0], right + damped[1])
        };
        let write = self.write;
        self.buffers[0][write] = finite_or_zero(write_left);
        self.buffers[1][write] = finite_or_zero(write_right);
        self.write = (write + 1) % self.buffers[0].len();

        (
            left * (1.0 - mix) + wet[0] * mix,
            right * (1.0 - mix) + wet[1] * mix,
        )
    }
}

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const TUNING_RATE: f32 = 44_100.0;
const REVERB_INPUT_GAIN: f32 = 0.015;
const REVERB_WET_GAIN: f32 = 3.0;
const ALLPASS_FEEDBACK: f32 = 0.5;
/// Comb lengths scale by `0.5 + size`, so buffers hold 1.5x the tuning.
const MAX_SIZE_SCALE: f32 = 1.5;

/// Buffer lengths shared by allocation and memory accounting.
struct ReverbLayout {
    /// Comb lengths at size scale 1.0, per channel.
    comb_base: [[usize; 8]; 2],
    allpass: [[usize; 4]; 2],
    predelay: usize,
}

impl ReverbLayout {
    fn new(sample_rate: u32) -> Self {
        let ratio = sample_rate as f32 / TUNING_RATE;
        let scale = |tuning: usize| ((tuning as f32 * ratio).round() as usize).max(1);
        let mut comb_base = [[0; 8]; 2];
        let mut allpass = [[0; 4]; 2];
        for (channel, spread) in [0, STEREO_SPREAD].into_iter().enumerate() {
            for (slot, tuning) in COMB_TUNING.iter().enumerate() {
                comb_base[channel][slot] = scale(tuning + spread);
            }
            for (slot, tuning) in ALLPASS_TUNING.iter().enumerate() {
                allpass[channel][slot] = scale(tuning + spread);
            }
        }
        let predelay = (MAX_PREDELAY_MS * sample_rate as f32 / 1000.0).round() as usize + 1;
        Self {
            comb_base,
            allpass,
            predelay,
        }
    }

    fn comb_capacity(base: usize) -> usize {
        (base as f32 * MAX_SIZE_SCALE).ceil() as usize + 1
    }

    fn total_samples(&self) -> usize {
        let combs: usize = self
            .comb_base
            .iter()
            .flatten()
            .map(|base| Self::comb_capacity(*base))
            .sum();
        let allpasses: usize = self.allpass.iter().flatten().sum();
        combs + allpasses + self.predelay
    }
}

#[derive(Debug, Clone)]
struct Comb {
    buffer: Vec<f32>,
    base: f32,
    write: usize,
    filter: f32,
}

impl Comb {
    fn new(base: usize) -> Self {
        Self {
            buffer: vec![0.0; ReverbLayout::comb_capacity(base)],
            base: base as f32,
            write: 0,
            filter: 0.0,
        }
    }

    fn process(&mut self, input: f32, scale: f32, feedback: f32, damping: f32) -> f32 {
        let capacity = self.buffer.len();
        let len = ((self.base * scale).round() as usize).clamp(1, capacity);
        let output = self.buffer[(self.write + capacity - len) % capacity];
        let filter = output * (1.0 - damping) + self.filter * damping;
        self.filter = if filter.abs() < 1.0e-20 { 0.0 } else { filter };
        self.buffer[self.write] = finite_or_zero(input + self.filter * feedback);
        self.write = (self.write + 1) % capacity;
        output
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
        self.filter = 0.0;
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.index];
        let stored = input + delayed * ALLPASS_FEEDBACK;
        self.buffer[self.index] = if stored.abs() < 1.0e-20 { 0.0 } else { stored };
        self.index = (self.index + 1) % self.buffer.len();
        delayed - input
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
    }
}

/// Freeverb-style reverb: 8 parallel damped combs and 4 series allpasses per
/// channel, with the right channel detuned by a fixed stereo spread.
#[derive(Debug, Clone)]
struct ReverbState {
    combs: [[Comb; 8]; 2],
    allpasses: [[Allpass; 4]; 2],
    predelay_buffer: Vec<f32>,
    predelay_write: usize,
    max_predelay: f32,
    predelay: SmoothedParam,
    size: SmoothedParam,
    feedback: SmoothedParam,
    damping: SmoothedParam,
    mix: SmoothedParam,
    width: SmoothedParam,
}

impl ReverbState {
    fn new(sample_rate: u32) -> Self {
        let layout = ReverbLayout::new(sample_rate);
        Self {
            combs: layout.comb_base.map(|channel| channel.map(Comb::new)),
            allpasses: layout.allpass.map(|channel| channel.map(Allpass::new)),
            predelay_buffer: vec![0.0; layout.predelay],
            predelay_write: 0,
            max_predelay: (layout.predelay - 1) as f32,
            predelay: SmoothedParam::new(0.0),
            size: SmoothedParam::new(0.5),
            feedback: SmoothedParam::new(0.84),
            damping: SmoothedParam::new(0.2),
            mix: SmoothedParam::new(0.0),
            width: SmoothedParam::new(1.0),
        }
    }

    fn clear(&mut self) {
        self.combs.iter_mut().flatten().for_each(Comb::clear);
        self.allpasses.iter_mut().flatten().for_each(Allpass::clear);
        self.predelay_buffer.fill(0.0);
        self.predelay_write = 0;
    }

    fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let scale = 0.5 + self.size.next_value();
        let feedback = self.feedback.next_value();
        let damping = self.damping.next_value();
        let predelay = self
            .predelay
            .next_value()
            .round()
            .clamp(0.0, self.max_predelay) as usize;
        let mix = self.mix.next_value();
        let width = self.width.next_value();

        let len = self.predelay_buffer.len();
        self.predelay_buffer[self.predelay_write] = (left + right) * REVERB_INPUT_GAIN;
        let input = self.predelay_buffer[(self.predelay_write + len - predelay) % len];
        self.predelay_write = (self.predelay_write + 1) % len;

        let mut out = [0.0f32; 2];
        for (channel, value) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for comb in &mut self.combs[channel] {
                sum += comb.process(input, scale, feedback, damping);
            }
            for allpass in &mut self.allpasses[channel] {
                sum = allpass.process(sum);
            }
            *value = finite_or_zero(sum);
        }
        let wet_main = (width * 0.5 + 0.5) * REVERB_WET_GAIN;
        let wet_cross = ((1.0 - width) * 0.5) * REVERB_WET_GAIN;
        let wet_left = out[0] * wet_main + out[1] * wet_cross;
        let wet_right = out[1] * wet_main + out[0] * wet_cross;
        (
            left * (1.0 - mix) + wet_left * mix,
            right * (1.0 - mix) + wet_right * mix,
        )
    }
}

/// Fixed-capacity chain processed in order.
#[derive(Debug, Clone, Default)]
pub struct EffectRack {
    effects: Vec<Effect>,
}

impl EffectRack {
    pub fn new(
        configs: &[EffectConfig],
        max_slots: usize,
        sample_rate: u32,
        bpm: f64,
    ) -> Result<Self, String> {
        if configs.len() > max_slots {
            return Err(format!(
                "effect rack holds at most {max_slots} effects, got {}",
                configs.len()
            ));
        }
        let effects = configs
            .iter()
            .enumerate()
            .map(|(slot, config)| {
                Effect::new(config, sample_rate, bpm)
                    .map_err(|error| format!("effect slot {slot}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { effects })
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn kind(&self, slot: EffectSlotId) -> Option<EffectKind> {
        self.effects.get(usize::from(slot.0)).map(Effect::kind)
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.effects
            .iter_mut()
            .fold((left, right), |(l, r), effect| effect.process(l, r))
    }

    pub fn set_param(&mut self, slot: EffectSlotId, param: EffectParamId, value: f32) -> bool {
        self.effects
            .get_mut(usize::from(slot.0))
            .is_some_and(|effect| effect.set_param(param, value))
    }

    pub fn param(&self, slot: EffectSlotId, param: EffectParamId) -> Option<f32> {
        self.effects
            .get(usize::from(slot.0))
            .and_then(|effect| effect.param(param))
    }

    pub fn set_bypassed(&mut self, slot: EffectSlotId, bypassed: bool) -> bool {
        match self.effects.get_mut(usize::from(slot.0)) {
            Some(effect) => {
                effect.set_bypassed(bypassed);
                true
            }
            None => false,
        }
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.effects
            .iter_mut()
            .for_each(|effect| effect.set_tempo(bpm));
    }

    pub fn reset(&mut self) {
        self.effects.iter_mut().for_each(Effect::reset);
    }
}
