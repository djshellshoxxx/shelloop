use crate::{
    clamp_cutoff, AdsrEnvelope, AdsrParams, FilterMode, FilterParams, StateVariableFilter,
};
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Oscillator {
    Sine,
    Triangle,
    Saw,
    Pulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
        }
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
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SynthVoice {
    sample_rate: f32,
    patch: SynthPatch,
    frequency_hz: f32,
    velocity: f32,
    phase: f32,
    amp_env: AdsrEnvelope,
    filter_env: AdsrEnvelope,
    filter: StateVariableFilter,
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
            velocity: 0.0,
            phase: 0.0,
            amp_env: AdsrEnvelope::new(sample_rate, patch.amp_env)?,
            filter_env: AdsrEnvelope::new(sample_rate, patch.filter_env)?,
            filter: StateVariableFilter::new(sample_rate, patch.filter)?,
        })
    }

    pub fn patch(&self) -> SynthPatch {
        self.patch
    }

    pub fn note_on(&mut self, frequency_hz: f32, velocity: f32) {
        let tuning_semitones = f32::from(self.patch.octave) * 12.0
            + f32::from(self.patch.semitone)
            + self.patch.fine_cents / 100.0;
        let tuned = frequency_hz * 2.0_f32.powf(tuning_semitones / 12.0);
        self.frequency_hz = if tuned.is_finite() {
            tuned.clamp(0.0, self.sample_rate.max(1.0) * 0.49)
        } else {
            0.0
        };
        self.velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.phase = 0.0;
        self.filter.reset();
        self.amp_env.note_on();
        self.filter_env.note_on();
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

        let raw = match self.patch.oscillator {
            Oscillator::Sine => (self.phase * TAU).sin(),
            Oscillator::Triangle => 1.0 - 4.0 * (self.phase - 0.5).abs(),
            Oscillator::Saw => 2.0 * self.phase - 1.0,
            Oscillator::Pulse => {
                if self.phase < self.patch.pulse_width {
                    1.0
                } else {
                    -1.0
                }
            }
        };

        let amp = self.amp_env.next_value();
        let filter_env = self.filter_env.next_value();
        let keytrack = if self.frequency_hz > 0.0 {
            (self.frequency_hz / 440.0)
                .max(0.01)
                .powf(self.patch.filter.key_tracking)
        } else {
            1.0
        };
        let env_multiplier = 2.0_f32.powf(self.patch.filter_env_amount * filter_env);
        let cutoff = clamp_cutoff(
            self.patch.filter.cutoff_hz * keytrack * env_multiplier,
            self.sample_rate,
        );
        self.filter.set_cutoff_hz(cutoff);

        let filtered = self.filter.process(raw * self.velocity);
        let sample = (filtered * amp * self.patch.output_gain).clamp(-1.0, 1.0);
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
