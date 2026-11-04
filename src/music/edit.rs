//! Pattern transforms used by the musical commands. Each function takes a
//! pattern by reference and returns a new, validated pattern; the controller
//! commits the result through the editor so undo and quantised revisions
//! keep working unchanged.

use crate::music::arp::Arpeggio;
use crate::music::euclid::euclidean;
use crate::music::scale::Scale;
use crate::pattern::{Pattern, PatternStep, StepChord};

pub const MAX_RATE_STEPS: usize = 16;

/// Default parameters for a step created by a musical command.
fn new_step(note: u8) -> PatternStep {
    PatternStep {
        note,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    }
}

/// Remove chords and ratios whose step is now empty or out of range, and
/// locks on removed steps, so the result validates.
fn prune(pattern: &mut Pattern) {
    let len = pattern.steps.len();
    let steps = &pattern.steps;
    pattern.chords.retain(|chord| {
        let index = usize::from(chord.step);
        index < len && steps[index].is_some()
    });
    pattern.ratios.retain(|ratio| {
        let index = usize::from(ratio.step);
        index < len && steps[index].is_some()
    });
    pattern.locks.retain(|entry| usize::from(entry.step) < len);
}

/// Snap every note (and chord voicing) to the scale.
pub fn quantize_notes(pattern: &Pattern, scale: Scale) -> Result<Pattern, String> {
    let mut next = pattern.clone();
    for step in next.steps.iter_mut().flatten() {
        step.note = scale.snap(step.note);
    }
    for chord in &mut next.chords {
        let index = usize::from(chord.step);
        let root = next.steps[index].map_or(0, |step| step.note);
        let mut snapped: Vec<u8> = Vec::with_capacity(chord.notes.len());
        for note in &chord.notes {
            let note = scale.snap(*note);
            if note != root && !snapped.contains(&note) {
                snapped.push(note);
            }
        }
        chord.notes = snapped;
    }
    next.chords.retain(|chord| !chord.notes.is_empty());
    next.validate()?;
    Ok(next)
}

/// Move every note by `semitones`. Fails, without changing anything, if any
/// note would leave MIDI 0..=127.
pub fn transpose_pattern(pattern: &Pattern, semitones: i32) -> Result<Pattern, String> {
    let shift = |note: u8| -> Result<u8, String> {
        let moved = i32::from(note) + semitones;
        u8::try_from(moved)
            .ok()
            .filter(|value| *value <= 127)
            .ok_or_else(|| format!("transpose by {semitones} moves note {note} out of range"))
    };
    let mut next = pattern.clone();
    for step in next.steps.iter_mut().flatten() {
        step.note = shift(step.note)?;
    }
    for chord in &mut next.chords {
        for note in &mut chord.notes {
            *note = shift(*note)?;
        }
    }
    next.validate()?;
    Ok(next)
}

/// Replace the pattern's rhythm with a Euclidean rhythm of `steps` steps,
/// `hits` onsets, rotated by `rotation`, all playing `note`.
pub fn apply_euclid(
    pattern: &Pattern,
    hits: usize,
    steps: usize,
    rotation: usize,
    note: u8,
) -> Result<Pattern, String> {
    let onsets = euclidean(hits, steps, rotation)?;
    let mut next = pattern.clone();
    next.steps = onsets
        .iter()
        .map(|hit| hit.then(|| new_step(note)))
        .collect();
    prune(&mut next);
    next.validate()?;
    Ok(next)
}

/// Write one chord at `index`: `notes[0]` becomes the step note and the
/// remaining notes are stored as the step's chord voicing.
pub fn apply_chord(pattern: &Pattern, index: usize, notes: &[u8]) -> Result<Pattern, String> {
    if index >= pattern.steps.len() {
        return Err(format!("step {} is beyond the pattern length", index + 1));
    }
    let Some(&root) = notes.first() else {
        return Err("chord has no notes".into());
    };
    let mut next = pattern.clone();
    let base = next.steps[index].unwrap_or_else(|| new_step(root));
    next.steps[index] = Some(PatternStep { note: root, ..base });
    next.chords.retain(|chord| usize::from(chord.step) != index);
    if notes.len() > 1 {
        next.chords.push(StepChord {
            step: index as u16,
            notes: notes[1..].to_vec(),
        });
        next.chords.sort_by_key(|chord| chord.step);
    }
    next.validate()?;
    Ok(next)
}

