//! Stable parameter addressing shared by MIDI learn, parameter locks, effects
//! and the full-screen UI. Display labels are never persistence keys; the
//! snake_case serde names below are.

use crate::{SynthParamId, TrackId};
use serde::{Deserialize, Serialize};

/// Response curve used when a normalized 0..=1 control value is mapped onto a
/// parameter range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParamCurve {
    #[default]
    Linear,
    /// Exponential sweep between positive endpoints (frequencies, times).
    Logarithmic,
    /// Integer steps between min and max.
    Discrete,
}

impl ParamCurve {
    /// Map a normalized value onto `min..=max`.
    pub fn map(self, normalized: f32, min: f32, max: f32) -> f32 {
        let unit = if normalized.is_finite() {
            normalized.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self {
            Self::Linear => min + (max - min) * unit,
            Self::Logarithmic if min > 0.0 && max > 0.0 => min * (max / min).powf(unit),
            Self::Logarithmic => min + (max - min) * unit,
            Self::Discrete => (min + (max - min) * unit)
                .round()
                .clamp(min.min(max), max.max(min)),
        }
    }

    /// Inverse of [`ParamCurve::map`], used for pickup comparisons.
    pub fn normalize(self, value: f32, min: f32, max: f32) -> f32 {
        if !value.is_finite() || (max - min).abs() <= f32::EPSILON {
            return 0.0;
        }
        let unit = match self {
            Self::Logarithmic if min > 0.0 && max > 0.0 && value > 0.0 => {
                (value / min).ln() / (max / min).ln()
            }
            _ => (value - min) / (max - min),
        };
        unit.clamp(0.0, 1.0)
    }
}

/// Static description of one addressable parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDescriptor {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub curve: ParamCurve,
    /// Whether per-step parameter locks may target this parameter.
    pub lockable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalParamId {
    MasterGain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackParamId {
    Gain,
    Pan,
    SendA,
    SendB,
    Mute,
    Solo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleParamId {
    /// Semitone offset added to the track's configured pitch.
    Pitch,
    /// Multiplier on the configured sample gain.
    Gain,
    /// 0 = configured direction, 1 = reversed.
    Reverse,
}

/// Effect insert/send/master slot index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct EffectSlotId(pub u8);

/// Index into an effect type's static parameter descriptor table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct EffectParamId(pub u8);

/// Where an effect rack lives in the signal graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectLocation {
    Track(TrackId),
    /// Send bus 0 (A) or 1 (B).
    Send(u8),
    Master,
}

/// Transport/scene actions a button can trigger. These are commands, not
/// numeric parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    TogglePlay,
    Restart,
    Panic,
    SceneNext,
    ScenePrev,
    SceneLaunch(u16),
}

/// Any parameter or action a controller, lock or UI can address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterTarget {
    Global(GlobalParamId),
    Track {
        track: TrackId,
        param: TrackParamId,
    },
    Synth {
        track: TrackId,
        param: SynthParamId,
    },
    Sample {
        track: TrackId,
        param: SampleParamId,
    },
    Effect {
        location: EffectLocation,
        slot: EffectSlotId,
        param: EffectParamId,
    },
    Action(ActionId),
}

pub const MASTER_GAIN: ParamDescriptor = ParamDescriptor {
    name: "master.gain",
    min: 0.0,
    max: 2.0,
    default: 1.0,
    curve: ParamCurve::Linear,
    lockable: false,
};

pub fn track_param_descriptor(param: TrackParamId) -> ParamDescriptor {
    let (name, min, max, default, curve, lockable) = match param {
        TrackParamId::Gain => ("gain", 0.0, 2.0, 1.0, ParamCurve::Linear, true),
        TrackParamId::Pan => ("pan", -1.0, 1.0, 0.0, ParamCurve::Linear, true),
        TrackParamId::SendA => ("send_a", 0.0, 1.0, 0.0, ParamCurve::Linear, true),
        TrackParamId::SendB => ("send_b", 0.0, 1.0, 0.0, ParamCurve::Linear, true),
        TrackParamId::Mute => ("mute", 0.0, 1.0, 0.0, ParamCurve::Discrete, false),
        TrackParamId::Solo => ("solo", 0.0, 1.0, 0.0, ParamCurve::Discrete, false),
    };
    ParamDescriptor {
        name,
        min,
        max,
        default,
        curve,
        lockable,
    }
}

