use shelloop::{EngineCommand, LiveSequencer, Pattern, PatternStep, QuantizeBoundary, TempoClock};

#[test]
fn tempo_clock_preserves_fractional_beat_phase_and_retimes_future_grid() {
    let mut clock = TempoClock::new(48_000, 120.0).unwrap();
    let old = clock;
    clock.set_bpm_at(240.0, 18_000).unwrap();
    assert_eq!(clock.beat_at_frame(18_000), 0.75);
    assert_eq!(
        clock
            .next_boundary(18_000, 4, QuantizeBoundary::Beat)
            .unwrap(),
        21_000
    );
    assert_eq!(clock.frame_at_beat(old.beat_at_frame(24_000)), 21_000);
    let before = clock;
    assert!(clock.set_bpm_at(f64::NAN, 18_000).is_err());
    assert_eq!(clock, before);
}

#[test]
fn sequencer_tempo_change_preserves_position_and_pending_note_release() {
    let step = PatternStep {
        note: 60,
        velocity: 1.0,
        gate: 1.0,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    };
    let pattern = Pattern::new("tempo", 1, 0.0, 0, vec![Some(step), None, None, None]).unwrap();
    let mut sequencer = LiveSequencer::new(48_000, 120.0, 4, 1, pattern).unwrap();
    let mut commands = Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME);
    for _ in 0..3_000 {
        sequencer.fill_commands(&mut commands);
    }
    sequencer.set_tempo(240.0).unwrap();
    assert_eq!(sequencer.position_frame(), 3_000);
    for _ in 3_000..4_500 {
        sequencer.fill_commands(&mut commands);
        assert!(!commands
            .iter()
            .any(|c| matches!(c, EngineCommand::NoteOff { .. })));
    }
    sequencer.fill_commands(&mut commands);
    assert!(commands
        .iter()
        .any(|c| matches!(c, EngineCommand::NoteOff { note: 60, .. })));
}

#[test]
fn repeated_fractional_tempo_changes_preserve_phase_to_sample_precision() {
    let mut clock = TempoClock::new(44_100, 127.3).unwrap();
    for (frame, bpm) in [
        (12_345, 91.7),
        (52_000, 233.1),
        (88_777, 63.25),
        (123_456, 127.3),
    ] {
        let beat = clock.beat_at_frame(frame);
        clock.set_bpm_at(bpm, frame).unwrap();
        assert!((clock.beat_at_frame(frame) - beat).abs() < 1e-10);
        let boundary = clock
            .next_boundary(frame, 4, QuantizeBoundary::Beat)
            .unwrap();
        assert!(boundary >= frame);
        assert_eq!(boundary, clock.frame_at_beat(beat.ceil()));
    }
}
