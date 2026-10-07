use crate::params::{
    sample_param_descriptor, synth_param_descriptor, track_param_descriptor, EffectParamId,
    EffectSlotId, SampleParamId, TrackParamId,
};
use crate::variation::InvariantProfile;
use crate::SynthParamId;
use serde::{Deserialize, Serialize};

pub const MAX_PATTERN_STEPS: usize = 256;
/// Maximum parameter locks on one step.
pub const MAX_LOCKS_PER_STEP: usize = 16;
/// Maximum parameter locks across a whole pattern; bounds the compiled pool.
pub const MAX_PATTERN_LOCKS: usize = 512;
/// Effect insert slots a lock may address on its own track.
pub const MAX_LOCK_EFFECT_SLOTS: u8 = 4;
/// Upper bound on effect parameter indices a lock may address.
pub const MAX_LOCK_EFFECT_PARAMS: u8 = 16;
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

/// What a per-step parameter lock overrides on its own track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockTarget {
    Track(TrackParamId),
    Synth(SynthParamId),
    Sample(SampleParamId),
    /// A parameter of one of this track's insert effects.
    Effect {
        slot: EffectSlotId,
        param: EffectParamId,
    },
}

impl LockTarget {
    /// Validate that the target is lockable and the value is in range.
    /// Effect parameter ranges are checked again when the engine resolves
    /// the effect type; here only the address bounds are enforced.
    pub fn validate_value(self, value: f32) -> Result<(), String> {
        if !value.is_finite() {
            return Err("lock value must be finite".into());
        }
        let descriptor = match self {
            Self::Track(param) => track_param_descriptor(param),
            Self::Synth(param) => synth_param_descriptor(param),
            Self::Sample(param) => sample_param_descriptor(param),
            Self::Effect { slot, param } => {
                if slot.0 >= MAX_LOCK_EFFECT_SLOTS {
                    return Err(format!(
                        "effect lock slot {} exceeds {} insert slots",
                        slot.0, MAX_LOCK_EFFECT_SLOTS
                    ));
                }
                if param.0 >= MAX_LOCK_EFFECT_PARAMS {
                    return Err(format!("effect lock parameter {} is out of range", param.0));
                }
                return Ok(());
            }
        };
        if !descriptor.lockable {
            return Err(format!("parameter {} is not lockable", descriptor.name));
        }
        if !(descriptor.min..=descriptor.max).contains(&value) {
            return Err(format!(
                "lock value {value} for {} must be between {} and {}",
                descriptor.name, descriptor.min, descriptor.max
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParameterLock {
    pub target: LockTarget,
    pub value: f32,
}

/// Locks attached to one zero-based step. A step whose `steps` entry is
/// `None` but has locks is a trigless lock step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepLocks {
    pub step: u16,
    pub locks: Vec<ParameterLock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub seed: u64,
    pub swing: f32,
    pub channel: u8,
    pub steps: Vec<Option<PatternStep>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locks: Vec<StepLocks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invariants: Option<InvariantProfile>,
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
            locks: Vec::new(),
            invariants: None,
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
            if step.note > 127 {
                return Err(format!("step {index} note must be a MIDI note 0..=127"));
            }
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
        self.validate_locks()?;
        if let Some(profile) = &self.invariants {
            profile.validate(self.steps.len())?;
        }
        Ok(())
    }

    fn validate_locks(&self) -> Result<(), String> {
        let mut total = 0_usize;
        let mut seen_steps = std::collections::HashSet::new();
        for entry in &self.locks {
            let index = usize::from(entry.step);
            if index >= self.steps.len() {
                return Err(format!(
                    "locks reference step {index} beyond pattern length"
                ));
            }
            if !seen_steps.insert(entry.step) {
                return Err(format!("step {index} has more than one lock list"));
            }
            if entry.locks.len() > MAX_LOCKS_PER_STEP {
                return Err(format!(
                    "step {index} has {} locks; the maximum is {MAX_LOCKS_PER_STEP}",
                    entry.locks.len()
                ));
            }
            for (position, lock) in entry.locks.iter().enumerate() {
                lock.target
                    .validate_value(lock.value)
                    .map_err(|error| format!("step {index} lock: {error}"))?;
                if entry.locks[..position]
                    .iter()
                    .any(|other| other.target == lock.target)
                {
                    return Err(format!("step {index} locks the same target twice"));
                }
            }
            total += entry.locks.len();
        }
        if total > MAX_PATTERN_LOCKS {
            return Err(format!(
                "pattern has {total} locks; the maximum is {MAX_PATTERN_LOCKS}"
            ));
        }
        Ok(())
    }

    /// Locks for a zero-based step, empty when none.
    pub fn step_locks(&self, step: usize) -> &[ParameterLock] {
        self.locks
            .iter()
            .find(|entry| usize::from(entry.step) == step)
            .map_or(&[], |entry| entry.locks.as_slice())
    }

    /// Replace a step's locks, dropping the entry when empty.
    pub fn set_step_locks(&mut self, step: usize, locks: Vec<ParameterLock>) {
        self.locks.retain(|entry| usize::from(entry.step) != step);
        if !locks.is_empty() {
            self.locks.push(StepLocks {
                step: step as u16,
                locks,
            });
            self.locks.sort_by_key(|entry| entry.step);
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

const EMPTY_LOCK: ParameterLock = ParameterLock {
    target: LockTarget::Track(TrackParamId::Gain),
    value: 0.0,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledPattern {
    seed: u64,
    swing: f32,
    channel: u8,
    len: u16,
    steps: [Option<PatternStep>; MAX_PATTERN_STEPS],
    /// (start, len) into `lock_pool` per step.
    lock_ranges: [(u16, u8); MAX_PATTERN_STEPS],
    lock_pool: [ParameterLock; MAX_PATTERN_LOCKS],
    lock_count: u16,
}

impl CompiledPattern {
    pub fn from_pattern(pattern: &Pattern) -> Result<Self, String> {
        pattern.validate()?;
        let mut steps = [None; MAX_PATTERN_STEPS];
        steps[..pattern.steps.len()].copy_from_slice(&pattern.steps);
        let mut lock_ranges = [(0_u16, 0_u8); MAX_PATTERN_STEPS];
        let mut lock_pool = [EMPTY_LOCK; MAX_PATTERN_LOCKS];
        let mut lock_count = 0_usize;
        for entry in &pattern.locks {
            let start = lock_count;
            for lock in &entry.locks {
                lock_pool[lock_count] = *lock;
                lock_count += 1;
            }
            lock_ranges[usize::from(entry.step)] = (start as u16, entry.locks.len() as u8);
        }

        Ok(Self {
            seed: pattern.seed,
            swing: pattern.swing,
            channel: pattern.channel,
            len: pattern.steps.len() as u16,
            steps,
            lock_ranges,
            lock_pool,
            lock_count: lock_count as u16,
        })
    }

    pub fn has_locks(&self) -> bool {
        self.lock_count > 0
    }

    /// Locks compiled for a zero-based step.
    pub fn step_locks(&self, step: usize) -> &[ParameterLock] {
        match self.lock_ranges.get(step) {
            Some(&(start, len)) if len > 0 => {
                let start = usize::from(start);
                &self.lock_pool[start..start + usize::from(len)]
            }
            _ => &[],
        }
    }

    pub fn swing(&self) -> f32 {
        self.swing
    }

    pub fn seed(&self) -> u64 {
        self.seed
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

/// A step boundary for parameter-lock ownership (see
/// [`PatternScheduler::schedule_lock_boundaries_into`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockBoundary {
    pub absolute_frame: u64,
    pub step_index: u16,
    pub triggered: bool,
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
            PatternScheduleView {
                seed: pattern.seed,
                swing: pattern.swing,
                channel: pattern.channel,
                steps: &pattern.steps,
            },
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
            PatternScheduleView {
                seed: pattern.seed,
                swing: pattern.swing,
                channel: pattern.channel,
                steps: pattern.steps(),
            },
            block_start_frame,
            block_frames,
            events,
        );
    }

    fn schedule_steps_into(
        &self,
        pattern: PatternScheduleView<'_>,
        block_start_frame: u64,
        block_frames: u32,
        events: &mut Vec<PatternEvent>,
    ) {
        let frames_per_step = self.frames_per_step();
        let loop_frames = frames_per_step * pattern.steps.len() as f64;
        let block_end = block_start_frame.saturating_add(block_frames as u64);
        let margin = loop_margin(pattern.steps, loop_frames);
        let estimated_first = (block_start_frame as f64 / loop_frames).floor() as i64 - margin;
        let estimated_last = (block_end as f64 / loop_frames).floor() as i64 + margin;
        let first_loop = estimated_first.max(0) as u64;
        let last_loop = estimated_last.max(0) as u64;

        'loops: for loop_index in first_loop..=last_loop {
            for (step_index, maybe_step) in pattern.steps.iter().enumerate() {
                let Some(step) = maybe_step else { continue };
                if !self.step_triggers(pattern.seed, loop_index, step_index, step.probability) {
                    continue;
                }

                let global_step = loop_index
                    .saturating_mul(pattern.steps.len() as u64)
                    .saturating_add(step_index as u64);
                let base_frame = (global_step as f64 * frames_per_step).round() as i128;
                let swing_frames = if step_index % 2 == 1 {
                    (frames_per_step * pattern.swing as f64).round() as i128
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
                        channel: pattern.channel,
                        note: step.note,
                        velocity: step.velocity,
                        duration_frames,
                        step_index: step_index as u16,
                        ratchet_index,
                    });
                }
            }
        }

        // Unstable sort: no scratch allocation on the audio thread; the key
        // is total, so the order is still deterministic.
        events.sort_unstable_by_key(|event| {
            (
                event.absolute_frame,
                event.step_index,
                event.ratchet_index,
                event.note,
            )
        });
    }

    /// Step boundaries used for parameter-lock ownership: one per grid step,
    /// at the step's swung (and, for active steps, microtimed) start frame.
    /// `triggered` carries the same deterministic probability decision used
    /// for notes, so probability gates trigger and locks together.
    pub fn schedule_lock_boundaries_into(
        &self,
        pattern: &CompiledPattern,
        block_start_frame: u64,
        block_frames: u32,
        boundaries: &mut Vec<LockBoundary>,
    ) {
        boundaries.clear();
        if block_frames == 0 || pattern.is_empty() || !pattern.has_locks() {
            return;
        }
        let steps = pattern.steps();
        let frames_per_step = self.frames_per_step();
        let loop_frames = frames_per_step * steps.len() as f64;
        let block_end = block_start_frame.saturating_add(u64::from(block_frames));
        let margin = loop_margin(steps, loop_frames);
        let first_loop =
            ((block_start_frame as f64 / loop_frames).floor() as i64 - margin).max(0) as u64;
        let last_loop = ((block_end as f64 / loop_frames).floor() as i64 + margin).max(0) as u64;
        'loops: for loop_index in first_loop..=last_loop {
            for (step_index, step) in steps.iter().enumerate() {
                let global_step = loop_index
                    .saturating_mul(steps.len() as u64)
                    .saturating_add(step_index as u64);
                let base = (global_step as f64 * frames_per_step).round() as i128;
                let swing = if step_index % 2 == 1 {
                    (frames_per_step * pattern.swing as f64).round() as i128
                } else {
                    0
                };
                let micro = step.map_or(0, |step| step.microtiming_frames as i128);
                let absolute = base + swing + micro;
                if absolute < 0 {
                    continue;
                }
                let absolute = absolute as u64;
                if absolute < block_start_frame || absolute >= block_end {
                    continue;
                }
                if boundaries.len() >= MAX_EVENTS_PER_BLOCK {
                    break 'loops;
                }
                let triggered = match step {
                    Some(step) => {
                        self.step_triggers(pattern.seed, loop_index, step_index, step.probability)
                    }
                    None => true,
                };
                boundaries.push(LockBoundary {
                    absolute_frame: absolute,
                    step_index: step_index as u16,
                    triggered,
                });
            }
        }
        boundaries.sort_unstable_by_key(|boundary| (boundary.absolute_frame, boundary.step_index));
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

#[derive(Debug, Clone, Copy)]
struct PatternScheduleView<'a> {
    seed: u64,
    swing: f32,
    channel: u8,
    steps: &'a [Option<PatternStep>],
}

/// Loops to scan either side of a block: microtiming may move a step by
/// more than a whole (short) loop.
fn loop_margin(steps: &[Option<PatternStep>], loop_frames: f64) -> i64 {
    let max_micro = steps
        .iter()
        .flatten()
        .map(|step| step.microtiming_frames.unsigned_abs())
        .max()
        .unwrap_or(0);
    (f64::from(max_micro) / loop_frames).ceil() as i64 + 1
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
