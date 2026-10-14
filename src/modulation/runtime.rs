//! Audio-thread modulation state. Everything here is fixed-size, `Copy`
//! and allocation-free.

use super::compile::CompiledModulation;
use super::model::{LfoWave, ModDest, ModSource, MAX_LFOS, MAX_ROUTES};

/// Samples between modulation evaluations. Offsets are linearly
/// interpolated across the interval.
pub const MOD_CONTROL_INTERVAL: usize = 16;

const TUNING_SEMITONES_PER_UNIT: f32 = 12.0;
const MAX_TUNING_SEMITONES: f32 = 24.0;
const PULSE_WIDTH_PER_UNIT: f32 = 0.45;
const CUTOFF_OCTAVES_PER_UNIT: f32 = 4.0;
const MAX_CUTOFF_OCTAVES: f32 = 8.0;

/// Per-sample voice state feeding the sources.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModInputs {
    pub velocity: f32,
    pub amp_env: f32,
    pub filter_env: f32,
    pub mod_wheel: f32,
}

/// Summed, bounded destination offsets in destination units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModOffsets {
    pub tuning_semitones: f32,
    pub pulse_width: f32,
    pub cutoff_octaves: f32,
    pub resonance: f32,
    /// Added to a gain of 1.0; the voice clamps the result to 0..=2.
    pub amplitude: f32,
    pub pan: f32,
    pub effect_wet: f32,
}

