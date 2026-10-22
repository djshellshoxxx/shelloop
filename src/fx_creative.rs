//! Spec 15 — creative effects: tempo-synced multimode filter, stereo
//! chorus/flanger and beat repeat. Spec 17 adds the bitcrusher.
//!
//! Each state type is owned by [`crate::effects::Effect`] (enum dispatch) and
//! follows the same contract as the v1 effects: every buffer is allocated in
//! `new`; `process`, `apply` and `clear` never allocate, lock or panic and
//! always produce finite output. Parameter index constants below match the
//! descriptor tables in `effects.rs` (index == `EffectParamId.0`).

use crate::filter::{clamp_cutoff, FilterMode, FilterParams, StateVariableFilter};
use crate::mixer::SmoothedParam;
use std::f32::consts::TAU;

pub(crate) mod flt {
    pub const MODE: usize = 0;
    pub const CUTOFF: usize = 1;
    pub const RESONANCE: usize = 2;
    pub const ENV_AMOUNT: usize = 3;
    pub const SYNC: usize = 4;
    pub const DEPTH: usize = 5;
    pub const MIX: usize = 6;
}

pub(crate) mod cho {
    pub const MODE: usize = 0;
    pub const RATE_HZ: usize = 1;
    pub const SYNC: usize = 2;
    pub const DEPTH: usize = 3;
    pub const FEEDBACK: usize = 4;
    pub const WIDTH: usize = 5;
    pub const MIX: usize = 6;
}

pub(crate) mod rpt {
    pub const DIVISION: usize = 0;
    pub const REPEATS: usize = 1;
    pub const REVERSE: usize = 2;
    pub const GATE: usize = 3;
    pub const MIX: usize = 4;
    pub const ACTIVE: usize = 5;
    pub const SEED: usize = 6;
}

/// Highest feedback magnitude accepted by the chorus/flanger.
pub const MAX_MOD_FEEDBACK: f32 = 0.9;
/// Highest filter resonance; keeps the SVF damping `k` above 0.19.
pub const MAX_FILTER_RESONANCE: f32 = 0.95;
/// Longest modulated delay line in seconds (flanger/chorus).
const MAX_MOD_DELAY_SECONDS: f32 = 0.035;
const CHORUS_CENTER_SECONDS: f32 = 0.015;
const CHORUS_DEPTH_SECONDS: f32 = 0.008;
const FLANGER_CENTER_SECONDS: f32 = 0.003;
const FLANGER_DEPTH_SECONDS: f32 = 0.0025;
/// Slowest tempo that capture buffers are sized for.
const MIN_BPM: f64 = 20.0;
/// Longest repeat slice: one beat at [`MIN_BPM`].
const MAX_SLICE_BEATS: f64 = 1.0;
const MIN_SLICE_FRAMES: usize = 32;
const FADE_SECONDS: f32 = 0.0015;
const ENGAGE_SECONDS: f32 = 0.003;
const FILTER_ENV_OCTAVES: f32 = 4.0;
const FILTER_LFO_OCTAVES: f32 = 2.0;