pub fn sample_param_descriptor(param: SampleParamId) -> ParamDescriptor {
    let (name, min, max, default, curve) = match param {
        SampleParamId::Pitch => ("pitch", -48.0, 48.0, 0.0, ParamCurve::Linear),
        SampleParamId::Gain => ("gain", 0.0, 2.0, 1.0, ParamCurve::Linear),
        SampleParamId::Reverse => ("reverse", 0.0, 1.0, 0.0, ParamCurve::Discrete),
    };
    ParamDescriptor {
        name,
        min,
        max,
        default,
        curve,
        lockable: true,
    }
}

/// Numeric synth parameters. Oscillator and filter mode are structural enum
/// choices and are therefore not lockable or continuously mappable.
pub fn synth_param_descriptor(param: SynthParamId) -> ParamDescriptor {
    use SynthParamId as P;
    let (name, min, max, default, curve, lockable) = match param {
        P::Oscillator => ("oscillator", 0.0, 3.0, 2.0, ParamCurve::Discrete, false),
        P::FilterMode => ("filter_mode", 0.0, 3.0, 0.0, ParamCurve::Discrete, false),
        P::Octave => ("octave", -4.0, 4.0, 0.0, ParamCurve::Discrete, true),
        P::Semitone => ("semitone", -12.0, 12.0, 0.0, ParamCurve::Discrete, true),
        P::FineCents => ("fine_cents", -100.0, 100.0, 0.0, ParamCurve::Linear, true),
        P::PulseWidth => ("pulse_width", 0.05, 0.95, 0.5, ParamCurve::Linear, true),
        P::AmpAttack => ("amp_attack", 0.0, 10.0, 0.0, ParamCurve::Linear, true),
        P::AmpDecay => ("amp_decay", 0.0, 10.0, 0.0, ParamCurve::Linear, true),
        P::AmpSustain => ("amp_sustain", 0.0, 1.0, 1.0, ParamCurve::Linear, true),
        P::AmpRelease => ("amp_release", 0.0, 10.0, 0.0, ParamCurve::Linear, true),
        P::FilterCutoff => (
            "filter_cutoff",
            20.0,
            20_000.0,
            20.0,
            ParamCurve::Logarithmic,
            true,
        ),
        P::FilterResonance => ("filter_resonance", 0.0, 1.0, 0.0, ParamCurve::Linear, true),
        P::FilterKeytrack => ("filter_keytrack", 0.0, 1.0, 0.0, ParamCurve::Linear, true),
        P::FilterAttack => ("filter_attack", 0.0, 10.0, 0.0, ParamCurve::Linear, true),
        P::FilterDecay => ("filter_decay", 0.0, 10.0, 0.0, ParamCurve::Linear, true),
        P::FilterSustain => ("filter_sustain", 0.0, 1.0, 0.0, ParamCurve::Linear, true),
        P::FilterRelease => ("filter_release", 0.0, 10.0, 0.0, ParamCurve::Linear, true),
        P::FilterEnvAmount => (
            "filter_env_amount",
            -8.0,
            8.0,
            0.0,
            ParamCurve::Linear,
            true,
        ),
        P::OutputGain => ("output_gain", 0.0, 2.0, 1.0, ParamCurve::Linear, true),
    };
    ParamDescriptor {
        name,
        min,
        max,
        default,
        curve,
        lockable,
    }
}

/// Parse a snake_case synth parameter name.
pub fn parse_synth_param(name: &str) -> Option<SynthParamId> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}
