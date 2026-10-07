use shelloop::{
    compile_pattern_revision, EngineProjectSnapshot, MultiTrackEngine, MultiTrackProject, Pattern,
    PatternEditor, PatternStep, QuantizeBoundary, TrackCommand, TrackDefinition, TrackId,
    TrackKind, MAX_REALTIME_TRACKS, MULTITRACK_PROJECT_SCHEMA_VERSION,
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
        synth_patch: None,
        sample: None,
        pattern_library: Vec::new(),
        inserts: Vec::new(),
        send_a: 0.0,
        send_b: 0.0,
    }
}

#[test]
fn multitrack_validates_stable_unique_ids_and_bounds() {
    let duplicate = vec![synth_track(1, "a", 60, 4), synth_track(1, "b", 62, 4)];
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
    assert!(
        !engine.track_is_audible(TrackId(2)),
        "a soloed track suppresses non-solo tracks"
    );

    engine.set_mute(TrackId(1), false).unwrap();
    assert!(engine.track_is_audible(TrackId(1)));
    assert!(!engine.track_is_audible(TrackId(2)));
}

#[test]
fn gain_pan_and_missing_track_updates_are_bounded_and_safe() {
    let mut engine =
        MultiTrackEngine::new(48_000, 120.0, 4, 99, 8, vec![synth_track(7, "lead", 72, 4)])
            .unwrap();

    engine.set_gain(TrackId(7), 0.5).unwrap();
    engine.set_pan(TrackId(7), -1.0).unwrap();
    assert!(engine.set_gain(TrackId(999), 0.5).is_err());
    assert!(engine.set_pan(TrackId(7), 2.0).is_err());

    let mut final_frames = Vec::new();
    for index in 0..512 {
        let frame = engine.next_stereo_frame();
        assert!(frame.0.is_finite() && frame.1.is_finite());
        if index >= 448 {
            final_frames.push(frame);
        }
    }
    assert!(final_frames.iter().any(|(left, _)| left.abs() > 0.0001));
    assert!(final_frames.iter().all(|(_, right)| right.abs() < 0.0001));
}

#[test]
fn restart_and_panic_apply_to_every_track_without_changing_ids() {
    let tracks = vec![synth_track(3, "bass", 36, 4), synth_track(9, "lead", 72, 7)];
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

#[test]
fn immutable_project_snapshot_validates_before_engine_construction() {
    let snapshot = EngineProjectSnapshot::new(
        42,
        vec![
            synth_track(11, "drums", 36, 16),
            synth_track(12, "bass", 48, 15),
        ],
    )
    .unwrap();

    assert_eq!(snapshot.revision(), 42);
    assert_eq!(snapshot.tracks().len(), 2);

    let duplicate = EngineProjectSnapshot::new(
        43,
        vec![synth_track(11, "a", 60, 4), synth_track(11, "b", 62, 4)],
    );
    assert!(duplicate.is_err());
}

#[test]
fn typed_track_commands_update_only_the_addressed_track() {
    let snapshot = EngineProjectSnapshot::new(
        1,
        vec![synth_track(1, "one", 60, 4), synth_track(2, "two", 67, 4)],
    )
    .unwrap();
    let mut engine = MultiTrackEngine::from_snapshot(48_000, 120.0, 4, 5, 8, snapshot).unwrap();

    engine
        .apply_command(TrackCommand::SetMute {
            track: TrackId(2),
            muted: true,
        })
        .unwrap();
    assert!(engine.track_is_audible(TrackId(1)));
    assert!(!engine.track_is_audible(TrackId(2)));

    engine
        .apply_command(TrackCommand::SetPan {
            track: TrackId(1),
            pan: -1.0,
        })
        .unwrap();
    assert!(engine
        .apply_command(TrackCommand::SetGain {
            track: TrackId(999),
            gain: 1.0,
        })
        .is_err());
}

#[test]
fn multitrack_project_round_trip_preserves_tracks_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multitrack.json");
    let project = MultiTrackProject {
        schema_version: MULTITRACK_PROJECT_SCHEMA_VERSION,
        revision: 9,
        seed: 12345,
        bpm: 132.0,
        steps_per_beat: 4,
        tracks: vec![
            synth_track(1, "drums", 36, 16),
            synth_track(2, "bass", 48, 15),
        ],
        master_gain: 1.0,
        effects: Default::default(),
        scenes: Vec::new(),
        chains: Vec::new(),
        midi_mappings: Vec::new(),
    };

    project.save_atomic(&path).unwrap();
    let loaded = MultiTrackProject::load(&path).unwrap();
    assert_eq!(loaded, project);
    assert_eq!(loaded.to_snapshot().unwrap().revision(), 9);

    let mut invalid = loaded;
    invalid.schema_version += 1;
    assert!(invalid.validate().is_err());
}