fn finite_or_zero(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

fn frames_for(seconds: f32, sample_rate: f32) -> u32 {
    (seconds * sample_rate).round().max(1.0) as u32
}

/// Beats per LFO cycle / repeat slice for the shared 1..=6 sync indices used
/// by the v1 delay: 1/4, 1/8, dotted 1/8, 1/16, 1/8 triplet, 1/2.
fn sync_cycle_beats(sync: f32) -> Option<f64> {
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

/// Beat-repeat slice lengths in beats: 1/4, 1/8, 1/16, 1/32, 1/8 triplet.
fn repeat_division_beats(division: f32) -> f64 {
    match division.round() as i32 {
        0 => 1.0,
        1 => 0.5,
        2 => 0.25,
        3 => 0.125,
        _ => 1.0 / 3.0,
    }
}

// ---------------------------------------------------------------- filter

/// Tempo-synced multimode (LP/BP/HP) filter with an optional envelope
/// follower and tempo-locked sweep.
#[derive(Debug, Clone)]
pub(crate) struct FilterFx {
    filters: [StateVariableFilter; 2],
    sample_rate: f32,
    mode: u8,
    cutoff: SmoothedParam,
    resonance: SmoothedParam,
    env_amount: SmoothedParam,
    depth: SmoothedParam,
    mix: SmoothedParam,
    /// LFO cycles per second derived from `sync` and tempo; 0 = off.
    lfo_hz: f64,
    lfo_phase: f64,
    follower: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl FilterFx {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let params = FilterParams {
            mode: FilterMode::LowPass,
            cutoff_hz: 2_000.0,
            resonance: 0.0,
            key_tracking: 0.0,
        };
        let build = || {
            StateVariableFilter::new(sample_rate, params)
                .expect("filter defaults are valid for a positive sample rate")
        };
        Self {
            filters: [build(), build()],
            sample_rate,
            mode: 0,
            cutoff: SmoothedParam::new(2_000.0),
            resonance: SmoothedParam::new(0.2),
            env_amount: SmoothedParam::new(0.0),
            depth: SmoothedParam::new(0.5),
            mix: SmoothedParam::new(1.0),
            lfo_hz: 0.0,
            lfo_phase: 0.0,
            follower: 0.0,
            attack_coeff: 1.0 - (-1.0 / (0.005 * sample_rate)).exp(),
            release_coeff: 1.0 - (-1.0 / (0.08 * sample_rate)).exp(),
        }
    }

    pub(crate) fn clear(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
        self.lfo_phase = 0.0;
        self.follower = 0.0;
    }

    pub(crate) fn apply(&mut self, index: usize, value: f32, smooth: u32) {
        match index {
            flt::MODE => self.mode = value as u8,
            flt::CUTOFF => self.cutoff.set_target(value, smooth),
            flt::RESONANCE => self.resonance.set_target(value, smooth),
            flt::ENV_AMOUNT => self.env_amount.set_target(value, smooth),
            flt::DEPTH => self.depth.set_target(value, smooth),
            flt::MIX => self.mix.set_target(value, smooth),
            _ => {}
        }
    }

    /// Re-derives the sweep rate from the sync index and tempo.
    pub(crate) fn set_sync(&mut self, sync: f32, bpm: f64) {
        self.lfo_hz = sync_cycle_beats(sync).map_or(0.0, |beats| bpm / 60.0 / beats);
    }

    pub(crate) fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let cutoff = self.cutoff.next_value();
        let resonance = self.resonance.next_value().clamp(0.0, MAX_FILTER_RESONANCE);
        let env_amount = self.env_amount.next_value();
        let depth = self.depth.next_value();
        let mix = self.mix.next_value().clamp(0.0, 1.0);

        let level = left.abs().max(right.abs()).min(8.0);
        let coeff = if level > self.follower {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.follower = finite_or_zero(self.follower + coeff * (level - self.follower));

        let mut octaves = env_amount * FILTER_ENV_OCTAVES * self.follower.min(1.0);
        if self.lfo_hz > 0.0 {
            octaves += depth * FILTER_LFO_OCTAVES * (self.lfo_phase as f32 * TAU).sin();
            self.lfo_phase = (self.lfo_phase + self.lfo_hz / f64::from(self.sample_rate)).fract();
        }
        let effective = clamp_cutoff(cutoff * octaves.clamp(-8.0, 8.0).exp2(), self.sample_rate);

        let mode = match self.mode {
            1 => FilterMode::BandPass,
            2 => FilterMode::HighPass,
            _ => FilterMode::LowPass,
        };
        let mut out = [left, right];
        for (filter, sample) in self.filters.iter_mut().zip(out.iter_mut()) {
            filter.set_mode_and_resonance(mode, resonance);
            filter.set_cutoff_hz(effective);
            let wet = filter.process(*sample);
            *sample = *sample * (1.0 - mix) + wet * mix;
        }
        (out[0], out[1])
    }
}

// -------------------------------------------------------- chorus/flanger

pub(crate) fn chorus_buffer_len(sample_rate: u32) -> usize {
    (MAX_MOD_DELAY_SECONDS * sample_rate as f32).ceil() as usize + 4
}

/// Stereo chorus/flanger: one bounded circular buffer per channel with
/// linearly interpolated fractional reads and independent per-channel LFO
/// phase (`width` spreads the right channel up to half a cycle).
#[derive(Debug, Clone)]
pub(crate) struct ChorusFx {
    buffers: [Vec<f32>; 2],
    write: usize,
    sample_rate: f32,
    flanger: bool,
    rate_hz: f32,
    /// Cycles per second from tempo sync; 0 = use `rate_hz`.
    sync_hz: f64,
    phase: f64,
    depth: SmoothedParam,
    feedback: SmoothedParam,
    width: SmoothedParam,
    mix: SmoothedParam,
}

impl ChorusFx {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let len = chorus_buffer_len(sample_rate);
        Self {
            buffers: [vec![0.0; len], vec![0.0; len]],
            write: 0,
            sample_rate: sample_rate as f32,
            flanger: false,
            rate_hz: 0.8,
            sync_hz: 0.0,
            phase: 0.0,
            depth: SmoothedParam::new(0.5),
            feedback: SmoothedParam::new(0.0),
            width: SmoothedParam::new(1.0),
            mix: SmoothedParam::new(0.5),
        }
    }

    pub(crate) fn clear(&mut self) {
        for buffer in &mut self.buffers {
            buffer.fill(0.0);
        }
        self.write = 0;
        self.phase = 0.0;
    }

    pub(crate) fn apply(&mut self, index: usize, value: f32, smooth: u32) {
        match index {
            cho::MODE => self.flanger = value >= 0.5,
            cho::RATE_HZ => self.rate_hz = value,
            cho::DEPTH => self.depth.set_target(value, smooth),
            cho::FEEDBACK => self
                .feedback
                .set_target(value.clamp(-MAX_MOD_FEEDBACK, MAX_MOD_FEEDBACK), smooth),
            cho::WIDTH => self.width.set_target(value, smooth),
            cho::MIX => self.mix.set_target(value, smooth),
            _ => {}
        }
    }

    pub(crate) fn set_sync(&mut self, sync: f32, bpm: f64) {
        self.sync_hz = sync_cycle_beats(sync).map_or(0.0, |beats| bpm / 60.0 / beats);
    }

    fn read(&self, channel: usize, delay: f32) -> f32 {
        let buffer = &self.buffers[channel];
        let len = buffer.len();
        let delay = delay.clamp(1.0, (len - 3) as f32);
        let whole = delay.floor();
        let frac = delay - whole;
        let whole = whole as usize;
        let first = buffer[(self.write + len - whole) % len];
        let second = buffer[(self.write + len - whole - 1) % len];
        first + (second - first) * frac
    }

    pub(crate) fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let depth = self.depth.next_value();
        let feedback = self.feedback.next_value();
        let width = self.width.next_value();
        let mix = self.mix.next_value().clamp(0.0, 1.0);
        let rate = if self.sync_hz > 0.0 {
            self.sync_hz
        } else {
            f64::from(self.rate_hz)
        };
        let (center, span) = if self.flanger {
            (FLANGER_CENTER_SECONDS, FLANGER_DEPTH_SECONDS)
        } else {
            (CHORUS_CENTER_SECONDS, CHORUS_DEPTH_SECONDS)
        };
        let phases = [self.phase as f32, (self.phase as f32 + 0.5 * width).fract()];
        self.phase = (self.phase + rate / f64::from(self.sample_rate)).fract();

        let input = [left, right];
        let mut wet = [0.0_f32; 2];
        for channel in 0..2 {
            let lfo = (phases[channel] * TAU).sin();
            let seconds = center + span * depth * lfo;
            wet[channel] = finite_or_zero(self.read(channel, seconds * self.sample_rate));
        }
        let write = self.write;
        for channel in 0..2 {
            self.buffers[channel][write] = finite_or_zero(input[channel] + wet[channel] * feedback);
        }
        self.write = (write + 1) % self.buffers[0].len();

        (
            input[0] * (1.0 - mix) + wet[0] * mix,
            input[1] * (1.0 - mix) + wet[1] * mix,
        )
    }
}

