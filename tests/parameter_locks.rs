use shelloop::{
    parse_pattern_json, CompiledPattern, CompiledPatternRevision, CompiledSynthPatch, LockTarget,
    MultiTrackEngine, Oscillator, ParameterLock, Pattern, PatternStep, QuantizedChange,
    SampleParamId, SynthParamId, SynthPatch, TrackDefinition, TrackId, TrackKind, TrackParamId,
    MAX_LOCKS_PER_STEP,
};

const RATE: u32 = 48_000;
// 120 BPM, 4 steps per beat: 6000 frames per step.
const STEP: u64 = 6_000;

fn note(probability: f32, ratchets: u8) -> PatternStep {
    PatternStep {
        note: 48,
        velocity: 0.8,
        gate: 0.5,
        probability,
        ratchets,
        microtiming_frames: 0,
    }
}

fn cutoff(value: f32) -> ParameterLock {
    ParameterLock {
        target: LockTarget::Synth(SynthParamId::FilterCutoff),
        value,
    }
}

fn base_patch() -> SynthPatch {
    let mut patch = SynthPatch::legacy(Oscillator::Saw);
    patch.filter.mode = shelloop::FilterMode::LowPass;
    patch.filter.cutoff_hz = 1_000.0;
    patch
}

fn engine_with(pattern: Pattern) -> MultiTrackEngine {
    let mut track = TrackDefinition::new(TrackId(1), "bass", TrackKind::Synth, pattern);
    track.synth_patch = Some(base_patch());
    MultiTrackEngine::new(RATE, 120.0, 4, 7, 8, vec![track]).unwrap()
}

fn render(engine: &mut MultiTrackEngine, frames: u64) {
    for _ in 0..frames {
        engine.next_stereo_frame();
    }
}

fn current_cutoff(engine: &MultiTrackEngine) -> f32 {
    engine
        .track_synth_patch(TrackId(1))
        .unwrap()
        .filter
        .cutoff_hz
}

fn pattern(steps: Vec<Option<PatternStep>>) -> Pattern {
    Pattern::new("locks", 1, 0.0, 0, steps).unwrap()
}

