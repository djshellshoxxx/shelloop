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
}

impl SynthVoice {
    pub fn new(sample_rate: f32, oscillator: Oscillator) -> Self {
        Self {
            sample_rate,
            oscillator,
            frequency_hz: 0.0,
            velocity: 0.0,
        }
    }

    pub fn note_on(&mut self, frequency_hz: f32, velocity: f32) {
        self.frequency_hz = frequency_hz;
        self.velocity = velocity;
    }

    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let _ = (
            self.sample_rate,
            self.oscillator,
            self.frequency_hz,
            self.velocity,
        );
        vec![0.0; frames]
    }
}
