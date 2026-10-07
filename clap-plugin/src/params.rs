//! Plugin parameter table, text conversion and patch construction.
//!
//! Parameter ids are stable: they are the CLAP `clap_id`s a host stores in
//! its automation lanes, so they must never be renumbered. The state blob is
//! keyed by [`ParamSpec::key`], which is equally stable.

use shelloop::{clamp_cutoff, AdsrParams, FilterMode, FilterParams, Oscillator, SynthPatch};
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of plugin parameters.
pub const PARAM_COUNT: usize = 15;

/// Number of synthesizer voices.
pub const POLYPHONY: usize = 16;

/// Upper bound for the cutoff parameter. The value actually used is clamped
/// to the filter's maximum for the active sample rate.
pub const CUTOFF_MAX_HZ: f64 = 20_000.0;

/// Stable parameter ids (the CLAP `clap_id` of each parameter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ParamId {
    Oscillator = 0,
    Octave = 1,
    Fine = 2,
    PulseWidth = 3,
    AmpAttack = 4,
    AmpDecay = 5,
    AmpSustain = 6,
    AmpRelease = 7,
    FilterMode = 8,
    Cutoff = 9,
    Resonance = 10,
    FilterEnvAmount = 11,
    OutputGain = 12,
    Sequencer = 13,
    MasterVolume = 14,
}

impl ParamId {
    pub const ALL: [ParamId; PARAM_COUNT] = [
        ParamId::Oscillator,
        ParamId::Octave,
        ParamId::Fine,
        ParamId::PulseWidth,
        ParamId::AmpAttack,
        ParamId::AmpDecay,
        ParamId::AmpSustain,
        ParamId::AmpRelease,
        ParamId::FilterMode,
        ParamId::Cutoff,
        ParamId::Resonance,
        ParamId::FilterEnvAmount,
        ParamId::OutputGain,
        ParamId::Sequencer,
        ParamId::MasterVolume,
    ];

    pub fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.get(raw as usize).copied()
    }

    pub fn raw(self) -> u32 {
        self as u32
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn spec(self) -> &'static ParamSpec {
        &PARAMS[self.index()]
    }
}

/// Display unit of a continuous parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Plain,
    Cents,
    Percent,
    Seconds,
    Hertz,
    Octaves,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    /// Stepped enumeration; the value is the index into the names.
    Enum(&'static [&'static str]),
    /// Stepped integer.
    Integer,
    Continuous(Unit),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    pub id: ParamId,
    /// Stable state key.
    pub key: &'static str,
    pub name: &'static str,
    pub module: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub kind: ParamKind,
}

pub const OSCILLATOR_NAMES: &[&str] = &["Sine", "Triangle", "Saw", "Pulse"];
pub const FILTER_MODE_NAMES: &[&str] = &["Bypass", "Low-pass", "High-pass", "Band-pass"];
pub const SWITCH_NAMES: &[&str] = &["Off", "On"];