#[test]
fn lock_applies_before_note_on_and_restores_at_next_step() {
    let mut p = pattern(vec![Some(note(1.0, 1)), None, Some(note(1.0, 1)), None]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    p.validate().unwrap();
    let mut engine = engine_with(p);

    // Frame 0 renders step 0: lock is applied in the same frame as note-on.
    render(&mut engine, 1);
    assert_eq!(current_cutoff(&engine), 4_800.0);
    assert_eq!(engine.track_lock_count(TrackId(1)), Some(1));
    render(&mut engine, STEP - 1);
    assert_eq!(current_cutoff(&engine), 4_800.0);
    // Step 1 boundary restores base.
    render(&mut engine, 1);
    assert_eq!(current_cutoff(&engine), 1_000.0);
    assert_eq!(engine.track_lock_count(TrackId(1)), Some(0));
}

#[test]
fn base_change_during_lock_becomes_restoration_target() {
    let mut p = pattern(vec![Some(note(1.0, 1)), None]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    let mut engine = engine_with(p);
    render(&mut engine, 10);
    let mut new_base = base_patch();
    new_base.filter.cutoff_hz = 2_000.0;
    assert!(engine.apply_synth_patch(
        TrackId(1),
        CompiledSynthPatch::new(RATE as f32, new_base).unwrap()
    ));
    assert_eq!(current_cutoff(&engine), 4_800.0, "lock keeps sounding");
    render(&mut engine, STEP);
    assert_eq!(current_cutoff(&engine), 2_000.0, "restores to newest base");
}

#[test]
fn consecutive_locks_transition_without_base_glitch() {
    let mut p = pattern(vec![Some(note(1.0, 1)), Some(note(1.0, 1))]);
    p.set_step_locks(0, vec![cutoff(4_000.0)]);
    p.set_step_locks(1, vec![cutoff(6_000.0)]);
    let mut engine = engine_with(p);
    for frame in 0..(STEP * 2) {
        engine.next_stereo_frame();
        let value = current_cutoff(&engine);
        assert_ne!(value, 1_000.0, "base value leaked at frame {frame}");
    }
}

#[test]
fn trigless_lock_changes_parameter_without_note() {
    let mut p = pattern(vec![None, None]);
    p.set_step_locks(
        0,
        vec![ParameterLock {
            target: LockTarget::Track(TrackParamId::Gain),
            value: 0.25,
        }],
    );
    let mut engine = engine_with(p);
    render(&mut engine, 1);
    assert_eq!(engine.track_effective_gain(TrackId(1)), Some(0.25));
    assert_eq!(engine.track_base_gain(TrackId(1)), Some(1.0));
    render(&mut engine, STEP);
    assert_eq!(engine.track_effective_gain(TrackId(1)), Some(1.0));
}

#[test]
fn probability_false_suppresses_trigger_and_locks() {
    let mut p = pattern(vec![Some(note(0.0, 1)), None]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    let mut engine = engine_with(p);
    render(&mut engine, 100);
    assert_eq!(current_cutoff(&engine), 1_000.0);
    assert_eq!(engine.track_lock_count(TrackId(1)), Some(0));
}

#[test]
fn ratchets_keep_one_lock_set() {
    let mut p = pattern(vec![Some(note(1.0, 4)), None]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    let mut engine = engine_with(p);
    for _ in 0..STEP {
        engine.next_stereo_frame();
        assert!(engine.track_lock_count(TrackId(1)).unwrap() <= 1);
    }
    assert_eq!(current_cutoff(&engine), 4_800.0);
}

#[test]
fn pattern_replacement_clears_old_locks_at_boundary() {
    let mut p = pattern(vec![Some(note(1.0, 1)); 4]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    let mut engine = engine_with(p);
    render(&mut engine, 10);
    assert_eq!(current_cutoff(&engine), 4_800.0);
    let replacement = pattern(vec![None; 4]);
    engine
        .queue_pattern_revision(
            TrackId(1),
            QuantizedChange {
                apply_at_frame: 100,
                value: CompiledPatternRevision {
                    revision: 1,
                    pattern: CompiledPattern::from_pattern(&replacement).unwrap(),
                },
            },
        )
        .unwrap();
    render(&mut engine, 90);
    assert_eq!(current_cutoff(&engine), 4_800.0, "not before the boundary");
    render(&mut engine, 1);
    assert_eq!(current_cutoff(&engine), 1_000.0);
}

#[test]
fn stop_and_panic_restore_base_while_pause_freezes() {
    let mut p = pattern(vec![Some(note(1.0, 1)), None]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    let mut engine = engine_with(p.clone());
    render(&mut engine, 10);
    engine.set_playing_all(false);
    render(&mut engine, STEP * 2);
    assert_eq!(current_cutoff(&engine), 4_800.0, "pause freezes");
    engine.panic_all();
    assert_eq!(current_cutoff(&engine), 1_000.0, "panic restores");

    let mut engine = engine_with(p);
    render(&mut engine, 10);
    engine.restart_all();
    assert_eq!(current_cutoff(&engine), 1_000.0, "restart restores");
}

#[test]
fn lock_validation_enforces_bounds_and_lockability() {
    let mut p = pattern(vec![Some(note(1.0, 1))]);
    p.set_step_locks(
        0,
        (0..=MAX_LOCKS_PER_STEP)
            .map(|index| ParameterLock {
                target: LockTarget::Effect {
                    slot: shelloop::EffectSlotId((index % 4) as u8),
                    param: shelloop::EffectParamId((index / 4) as u8),
                },
                value: 0.5,
            })
            .collect(),
    );
    assert!(p.validate().is_err(), "17 locks rejected");

    let mut p = pattern(vec![Some(note(1.0, 1))]);
    p.set_step_locks(
        0,
        vec![ParameterLock {
            target: LockTarget::Synth(SynthParamId::Oscillator),
            value: 1.0,
        }],
    );
    assert!(p.validate().is_err(), "structural parameter rejected");

    let mut p = pattern(vec![Some(note(1.0, 1))]);
    p.set_step_locks(0, vec![cutoff(100.0), cutoff(200.0)]);
    assert!(p.validate().is_err(), "duplicate target rejected");

    let mut p = pattern(vec![Some(note(1.0, 1))]);
    p.set_step_locks(
        0,
        vec![ParameterLock {
            target: LockTarget::Track(TrackParamId::Mute),
            value: 1.0,
        }],
    );
    assert!(p.validate().is_err(), "mute is not lockable");
}

#[test]
fn locks_persist_in_pattern_json_and_legacy_json_still_loads() {
    let legacy = r#"{"name":"x","seed":1,"swing":0.0,"channel":0,"steps":[null]}"#;
    let pattern = parse_pattern_json(legacy).unwrap();
    assert!(pattern.locks.is_empty());
    assert!(!serde_json::to_string(&pattern).unwrap().contains("locks"));

    let mut locked = pattern.clone();
    locked.set_step_locks(
        0,
        vec![ParameterLock {
            target: LockTarget::Sample(SampleParamId::Pitch),
            value: -12.0,
        }],
    );
    let json = serde_json::to_string(&locked).unwrap();
    assert!(json.contains(r#""target":{"sample":"pitch"}"#), "{json}");
    assert_eq!(parse_pattern_json(&json).unwrap(), locked);
}

#[test]
fn locked_output_is_identical_across_block_partitions() {
    let mut p = pattern(vec![Some(note(1.0, 2)), None, Some(note(0.5, 1)), None]);
    p.set_step_locks(0, vec![cutoff(4_800.0)]);
    p.set_step_locks(
        1,
        vec![ParameterLock {
            target: LockTarget::Track(TrackParamId::Pan),
            value: -0.5,
        }],
    );
    let mut a = engine_with(p.clone());
    let mut b = engine_with(p);
    let mut out_a = Vec::new();
    for _ in 0..(STEP * 6) {
        out_a.push(a.next_stereo_frame());
    }
    let mut out_b = Vec::new();
    let mut remaining = STEP * 6;
    let mut block = 1;
    while remaining > 0 {
        let take = block.min(remaining);
        for _ in 0..take {
            out_b.push(b.next_stereo_frame());
        }
        remaining -= take;
        block = block * 3 % 997 + 1;
    }
    assert_eq!(out_a, out_b);
}
