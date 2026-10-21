//! Scheduler behaviour for chords, ratio conditions, humanize and live
//! retime. Pure data: no audio device.

use shelloop::{CompiledPattern, Humanize, Pattern, PatternScheduler, PatternStep, StepChord, StepRatio};

const RATE: u32 = 48_000;

fn step(note: u8) -> Option<PatternStep> {
    Some(PatternStep {
        note,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    })
}

fn pattern(steps: Vec<Option<PatternStep>>) -> Pattern {
    Pattern::new("musical", 7, 0.0, 0, steps).unwrap()
}

fn scheduler(bpm: f64) -> PatternScheduler {
    PatternScheduler::new(RATE, bpm, 4, 99).unwrap()
}

/// Every event over `total` frames, scheduled in blocks of `block` frames.
fn collect(
    schedule: &PatternScheduler,
    pattern: &Pattern,
    total: u64,
    block: u32,
) -> Vec<(u64, u8, u32, u32)> {
    let mut out = Vec::new();
    let mut events = Vec::new();
    let mut start = 0_u64;
    while start < total {
        schedule.schedule_block_into(pattern, start, block, &mut events);
        for event in &events {
            out.push((
                event.absolute_frame,
                event.note,
                event.velocity.to_bits(),
                event.duration_frames,
            ));
        }
        start += u64::from(block);
    }
    out.sort_unstable();
    out
}

#[test]
fn chord_notes_sound_on_the_same_frame_as_the_step() {
    let mut chord_pattern = pattern(vec![step(60), None, None, None]);
    chord_pattern.chords.push(StepChord {
        step: 0,
        notes: vec![64, 67],
    });
    let mut events = Vec::new();
    scheduler(120.0).schedule_block_into(&chord_pattern, 0, 1_000, &mut events);
    let notes: Vec<u8> = events.iter().map(|event| event.note).collect();
    assert_eq!(notes, [60, 64, 67]);
    assert!(events.iter().all(|event| event.absolute_frame == 0));
}

#[test]
fn compiled_and_editable_paths_agree_for_musical_data() {
    let mut source = pattern(vec![step(60), None, step(62), step(64)]);
    source.chords.push(StepChord { step: 0, notes: vec![67] });
    source.ratios.push(StepRatio { step: 2, play: 1, of: 2 });
    source.humanize = Humanize {
        timing_frames: 240,
        velocity: 0.2,
    };
    let compiled = CompiledPattern::from_pattern(&source).unwrap();
    let schedule = scheduler(140.0);
    let mut editable = Vec::new();
    let mut from_compiled = Vec::new();
    for block_start in (0..96_000u64).step_by(512) {
        schedule.schedule_block_into(&source, block_start, 512, &mut editable);
        schedule.schedule_compiled_block_into(&compiled, block_start, 512, &mut from_compiled);
        assert_eq!(editable, from_compiled, "block at {block_start}");
    }
}

#[test]
fn ratio_one_of_two_plays_every_other_loop() {
    let mut source = pattern(vec![step(60), None, None, None]);
    source.ratios.push(StepRatio { step: 0, play: 1, of: 2 });
    let schedule = scheduler(120.0);
    // One loop is four steps of 0.125 s at 120 BPM with 4 steps per beat: 24_000 frames.
    let loop_frames = u64::from(RATE) / 2;
    let events = collect(&schedule, &source, loop_frames * 4, 512);
    let loops_with_notes: Vec<u64> = (0..4)
        .filter(|loop_index| {
            events
                .iter()
                .any(|(frame, ..)| frame / loop_frames == *loop_index)
        })
        .collect();
    assert_eq!(loops_with_notes, [0, 2]);
}

#[test]
fn humanize_is_independent_of_block_size() {
    let mut source = pattern(vec![step(60), step(62), step(64), step(65)]);
    source.humanize = Humanize {
        timing_frames: 300,
        velocity: 0.25,
    };
    let schedule = scheduler(128.0);
    let whole = collect(&schedule, &source, 96_000, 96_000);
    let small = collect(&schedule, &source, 96_000, 37);
    assert_eq!(whole, small);
    assert!(!whole.is_empty());
}

#[test]
fn humanize_moves_onsets_and_velocities_within_bounds() {
    let plain = pattern(vec![step(60), step(62), step(64), step(65)]);
    let mut humanized = plain.clone();
    humanized.humanize = Humanize {
        timing_frames: 300,
        velocity: 0.25,
    };
    let schedule = scheduler(128.0);
    let base = collect(&schedule, &plain, 96_000, 4096);
    let jittered = collect(&schedule, &humanized, 96_000, 4096);
    assert_eq!(base.len(), jittered.len());
    let mut moved = 0;
    for (a, b) in base.iter().zip(&jittered) {
        let delta = b.0 as i64 - a.0 as i64;
        assert!(delta.abs() <= 300 + 1, "timing jitter {delta}");
        if delta != 0 {
            moved += 1;
        }
        let velocity_delta = f32::from_bits(b.2) - f32::from_bits(a.2);
        assert!(velocity_delta.abs() <= 0.25 + 1e-6);
        assert!((0.0..=1.0).contains(&f32::from_bits(b.2)));
    }
    assert!(moved > 0, "some onsets should move");
}

#[test]
fn retime_does_not_repeat_or_drop_events() {
    let source = pattern(vec![step(60), step(62), step(64), step(65)]);
    let before = scheduler(120.0);
    let change_at = 30_000_u64;
    let after = before.retimed(180.0, change_at).unwrap();
    let mut events = Vec::new();
    let mut seen = Vec::new();
    // Blocks before the change use the old grid; blocks after use the new one.
    // The sequencer only ever emits frames at or after its playhead, so each
    // grid contributes just its own side of the change point.
    for start in (0..change_at).step_by(480) {
        before.schedule_block_into(&source, start, 480, &mut events);
        seen.extend(
            events
                .iter()
                .filter(|event| event.absolute_frame < change_at)
                .map(|event| (event.absolute_frame, event.note)),
        );
    }
    for start in (change_at..change_at + 96_000).step_by(480) {
        after.schedule_block_into(&source, start, 480, &mut events);
        seen.extend(
            events
                .iter()
                .filter(|event| event.absolute_frame >= change_at)
                .map(|event| (event.absolute_frame, event.note)),
        );
    }
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no event may repeat");
    // The faster grid keeps producing notes after the change.
    assert!(seen.iter().filter(|(frame, _)| *frame >= change_at).count() > 0);
}

#[test]
fn retime_keeps_the_step_under_the_change_point() {
    let before = scheduler(120.0);
    let frames_per_step = before.frames_per_step();
    let at = (frames_per_step * 5.5).round() as u64;
    let after = before.retimed(240.0, at).unwrap();
    // Fractional step 5.5 at the change point, now a step lasting half as long.
    assert!((after.step_position(at) - before.step_position(at)).abs() < 1e-6);
    assert!((after.frames_per_step() - frames_per_step / 2.0).abs() < 1e-6);
    assert_eq!(after.bpm(), 240.0);
    assert!(before.retimed(500.0, at).is_err());
}

#[test]
fn default_scheduler_grid_is_unchanged() {
    let schedule = scheduler(120.0);
    assert_eq!(schedule.step_position(0), 0.0);
    assert_eq!(schedule.beat_index_at(0), Some(0));
    // Four steps per beat at 120 BPM: one beat is 24_000 frames.
    assert_eq!(schedule.beat_index_at(24_000), Some(1));
}