pub static PARAMS: [ParamSpec; PARAM_COUNT] = [
    ParamSpec {
        id: ParamId::Oscillator,
        key: "oscillator",
        name: "Oscillator",
        module: "Oscillator",
        min: 0.0,
        max: 3.0,
        default: 2.0,
        kind: ParamKind::Enum(OSCILLATOR_NAMES),
    },
    ParamSpec {
        id: ParamId::Octave,
        key: "octave",
        name: "Octave",
        module: "Oscillator",
        min: -4.0,
        max: 4.0,
        default: 0.0,
        kind: ParamKind::Integer,
    },
    ParamSpec {
        id: ParamId::Fine,
        key: "fine_cents",
        name: "Fine",
        module: "Oscillator",
        min: -100.0,
        max: 100.0,
        default: 0.0,
        kind: ParamKind::Continuous(Unit::Cents),
    },
    ParamSpec {
        id: ParamId::PulseWidth,
        key: "pulse_width",
        name: "Pulse width",
        module: "Oscillator",
        min: 0.05,
        max: 0.95,
        default: 0.5,
        kind: ParamKind::Continuous(Unit::Percent),
    },
    ParamSpec {
        id: ParamId::AmpAttack,
        key: "amp_attack",
        name: "Amp attack",
        module: "Amp envelope",
        min: 0.0,
        max: 10.0,
        default: 0.005,
        kind: ParamKind::Continuous(Unit::Seconds),
    },
    ParamSpec {
        id: ParamId::AmpDecay,
        key: "amp_decay",
        name: "Amp decay",
        module: "Amp envelope",
        min: 0.0,
        max: 10.0,
        default: 0.2,
        kind: ParamKind::Continuous(Unit::Seconds),
    },
    ParamSpec {
        id: ParamId::AmpSustain,
        key: "amp_sustain",
        name: "Amp sustain",
        module: "Amp envelope",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        kind: ParamKind::Continuous(Unit::Percent),
    },
    ParamSpec {
        id: ParamId::AmpRelease,
        key: "amp_release",
        name: "Amp release",
        module: "Amp envelope",
        min: 0.0,
        max: 10.0,
        default: 0.25,
        kind: ParamKind::Continuous(Unit::Seconds),
    },
    ParamSpec {
        id: ParamId::FilterMode,
        key: "filter_mode",
        name: "Filter mode",
        module: "Filter",
        min: 0.0,
        max: 3.0,
        default: 1.0,
        kind: ParamKind::Enum(FILTER_MODE_NAMES),
    },
    ParamSpec {
        id: ParamId::Cutoff,
        key: "filter_cutoff",
        name: "Cutoff",
        module: "Filter",
        min: 20.0,
        max: CUTOFF_MAX_HZ,
        default: 1_800.0,
        kind: ParamKind::Continuous(Unit::Hertz),
    },
    ParamSpec {
        id: ParamId::Resonance,
        key: "filter_resonance",
        name: "Resonance",
        module: "Filter",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        kind: ParamKind::Continuous(Unit::Plain),
    },
    ParamSpec {
        id: ParamId::FilterEnvAmount,
        key: "filter_env_amount",
        name: "Filter env amount",
        module: "Filter",
        min: -8.0,
        max: 8.0,
        default: 2.0,
        kind: ParamKind::Continuous(Unit::Octaves),
    },
    ParamSpec {
        id: ParamId::OutputGain,
        key: "output_gain",
        name: "Output gain",
        module: "Output",
        min: 0.0,
        max: 2.0,
        default: 0.8,
        kind: ParamKind::Continuous(Unit::Plain),
    },
    ParamSpec {
        id: ParamId::Sequencer,
        key: "sequencer",
        name: "Sequencer",
        module: "Sequencer",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        kind: ParamKind::Enum(SWITCH_NAMES),
    },
    ParamSpec {
        id: ParamId::MasterVolume,
        key: "master_volume",
        name: "Master volume",
        module: "Output",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        kind: ParamKind::Continuous(Unit::Plain),
    },
];

/// A plain snapshot of every parameter value, indexed by [`ParamId::index`].
pub type ParamValues = [f64; PARAM_COUNT];

pub fn default_values() -> ParamValues {
    std::array::from_fn(|index| PARAMS[index].default)
}

impl ParamSpec {
    pub fn is_stepped(&self) -> bool {
        matches!(self.kind, ParamKind::Enum(_) | ParamKind::Integer)
    }

    /// Clamp (and round, for stepped parameters) a host value into range.
    /// Returns `None` for non-finite input, which callers ignore.
    /// Allocation-free; safe on the audio thread.
    pub fn sanitize(&self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        let value = if self.is_stepped() {
            value.round()
        } else {
            value
        };
        Some(value.clamp(self.min, self.max))
    }

    /// Human-readable value. Main thread only (allocates).
    pub fn format(&self, value: f64) -> String {
        let value = self.sanitize(value).unwrap_or(self.default);
        match self.kind {
            ParamKind::Enum(names) => names[value as usize].to_owned(),
            ParamKind::Integer => {
                if value == 0.0 {
                    "0".to_owned()
                } else {
                    format!("{value:+.0}")
                }
            }
            ParamKind::Continuous(unit) => match unit {
                Unit::Plain => format!("{value:.3}"),
                Unit::Cents => format!("{value:+.1} ct"),
                Unit::Percent => format!("{:.1} %", value * 100.0),
                Unit::Seconds if value < 1.0 => format!("{:.1} ms", value * 1000.0),
                Unit::Seconds => format!("{value:.3} s"),
                Unit::Hertz if value >= 1000.0 => format!("{:.2} kHz", value / 1000.0),
                Unit::Hertz => format!("{value:.1} Hz"),
                Unit::Octaves => format!("{value:+.2} oct"),
            },
        }
    }