/// Write an arpeggio from `start` to the end of the pattern: one note every
/// `rate` steps, the rest silent. Chords and ratios in the range are cleared.
pub fn apply_arp(
    pattern: &Pattern,
    start: usize,
    arpeggio: &Arpeggio,
    rate: usize,
) -> Result<Pattern, String> {
    let len = pattern.steps.len();
    if start >= len {
        return Err(format!("step {} is beyond the pattern length", start + 1));
    }
    if !(1..=MAX_RATE_STEPS).contains(&rate) {
        return Err(format!(
            "arp rate must be between 1 and {MAX_RATE_STEPS} steps"
        ));
    }
    let mut next = pattern.clone();
    for index in start..len {
        let offset = index - start;
        next.steps[index] = offset
            .is_multiple_of(rate)
            .then(|| new_step(arpeggio.note_at(offset / rate)));
    }
    prune(&mut next);
    next.validate()?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::arp::ArpMode;
    use crate::music::scale::ScaleKind;
    use crate::params::TrackParamId;
    use crate::pattern::{LockTarget, ParameterLock, StepLocks};

    fn base(len: usize) -> Pattern {
        Pattern::new("p", 1, 0.0, 0, vec![None; len]).unwrap()
    }

    #[test]
    fn euclid_writes_onsets_and_prunes_chords() {
        let mut pattern = base(16);
        pattern = apply_chord(&pattern, 0, &[60, 64, 67]).unwrap();
        assert_eq!(pattern.chords.len(), 1);
        let next = apply_euclid(&pattern, 3, 8, 0, 36).unwrap();
        assert_eq!(next.steps.len(), 8);
        let onsets: Vec<usize> = next
            .steps
            .iter()
            .enumerate()
            .filter_map(|(i, step)| step.map(|_| i))
            .collect();
        assert_eq!(onsets, [0, 3, 6]);
        // Step 0 is still an onset, so its chord survives with the new root.
        assert_eq!(
            next.chords,
            vec![StepChord {
                step: 0,
                notes: vec![64, 67]
            }]
        );
        // A chord on a step that becomes silent is removed.
        let mut silent = base(16);
        silent = apply_chord(&silent, 1, &[60, 64]).unwrap();
        let cleared = apply_euclid(&silent, 3, 8, 0, 36).unwrap();
        assert!(cleared.chords.is_empty());
    }

    #[test]
    fn chord_writes_root_and_voicing() {
        let next = apply_chord(&base(8), 2, &[60, 64, 67]).unwrap();
        assert_eq!(next.steps[2].unwrap().note, 60);
        assert_eq!(
            next.chords,
            vec![StepChord {
                step: 2,
                notes: vec![64, 67]
            }]
        );
        assert!(apply_chord(&base(8), 8, &[60]).is_err());
        assert!(apply_chord(&base(8), 0, &[]).is_err());
    }

    #[test]
    fn arp_writes_every_rate_steps_from_start() {
        let arp = Arpeggio::new(&[60, 64, 67], ArpMode::Up, 1, 0).unwrap();
        let next = apply_arp(&base(8), 2, &arp, 2).unwrap();
        let notes: Vec<Option<u8>> = next.steps.iter().map(|s| s.map(|s| s.note)).collect();
        assert_eq!(
            notes,
            [None, None, Some(60), None, Some(64), None, Some(67), None]
        );
        assert!(apply_arp(&base(8), 2, &arp, 0).is_err());
        assert!(apply_arp(&base(8), 9, &arp, 1).is_err());
    }

    #[test]
    fn quantize_snaps_notes_and_chords() {
        let scale = Scale::new(0, ScaleKind::Major).unwrap();
        let mut pattern = base(4);
        pattern.steps[0] = Some(new_step(61)); // C# -> C
        pattern.steps[1] = Some(new_step(66)); // F# -> F
        pattern.chords.push(StepChord {
            step: 1,
            notes: vec![67, 70],
        }); // G, Bb -> G, B
        let next = quantize_notes(&pattern, scale).unwrap();
        assert_eq!(next.steps[0].unwrap().note, 60);
        assert_eq!(next.steps[1].unwrap().note, 65);
        assert_eq!(next.chords[0].notes, vec![67, 69]); // Bb snaps down to A
    }

    #[test]
    fn transpose_is_all_or_nothing() {
        let mut pattern = base(4);
        pattern.steps[0] = Some(new_step(60));
        pattern.steps[1] = Some(new_step(126));
        assert!(transpose_pattern(&pattern, 2).is_err());
        assert_eq!(
            transpose_pattern(&pattern, -12).unwrap().steps[0]
                .unwrap()
                .note,
            48
        );
        assert_eq!(
            transpose_pattern(&pattern, -12).unwrap().steps[1]
                .unwrap()
                .note,
            114
        );
    }

    #[test]
    fn locks_on_removed_steps_are_dropped() {
        let mut pattern = base(8);
        pattern.steps[7] = Some(new_step(60));
        pattern.locks.push(StepLocks {
            step: 7,
            locks: vec![ParameterLock {
                target: LockTarget::Track(TrackParamId::Gain),
                value: 0.5,
            }],
        });
        let next = apply_euclid(&pattern, 1, 4, 0, 60).unwrap();
        assert!(next.locks.is_empty());
    }
}