// ----------------------------------------------------------- beat repeat

/// Longest slice (frames) the repeat buffers are sized for.
pub(crate) fn beat_repeat_max_frames(sample_rate: u32) -> usize {
    (MAX_SLICE_BEATS * 60.0 / MIN_BPM * f64::from(sample_rate)).ceil() as usize + 1
}

/// Quantized beat repeat / stutter.
///
/// The input is continuously written into a ring buffer. When `active` rises
/// the effect arms and, at the next transport slice boundary (or its local
/// grid for legacy callers) *and only once a full slice has been captured*, snapshots the last slice into the hold buffer and loops it.
/// Playback therefore never reads uninitialised or stale audio. `reset`
/// (stop, panic, project switch) clears every buffer and the capture count.
#[derive(Debug, Clone)]
pub(crate) struct BeatRepeatFx {
    ring: [Vec<f32>; 2],
    hold: [Vec<f32>; 2],
    ring_write: usize,
    filled: usize,
    frame: u64,
    last_transport_frame: Option<u64>,
    sample_rate: f32,
    bpm: f64,
    division: f32,
    slice_len: usize,
    repeats: u32,
    reverse: bool,
    gate: f32,
    seed: u32,
    mix: SmoothedParam,
    engage: SmoothedParam,
    engage_frames: u32,
    fade_frames: usize,
    active: bool,
    armed: bool,
    playing: bool,
    pos: usize,
    passes_done: u32,
    pass_muted: bool,
    rng: u32,
}