    /// Parse display text (with or without its unit) back into a value.
    pub fn parse(&self, text: &str) -> Option<f64> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let ParamKind::Enum(names) = self.kind {
            let wanted = normalize_name(text);
            if let Some(index) = names.iter().position(|name| normalize_name(name) == wanted) {
                return Some(index as f64);
            }
        }
        let (number, suffix) = split_number(text)?;
        let scale = match (self.kind, suffix.as_str()) {
            (_, "") => 1.0,
            (ParamKind::Continuous(Unit::Cents), "ct" | "cent" | "cents") => 1.0,
            (ParamKind::Continuous(Unit::Percent), "%") => 0.01,
            (ParamKind::Continuous(Unit::Seconds), "ms") => 0.001,
            (ParamKind::Continuous(Unit::Seconds), "s" | "sec") => 1.0,
            (ParamKind::Continuous(Unit::Hertz), "hz") => 1.0,
            (ParamKind::Continuous(Unit::Hertz), "khz") => 1000.0,
            (ParamKind::Continuous(Unit::Octaves), "oct") => 1.0,
            (ParamKind::Continuous(Unit::Plain), "x") => 1.0,
            _ => return None,
        };
        self.sanitize(number * scale)
    }
}

fn normalize_name(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn split_number(text: &str) -> Option<(f64, String)> {
    let split = text
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.')))
        .map_or(text.len(), |(index, _)| index);
    let number = text[..split].trim().parse::<f64>().ok()?;
    let suffix = text[split..].trim().to_ascii_lowercase();
    Some((number, suffix))
}

/// Lock-free parameter storage shared between the main and audio threads.
#[derive(Debug)]
pub struct ParamStore {
    values: [AtomicU64; PARAM_COUNT],
}

impl Default for ParamStore {
    fn default() -> Self {
        Self {
            values: std::array::from_fn(|index| AtomicU64::new(PARAMS[index].default.to_bits())),
        }
    }
}

impl ParamStore {
    pub fn get(&self, id: ParamId) -> f64 {
        f64::from_bits(self.values[id.index()].load(Ordering::Acquire))
    }

    /// Store an already sanitized value.
    pub fn set(&self, id: ParamId, value: f64) {
        self.values[id.index()].store(value.to_bits(), Ordering::Release);
    }

    pub fn snapshot(&self) -> ParamValues {
        std::array::from_fn(|index| f64::from_bits(self.values[index].load(Ordering::Acquire)))
    }

    pub fn store_all(&self, values: &ParamValues) {
        for (slot, value) in self.values.iter().zip(values) {
            slot.store(value.to_bits(), Ordering::Release);
        }
    }
}

fn oscillator_from(value: f64) -> Oscillator {
    match value.round() as i64 {
        0 => Oscillator::Sine,
        1 => Oscillator::Triangle,
        3 => Oscillator::Pulse,
        _ => Oscillator::Saw,
    }
}

fn filter_mode_from(value: f64) -> FilterMode {
    match value.round() as i64 {
        0 => FilterMode::Bypass,
        2 => FilterMode::HighPass,
        3 => FilterMode::BandPass,
        _ => FilterMode::LowPass,
    }
}

/// Fixed filter envelope: a plucky decay so "Filter env amount" is audible.
pub const FILTER_ENVELOPE: AdsrParams = AdsrParams {
    attack_secs: 0.0,
    decay_secs: 0.35,
    sustain: 0.0,
    release_secs: 0.2,
};

/// Build a synth patch from parameter values. Every value is clamped into
/// the engine's accepted range (cutoff against the filter maximum for
/// `sample_rate`), so the result validates for any sanitized input. It does
/// not allocate and is safe on the audio thread.
pub fn build_patch(values: &ParamValues, sample_rate: f32) -> SynthPatch {
    let value = |id: ParamId| {
        let spec = id.spec();
        spec.sanitize(values[id.index()]).unwrap_or(spec.default)
    };
    let mut patch = SynthPatch::legacy(oscillator_from(value(ParamId::Oscillator)));
    patch.octave = value(ParamId::Octave) as i8;
    patch.fine_cents = value(ParamId::Fine) as f32;
    patch.pulse_width = value(ParamId::PulseWidth) as f32;
    patch.amp_env = AdsrParams {
        attack_secs: value(ParamId::AmpAttack) as f32,
        decay_secs: value(ParamId::AmpDecay) as f32,
        sustain: value(ParamId::AmpSustain) as f32,
        release_secs: value(ParamId::AmpRelease) as f32,
    };
    patch.filter = FilterParams {
        mode: filter_mode_from(value(ParamId::FilterMode)),
        cutoff_hz: clamp_cutoff(value(ParamId::Cutoff) as f32, sample_rate),
        resonance: value(ParamId::Resonance) as f32,
        key_tracking: 0.0,
    };
    patch.filter_env = FILTER_ENVELOPE;
    patch.filter_env_amount = value(ParamId::FilterEnvAmount) as f32;
    patch.output_gain = value(ParamId::OutputGain) as f32;
    patch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_table_order() {
        for (index, spec) in PARAMS.iter().enumerate() {
            assert_eq!(spec.id.index(), index);
            assert_eq!(ParamId::from_raw(index as u32), Some(spec.id));
            assert!(spec.min <= spec.default && spec.default <= spec.max);
        }
        assert_eq!(ParamId::from_raw(PARAM_COUNT as u32), None);
    }

    #[test]
    fn text_round_trips_for_every_parameter() {
        for spec in &PARAMS {
            for value in [spec.min, spec.default, spec.max] {
                let text = spec.format(value);
                let parsed = spec.parse(&text).unwrap_or_else(|| panic!("{text}"));
                let tolerance = (spec.max - spec.min) * 1e-3;
                assert!((parsed - value).abs() <= tolerance, "{}: {text}", spec.name);
            }
        }
    }

    #[test]
    fn parses_names_and_units() {
        assert_eq!(ParamId::Oscillator.spec().parse("pulse"), Some(3.0));
        assert_eq!(ParamId::FilterMode.spec().parse("highpass"), Some(2.0));
        assert_eq!(ParamId::Sequencer.spec().parse("ON"), Some(1.0));
        assert_eq!(ParamId::Cutoff.spec().parse("2.5 kHz"), Some(2500.0));
        assert_eq!(ParamId::AmpAttack.spec().parse("250 ms"), Some(0.25));
        assert_eq!(ParamId::AmpSustain.spec().parse("50 %"), Some(0.5));
        assert_eq!(ParamId::Octave.spec().parse("+2"), Some(2.0));
        assert_eq!(ParamId::Octave.spec().parse("9"), Some(4.0));
        assert_eq!(ParamId::Cutoff.spec().parse("loud"), None);
        assert_eq!(ParamId::Cutoff.spec().parse("12 parsecs"), None);
    }

    #[test]
    fn sanitize_rejects_non_finite_and_rounds_steps() {
        assert_eq!(ParamId::Octave.spec().sanitize(f64::NAN), None);
        assert_eq!(ParamId::Octave.spec().sanitize(1.6), Some(2.0));
        assert_eq!(ParamId::Resonance.spec().sanitize(3.0), Some(1.0));
    }

    #[test]
    fn patch_validates_and_clamps_cutoff_to_sample_rate() {
        let mut values = default_values();
        values[ParamId::Cutoff.index()] = CUTOFF_MAX_HZ;
        for rate in [22_050.0, 44_100.0, 48_000.0, 96_000.0] {
            let patch = build_patch(&values, rate);
            patch.validate(rate).expect("patch validates");
            assert!(patch.filter.cutoff_hz <= shelloop::max_cutoff(rate));
        }
    }
}
