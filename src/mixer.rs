#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothedParam {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
}

impl SmoothedParam {
    pub fn new(value: f32) -> Self {
        let value = if value.is_finite() { value } else { 0.0 };
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    pub fn current(&self) -> f32 {
        self.current
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn set_target(&mut self, target: f32, frames: u32) {
        let target = if target.is_finite() { target } else { 0.0 };
        self.target = target;
        if frames == 0 {
            self.current = target;
            self.step = 0.0;
            self.remaining = 0;
            return;
        }

        self.step = (target - self.current) / frames as f32;
        self.remaining = frames;
    }

    pub fn next_value(&mut self) -> f32 {
        if self.remaining == 0 {
            return self.current;
        }

        if self.remaining == 1 {
            self.current = self.target;
            self.remaining = 0;
            self.step = 0.0;
            return self.current;
        }

        self.current += self.step;
        self.remaining -= 1;
        self.current
    }
}

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

    /// Stereo sources use a balance law instead of the mono constant-power
    /// pan: centre leaves both channels untouched, and panning attenuates the
    /// opposite side only, so a stereo image is never folded or boosted.
    pub fn process_stereo(&self, left: f32, right: f32) -> (f32, f32) {
        if self.muted || !left.is_finite() || !right.is_finite() {
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
        let left_gain = (1.0 - pan).min(1.0);
        let right_gain = (1.0 + pan).min(1.0);
        (
            protect_master(left * gain * left_gain),
            protect_master(right * gain * right_gain),
        )
    }
}

pub fn protect_master(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    sample.tanh().clamp(-1.0, 1.0)
}
