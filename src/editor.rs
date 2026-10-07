use crate::variation::InvariantProfile;
use crate::{CompiledPattern, Pattern, PatternStep, QuantizeBoundary, QuantizedChange};
use std::collections::VecDeque;

const MAX_EDITOR_PATTERN_STEPS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct PatternRevision {
    pub revision: u64,
    pub pattern: Pattern,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledPatternRevision {
    pub revision: u64,
    pub pattern: CompiledPattern,
}

pub fn compile_pattern_revision(
    change: &QuantizedChange<PatternRevision>,
) -> Result<QuantizedChange<CompiledPatternRevision>, String> {
    Ok(QuantizedChange {
        apply_at_frame: change.apply_at_frame,
        value: CompiledPatternRevision {
            revision: change.value.revision,
            pattern: CompiledPattern::from_pattern(&change.value.pattern)?,
        },
    })
}

#[derive(Debug, Clone)]
pub struct PatternEditor {
    pattern: Pattern,
    revision: u64,
    undo: VecDeque<Pattern>,
    redo: VecDeque<Pattern>,
    history_capacity: usize,
    clipboard: Option<Option<PatternStep>>,
}

impl PatternEditor {
    pub fn new(pattern: Pattern, history_capacity: usize) -> Result<Self, String> {
        pattern.validate()?;
        if history_capacity == 0 {
            return Err("pattern editor history capacity must be greater than zero".into());
        }

        Ok(Self {
            pattern,
            revision: 0,
            undo: VecDeque::with_capacity(history_capacity),
            redo: VecDeque::with_capacity(history_capacity),
            history_capacity,
            clipboard: None,
        })
    }

    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn toggle_step(&mut self, index: usize, default_step: PatternStep) -> Result<u64, String> {
        self.ensure_step_index(index)?;
        let mut candidate = self.pattern.clone();
        candidate.steps[index] = match candidate.steps[index] {
            Some(_) => None,
            None => Some(default_step),
        };
        self.commit(candidate)
    }

    pub fn set_step(&mut self, index: usize, step: Option<PatternStep>) -> Result<u64, String> {
        self.ensure_step_index(index)?;
        let mut candidate = self.pattern.clone();
        candidate.steps[index] = step;
        self.commit(candidate)
    }

    pub fn set_length(&mut self, len: usize) -> Result<u64, String> {
        if !(1..=MAX_EDITOR_PATTERN_STEPS).contains(&len) {
            return Err(format!(
                "pattern length must be between 1 and {MAX_EDITOR_PATTERN_STEPS} steps"
            ));
        }
        let mut candidate = self.pattern.clone();
        candidate.steps.resize(len, None);
        // Locks and anchors on removed steps go with them.
        remap_step_metadata(&mut candidate, |step| (step < len).then_some(step));
        self.commit(candidate)
    }

    pub fn set_swing(&mut self, swing: f32) -> Result<u64, String> {
        let mut candidate = self.pattern.clone();
        candidate.swing = swing;
        self.commit(candidate)
    }

    pub fn rotate_left(&mut self, amount: usize) -> Result<u64, String> {
        let mut candidate = self.pattern.clone();
        let len = candidate.steps.len();
        let shift = amount % len;
        candidate.steps.rotate_left(shift);
        remap_step_metadata(&mut candidate, |step| Some((step + len - shift) % len));
        self.commit(candidate)
    }

    pub fn rotate_right(&mut self, amount: usize) -> Result<u64, String> {
        let mut candidate = self.pattern.clone();
        let len = candidate.steps.len();
        let shift = amount % len;
        candidate.steps.rotate_right(shift);
        remap_step_metadata(&mut candidate, |step| Some((step + shift) % len));
        self.commit(candidate)
    }

    pub fn copy_step(&mut self, index: usize) -> Result<(), String> {
        self.ensure_step_index(index)?;
        self.clipboard = Some(self.pattern.steps[index]);
        Ok(())
    }

    pub fn paste_step(&mut self, index: usize) -> Result<u64, String> {
        self.ensure_step_index(index)?;
        let step = self
            .clipboard
            .ok_or_else(|| "pattern editor clipboard is empty".to_string())?;
        self.set_step(index, step)
    }

    pub fn clear_step(&mut self, index: usize) -> Result<u64, String> {
        self.set_step(index, None)
    }

    pub fn clear_pattern(&mut self) -> Result<u64, String> {
        let mut candidate = self.pattern.clone();
        candidate.steps.fill(None);
        self.commit(candidate)
    }

    /// Replace the whole pattern (e.g. an accepted variation) as one
    /// undoable transaction. The pattern is validated first; on error the
    /// editor is unchanged.
    pub fn replace_pattern(&mut self, pattern: Pattern) -> Result<u64, String> {
        self.commit(pattern)
    }

    /// Replace the pattern's invariant profile as one undoable transaction.
    pub fn set_invariants(&mut self, profile: Option<InvariantProfile>) -> Result<u64, String> {
        let mut candidate = self.pattern.clone();
        candidate.invariants = profile;
        self.commit(candidate)
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.push_redo(self.pattern.clone());
        self.pattern = previous;
        self.revision = self.revision.saturating_add(1);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        self.push_undo(self.pattern.clone());
        self.pattern = next;
        self.revision = self.revision.saturating_add(1);
        true
    }

    pub fn queue_revision(
        &self,
        current_frame: u64,
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        boundary: QuantizeBoundary,
    ) -> Result<QuantizedChange<PatternRevision>, String> {
        self.pattern.validate()?;
        QuantizedChange::new(
            PatternRevision {
                revision: self.revision,
                pattern: self.pattern.clone(),
            },
            current_frame,
            sample_rate,
            bpm,
            steps_per_beat,
            boundary,
        )
    }

    fn ensure_step_index(&self, index: usize) -> Result<(), String> {
        if index >= self.pattern.steps.len() {
            return Err(format!(
                "step index {index} is outside pattern length {}",
                self.pattern.steps.len()
            ));
        }
        Ok(())
    }

    fn commit(&mut self, candidate: Pattern) -> Result<u64, String> {
        candidate.validate()?;
        self.push_undo(self.pattern.clone());
        self.redo.clear();
        self.pattern = candidate;
        self.revision = self.revision.saturating_add(1);
        Ok(self.revision)
    }

    fn push_undo(&mut self, pattern: Pattern) {
        if self.undo.len() == self.history_capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(pattern);
    }

    fn push_redo(&mut self, pattern: Pattern) {
        if self.redo.len() == self.history_capacity {
            self.redo.pop_front();
        }
        self.redo.push_back(pattern);
    }
}

/// Move per-step locks and anchor invariants with their steps. `map` takes
/// a zero-based step and returns its new index, or `None` to drop it.
fn remap_step_metadata(pattern: &mut Pattern, map: impl Fn(usize) -> Option<usize>) {
    let locks = std::mem::take(&mut pattern.locks);
    for mut entry in locks {
        if let Some(step) = map(usize::from(entry.step)) {
            entry.step = step as u16;
            pattern.locks.push(entry);
        }
    }
    pattern.locks.sort_by_key(|entry| entry.step);
    if let Some(profile) = pattern.invariants.as_mut() {
        for rule in &mut profile.rules {
            if let crate::InvariantKind::AnchorSteps { steps } = &mut rule.kind {
                let mut remapped: Vec<u16> = steps
                    .iter()
                    .filter_map(|step| map(usize::from(*step).saturating_sub(1)))
                    .map(|step| step as u16 + 1)
                    .collect();
                remapped.sort_unstable();
                remapped.dedup();
                *steps = remapped;
            }
        }
        // An anchor rule left with no steps would no longer validate.
        profile.rules.retain(|rule| {
            !matches!(&rule.kind, crate::InvariantKind::AnchorSteps { steps } if steps.is_empty())
        });
        if profile.rules.is_empty() {
            pattern.invariants = None;
        }
    }
}