#[test]
fn queued_pattern_revision_activates_on_exact_frame_for_only_target_track() {
    let tracks = vec![synth_track(1, "one", 60, 1), synth_track(2, "two", 67, 1)];
    let mut engine = MultiTrackEngine::new(100, 60.0, 1, 9, 8, tracks).unwrap();

    let replacement = pattern("replacement", 444, 72, 1);
    let mut editor = PatternEditor::new(replacement, 8).unwrap();
    editor.set_swing(0.1).unwrap();
    let queued = editor
        .queue_revision(1, 100, 60.0, 1, QuantizeBoundary::Step)
        .unwrap();
    let compiled = compile_pattern_revision(&queued).unwrap();

    engine.queue_pattern_revision(TrackId(1), compiled).unwrap();

    for _ in 0..100 {
        engine.next_stereo_frame();
    }
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(0));
    assert_eq!(engine.track_active_revision(TrackId(2)), Some(0));

    engine.next_stereo_frame();
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(1));
    assert_eq!(engine.track_active_revision(TrackId(2)), Some(0));
}

#[test]
fn pending_pattern_revision_slot_is_bounded_and_newest_revision_wins() {
    let mut engine =
        MultiTrackEngine::new(100, 60.0, 1, 9, 8, vec![synth_track(1, "one", 60, 1)]).unwrap();

    let mut first = PatternEditor::new(pattern("first", 1, 61, 1), 8).unwrap();
    first.set_swing(0.1).unwrap();
    let first = compile_pattern_revision(
        &first
            .queue_revision(1, 100, 60.0, 1, QuantizeBoundary::Step)
            .unwrap(),
    )
    .unwrap();

    let mut second = PatternEditor::new(pattern("second", 2, 62, 1), 8).unwrap();
    second.set_swing(0.2).unwrap();
    second.set_swing(0.3).unwrap();
    let second = compile_pattern_revision(
        &second
            .queue_revision(1, 100, 60.0, 1, QuantizeBoundary::Step)
            .unwrap(),
    )
    .unwrap();

    engine.queue_pattern_revision(TrackId(1), first).unwrap();
    engine.queue_pattern_revision(TrackId(1), second).unwrap();

    for _ in 0..=100 {
        engine.next_stereo_frame();
    }
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(2));
}

#[test]
fn late_compiled_revision_activates_on_next_rendered_frame() {
    let mut engine =
        MultiTrackEngine::new(100, 60.0, 1, 9, 8, vec![synth_track(1, "one", 60, 1)]).unwrap();

    for _ in 0..101 {
        engine.next_stereo_frame();
    }

    let mut editor = PatternEditor::new(pattern("late", 8, 75, 1), 8).unwrap();
    editor.set_swing(0.1).unwrap();
    let queued = editor
        .queue_revision(1, 100, 60.0, 1, QuantizeBoundary::Step)
        .unwrap();
    assert_eq!(queued.apply_at_frame, 100);

    engine
        .queue_pattern_revision(TrackId(1), compile_pattern_revision(&queued).unwrap())
        .unwrap();
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(0));

    engine.next_stereo_frame();
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(1));
}

#[test]
fn legacy_projects_keep_the_same_sound_without_an_embedded_patch() {
    let project: MultiTrackProject =
        serde_json::from_str(include_str!("../projects/example-multitrack.json")).unwrap();
    assert!(project
        .tracks
        .iter()
        .all(|track| track.synth_patch.is_none()));
    let original = synth_track(1, "legacy", 60, 4);
    let mut explicit = original.clone();
    explicit.synth_patch = Some(shelloop::SynthPatch::legacy(shelloop::Oscillator::Saw));
    let mut a = MultiTrackEngine::new(48_000, 120.0, 4, 5, 4, vec![original]).unwrap();
    let mut b = MultiTrackEngine::new(48_000, 120.0, 4, 5, 4, vec![explicit]).unwrap();
    for _ in 0..512 {
        assert_eq!(a.next_stereo_frame(), b.next_stereo_frame());
    }
}