impl ModOffsets {
    fn from_array(values: [f32; ModDest::COUNT]) -> Self {
        Self {
            tuning_semitones: values[ModDest::Tuning.index()],
            pulse_width: values[ModDest::PulseWidth.index()],
            cutoff_octaves: values[ModDest::FilterCutoff.index()],
            resonance: values[ModDest::FilterResonance.index()],
            amplitude: values[ModDest::Amplitude.index()],
            pan: values[ModDest::Pan.index()],
            effect_wet: values[ModDest::EffectWet.index()],
        }
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// LFO output in -1.0..=1.0 at `cycles` (phase = fractional part).
pub fn lfo_value(wave: LfoWave, cycles: f64, seed: u32) -> f32 {
    let phase = cycles.fract() as f32;
    match wave {
        LfoWave::Sine => (phase * std::f32::consts::TAU).sin(),
        LfoWave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        LfoWave::Saw => 2.0 * phase - 1.0,
        LfoWave::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoWave::SampleHold => {
            let cycle = cycles.floor() as i64 as u64;
            let hash = splitmix64(u64::from(seed) ^ splitmix64(cycle));
            (hash >> 40) as f32 / 8_388_608.0 - 1.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModRuntime {
    compiled: CompiledModulation,
    sample_rate: f32,
    bpm: f64,
    lfo_cycles: [f64; MAX_LFOS],
    /// Note-on velocity and key tracking are latched per voice.
    key_track: f32,
    smoothed: [f32; MAX_ROUTES],
    current: [f32; ModDest::COUNT],
    step: [f32; ModDest::COUNT],
    counter: usize,
    snap: bool,
}

impl ModRuntime {
    pub fn new(sample_rate: f32, bpm: f64, compiled: CompiledModulation) -> Self {
        Self {
            compiled,
            sample_rate,
            bpm: sanitize_bpm(bpm),
            lfo_cycles: [0.0; MAX_LFOS],
            key_track: 0.0,
            smoothed: [0.0; MAX_ROUTES],
            current: [0.0; ModDest::COUNT],
            step: [0.0; ModDest::COUNT],
            counter: 0,
            snap: true,
        }
    }

    pub fn compiled(&self) -> &CompiledModulation {
        &self.compiled
    }

    pub fn is_active(&self) -> bool {
        self.compiled.is_active()
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.bpm = sanitize_bpm(bpm);
    }

    /// Swaps the compiled routes without restarting LFO phase or
    /// interrupting interpolation; offsets glide to the new targets.
    pub fn apply(&mut self, compiled: CompiledModulation) {
        self.compiled = compiled;
    }

    /// `free_cycles` is the shared free-running phase used by LFOs whose
    /// `retrigger` flag is off.
    pub fn note_on(&mut self, note: u8, free_cycles: [f64; MAX_LFOS]) {
        for (index, cycles) in self.lfo_cycles.iter_mut().enumerate() {
            *cycles = if self.compiled.lfos[index].retrigger {
                0.0
            } else {
                free_cycles[index]
            };
        }
        self.key_track = ((f32::from(note) - 60.0) / 60.0).clamp(-1.0, 1.0);
        self.smoothed = [0.0; MAX_ROUTES];
        self.current = [0.0; ModDest::COUNT];
        self.step = [0.0; ModDest::COUNT];
        self.counter = 0;
        self.snap = true;
    }

    /// Advances one sample and returns the interpolated offsets.
    pub fn next(&mut self, inputs: &ModInputs) -> ModOffsets {
        if self.counter == 0 {
            self.tick(inputs);
        }
        self.counter = (self.counter + 1) % MOD_CONTROL_INTERVAL;
        for (value, step) in self.current.iter_mut().zip(self.step) {
            *value += step;
        }
        ModOffsets::from_array(self.current)
    }

    fn source_value(&self, source: ModSource, lfos: &[f32; MAX_LFOS], inputs: &ModInputs) -> f32 {
        match source {
            ModSource::Lfo1 => lfos[0],
            ModSource::Lfo2 => lfos[1],
            ModSource::AmpEnv => inputs.amp_env,
            ModSource::FilterEnv => inputs.filter_env,
            ModSource::Velocity => inputs.velocity,
            ModSource::KeyTrack => self.key_track,
            ModSource::ModWheel => inputs.mod_wheel,
            ModSource::Macro1 => self.compiled.macros[0],
            ModSource::Macro2 => self.compiled.macros[1],
        }
    }

    fn tick(&mut self, inputs: &ModInputs) {
        let mut lfos = [0.0_f32; MAX_LFOS];
        for (index, value) in lfos.iter_mut().enumerate() {
            let lfo = self.compiled.lfos[index];
            *value = lfo_value(lfo.wave, self.lfo_cycles[index], lfo.seed);
            let hz = self.compiled.lfo_hz(index, self.bpm);
            self.lfo_cycles[index] +=
                hz * MOD_CONTROL_INTERVAL as f64 / f64::from(self.sample_rate);
        }

        let mut target = [0.0_f32; ModDest::COUNT];
        for slot in 0..self.compiled.route_count {
            let route = self.compiled.routes[slot];
            let raw = self.source_value(route.source, &lfos, inputs) * route.amount;
            let smoothed = &mut self.smoothed[slot];
            *smoothed = if self.snap || route.smooth_coeff >= 1.0 {
                raw
            } else {
                *smoothed + route.smooth_coeff * (raw - *smoothed)
            };
            target[route.dest.index()] += *smoothed;
        }

        target[ModDest::Tuning.index()] = (target[ModDest::Tuning.index()]
            * TUNING_SEMITONES_PER_UNIT)
            .clamp(-MAX_TUNING_SEMITONES, MAX_TUNING_SEMITONES);
        target[ModDest::PulseWidth.index()] =
            (target[ModDest::PulseWidth.index()] * PULSE_WIDTH_PER_UNIT).clamp(-0.9, 0.9);
        target[ModDest::FilterCutoff.index()] = (target[ModDest::FilterCutoff.index()]
            * CUTOFF_OCTAVES_PER_UNIT)
            .clamp(-MAX_CUTOFF_OCTAVES, MAX_CUTOFF_OCTAVES);
        target[ModDest::FilterResonance.index()] =
            target[ModDest::FilterResonance.index()].clamp(-1.0, 1.0);
        target[ModDest::Amplitude.index()] = target[ModDest::Amplitude.index()].clamp(-1.0, 1.0);
        target[ModDest::Pan.index()] = target[ModDest::Pan.index()].clamp(-1.0, 1.0);
        target[ModDest::EffectWet.index()] = target[ModDest::EffectWet.index()].clamp(-1.0, 1.0);

        let snap = self.snap;
        for ((current, step), target) in self.current.iter_mut().zip(&mut self.step).zip(target) {
            if snap {
                *current = target;
                *step = 0.0;
            } else {
                *step = (target - *current) / MOD_CONTROL_INTERVAL as f32;
            }
        }
        self.snap = false;
    }
}

fn sanitize_bpm(bpm: f64) -> f64 {
    if bpm.is_finite() && bpm > 0.0 {
        bpm.clamp(20.0, 999.0)
    } else {
        120.0
    }
}
