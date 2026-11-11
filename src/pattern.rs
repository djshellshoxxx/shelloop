use crate::music::humanize::{splitmix64, Humanize};
use crate::music::ratio::{ratio_fires, StepRatio};
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
/// Extra simultaneous notes per step (a chord of up to six notes).
pub const MAX_CHORD_EXTRA_NOTES: usize = 5;
/// At most one chord per step.
pub const MAX_PATTERN_CHORDS: usize = MAX_PATTERN_STEPS;
const HUMANIZE_TIMING_SALT: u64 = 0x7419_A3C5_0F2E_8B11;
const HUMANIZE_VELOCITY_SALT: u64 = 0x51D7_C2F9_44AB_9E63;

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

/// Extra notes played together with a step's own note (the step note is
/// always the lowest voice of the chord). Ascending order is not required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepChord {
    pub step: u16,
    pub notes: Vec<u8>,
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
    /// Extra chord notes per step. Absent from JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chords: Vec<StepChord>,
    /// Ratio conditions (`1:2`, `3:4`) per step. Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ratios: Vec<StepRatio>,
    /// Timing and velocity jitter. Absent when default.
    #[serde(default, skip_serializing_if = "Humanize::is_default")]
    pub humanize: Humanize,
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
            chords: Vec::new(),
            ratios: Vec::new(),
            humanize: Humanize::default(),
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
        self.validate_musical()?;
        if let Some(profile) = &self.invariants {
            profile.validate(self.steps.len())?;
        }
        Ok(())
    }

    fn validate_musical(&self) -> Result<(), String> {
        self.humanize.validate()?;
        if self.chords.len() > MAX_PATTERN_CHORDS {
            return Err(format!("pattern has more than {MAX_PATTERN_CHORDS} chords"));
        }
        let mut chord_steps = std::collections::HashSet::new();
        for chord in &self.chords {
            let index = usize::from(chord.step);
            if index >= self.steps.len() {
                return Err(format!(
                    "chord references step {index} beyond pattern length"
                ));
            }
            let Some(step) = self.steps[index] else {
                return Err(format!("step {index} has a chord but no note"));
            };
            if !chord_steps.insert(chord.step) {
                return Err(format!("step {index} has more than one chord"));
            }
            if chord.notes.is_empty() || chord.notes.len() > MAX_CHORD_EXTRA_NOTES {
                return Err(format!(
                    "step {index} chord must have 1..={MAX_CHORD_EXTRA_NOTES} extra notes"
                ));
            }
            for (position, note) in chord.notes.iter().enumerate() {
                if *note > 127 {
                    return Err(format!("step {index} chord note must be 0..=127"));
                }
                if *note == step.note || chord.notes[..position].contains(note) {
                    return Err(format!("step {index} chord repeats a note"));
                }
            }
        }
        let mut ratio_steps = std::collections::HashSet::new();
        for ratio in &self.ratios {
            ratio.validate(self.steps.len())?;
            if self.steps[usize::from(ratio.step)].is_none() {
                return Err(format!("step {} has a ratio but no note", ratio.step));
            }
            if !ratio_steps.insert(ratio.step) {
                return Err(format!("step {} has more than one ratio", ratio.step));
            }
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
    /// Extra chord notes per step; `chord_len[step]` entries are valid.
    chord_notes: [[u8; MAX_CHORD_EXTRA_NOTES]; MAX_PATTERN_STEPS],
    chord_len: [u8; MAX_PATTERN_STEPS],
    /// (play, of) per step; `of == 0` means no ratio condition.
    ratios: [(u8, u8); MAX_PATTERN_STEPS],
    humanize: Humanize,
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

        let mut chord_notes = [[0_u8; MAX_CHORD_EXTRA_NOTES]; MAX_PATTERN_STEPS];
        let mut chord_len = [0_u8; MAX_PATTERN_STEPS];
        for chord in &pattern.chords {
            let index = usize::from(chord.step);
            chord_notes[index][..chord.notes.len()].copy_from_slice(&chord.notes);
            chord_len[index] = chord.notes.len() as u8;
        }
        let mut ratios = [(0_u8, 0_u8); MAX_PATTERN_STEPS];
        for ratio in &pattern.ratios {
            ratios[usize::from(ratio.step)] = (ratio.play, ratio.of);
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
            chord_notes,
            chord_len,
            ratios,
            humanize: pattern.humanize,
        })
    }

    pub fn has_locks(&self) -> bool {
        self.lock_count > 0
    }

    /// Extra chord notes compiled for a zero-based step.
    fn chord_notes_for(&self, step: usize) -> &[u8] {
        match self.chord_len.get(step) {
            Some(&len) if len > 0 => &self.chord_notes[step][..usize::from(len)],
            _ => &[],
        }
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
    /// Tempo origin: `origin_step` is the (fractional) grid step that falls
    /// on `origin_frame`. Zero for a scheduler that was never retimed, so
    /// the grid is the same as before tempo changes existed.
    origin_frame: u64,
    origin_step: f64,
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
            origin_frame: 0,
            origin_step: 0.0,
        })
    }

    pub fn frames_per_step(&self) -> f64 {
        (self.sample_rate as f64 * 60.0) / (self.bpm * self.steps_per_beat as f64)
    }

    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    pub fn steps_per_beat(&self) -> u32 {
        self.steps_per_beat
    }

    /// Fractional grid step at `frame` under the current tempo origin.
    pub fn step_position(&self, frame: u64) -> f64 {
        self.origin_step + (frame as f64 - self.origin_frame as f64) / self.frames_per_step()
    }

    /// Frame at which grid step `global_step` starts.
    fn frame_of_step(&self, global_step: u64) -> f64 {
        self.origin_frame as f64 + (global_step as f64 - self.origin_step) * self.frames_per_step()
    }

    /// Beat index (grid steps divided by steps per beat) at `frame`, or
    /// `None` before the tempo origin.
    pub fn beat_index_at(&self, frame: u64) -> Option<u64> {
        let position = self.step_position(frame);
        (position >= 0.0).then(|| position.floor() as u64 / u64::from(self.steps_per_beat))
    }

    /// The same grid at a new tempo, re-anchored so the step under
    /// `at_frame` stays under it. Earlier steps are never re-emitted.
    pub fn retimed(&self, bpm: f64, at_frame: u64) -> Result<Self, String> {
        let origin_step = self.step_position(at_frame);
        let mut next = Self::new(
            self.sample_rate,
            bpm,
            self.steps_per_beat,
            self.project_seed,
        )?;
        next.origin_frame = at_frame;
        next.origin_step = origin_step;
        Ok(next)
    }

    /// Per-step key for humanize randomness. Independent of block layout.
    fn humanize_key(&self, seed: u64, loop_index: u64, step_index: usize, salt: u64) -> u64 {
        let mut key = self.project_seed ^ seed.rotate_left(17);
        key ^= loop_index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        key ^= (step_index as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        key ^ salt
    }

    fn humanize_timing(
        &self,
        seed: u64,
        humanize: Humanize,
        loop_index: u64,
        step_index: usize,
    ) -> i64 {
        humanize.timing_offset(self.humanize_key(
            seed,
            loop_index,
            step_index,
            HUMANIZE_TIMING_SALT,
        ))
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
                humanize: pattern.humanize,
                extras: StepExtras::Editable(pattern),
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
                humanize: pattern.humanize,
                extras: StepExtras::Compiled(pattern),
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
        let margin = loop_margin(pattern.steps, pattern.humanize, loop_frames);
        let length = pattern.steps.len() as f64;
        let estimated_first =
            (self.step_position(block_start_frame) / length).floor() as i64 - margin;
        let estimated_last = (self.step_position(block_end) / length).floor() as i64 + margin;
        let first_loop = estimated_first.max(0) as u64;
        let last_loop = estimated_last.max(0) as u64;

        'loops: for loop_index in first_loop..=last_loop {
            for (step_index, maybe_step) in pattern.steps.iter().enumerate() {
                let Some(step) = maybe_step else { continue };
                if !self.step_triggers(pattern.seed, loop_index, step_index, step.probability) {
                    continue;
                }
                if !pattern.extras.ratio_allows(step_index, loop_index) {
                    continue;
                }

                let global_step = loop_index
                    .saturating_mul(pattern.steps.len() as u64)
                    .saturating_add(step_index as u64);
                let base_frame = self.frame_of_step(global_step).round() as i128;
                let swing_frames = if step_index % 2 == 1 {
                    (frames_per_step * pattern.swing as f64).round() as i128
                } else {
                    0
                };
                let human_frames =
                    self.humanize_timing(pattern.seed, pattern.humanize, loop_index, step_index);
                let step_start = base_frame
                    + swing_frames
                    + step.microtiming_frames as i128
                    + human_frames as i128;
                let velocity = if pattern.humanize.velocity == 0.0 {
                    step.velocity
                } else {
                    let key = self.humanize_key(
                        pattern.seed,
                        loop_index,
                        step_index,
                        HUMANIZE_VELOCITY_SALT,
                    );
                    (step.velocity + pattern.humanize.velocity_delta(key)).clamp(0.0, 1.0)
                };
                let ratchet_spacing = frames_per_step / step.ratchets as f64;
                let duration_frames = ((ratchet_spacing * step.gate as f64).round() as u64)
                    .clamp(1, u32::MAX as u64) as u32;
                let extra_notes = pattern.extras.chord_notes(step_index);

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
                    for note in std::iter::once(step.note).chain(extra_notes.iter().copied()) {
                        if events.len() >= MAX_EVENTS_PER_BLOCK {
                            break 'loops;
                        }
                        events.push(PatternEvent {
                            absolute_frame,
                            frame_offset: (absolute_frame - block_start_frame) as u32,
                            channel: pattern.channel,
                            note,
                            velocity,
                            duration_frames,
                            step_index: step_index as u16,
                            ratchet_index,
                        });
                    }
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
        let margin = loop_margin(steps, pattern.humanize, loop_frames);
        let length = steps.len() as f64;
        let first_loop = ((self.step_position(block_start_frame) / length).floor() as i64 - margin)
            .max(0) as u64;
        let last_loop =
            ((self.step_position(block_end) / length).floor() as i64 + margin).max(0) as u64;
        'loops: for loop_index in first_loop..=last_loop {
            for (step_index, step) in steps.iter().enumerate() {
                let global_step = loop_index
                    .saturating_mul(steps.len() as u64)
                    .saturating_add(step_index as u64);
                let base = self.frame_of_step(global_step).round() as i128;
                let swing = if step_index % 2 == 1 {
                    (frames_per_step * pattern.swing as f64).round() as i128
                } else {
                    0
                };
                let micro = step.map_or(0, |step| step.microtiming_frames as i128);
                let human =
                    self.humanize_timing(pattern.seed, pattern.humanize, loop_index, step_index)
                        as i128;
                let absolute = base + swing + micro + human;
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
                            && StepExtras::Compiled(pattern).ratio_allows(step_index, loop_index)
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
    humanize: Humanize,
    extras: StepExtras<'a>,
}

/// Per-step data that lives beside the step array: chords and ratios. The
/// editable and compiled forms answer the same questions.
#[derive(Debug, Clone, Copy)]
enum StepExtras<'a> {
    Editable(&'a Pattern),
    Compiled(&'a CompiledPattern),
}

impl<'a> StepExtras<'a> {
    fn chord_notes(self, step: usize) -> &'a [u8] {
        match self {
            Self::Editable(pattern) => pattern
                .chords
                .iter()
                .find(|chord| usize::from(chord.step) == step)
                .map_or(&[][..], |chord| chord.notes.as_slice()),
            Self::Compiled(pattern) => pattern.chord_notes_for(step),
        }
    }

    fn ratio_allows(self, step: usize, loop_index: u64) -> bool {
        let (play, of) = match self {
            Self::Editable(pattern) => {
                match pattern
                    .ratios
                    .iter()
                    .find(|ratio| usize::from(ratio.step) == step)
                {
                    Some(ratio) => (ratio.play, ratio.of),
                    None => return true,
                }
            }
            Self::Compiled(pattern) => match pattern.ratios.get(step) {
                Some(&(play, of)) if of > 0 => (play, of),
                _ => return true,
            },
        };
        ratio_fires(play, of, loop_index)
    }
}

/// Loops to scan either side of a block: microtiming and humanize may move
/// a step by more than a whole (short) loop.
fn loop_margin(steps: &[Option<PatternStep>], humanize: Humanize, loop_frames: f64) -> i64 {
    let max_micro = steps
        .iter()
        .flatten()
        .map(|step| step.microtiming_frames.unsigned_abs())
        .max()
        .unwrap_or(0)
        .saturating_add(humanize.timing_frames);
    (f64::from(max_micro) / loop_frames).ceil() as i64 + 1
}
