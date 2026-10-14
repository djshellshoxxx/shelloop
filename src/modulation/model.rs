//! Serialisable modulation data. Omitted modulation deserialises to
//! [`ModulationConfig::default`] (no routes) and a default config is never
//! written back, so legacy patches and projects round-trip unchanged.

use serde::{Deserialize, Serialize};

pub const MAX_LFOS: usize = 2;
pub const MAX_MACROS: usize = 2;
pub const MAX_ROUTES: usize = 8;
pub const MODULATION_VERSION: u32 = 1;

pub const MIN_LFO_HZ: f32 = 0.01;
pub const MAX_LFO_HZ: f32 = 50.0;
pub const MAX_SMOOTHING_MS: f32 = 2_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoWave {
    Sine,
    Triangle,
    Saw,
    Square,
    /// Deterministic sample-and-hold: one hashed value per LFO cycle.
    SampleHold,
}

impl LfoWave {
    pub fn from_index(index: i32) -> Option<Self> {
        Some(match index {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Square,
            4 => Self::SampleHold,
            _ => return None,
        })
    }

    pub fn parse(name: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
    }
}

/// Tempo-relative LFO period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncDivision {
    #[serde(rename = "1/1")]
    Whole,
    #[serde(rename = "1/2")]
    Half,
    #[serde(rename = "1/4")]
    Quarter,
    #[serde(rename = "1/8")]
    Eighth,
    #[serde(rename = "1/16")]
    Sixteenth,
    #[serde(rename = "1/32")]
    ThirtySecond,
    #[serde(rename = "1/4t")]
    QuarterTriplet,
    #[serde(rename = "1/8t")]
    EighthTriplet,
    #[serde(rename = "1/16t")]
    SixteenthTriplet,
    #[serde(rename = "1/4d")]
    DottedQuarter,
    #[serde(rename = "1/8d")]
    DottedEighth,
}

