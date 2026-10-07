use shelloop::{
    decode_message, protect_master, ChannelStrip, MidiEvent, MidiPerformanceState, Project,
    SmoothedParam, Track,
};

#[test]
fn project_round_trip_preserves_valid_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("set.json");
    let project = Project {
        version: 1,
        bpm: 174.0,
        tracks: vec![Track {
            name: "drums".into(),
        }],
    };

    project.save_atomic(&path).unwrap();
    let loaded = Project::load(&path).unwrap();

    assert_eq!(loaded, project);
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn invalid_project_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    let project = Project {
        version: 1,
        bpm: 0.0,
        tracks: Vec::new(),
    };

    assert!(project.save_atomic(&path).is_err());
    assert!(!path.exists());
}

#[test]
fn midi_note_on_zero_velocity_decodes_as_note_off() {
    assert_eq!(
        decode_message(&[0x90, 60, 0]).unwrap(),
        MidiEvent::NoteOff {
            channel: 0,
            note: 60,
            velocity: 0,
        }
    );
}

#[test]
fn midi_decodes_channel_voice_messages_and_rejects_malformed_input() {
    assert_eq!(
        decode_message(&[0x92, 64, 100]).unwrap(),
        MidiEvent::NoteOn {
            channel: 2,
            note: 64,
            velocity: 100,
        }
    );
    assert_eq!(
        decode_message(&[0xB1, 64, 127]).unwrap(),
        MidiEvent::ControlChange {
            channel: 1,
            controller: 64,
            value: 127,
        }
    );
    assert!(decode_message(&[0x90, 60]).is_err());
    assert!(decode_message(&[0xF8]).is_err());
}

#[test]
fn midi_state_tracks_sustain_and_centered_pitch_bend() {
    let mut state = MidiPerformanceState::default();
    state.apply(decode_message(&[0xB3, 64, 127]).unwrap());
    assert!(state.sustain(3));
    state.apply(decode_message(&[0xB3, 64, 0]).unwrap());
    assert!(!state.sustain(3));

    state.apply(decode_message(&[0xE3, 0x00, 0x40]).unwrap());
    assert_eq!(state.pitch_bend(3), 0);
    state.apply(decode_message(&[0xE3, 0x7F, 0x7F]).unwrap());
    assert_eq!(state.pitch_bend(3), 8191);
}

#[test]
fn channel_strip_mutes_and_pans_without_nonfinite_output() {
    let center = ChannelStrip::default();
    let (left, right) = center.process_mono(0.5);
    assert!(left.is_finite() && right.is_finite());
    assert!((left - right).abs() < 0.0001);

    let hard_right = ChannelStrip {
        gain: 1.0,
        pan: 1.0,
        muted: false,
    };
    let (left, right) = hard_right.process_mono(0.5);
    assert!(left.abs() < 0.0001);
    assert!(right > 0.0);

    let muted = ChannelStrip {
        muted: true,
        ..ChannelStrip::default()
    };
    assert_eq!(muted.process_mono(1.0), (0.0, 0.0));
}

#[test]
fn master_protection_is_finite_and_bounded() {
    for sample in [-1000.0, -2.0, -0.5, 0.0, 0.5, 2.0, 1000.0, f32::NAN] {
        let protected = protect_master(sample);
        assert!(protected.is_finite());
        assert!(protected.abs() <= 1.0);
    }
}

#[test]
fn smoothed_parameter_reaches_target_without_overshoot() {
    let mut value = SmoothedParam::new(0.0);
    value.set_target(1.0, 4);

    assert!((value.next_value() - 0.25).abs() < 0.0001);
    assert!((value.next_value() - 0.50).abs() < 0.0001);
    assert!((value.next_value() - 0.75).abs() < 0.0001);
    assert!((value.next_value() - 1.00).abs() < 0.0001);
    assert_eq!(value.next_value(), 1.0);
    assert_eq!(value.target(), 1.0);
}

#[test]
fn stereo_balance_keeps_centre_unity_and_only_attenuates_the_far_side() {
    use shelloop::ChannelStrip;
    let strip = |pan: f32| ChannelStrip {
        gain: 1.0,
        pan,
        muted: false,
    };
    let (left, right) = strip(0.0).process_stereo(0.3, -0.2);
    assert!((left - 0.3_f32.tanh()).abs() < 1e-6);
    assert!((right - (-0.2_f32).tanh()).abs() < 1e-6);

    let (left, right) = strip(-1.0).process_stereo(0.3, 0.3);
    assert!((left - 0.3_f32.tanh()).abs() < 1e-6);
    assert_eq!(right, 0.0);

    let (left, right) = strip(0.5).process_stereo(0.3, 0.3);
    assert!((left - 0.15_f32.tanh()).abs() < 1e-6);
    assert!((right - 0.3_f32.tanh()).abs() < 1e-6);

    assert_eq!(strip(0.0).process_stereo(f32::NAN, 0.1), (0.0, 0.0));
    let muted = ChannelStrip {
        muted: true,
        ..strip(0.0)
    };
    assert_eq!(muted.process_stereo(0.5, 0.5), (0.0, 0.0));
}
