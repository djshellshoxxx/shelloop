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
    pub fn process_mono(&self, _sample: f32) -> (f32, f32) {
        (0.0, 0.0)
    }
}

pub fn protect_master(_sample: f32) -> f32 {
    0.0
}
