use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    Bypass,
    LowPass,
    HighPass,
    BandPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterParams {
    pub mode: FilterMode,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub key_tracking: f32,
}

impl FilterParams {
    pub fn validate(&self, sample_rate: f32) -> Result<(), String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("filter sample rate must be finite and greater than zero".into());
        }
        let max_cutoff = max_cutoff(sample_rate);
        if !self.cutoff_hz.is_finite() || !(20.0..=max_cutoff).contains(&self.cutoff_hz) {
            return Err(format!(
                "filter cutoff must be finite and between 20 Hz and {max_cutoff:.1} Hz"
            ));
        }
        if !self.resonance.is_finite() || !(0.0..=1.0).contains(&self.resonance) {
            return Err("filter resonance must be finite and between 0.0 and 1.0".into());
        }
        if !self.key_tracking.is_finite() || !(0.0..=1.0).contains(&self.key_tracking) {
            return Err("filter key tracking must be finite and between 0.0 and 1.0".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct StateVariableFilter {
    sample_rate: f32,
    params: FilterParams,
    ic1eq: f32,
    ic2eq: f32,
}

impl StateVariableFilter {
    pub fn new(sample_rate: f32, params: FilterParams) -> Result<Self, String> {
        params.validate(sample_rate)?;
        Ok(Self {
            sample_rate,
            params,
            ic1eq: 0.0,
            ic2eq: 0.0,
        })
    }

    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    pub fn set_cutoff_hz(&mut self, cutoff_hz: f32) {
        self.params.cutoff_hz = clamp_cutoff(cutoff_hz, self.sample_rate);
    }

    pub fn process(&mut self, input: f32) -> f32 {
        if !input.is_finite() {
            return 0.0;
        }
        if self.params.mode == FilterMode::Bypass {
            return input;
        }

        let cutoff = clamp_cutoff(self.params.cutoff_hz, self.sample_rate);
        let g = (PI * cutoff / self.sample_rate).tan();
        let k = 2.0 - 1.9 * self.params.resonance;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        let v3 = input - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;

        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;

        let output = match self.params.mode {
            FilterMode::Bypass => input,
            FilterMode::LowPass => v2,
            FilterMode::BandPass => v1,
            FilterMode::HighPass => input - k * v1 - v2,
        };

        if output.is_finite() {
            output
        } else {
            self.reset();
            0.0
        }
    }
}

pub fn max_cutoff(sample_rate: f32) -> f32 {
    20_000.0_f32.min(sample_rate * 0.45).max(20.0)
}

pub fn clamp_cutoff(cutoff_hz: f32, sample_rate: f32) -> f32 {
    if !cutoff_hz.is_finite() {
        return 20.0;
    }
    cutoff_hz.clamp(20.0, max_cutoff(sample_rate))
}