impl SyncDivision {
    /// Quarter-note beats per LFO cycle.
    pub fn beats(self) -> f32 {
        match self {
            Self::Whole => 4.0,
            Self::Half => 2.0,
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
            Self::QuarterTriplet => 2.0 / 3.0,
            Self::EighthTriplet => 1.0 / 3.0,
            Self::SixteenthTriplet => 1.0 / 6.0,
            Self::DottedQuarter => 1.5,
            Self::DottedEighth => 0.75,
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfoConfig {
    #[serde(default = "default_wave")]
    pub wave: LfoWave,
    /// Free-running rate; ignored while `sync` is set.
    #[serde(default = "default_rate")]
    pub rate_hz: f32,
    /// Tempo-relative rate; takes precedence over `rate_hz`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncDivision>,
    /// `true` restarts the phase at every note-on; `false` free-runs, so all
    /// notes share one phase.
    #[serde(default)]
    pub retrigger: bool,
    /// Sample-and-hold stream seed.
    #[serde(default = "default_seed")]
    pub seed: u32,
}

fn default_wave() -> LfoWave {
    LfoWave::Sine
}
fn default_rate() -> f32 {
    1.0
}
fn default_seed() -> u32 {
    1
}
fn default_true() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

impl Default for LfoConfig {
    fn default() -> Self {
        Self {
            wave: default_wave(),
            rate_hz: default_rate(),
            sync: None,
            retrigger: false,
            seed: default_seed(),
        }
    }
}

impl LfoConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.rate_hz.is_finite() || !(MIN_LFO_HZ..=MAX_LFO_HZ).contains(&self.rate_hz) {
            return Err(format!(
                "LFO rate must be finite and between {MIN_LFO_HZ} and {MAX_LFO_HZ} Hz"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModSource {
    Lfo1,
    Lfo2,
    AmpEnv,
    FilterEnv,
    Velocity,
    KeyTrack,
    ModWheel,
    Macro1,
    Macro2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDest {
    /// Oscillator pitch; amount 1.0 = 12 semitones, total bounded to ±24.
    Tuning,
    /// Amount 1.0 = 0.45 of the pulse cycle.
    PulseWidth,
    /// Amount 1.0 = 4 octaves, total bounded to ±8.
    FilterCutoff,
    /// Amount 1.0 = full resonance range.
    FilterResonance,
    /// Gain multiplier `1 + sum`, bounded to 0..=2.
    Amplitude,
    /// Added to the track pan, bounded to ±1.
    Pan,
    /// Added to the track's send levels (the wet amount of the send
    /// effects), bounded to 0..=1 after the sum.
    EffectWet,
}

impl ModDest {
    pub const COUNT: usize = 7;

    pub fn index(self) -> usize {
        self as usize
    }
}

pub fn parse_mod_source(name: &str) -> Option<ModSource> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

pub fn parse_mod_dest(name: &str) -> Option<ModDest> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModRoute {
    pub source: ModSource,
    pub dest: ModDest,
    /// Bipolar depth, -1.0..=1.0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub amount: f32,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// One-pole smoothing time in milliseconds (0 = none).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub smoothing_ms: f32,
}

impl ModRoute {
    pub fn new(source: ModSource, dest: ModDest, amount: f32) -> Self {
        Self {
            source,
            dest,
            amount,
            enabled: true,
            smoothing_ms: 0.0,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.amount.is_finite() || !(-1.0..=1.0).contains(&self.amount) {
            return Err("modulation amount must be finite and between -1.0 and 1.0".into());
        }
        if !self.smoothing_ms.is_finite() || !(0.0..=MAX_SMOOTHING_MS).contains(&self.smoothing_ms)
        {
            return Err(format!(
                "modulation smoothing must be finite and between 0 and {MAX_SMOOTHING_MS} ms"
            ));
        }
        Ok(())
    }
}

const PLACEHOLDER_ROUTE: ModRoute = ModRoute {
    source: ModSource::Lfo1,
    dest: ModDest::Tuning,
    amount: 0.0,
    enabled: true,
    smoothing_ms: 0.0,
};

/// Per-patch modulation: fixed capacity, `Copy`, no heap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ModulationRepr", into = "ModulationRepr")]
pub struct ModulationConfig {
    pub lfos: [LfoConfig; MAX_LFOS],
    /// Macro knob positions, 0.0..=1.0.
    pub macros: [f32; MAX_MACROS],
    routes: [ModRoute; MAX_ROUTES],
    route_count: u8,
}

impl Default for ModulationConfig {
    fn default() -> Self {
        Self {
            lfos: [LfoConfig::default(); MAX_LFOS],
            macros: [0.0; MAX_MACROS],
            routes: [PLACEHOLDER_ROUTE; MAX_ROUTES],
            route_count: 0,
        }
    }
}

impl ModulationConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn routes(&self) -> &[ModRoute] {
        &self.routes[..usize::from(self.route_count)]
    }

    pub fn add_route(&mut self, route: ModRoute) -> Result<usize, String> {
        route.validate()?;
        let count = usize::from(self.route_count);
        if count >= MAX_ROUTES {
            return Err(format!("a patch supports at most {MAX_ROUTES} modulation routes"));
        }
        self.routes[count] = route;
        self.route_count += 1;
        Ok(count)
    }

    pub fn set_route(&mut self, index: usize, route: ModRoute) -> Result<(), String> {
        route.validate()?;
        if index >= usize::from(self.route_count) {
            return Err(format!("modulation route {} does not exist", index + 1));
        }
        self.routes[index] = route;
        Ok(())
    }

    pub fn remove_route(&mut self, index: usize) -> Result<(), String> {
        let count = usize::from(self.route_count);
        if index >= count {
            return Err(format!("modulation route {} does not exist", index + 1));
        }
        self.routes.copy_within(index + 1..count, index);
        self.routes[count - 1] = PLACEHOLDER_ROUTE;
        self.route_count -= 1;
        Ok(())
    }

    pub fn clear_routes(&mut self) {
        self.routes = [PLACEHOLDER_ROUTE; MAX_ROUTES];
        self.route_count = 0;
    }

    pub fn validate(&self) -> Result<(), String> {
        for lfo in &self.lfos {
            lfo.validate()?;
        }
        for value in self.macros {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err("macro values must be finite and between 0.0 and 1.0".into());
            }
        }
        for route in self.routes() {
            route.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModulationRepr {
    #[serde(default = "current_version")]
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    lfos: Vec<LfoConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    macros: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    routes: Vec<ModRoute>,
}

fn current_version() -> u32 {
    MODULATION_VERSION
}

impl TryFrom<ModulationRepr> for ModulationConfig {
    type Error = String;

    fn try_from(repr: ModulationRepr) -> Result<Self, String> {
        if repr.version != MODULATION_VERSION {
            return Err(format!(
                "unsupported modulation version {} (expected {MODULATION_VERSION})",
                repr.version
            ));
        }
        if repr.lfos.len() > MAX_LFOS {
            return Err(format!("at most {MAX_LFOS} LFOs are supported"));
        }
        if repr.macros.len() > MAX_MACROS {
            return Err(format!("at most {MAX_MACROS} macros are supported"));
        }
        if repr.routes.len() > MAX_ROUTES {
            return Err(format!("at most {MAX_ROUTES} modulation routes are supported"));
        }
        let mut config = Self::default();
        for (slot, lfo) in repr.lfos.into_iter().enumerate() {
            config.lfos[slot] = lfo;
        }
        for (slot, value) in repr.macros.into_iter().enumerate() {
            config.macros[slot] = value;
        }
        for route in repr.routes {
            config.add_route(route)?;
        }
        config.validate()?;
        Ok(config)
    }
}

impl From<ModulationConfig> for ModulationRepr {
    fn from(config: ModulationConfig) -> Self {
        let trim = |lfos: &[LfoConfig; MAX_LFOS]| {
            let mut out: Vec<LfoConfig> = lfos.to_vec();
            while out.last() == Some(&LfoConfig::default()) {
                out.pop();
            }
            out
        };
        let mut macros = config.macros.to_vec();
        while macros.last() == Some(&0.0) {
            macros.pop();
        }
        Self {
            version: MODULATION_VERSION,
            lfos: trim(&config.lfos),
            macros,
            routes: config.routes().to_vec(),
        }
    }
}