impl BeatRepeatFx {
    pub(crate) fn new(sample_rate: u32, bpm: f64) -> Self {
        let max = beat_repeat_max_frames(sample_rate);
        let rate = sample_rate as f32;
        let mut fx = Self {
            ring: [vec![0.0; max], vec![0.0; max]],
            hold: [vec![0.0; max], vec![0.0; max]],
            ring_write: 0,
            filled: 0,
            frame: 0,
            last_transport_frame: None,
            sample_rate: rate,
            bpm,
            division: 1.0,
            slice_len: MIN_SLICE_FRAMES,
            repeats: 0,
            reverse: false,
            gate: 0.0,
            seed: 1,
            mix: SmoothedParam::new(1.0),
            engage: SmoothedParam::new(0.0),
            engage_frames: frames_for(ENGAGE_SECONDS, rate),
            fade_frames: frames_for(FADE_SECONDS, rate) as usize,
            active: false,
            armed: false,
            playing: false,
            pos: 0,
            passes_done: 0,
            pass_muted: false,
            rng: 1,
        };
        fx.update_slice();
        fx
    }

    pub(crate) fn clear(&mut self) {
        for buffer in self.ring.iter_mut().chain(self.hold.iter_mut()) {
            buffer.fill(0.0);
        }
        self.ring_write = 0;
        self.filled = 0;
        self.frame = 0;
        self.last_transport_frame = None;
        self.armed = self.active;
        self.playing = false;
        self.pos = 0;
        self.passes_done = 0;
        self.pass_muted = false;
        self.engage.set_target(0.0, 0);
    }

    pub(crate) fn set_tempo(&mut self, bpm: f64) {
        self.bpm = bpm;
        self.update_slice();
    }

    fn update_slice(&mut self) {
        let max = self.ring[0].len();
        let beats = repeat_division_beats(self.division);
        let frames = (beats * 60.0 / self.bpm * f64::from(self.sample_rate)).round() as usize;
        self.slice_len = frames.clamp(MIN_SLICE_FRAMES, max);
    }

    pub(crate) fn apply(&mut self, index: usize, value: f32, smooth: u32) {
        match index {
            rpt::DIVISION => {
                self.division = value;
                self.update_slice();
            }
            rpt::REPEATS => self.repeats = value as u32,
            rpt::REVERSE => self.reverse = value >= 0.5,
            rpt::GATE => self.gate = value.clamp(0.0, 1.0),
            rpt::MIX => self.mix.set_target(value, smooth),
            rpt::SEED => self.seed = value as u32,
            rpt::ACTIVE => {
                let active = value >= 0.5;
                if active && !self.active {
                    self.armed = true;
                }
                if !active {
                    self.armed = false;
                    self.stop_playback();
                }
                self.active = active;
            }
            _ => {}
        }
    }

    fn stop_playback(&mut self) {
        self.playing = false;
        self.pos = 0;
        self.passes_done = 0;
        self.engage.set_target(0.0, self.engage_frames);
    }

    fn next_random(&mut self) -> f32 {
        // xorshift32: deterministic for a fixed seed and transport.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    fn begin_pass(&mut self) {
        self.pos = 0;
        self.pass_muted = self.gate > 0.0 && self.next_random() < self.gate;
    }

    fn start_playback(&mut self) {
        let len = self.slice_len;
        let cap = self.ring[0].len();
        for channel in 0..2 {
            for i in 0..len {
                let index = (self.ring_write + cap - len + i) % cap;
                self.hold[channel][i] = self.ring[channel][index];
            }
        }
        self.rng = self.seed.wrapping_mul(2_654_435_761).max(1);
        self.passes_done = 0;
        self.playing = true;
        self.armed = false;
        self.begin_pass();
        self.engage.set_target(1.0, self.engage_frames);
    }

