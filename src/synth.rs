use crate::modulation::{
    CompiledModulation, LfoWave, ModInputs, ModOffsets, ModRuntime, ModulationConfig, SyncDivision,
    MAX_LFOS,
};
use crate::{
    clamp_cutoff, AdsrEnvelope, AdsrParams, FilterMode, FilterParams, SmoothedParam,
    StateVariableFilter,
};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Oscillator {
    Sine,
    Triangle,
    Saw,
    Pulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthParamId {
    Oscillator,
    Octave,
    Semitone,
    FineCents,
    PulseWidth,
    AmpAttack,
    AmpDecay,
    AmpSustain,
    AmpRelease,
    FilterMode,
    FilterCutoff,
    FilterResonance,
    FilterKeytrack,
    FilterAttack,
    FilterDecay,
    FilterSustain,
    FilterRelease,
    FilterEnvAmount,
    OutputGain,
    Macro1,
    Macro2,
    Lfo1Rate,
    Lfo2Rate,
    Lfo1Wave,
    Lfo2Wave,
    Lfo1Sync,
    Lfo2Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynthPatch {
    pub oscillator: Oscillator,
    pub octave: i8,
    pub semitone: i8,
    pub fine_cents: f32,
    pub pulse_width: f32,
    pub amp_env: AdsrParams,
    pub filter: FilterParams,
    pub filter_env: AdsrParams,
    pub filter_env_amount: f32,
    pub output_gain: f32,
    /// Spec 13 modulation matrix; omitted from JSON when unused.
    #[serde(default, skip_serializing_if = "ModulationConfig::is_default")]
    pub modulation: ModulationConfig,
}

impl SynthPatch {
    pub fn legacy(oscillator: Oscillator) -> Self {
        Self {
            oscillator,
            octave: 0,
            semitone: 0,
            fine_cents: 0.0,
            pulse_width: 0.5,
            amp_env: AdsrParams {
                attack_secs: 0.0,
                decay_secs: 0.0,
                sustain: 1.0,
                release_secs: 0.0,
            },
            filter: FilterParams {
                mode: FilterMode::Bypass,
                cutoff_hz: 20.0,
                resonance: 0.0,
                key_tracking: 0.0,
            },
            filter_env: AdsrParams {
                attack_secs: 0.0,
                decay_secs: 0.0,
                sustain: 0.0,
                release_secs: 0.0,
            },
            filter_env_amount: 0.0,
            output_gain: 1.0,
            modulation: ModulationConfig::default(),
        }
    }

    pub fn with_parameter(
        mut self,
        id: SynthParamId,
        value: SynthParamValue,
        sample_rate: f32,
    ) -> Result<Self, String> {
        match (id, value) {
            (SynthParamId::Oscillator, SynthParamValue::Oscillator(value)) => {
                self.oscillator = value
            }
            (SynthParamId::FilterMode, SynthParamValue::FilterMode(value)) => {
                self.filter.mode = value
            }
            (SynthParamId::Lfo1Wave, SynthParamValue::LfoWave(value)) => {
                self.modulation.lfos[0].wave = value
            }
            (SynthParamId::Lfo2Wave, SynthParamValue::LfoWave(value)) => {
                self.modulation.lfos[1].wave = value
            }
            (id, SynthParamValue::Number(value)) => {
                if !value.is_finite() {
                    return Err("synth value must be finite".into());
                }
                match id {
                    SynthParamId::Octave | SynthParamId::Semitone => {
                        if value.fract() != 0.0 || !(-12.0..=12.0).contains(&value) {
                            return Err("tuning offsets must be integers in range".into());
                        }
                        if id == SynthParamId::Octave {
                            self.octave = value as i8;
                        } else {
                            self.semitone = value as i8;
                        }
                    }
                    SynthParamId::FineCents => self.fine_cents = value,
                    SynthParamId::PulseWidth => self.pulse_width = value,
                    SynthParamId::AmpAttack => self.amp_env.attack_secs = value,
                    SynthParamId::AmpDecay => self.amp_env.decay_secs = value,
                    SynthParamId::AmpSustain => self.amp_env.sustain = value,
                    SynthParamId::AmpRelease => self.amp_env.release_secs = value,
                    SynthParamId::FilterCutoff => self.filter.cutoff_hz = value,
                    SynthParamId::FilterResonance => self.filter.resonance = value,
                    SynthParamId::FilterKeytrack => self.filter.key_tracking = value,
                    SynthParamId::FilterAttack => self.filter_env.attack_secs = value,
                    SynthParamId::FilterDecay => self.filter_env.decay_secs = value,
                    SynthParamId::FilterSustain => self.filter_env.sustain = value,
                    SynthParamId::FilterRelease => self.filter_env.release_secs = value,
                    SynthParamId::FilterEnvAmount => self.filter_env_amount = value,
                    SynthParamId::OutputGain => self.output_gain = value,
                    SynthParamId::Macro1 => self.modulation.macros[0] = value,
                    SynthParamId::Macro2 => self.modulation.macros[1] = value,
                    SynthParamId::Lfo1Rate => self.modulation.lfos[0].rate_hz = value,
                    SynthParamId::Lfo2Rate => self.modulation.lfos[1].rate_hz = value,
                    SynthParamId::Lfo1Wave | SynthParamId::Lfo2Wave => {
                        let wave = (value.fract() == 0.0)
                            .then(|| LfoWave::from_index(value as i32))
                            .flatten()
                            .ok_or("LFO wave must be an index from 0 (sine) to 4 (sample-hold)")?;
                        let lfo = usize::from(id == SynthParamId::Lfo2Wave);
                        self.modulation.lfos[lfo].wave = wave;
                    }
                    SynthParamId::Lfo1Sync | SynthParamId::Lfo2Sync => {
                        let sync = (value.fract() == 0.0)
                            .then(|| sync_from_index(value as i32))
                            .flatten()
                            .ok_or("LFO sync must be 0 (off) or an index from 1 to 11")?;
                        let lfo = usize::from(id == SynthParamId::Lfo2Sync);
                        self.modulation.lfos[lfo].sync = sync;
                    }
                    SynthParamId::Oscillator | SynthParamId::FilterMode => {
                        return Err("parameter requires a typed enum value".into())
                    }
                }
            }
            _ => return Err("value type does not match synth parameter".into()),
        }
        self.validate(sample_rate)?;
        Ok(self)
    }

    pub fn validate(&self, sample_rate: f32) -> Result<(), String> {
        if !(-4..=4).contains(&self.octave) {
            return Err("synth octave must be between -4 and 4".into());
        }
        if !(-12..=12).contains(&self.semitone) {
            return Err("synth semitone offset must be between -12 and 12".into());
        }
        if !self.fine_cents.is_finite() || !(-100.0..=100.0).contains(&self.fine_cents) {
            return Err("synth fine tuning must be finite and between -100 and 100 cents".into());
        }
        if !self.pulse_width.is_finite() || !(0.05..=0.95).contains(&self.pulse_width) {
            return Err("pulse width must be finite and between 0.05 and 0.95".into());
        }
        self.amp_env.validate()?;
        self.filter.validate(sample_rate)?;
        self.filter_env.validate()?;
        if !self.filter_env_amount.is_finite() || !(-8.0..=8.0).contains(&self.filter_env_amount) {
            return Err(
                "filter envelope amount must be finite and between -8 and 8 octaves".into(),
            );
        }
        if !self.output_gain.is_finite() || !(0.0..=2.0).contains(&self.output_gain) {
            return Err("synth output gain must be finite and between 0.0 and 2.0".into());
        }
        self.modulation.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SynthParamValue {
    Number(f32),
    Oscillator(Oscillator),
    FilterMode(FilterMode),
    LfoWave(LfoWave),
}

/// 0 = free-running, otherwise 1-based position in [`SYNC_DIVISIONS`].
pub const SYNC_DIVISIONS: [SyncDivision; 11] = [
    SyncDivision::Whole,
    SyncDivision::Half,
    SyncDivision::Quarter,
    SyncDivision::Eighth,
    SyncDivision::Sixteenth,
    SyncDivision::ThirtySecond,
    SyncDivision::QuarterTriplet,
    SyncDivision::EighthTriplet,
    SyncDivision::SixteenthTriplet,
    SyncDivision::DottedQuarter,
    SyncDivision::DottedEighth,
];

fn sync_from_index(index: i32) -> Option<Option<SyncDivision>> {
    match index {
        0 => Some(None),
        1..=11 => Some(Some(SYNC_DIVISIONS[(index - 1) as usize])),
        _ => None,
    }
}

pub fn sync_to_index(sync: Option<SyncDivision>) -> f32 {
    sync.map_or(0.0, |division| {
        SYNC_DIVISIONS
            .iter()
            .position(|candidate| *candidate == division)
            .map_or(0.0, |position| (position + 1) as f32)
    })
}

/// Validated on the control thread; applying it does not allocate or parse.
#[derive(Debug, Clone, Copy)]
pub struct CompiledSynthPatch {
    patch: SynthPatch,
    sample_rate: f32,
    smoothing_frames: u32,
    parameters: [f32; 7],
    modulation: CompiledModulation,
}

impl CompiledSynthPatch {
    pub fn new(sample_rate: f32, patch: SynthPatch) -> Result<Self, String> {
        patch.validate(sample_rate)?;
        Ok(Self {
            patch,
            sample_rate,
            smoothing_frames: (sample_rate * 0.005).round().max(1.0) as u32,
            parameters: patch_parameters(patch),
            modulation: CompiledModulation::compile(&patch.modulation, sample_rate)?,
        })
    }

    pub fn patch(&self) -> SynthPatch {
        self.patch
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
}

pub fn parse_synth_parameter_command(
    line: &str,
) -> Result<(SynthParamId, SynthParamValue), String> {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("synth") {
        return Err("expected synth <parameter> <value>".into());
    }
    let name = parts.next().ok_or("missing synth parameter")?;
    let raw = parts.next().ok_or("missing synth value")?;
    if parts.next().is_some() {
        return Err("expected synth <parameter> <value>".into());
    }
    let id: SynthParamId = serde_json::from_value(serde_json::Value::String(name.to_owned()))
        .map_err(|_| format!("unknown synth parameter: {name}"))?;
    let value = match id {
        SynthParamId::Oscillator => SynthParamValue::Oscillator(
            serde_json::from_value(serde_json::Value::String(raw.to_owned()))
                .map_err(|_| format!("unknown oscillator: {raw}"))?,
        ),
        SynthParamId::FilterMode => SynthParamValue::FilterMode(
            serde_json::from_value(serde_json::Value::String(raw.to_owned()))
                .map_err(|_| format!("unknown filter mode: {raw}"))?,
        ),
        SynthParamId::Lfo1Wave | SynthParamId::Lfo2Wave => match LfoWave::parse(raw) {
            Some(wave) => SynthParamValue::LfoWave(wave),
            None => SynthParamValue::Number(
                raw.parse::<f32>()
                    .map_err(|_| format!("unknown LFO wave: {raw}"))?,
            ),
        },
        SynthParamId::Lfo1Sync | SynthParamId::Lfo2Sync => {
            let index = if raw == "off" {
                0.0
            } else if let Some(division) = SyncDivision::parse(raw) {
                sync_to_index(Some(division))
            } else {
                raw.parse::<f32>()
                    .map_err(|_| format!("unknown LFO sync: {raw}"))?
            };
            SynthParamValue::Number(index)
        }
        _ => {
            let number = raw
                .parse::<f32>()
                .map_err(|_| "synth value must be numeric")?;
            if !number.is_finite() {
                return Err("synth value must be finite".into());
            }
            SynthParamValue::Number(number)
        }
    };
    Ok((id, value))
}

#[derive(Debug, Clone)]
pub struct SynthVoice {
    sample_rate: f32,
    patch: SynthPatch,
    frequency_hz: f32,
    base_frequency_hz: f32,
    parameters: [SmoothedParam; 7],
    velocity: f32,
    phase: f32,
    amp_env: AdsrEnvelope,
    filter_env: AdsrEnvelope,
    filter: StateVariableFilter,
    modulation: ModRuntime,
    mod_wheel: f32,
    last_offsets: ModOffsets,
}

impl SynthVoice {
    pub fn new(sample_rate: f32, oscillator: Oscillator) -> Self {
        let safe_sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let patch = SynthPatch::legacy(oscillator);
        let mut voice = Self::with_patch(safe_sample_rate, patch)
            .expect("legacy synth patch is valid for a positive sample rate");
        voice.sample_rate = sample_rate;
        voice
    }

    pub fn with_patch(sample_rate: f32, patch: SynthPatch) -> Result<Self, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("synth sample rate must be finite and greater than zero".into());
        }
        patch.validate(sample_rate)?;

        Ok(Self {
            sample_rate,
            patch,
            frequency_hz: 0.0,
            base_frequency_hz: 0.0,
            parameters: patch_parameters(patch).map(SmoothedParam::new),
            velocity: 0.0,
            phase: 0.0,
            amp_env: AdsrEnvelope::new(sample_rate, patch.amp_env)?,
            filter_env: AdsrEnvelope::new(sample_rate, patch.filter_env)?,
            filter: StateVariableFilter::new(sample_rate, patch.filter)?,
            modulation: ModRuntime::new(
                sample_rate,
                120.0,
                CompiledModulation::compile(&patch.modulation, sample_rate)?,
            ),
            mod_wheel: 0.0,
            last_offsets: ModOffsets::default(),
        })
    }

    pub fn patch(&self) -> SynthPatch {
        self.patch
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.modulation.set_tempo(bpm);
    }

    pub fn set_mod_wheel(&mut self, value: f32) {
        self.mod_wheel = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// Pan and send-level offsets from the last rendered sample.
    pub fn modulation_offsets(&self) -> ModOffsets {
        self.last_offsets
    }

    pub fn note_on(&mut self, frequency_hz: f32, velocity: f32) {
        self.note_on_with_phase(frequency_hz, velocity, [0.0; MAX_LFOS]);
    }

    /// As [`SynthVoice::note_on`], with the shared free-running LFO phase for
    /// LFOs whose `retrigger` flag is off.
    pub fn note_on_with_phase(
        &mut self,
        frequency_hz: f32,
        velocity: f32,
        free_cycles: [f64; MAX_LFOS],
    ) {
        self.base_frequency_hz = if frequency_hz.is_finite() {
            frequency_hz.max(0.0)
        } else {
            0.0
        };
        // Newly triggered/stolen voices start at the current patch target.
        self.parameters = patch_parameters(self.patch).map(SmoothedParam::new);
        self.velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.phase = 0.0;
        self.filter.reset();
        let note = if self.base_frequency_hz > 0.0 {
            (69.0 + 12.0 * (self.base_frequency_hz / 440.0).log2())
                .round()
                .clamp(0.0, 127.0) as u8
        } else {
            60
        };
        self.modulation.note_on(note, free_cycles);
        self.last_offsets = ModOffsets::default();
        self.amp_env.note_on();
        self.filter_env.note_on();
    }

    pub fn apply_patch(&mut self, compiled: CompiledSynthPatch) -> bool {
        if compiled.sample_rate != self.sample_rate {
            return false;
        }
        self.patch = compiled.patch;
        for (parameter, target) in self.parameters.iter_mut().zip(compiled.parameters) {
            parameter.set_target(target, compiled.smoothing_frames);
        }
        self.amp_env.update_params(self.patch.amp_env);
        self.filter_env.update_params(self.patch.filter_env);
        self.modulation.apply(compiled.modulation);
        true
    }

    pub fn note_off(&mut self) {
        self.amp_env.note_off();
        self.filter_env.note_off();
    }

    pub fn force_stop(&mut self) {
        self.velocity = 0.0;
        self.amp_env.reset();
        self.filter_env.reset();
        self.filter.reset();
    }

    pub fn is_active(&self) -> bool {
        self.velocity > 0.0 && self.amp_env.is_active()
    }

    pub fn next_sample(&mut self) -> f32 {
        if self.sample_rate <= 0.0 || !self.sample_rate.is_finite() || !self.is_active() {
            return 0.0;
        }

        let [mut tuning, mut pulse_width, base_cutoff, mut resonance, key_tracking, env_amount, mut gain] =
            std::array::from_fn(|index| self.parameters[index].next_value());
        let amp = self.amp_env.next_value();
        let filter_env = self.filter_env.next_value();

        // Order: base (smoothed) value -> modulation sum -> final clamp.
        // With no active routes none of this runs, so the legacy path is
        // bit-identical.
        let mut cutoff_octaves = 0.0;
        if self.modulation.is_active() {
            let offsets = self.modulation.next(&ModInputs {
                velocity: self.velocity,
                amp_env: amp,
                filter_env,
                mod_wheel: self.mod_wheel,
            });
            self.last_offsets = offsets;
            tuning *= (offsets.tuning_semitones / 12.0).exp2();
            pulse_width = (pulse_width + offsets.pulse_width).clamp(0.05, 0.95);
            resonance = (resonance + offsets.resonance).clamp(0.0, 1.0);
            gain *= (1.0 + offsets.amplitude).clamp(0.0, 2.0);
            cutoff_octaves = offsets.cutoff_octaves;
        } else {
            self.last_offsets = ModOffsets::default();
        }

        self.frequency_hz = (self.base_frequency_hz * tuning).clamp(0.0, self.sample_rate * 0.49);
        self.filter
            .set_mode_and_resonance(self.patch.filter.mode, resonance);
        let raw = match self.patch.oscillator {
            Oscillator::Sine => (self.phase * TAU).sin(),
            Oscillator::Triangle => 1.0 - 4.0 * (self.phase - 0.5).abs(),
            Oscillator::Saw => 2.0 * self.phase - 1.0,
            Oscillator::Pulse => {
                if self.phase < pulse_width {
                    1.0
                } else {
                    -1.0
                }
            }
        };

        let keytrack = if self.frequency_hz > 0.0 {
            (self.frequency_hz / 440.0).max(0.01).powf(key_tracking)
        } else {
            1.0
        };
        let env_multiplier = 2.0_f32.powf(env_amount * filter_env);
        let mut cutoff_target = base_cutoff * keytrack * env_multiplier;
        if cutoff_octaves != 0.0 {
            cutoff_target *= cutoff_octaves.exp2();
        }
        let cutoff = clamp_cutoff(cutoff_target, self.sample_rate);
        self.filter.set_cutoff_hz(cutoff);

        let filtered = self.filter.process(raw * self.velocity);
        let sample = (filtered * amp * gain).clamp(-1.0, 1.0);
        let phase_increment = self.frequency_hz / self.sample_rate;
        self.phase = (self.phase + phase_increment).fract();

        if self.amp_env.is_active() {
            sample
        } else {
            self.velocity = 0.0;
            0.0
        }
    }

    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = Vec::with_capacity(frames);
        for _ in 0..frames {
            output.push(self.next_sample());
        }
        output
    }
}

fn patch_parameters(patch: SynthPatch) -> [f32; 7] {
    [
        2.0_f32.powf(
            (f32::from(patch.octave) * 12.0 + f32::from(patch.semitone) + patch.fine_cents / 100.0)
                / 12.0,
        ),
        patch.pulse_width,
        patch.filter.cutoff_hz,
        patch.filter.resonance,
        patch.filter.key_tracking,
        patch.filter_env_amount,
        patch.output_gain,
    ]
}
