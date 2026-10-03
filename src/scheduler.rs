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

    pub fn frames_per_step(&self) -> Option<f64> {
        if self.sample_rate == 0
            || self.steps_per_beat == 0
            || !self.bpm.is_finite()
            || self.bpm <= 0.0
        {
            return None;
        }

        Some((self.sample_rate as f64 * 60.0) / (self.bpm * self.steps_per_beat as f64))
    }

    pub fn schedule_block(
        &self,
        block_start_frame: u64,
        block_frames: u32,
        events: &[StepEvent],
    ) -> Vec<ScheduledEvent> {
        let Some(frames_per_step) = self.frames_per_step() else {
            return Vec::new();
        };

        let block_end = block_start_frame.saturating_add(block_frames as u64);
        let mut scheduled = Vec::with_capacity(events.len().min(block_frames as usize));

        for event in events {
            let event_frame = (event.step as f64 * frames_per_step).round() as u64;
            if event_frame >= block_start_frame && event_frame < block_end {
                scheduled.push(ScheduledEvent {
                    frame_offset: (event_frame - block_start_frame) as u32,
                    note: event.note,
                    velocity: event.velocity.clamp(0.0, 1.0),
                });
            }
        }

        scheduled.sort_by_key(|event| event.frame_offset);
        scheduled
    }
}
