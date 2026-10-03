#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone)]
pub struct Transport {
    state: TransportState,
    frame_position: u64,
    bpm: f64,
}

impl Transport {
    pub fn new(bpm: f64) -> Self {
        Self {
            state: TransportState::Stopped,
            frame_position: 0,
            bpm: if bpm.is_finite() && (20.0..=400.0).contains(&bpm) {
                bpm
            } else {
                120.0
            },
        }
    }

    pub fn state(&self) -> TransportState {
        self.state
    }

    pub fn frame_position(&self) -> u64 {
        self.frame_position
    }

    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    pub fn play(&mut self) {
        self.state = TransportState::Playing;
    }

    pub fn pause(&mut self) {
        if self.state == TransportState::Playing {
            self.state = TransportState::Paused;
        }
    }

    pub fn stop(&mut self) {
        self.state = TransportState::Stopped;
        self.frame_position = 0;
    }

    pub fn restart(&mut self) {
        self.frame_position = 0;
        self.state = TransportState::Playing;
    }

    pub fn set_bpm(&mut self, bpm: f64) -> Result<(), String> {
        if !bpm.is_finite() || !(20.0..=400.0).contains(&bpm) {
            return Err("bpm must be finite and between 20 and 400".into());
        }
        self.bpm = bpm;
        Ok(())
    }

    pub fn advance(&mut self, frames: u32) {
        if self.state == TransportState::Playing {
            self.frame_position = self.frame_position.saturating_add(frames as u64);
        }
    }
}
