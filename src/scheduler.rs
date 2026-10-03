#[derive(Debug, Clone, PartialEq)]
pub struct StepEvent {
    pub step: u32,
    pub note: u8,
    pub velocity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledEvent {
    pub frame_offset: u32,
    pub note: u8,
    pub velocity: f32,
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    sample_rate: u32,
    bpm: f64,
    steps_per_beat: u32,
}

impl Scheduler {
    pub fn new(sample_rate: u32, bpm: f64, steps_per_beat: u32) -> Self {
        Self {
            sample_rate,
            bpm,
            steps_per_beat,
        }
    }

    pub fn schedule_block(
        &self,
        _block_start_frame: u64,
        _block_frames: u32,
        _events: &[StepEvent],
    ) -> Vec<ScheduledEvent> {
        let _ = (self.sample_rate, self.bpm, self.steps_per_beat);
        Vec::new()
    }
}
