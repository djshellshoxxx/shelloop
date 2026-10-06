use serde::{Deserialize, Serialize};

pub const MAX_PATTERN_STEPS: usize = 256;
const MAX_RATCHETS: u8 = 8;
const MAX_MICROTIMING_FRAMES: i32 = 192_000;
const MAX_EVENTS_PER_BLOCK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PatternStep {
    pub note: u8,
    pub velocity: f32,
    pub gate: f32,
    pub probability: f32,
    pub ratchets: u8,
    pub microtiming_frames: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub seed: u64,
    pub swing: f32,
    pub channel: u8,
    pub steps: Vec<Option<PatternStep>>,
}

impl Pattern {
    pub fn new(
        name: impl Into<String>,
        seed: u64,
        swing: f32,
        channel: u8,
        steps: Vec<Option<PatternStep>>,
    ) -> Result<Self, String> {
        let pattern = Self {
            name: name.into(),
            seed,
            swing,
            channel,
            steps,
        };
        pattern.validate()?;
        Ok(pattern)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("pattern name may not be empty".into());
        }
        if self.steps.is_empty() || self.steps.len() > MAX_PATTERN_STEPS {
            return Err(format!(
                "pattern length must be between 1 and {MAX_PATTERN_STEPS} steps"
            ));
        }
        if !self.swing.is_finite() || !(0.0..=0.75).contains(&self.swing) {
            return Err("swing must be finite and between 0.0 and 0.75 step".into());
        }
        if self.channel > 15 {
            return Err("MIDI channel must be in the zero-based range 0..=15".into());
        }