#[test]
fn embedded_patch_controls_the_addressed_tracks_sound() {
    let mut silent = synth_track(1, "silent", 60, 4);
    let mut patch = shelloop::SynthPatch::legacy(shelloop::Oscillator::Pulse);
    patch.output_gain = 0.0;
    silent.synth_patch = Some(patch);
    silent.pan = -1.0;
    let mut audible = synth_track(2, "audible", 67, 4);
    audible.pan = 1.0;
    audible.synth_patch = Some(shelloop::SynthPatch::legacy(shelloop::Oscillator::Triangle));
    let mut engine = MultiTrackEngine::new(48_000, 120.0, 4, 5, 4, vec![silent, audible]).unwrap();
    let frames: Vec<_> = (0..512).map(|_| engine.next_stereo_frame()).collect();
    assert!(frames.iter().all(|frame| frame.0.abs() < 0.0001));
    assert!(frames.iter().any(|frame| frame.1.abs() > 0.001));
}

#[test]
fn embedded_patches_round_trip_and_reject_invalid_fields() {
    let mut project: MultiTrackProject =
        serde_json::from_str(include_str!("../projects/example-multitrack.json")).unwrap();
    let mut patch = shelloop::SynthPatch::legacy(shelloop::Oscillator::Pulse);
    patch.amp_env = shelloop::AdsrParams::new(0.01, 0.2, 0.6, 0.3).unwrap();
    patch.filter.mode = shelloop::FilterMode::LowPass;
    patch.filter.cutoff_hz = 1200.0;
    project.tracks[0].synth_patch = Some(patch);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("patched.json");
    project.save_atomic(&path).unwrap();
    assert_eq!(MultiTrackProject::load(&path).unwrap(), project);
    let mut json = serde_json::to_value(&project).unwrap();
    json["tracks"][0]["synth_patch"]["output_gian"] = serde_json::json!(1.0);
    assert!(serde_json::from_value::<MultiTrackProject>(json).is_err());
    patch.pulse_width = 2.0;
    project.tracks[0].synth_patch = Some(patch);
    assert!(project.validate().is_err());
    assert!(project.save_atomic(&path).is_err());
    assert!(MultiTrackProject::load(&path).unwrap().validate().is_ok());
}

#[test]
fn patch_cutoff_is_validated_against_the_actual_output_rate() {
    let mut track = synth_track(1, "cutoff", 60, 4);
    let mut patch = shelloop::SynthPatch::legacy(shelloop::Oscillator::Sine);
    patch.filter.cutoff_hz = 20_000.0;
    track.synth_patch = Some(patch);
    assert!(track.validate().is_ok());
    assert!(MultiTrackEngine::new(32_000, 120.0, 4, 5, 4, vec![track.clone()]).is_err());
    assert!(MultiTrackEngine::new(48_000, 120.0, 4, 5, 4, vec![track]).is_ok());
}

#[test]
fn example_synth_patches_load_and_render_at_common_device_rates() {
    let project: MultiTrackProject =
        serde_json::from_str(include_str!("../projects/example-synth-patches.json")).unwrap();
    project.validate().unwrap();
    for rate in [32_000, 44_100, 48_000, 96_000] {
        let mut engine = MultiTrackEngine::from_snapshot(
            rate,
            project.bpm,
            u32::from(project.steps_per_beat),
            project.seed,
            8,
            project.to_snapshot().unwrap(),
        )
        .unwrap();
        let mut energy = 0.0;
        for _ in 0..rate {
            let (left, right) = engine.next_stereo_frame();
            assert!(left.is_finite() && right.is_finite());
            assert!(left.abs() <= 1.0 && right.abs() <= 1.0);
            energy += left.abs() + right.abs();
        }
        assert!(energy > 1.0);
    }
}
