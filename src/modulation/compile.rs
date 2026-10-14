//! Control-thread compilation of [`ModulationConfig`] into a `Copy` form
//! with typed enums and precomputed coefficients, so applying it on the
//! audio thread never parses, validates or allocates.

use super::model::{
    LfoWave, ModDest, ModSource, ModulationConfig, MAX_LFOS, MAX_MACROS, MAX_ROUTES,
};
use super::runtime::MOD_CONTROL_INTERVAL;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledRoute {
    pub source: ModSource,
    pub dest: ModDest,
    pub amount: f32,
    /// One-pole coefficient per control tick; 1.0 = unsmoothed.
    pub smooth_coeff: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledLfo {
    pub wave: LfoWave,
    pub rate_hz: f32,
    /// Quarter-note beats per cycle; 0 = use `rate_hz`.
    pub sync_beats: f32,
    pub retrigger: bool,
    pub seed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledModulation {
    pub routes: [CompiledRoute; MAX_ROUTES],
    pub route_count: usize,
    pub lfos: [CompiledLfo; MAX_LFOS],
    pub macros: [f32; MAX_MACROS],
}

impl Default for CompiledModulation {
    fn default() -> Self {
        Self::inactive()
    }
}

impl CompiledModulation {
    pub fn inactive() -> Self {
        Self::compile(&ModulationConfig::default(), 48_000.0)
            .expect("the default modulation config is valid")
    }

    pub fn compile(config: &ModulationConfig, sample_rate: f32) -> Result<Self, String> {
        config.validate()?;
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("modulation sample rate must be finite and greater than zero".into());
        }
        let mut routes = [CompiledRoute {
            source: ModSource::Lfo1,
            dest: ModDest::Tuning,
            amount: 0.0,
            smooth_coeff: 1.0,
        }; MAX_ROUTES];
        let mut route_count = 0;
        for route in config.routes() {
            // Disabled and zero-depth routes cost nothing at run time.
            if !route.enabled || route.amount == 0.0 {
                continue;
            }
            let smooth_coeff = if route.smoothing_ms <= 0.0 {
                1.0
            } else {
                let tau = route.smoothing_ms / 1000.0 * sample_rate;
                1.0 - (-(MOD_CONTROL_INTERVAL as f32) / tau).exp()
            };
            routes[route_count] = CompiledRoute {
                source: route.source,
                dest: route.dest,
                amount: route.amount,
                smooth_coeff,
            };
            route_count += 1;
        }
        let mut lfos = [CompiledLfo {
            wave: LfoWave::Sine,
            rate_hz: 1.0,
            sync_beats: 0.0,
            retrigger: false,
            seed: 1,
        }; MAX_LFOS];
        for (compiled, lfo) in lfos.iter_mut().zip(config.lfos) {
            *compiled = CompiledLfo {
                wave: lfo.wave,
                rate_hz: lfo.rate_hz,
                sync_beats: lfo.sync.map_or(0.0, |division| division.beats()),
                retrigger: lfo.retrigger,
                seed: lfo.seed,
            };
        }
        Ok(Self {
            routes,
            route_count,
            lfos,
            macros: config.macros,
        })
    }

    /// Whether any route can change the sound. When false the voice takes
    /// the legacy code path, so unmodulated rendering is bit-identical.
    pub fn is_active(&self) -> bool {
        self.route_count > 0
    }

    pub fn active_routes(&self) -> &[CompiledRoute] {
        &self.routes[..self.route_count]
    }

    /// Free-run LFO cycles per second at `bpm`.
    pub fn lfo_hz(&self, lfo: usize, bpm: f64) -> f64 {
        let lfo = &self.lfos[lfo.min(MAX_LFOS - 1)];
        if lfo.sync_beats > 0.0 {
            bpm / 60.0 / f64::from(lfo.sync_beats)
        } else {
            f64::from(lfo.rate_hz)
        }
    }
}
