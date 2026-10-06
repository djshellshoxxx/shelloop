#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrParams {
    pub attack_secs: f32,
    pub decay_secs: f32,
    pub sustain: f32,
    pub release_secs: f32,
}

impl AdsrParams {
    pub fn new(
        attack_secs: f32,
        decay_secs: f32,
        sustain: f32,
        release_secs: f32,
    ) -> Result<Self, String> {
        let params = Self {
            attack_secs,
            decay_secs,
            sustain,
            release_secs,
        };
        params.validate()?;
        Ok(params)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value, max) in [
            ("attack", self.attack_secs, 30.0),
            ("decay", self.decay_secs, 30.0),
            ("release", self.release_secs, 60.0),
        ] {
            if !value.is_finite() || !(0.0..=max).contains(&value) {
                return Err(format!("{name} must be finite and between 0 and {max} seconds"));
            }
        }
        if !self.sustain.is_finite() || !(0.0..=1.0).contains(&self.sustain) {
            return Err("sustain must be finite and between 0.0 and 1.0".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone)]
pub struct AdsrEnvelope {
    sample_rate: f32,
    params: AdsrParams,
    stage: EnvelopeStage,
    level: f32,
    step: f32,
    remaining: u32,
}

impl AdsrEnvelope {
    pub fn new(sample_rate: f32, params: AdsrParams) -> Result<Self, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("envelope sample rate must be finite and greater than zero".into());
        }
        params.validate()?;
        Ok(Self {
            sample_rate,
            params,
            stage: EnvelopeStage::Idle,
            level: 0.0,
            step: 0.0,
            remaining: 0,
        })
    }

    pub fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_active(&self) -> bool {
        self.stage != EnvelopeStage::Idle
    }

    pub fn note_on(&mut self) {
        self.level = 0.0;
        let samples = self.samples(self.params.attack_secs);
        if samples == 0 {
            self.level = 1.0;
            self.begin_decay();
        } else {
            self.stage = EnvelopeStage::Attack;
            self.remaining = samples;
            self.step = 1.0 / samples as f32;
        }
    }

    pub fn note_off(&mut self) {
        if self.stage == EnvelopeStage::Idle {
            return;
        }
        let samples = self.samples(self.params.release_secs);
        if samples == 0 {
            self.reset();
        } else {
            self.stage = EnvelopeStage::Release;
            self.remaining = samples;
            self.step = -self.level / samples as f32;
        }
    }

    pub fn reset(&mut self) {
        self.stage = EnvelopeStage::Idle;
        self.level = 0.0;
        self.step = 0.0;
        self.remaining = 0;
    }

    pub fn next_value(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => return 0.0,
            EnvelopeStage::Sustain => return self.level,
            EnvelopeStage::Attack | EnvelopeStage::Decay | EnvelopeStage::Release => {}
        }

        self.level = (self.level + self.step).clamp(0.0, 1.0);
        self.remaining = self.remaining.saturating_sub(1);

        if self.remaining == 0 {
            match self.stage {
                EnvelopeStage::Attack => {
                    self.level = 1.0;
                    self.begin_decay();
                }
                EnvelopeStage::Decay => {
                    self.level = self.params.sustain;
                    self.stage = EnvelopeStage::Sustain;
                    self.step = 0.0;
                }
                EnvelopeStage::Release => self.reset(),
                EnvelopeStage::Idle | EnvelopeStage::Sustain => {}
            }
        }

        self.level
    }

    fn begin_decay(&mut self) {
        let samples = self.samples(self.params.decay_secs);
        if samples == 0 {
            self.level = self.params.sustain;
            self.stage = EnvelopeStage::Sustain;
            self.step = 0.0;
            self.remaining = 0;
        } else {
            self.stage = EnvelopeStage::Decay;
            self.remaining = samples;
            self.step = (self.params.sustain - 1.0) / samples as f32;
        }
    }

    fn samples(&self, seconds: f32) -> u32 {
        if seconds <= 0.0 {
            0
        } else {
            (seconds * self.sample_rate).round().max(1.0) as u32
        }
    }
}
