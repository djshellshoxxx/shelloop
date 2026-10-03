use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Oscillator {
    Sine,
    Triangle,
    Saw,
    Pulse,
}

#[derive(Debug, Clone)]
pub struct SynthVoice {
    sample_rate: f32,
    oscillator: Oscillator,
    frequency_hz: f32,
    velocity: f32,
    phase: f32,
}

impl SynthVoice {
    pub fn new(sample_rate: f32, oscillator: Oscillator) -> Self {
        Self {
            sample_rate,
            oscillator,
            frequency_hz: 0.0,
            velocity: 0.0,
            phase: 0.0,
        }
    }

    pub fn note_on(&mut self, frequency_hz: f32, velocity: f32) {
        self.frequency_hz = if frequency_hz.is_finite() {
            frequency_hz.clamp(0.0, self.sample_rate.max(1.0) * 0.49)
        } else {
            0.0
        };
        self.velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    pub fn note_off(&mut self) {
        self.velocity = 0.0;
    }

    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = Vec::with_capacity(frames);
        if self.sample_rate <= 0.0 || !self.sample_rate.is_finite() {
            output.resize(frames, 0.0);
            return output;
        }

        let phase_increment = self.frequency_hz / self.sample_rate;
        for _ in 0..frames {
            let raw = match self.oscillator {
                Oscillator::Sine => (self.phase * TAU).sin(),
                Oscillator::Triangle => 1.0 - 4.0 * (self.phase - 0.5).abs(),
                Oscillator::Saw => 2.0 * self.phase - 1.0,
                Oscillator::Pulse => {
                    if self.phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            output.push((raw * self.velocity).clamp(-1.0, 1.0));
            self.phase = (self.phase + phase_increment).fract();
        }

        output
    }
}