        for (index, step) in self.steps.iter().enumerate() {
            let Some(step) = step else { continue };
            if !step.velocity.is_finite() || !(0.0..=1.0).contains(&step.velocity) {
                return Err(format!(
                    "step {index} velocity must be finite and 0.0..=1.0"
                ));
            }
            if !step.gate.is_finite() || !(0.0..=1.0).contains(&step.gate) {
                return Err(format!("step {index} gate must be finite and 0.0..=1.0"));
            }
            if !step.probability.is_finite() || !(0.0..=1.0).contains(&step.probability) {
                return Err(format!(
                    "step {index} probability must be finite and 0.0..=1.0"
                ));
            }
            if !(1..=MAX_RATCHETS).contains(&step.ratchets) {
                return Err(format!(
                    "step {index} ratchets must be between 1 and {MAX_RATCHETS}"
                ));
            }
            if step.microtiming_frames.unsigned_abs() > MAX_MICROTIMING_FRAMES as u32 {
                return Err(format!(
                    "step {index} microtiming exceeds {MAX_MICROTIMING_FRAMES} frames"
                ));
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledPattern {
    seed: u64,
    swing: f32,
    channel: u8,
    len: u16,
    steps: [Option<PatternStep>; MAX_PATTERN_STEPS],
}

impl CompiledPattern {
    pub fn from_pattern(pattern: &Pattern) -> Result<Self, String> {
        pattern.validate()?;
        let mut steps = [None; MAX_PATTERN_STEPS];
        steps[..pattern.steps.len()].copy_from_slice(&pattern.steps);

        Ok(Self {
            seed: pattern.seed,
            swing: pattern.swing,
            channel: pattern.channel,
            len: pattern.steps.len() as u16,
            steps,
        })
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn steps(&self) -> &[Option<PatternStep>] {
        &self.steps[..self.len()]
    }
}

pub fn parse_pattern_json(json: &str) -> Result<Pattern, String> {
    let pattern: Pattern =
        serde_json::from_str(json).map_err(|error| format!("invalid pattern JSON: {error}"))?;
    pattern.validate()?;
    Ok(pattern)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternEvent {
    pub absolute_frame: u64,
    pub frame_offset: u32,
    pub channel: u8,
    pub note: u8,
    pub velocity: f32,
    pub duration_frames: u32,
    pub step_index: u16,
    pub ratchet_index: u8,
}

#[derive(Debug, Clone)]
pub struct PatternScheduler {
    sample_rate: u32,
    bpm: f64,
    steps_per_beat: u32,
    project_seed: u64,
}

impl PatternScheduler {
    pub fn new(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
    ) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be greater than zero".into());
        }
        if !bpm.is_finite() || !(20.0..=400.0).contains(&bpm) {
            return Err("bpm must be finite and between 20 and 400".into());
        }
        if steps_per_beat == 0 || steps_per_beat > 64 {
            return Err("steps per beat must be between 1 and 64".into());
        }
        Ok(Self {
            sample_rate,
            bpm,
            steps_per_beat,
            project_seed,
        })
    }

    pub fn frames_per_step(&self) -> f64 {
        (self.sample_rate as f64 * 60.0) / (self.bpm * self.steps_per_beat as f64)
    }

    pub fn schedule_block(
        &self,
        pattern: &Pattern,
        block_start_frame: u64,
        block_frames: u32,
    ) -> Vec<PatternEvent> {
        let mut events = Vec::with_capacity(MAX_EVENTS_PER_BLOCK);
        self.schedule_block_into(pattern, block_start_frame, block_frames, &mut events);
        events
    }

    pub fn schedule_block_into(
        &self,
        pattern: &Pattern,
        block_start_frame: u64,
        block_frames: u32,
        events: &mut Vec<PatternEvent>,
    ) {
        events.clear();
        if block_frames == 0 || pattern.validate().is_err() {
            return;
        }
        self.schedule_steps_into(
            pattern.seed,
            pattern.swing,
            pattern.channel,
            &pattern.steps,
            block_start_frame,
            block_frames,
            events,
        );
    }

    pub fn schedule_compiled_block_into(
        &self,
        pattern: &CompiledPattern,
        block_start_frame: u64,
        block_frames: u32,
        events: &mut Vec<PatternEvent>,
    ) {
        events.clear();
        if block_frames == 0 || pattern.is_empty() {
            return;
        }
        self.schedule_steps_into(
            pattern.seed,
            pattern.swing,
            pattern.channel,
            pattern.steps(),
            block_start_frame,
            block_frames,
            events,
        );
    }

    fn schedule_steps_into(
        &self,
        seed: u64,
        swing: f32,
        channel: u8,
        steps: &[Option<PatternStep>],
        block_start_frame: u64,
        block_frames: u32,
        events: &mut Vec<PatternEvent>,
    ) {
        let frames_per_step = self.frames_per_step();
        let loop_frames = frames_per_step * steps.len() as f64;
        let block_end = block_start_frame.saturating_add(block_frames as u64);
        let estimated_first = (block_start_frame as f64 / loop_frames).floor() as i64 - 1;
        let estimated_last = (block_end as f64 / loop_frames).floor() as i64 + 1;
        let first_loop = estimated_first.max(0) as u64;
        let last_loop = estimated_last.max(0) as u64;

        'loops: for loop_index in first_loop..=last_loop {
            for (step_index, maybe_step) in steps.iter().enumerate() {
                let Some(step) = maybe_step else { continue };
                if !self.step_triggers(seed, loop_index, step_index, step.probability) {
                    continue;
                }

                let global_step = loop_index
                    .saturating_mul(steps.len() as u64)
                    .saturating_add(step_index as u64);
                let base_frame = (global_step as f64 * frames_per_step).round() as i128;
                let swing_frames = if step_index % 2 == 1 {
                    (frames_per_step * swing as f64).round() as i128
                } else {
                    0
                };
                let step_start = base_frame + swing_frames + step.microtiming_frames as i128;
                let ratchet_spacing = frames_per_step / step.ratchets as f64;
                let duration_frames = ((ratchet_spacing * step.gate as f64).round() as u64)
                    .clamp(1, u32::MAX as u64) as u32;

                for ratchet_index in 0..step.ratchets {
                    let ratchet_offset = (ratchet_spacing * ratchet_index as f64).round() as i128;
                    let absolute = step_start + ratchet_offset;
                    if absolute < 0 {
                        continue;
                    }
                    let absolute_frame = absolute as u64;
                    if absolute_frame < block_start_frame || absolute_frame >= block_end {
                        continue;
                    }
                    if events.len() >= MAX_EVENTS_PER_BLOCK {
                        break 'loops;
                    }
                    events.push(PatternEvent {
                        absolute_frame,
                        frame_offset: (absolute_frame - block_start_frame) as u32,
                        channel,
                        note: step.note,
                        velocity: step.velocity,
                        duration_frames,
                        step_index: step_index as u16,
                        ratchet_index,
                    });
                }
            }
        }

        events.sort_by_key(|event| {
            (
                event.absolute_frame,
                event.step_index,
                event.ratchet_index,
                event.note,
            )
        });
    }

    fn step_triggers(
        &self,
        pattern_seed: u64,
        loop_index: u64,
        step_index: usize,
        probability: f32,
    ) -> bool {
        if probability <= 0.0 {
            return false;
        }
        if probability >= 1.0 {
            return true;
        }

        let mut key = self.project_seed ^ pattern_seed.rotate_left(17);
        key ^= loop_index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        key ^= (step_index as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let random = splitmix64(key);
        let unit = (random >> 11) as f64 * (1.0 / ((1u64 << 53) as f64));
        unit < probability as f64
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
