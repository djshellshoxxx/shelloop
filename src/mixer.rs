use std::f32::consts::FRAC_PI_4;

#[derive(Debug, Clone, Copy)]
pub struct ChannelStrip {
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
}

impl Default for ChannelStrip {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            muted: false,
        }
    }
}

impl ChannelStrip {
    pub fn process_mono(&self, sample: f32) -> (f32, f32) {
        if self.muted || !sample.is_finite() {
            return (0.0, 0.0);
        }

        let gain = if self.gain.is_finite() {
            self.gain.max(0.0)
        } else {
            0.0
        };
        let pan = if self.pan.is_finite() {
            self.pan.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let angle = (pan + 1.0) * FRAC_PI_4;
        let scaled = sample * gain;
        let left = protect_master(scaled * angle.cos());
        let right = protect_master(scaled * angle.sin());
        (left, right)
    }
}

pub fn protect_master(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    sample.tanh().clamp(-1.0, 1.0)
}
