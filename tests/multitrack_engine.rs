use shelloop::{
    MultiTrackEngine, Pattern, PatternStep, TrackDefinition, TrackId, TrackKind, MAX_REALTIME_TRACKS,
};

fn pattern(name: &str, seed: u64, note: u8, steps: usize) -> Pattern {
    let mut contents = vec![None; steps];
    contents[0] = Some(PatternStep {
        note,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    });
    Pattern::new(name, seed, 0.0, 0, contents).unwrap()
}

fn synth_track(id: u16, name: &str, note: u8, steps: usize) -> TrackDefinition {
    TrackDefinition {
        id: TrackId(id),
        name: name.into(),
        kind: TrackKind::Synth,
        gain: 1.0,
        pan: 0.0,
        muted: false,
        soloed: false,
        pattern: pattern(name, u64::from(id), note, steps),
    }
}

#[test]
fn multitrack_validates_stable_unique_ids_and_bounds() {
    let duplicate = vec![
        synth_track(1, "a", 60, 4),
        synth_track(1, "b", 62, 4),
    ];
    assert!(MultiTrackEngine::new(48_000, 120.0, 4, 99, 8, duplicate).is_err());

    let too_many = (0..=MAX_REALTIME_TRACKS)
        .map(|index| synth_track(index as u16 + 1, &format!("t{index}"), 60, 4))
        .collect();
    assert!(MultiTrackEngine::new(48_000, 120.0, 4, 99, 8, too_many).is_err());
}

#[test]
fn independent_track_lengths_advance_on_one_shared_frame_clock() {
    let tracks = vec![
        synth_track(10, "fifteen", 60, 15),
        synth_track(20, "sixteen", 67, 16),
    ];
    let mut engine = MultiTrackEngine::new(48_000, 120.0, 4, 99, 8, tracks).unwrap();

    for _ in 0..12_345 {
        let (left, right) = engine.next_stereo_frame();
        assert!(left.is_finite() && right.is_finite());
    }

    assert_eq!(engine.track_position(TrackId(10)), Some(12_345));
    assert_eq!(engine.track_position(TrackId(20)), Some(12_345));
}

#[test]
fn mute_and_solo_follow_explicit_truth_table() {
    let tracks = vec![
        synth_track(1, "left", 60, 4),
        synth_track(2, "right", 67, 4),
    ];
    let mut engine = MultiTrackEngine::new(48_000, 120.0, 4, 99, 8, tracks).unwrap();

    assert!(engine.track_is_audible(TrackId(1)));
    assert!(engine.track_is_audible(TrackId(2)));

    engine.set_mute(TrackId(1), true).unwrap();
    assert!(!engine.track_is_audible(TrackId(1)));
    assert!(engine.track_is_audible(TrackId(2)));

    engine.set_solo(TrackId(1), true).unwrap();
    assert!(!engine.track_is_audible(TrackId(1)), "mute wins over solo");
    assert!(!engine.track_is_audible(TrackId(2)), "a soloed track suppresses non-solo tracks");

    engine.set_mute(TrackId(1), false).unwrap();
    assert!(engine.track_is_audible(TrackId(1)));
    assert!(!engine.track_is_audible(TrackId(2)));
}

#[test]
fn gain_pan_and_missing_track_updates_are_bounded_and_safe() {
    let mut engine = MultiTrackEngine::new(
        48_000,
        120.0,
        4,
        99,
        8,
        vec![synth_track(7, "lead", 72, 4)],
    )
    .unwrap();

    engine.set_gain(TrackId(7), 0.5).unwrap();
    engine.set_pan(TrackId(7), -1.0).unwrap();
    assert!(engine.set_gain(TrackId(999), 0.5).is_err());
    assert!(engine.set_pan(TrackId(7), 2.0).is_err());

    let mut saw_nonzero = false;
    for _ in 0..128 {
        let (left, right) = engine.next_stereo_frame();
        assert!(left.is_finite() && right.is_finite());
        if left.abs() > 0.0001 {
            saw_nonzero = true;
        }
        assert!(right.abs() < 0.0001);
    }
    assert!(saw_nonzero);
}

#[test]
fn restart_and_panic_apply_to_every_track_without_changing_ids() {
    let tracks = vec![
        synth_track(3, "bass", 36, 4),
        synth_track(9, "lead", 72, 7),
    ];
    let mut engine = MultiTrackEngine::new(44_100, 128.0, 4, 1234, 8, tracks).unwrap();

    for _ in 0..500 {
        engine.next_stereo_frame();
    }
    assert_eq!(engine.track_position(TrackId(3)), Some(500));
    assert_eq!(engine.track_position(TrackId(9)), Some(500));

    engine.restart_all();
    assert_eq!(engine.track_position(TrackId(3)), Some(0));
    assert_eq!(engine.track_position(TrackId(9)), Some(0));

    engine.panic_all();
    assert_eq!(engine.track_ids(), vec![TrackId(3), TrackId(9)]);
}

#[test]
fn single_track_engine_preserves_centered_stereo_output() {
    let mut engine = MultiTrackEngine::new(
        48_000,
        120.0,
        4,
        77,
        8,
        vec![synth_track(1, "single", 60, 4)],
    )
    .unwrap();

    for _ in 0..64 {
        let (left, right) = engine.next_stereo_frame();
        assert!((left - right).abs() < 0.0001);
    }
}
