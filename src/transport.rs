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
            bpm,
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

    pub fn play(&mut self) {}

    pub fn pause(&mut self) {}

    pub fn stop(&mut self) {}

    pub fn restart(&mut self) {}

    pub fn set_bpm(&mut self, _bpm: f64) -> Result<(), String> {
        Err("tempo changes not implemented".into())
    }

    pub fn advance(&mut self, _frames: u32) {}
}