    /// Use the project grid rather than time since this effect was inserted.
    /// Pauses and seeks invalidate capture without clearing/allocating buffers:
    /// `filled` prevents any stale data from being read on reactivation.
    pub(crate) fn process_with_transport(
        &mut self,
        left: f32,
        right: f32,
        frame: u64,
        playing: bool,
    ) -> (f32, f32) {
        let discontinuity = self
            .last_transport_frame
            .is_some_and(|previous| previous.checked_add(1) != Some(frame));
        self.last_transport_frame = Some(frame);
        if !playing || discontinuity {
            self.filled = 0;
            self.playing = false;
            self.armed = self.active;
            self.pos = 0;
            self.engage.set_target(0.0, 0);
        }
        self.frame = frame;
        if !playing {
            return (left, right);
        }
        // Round each absolute musical boundary, rather than multiplying a
        // rounded slice duration (which accumulates drift at fractional BPM).
        let period =
            repeat_division_beats(self.division) * 60.0 / self.bpm * f64::from(self.sample_rate);
        let boundary = ((frame as f64 / period).round() * period).round() as u64;
        self.process_on_boundary(left, right, boundary == frame)
    }

    pub(crate) fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_on_boundary(
            left,
            right,
            self.frame.is_multiple_of(self.slice_len as u64),
        )
    }

    fn process_on_boundary(&mut self, left: f32, right: f32, boundary: bool) -> (f32, f32) {
        let mix = self.mix.next_value().clamp(0.0, 1.0);

        if self.armed && boundary && self.filled >= self.slice_len {
            self.start_playback();
        }

        let mut wet = [0.0_f32; 2];
        if self.playing {
            let len = self.slice_len;
            let index = if self.reverse {
                len - 1 - self.pos
            } else {
                self.pos
            };
            let fade = self.fade_frames.min(len / 2).max(1);
            let edge = self.pos.min(len - 1 - self.pos);
            let envelope = if edge < fade {
                (edge + 1) as f32 / (fade + 1) as f32
            } else {
                1.0
            };
            let gain = if self.pass_muted { 0.0 } else { envelope };
            wet = [self.hold[0][index] * gain, self.hold[1][index] * gain];
            self.pos += 1;
            if self.pos >= len {
                self.passes_done += 1;
                if self.repeats != 0 && self.passes_done >= self.repeats {
                    self.stop_playback();
                    self.active = false;
                } else {
                    self.begin_pass();
                }
            }
        }

        let engage = self.engage.next_value().clamp(0.0, 1.0);
        let amount = mix * engage;
        let input = [left, right];
        let cap = self.ring[0].len();
        self.ring[0][self.ring_write] = finite_or_zero(left);
        self.ring[1][self.ring_write] = finite_or_zero(right);
        self.ring_write = (self.ring_write + 1) % cap;
        self.filled = (self.filled + 1).min(cap);
        self.frame = self.frame.wrapping_add(1);

        (
            input[0] * (1.0 - amount) + wet[0] * amount,
            input[1] * (1.0 - amount) + wet[1] * amount,
        )
    }
}

// -------------------------------------------------------------- bitcrusher

pub(crate) mod bcr {
    pub const BITS: usize = 0;
    pub const DOWNSAMPLE: usize = 1;
    pub const MIX: usize = 2;
}

/// Spec 17: sample-and-hold rate reduction and signed bit-depth quantiser.
/// No buffers: the held frame is two `f32` values.
#[derive(Debug, Clone)]
pub(crate) struct BitCrushFx {
    /// Quantiser levels per unit: `2^(bits-1)`.
    levels: f32,
    /// Hold length in frames (>= 1).
    hold: u32,
    counter: u32,
    held: [f32; 2],
    mix: SmoothedParam,
}

impl BitCrushFx {
    pub(crate) fn new() -> Self {
        Self {
            levels: 128.0,
            hold: 1,
            counter: 0,
            held: [0.0; 2],
            mix: SmoothedParam::new(1.0),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.counter = 0;
        self.held = [0.0; 2];
    }

    pub(crate) fn apply(&mut self, index: usize, value: f32, smooth: u32) {
        match index {
            bcr::BITS => self.levels = 2f32.powf(value.round().clamp(1.0, 16.0) - 1.0),
            bcr::DOWNSAMPLE => self.hold = value.round().clamp(1.0, 32.0) as u32,
            bcr::MIX => self.mix.set_target(value, smooth),
            _ => {}
        }
    }

    pub(crate) fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mix = self.mix.next_value().clamp(0.0, 1.0);
        if self.counter == 0 {
            let levels = self.levels;
            self.held = [
                (left * levels).round() / levels,
                (right * levels).round() / levels,
            ];
        }
        self.counter += 1;
        if self.counter >= self.hold {
            self.counter = 0;
        }
        let wet = self.held;
        (
            left * (1.0 - mix) + finite_or_zero(wet[0]) * mix,
            right * (1.0 - mix) + finite_or_zero(wet[1]) * mix,
        )
    }
}
