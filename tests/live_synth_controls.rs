use shelloop::{
    parse_synth_parameter_command, CompiledSynthPatch, EngineCommand, Oscillator, RealtimeSynth,
    SynthParamId, SynthParamValue, SynthPatch, SynthVoice,
};

#[test]
fn parameter_updates_are_typed_and_atomic() {
    let patch = SynthPatch::legacy(Oscillator::Saw);
    let changed = patch
        .with_parameter(
            SynthParamId::OutputGain,
            SynthParamValue::Number(0.4),
            48_000.0,
        )
        .unwrap();
    assert_eq!(changed.output_gain, 0.4);
    assert_eq!(patch.output_gain, 1.0);
    for value in [f32::NAN, f32::INFINITY, -1.0, 3.0] {
        assert!(patch
            .with_parameter(
                SynthParamId::OutputGain,
                SynthParamValue::Number(value),
                48_000.0
            )
            .is_err());
    }
    assert!(patch
        .with_parameter(SynthParamId::Octave, SynthParamValue::Number(0.5), 48_000.0)
        .is_err());
    assert!(patch
        .with_parameter(
            SynthParamId::Oscillator,
            SynthParamValue::Number(1.0),
            48_000.0
        )
        .is_err());
}

#[test]
fn gain_change_preserves_held_voice_and_reaches_target_after_five_ms() {
    let mut voice = SynthVoice::with_patch(1_000.0, SynthPatch::legacy(Oscillator::Pulse)).unwrap();
    voice.note_on(1.0, 1.0);
    assert_eq!(voice.next_sample(), 1.0);
    let mut patch = voice.patch();
    patch.output_gain = 0.0;
    assert!(voice.apply_patch(CompiledSynthPatch::new(1_000.0, patch).unwrap()));
    let samples: Vec<_> = (0..5).map(|_| voice.next_sample()).collect();
    assert!(samples[0] > 0.0 && samples[0] < 1.0);
    assert!(samples.windows(2).all(|pair| pair[1] <= pair[0]));
    assert_eq!(samples[4], 0.0);
    assert!(voice.is_active());
}

#[test]
fn compiled_patch_rate_mismatch_does_not_modify_synth() {
    let mut synth = RealtimeSynth::new(48_000.0, Oscillator::Sine, 4).unwrap();
    let patch = SynthPatch::legacy(Oscillator::Pulse);
    assert!(!synth.apply_patch(CompiledSynthPatch::new(44_100.0, patch).unwrap()));
    assert_eq!(synth.patch().oscillator, Oscillator::Sine);
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 69,
        velocity: 0.8,
    });
    assert!(synth.apply_patch(CompiledSynthPatch::new(48_000.0, patch).unwrap()));
    assert_eq!(synth.active_voice_count(), 1);
    assert_eq!(synth.patch(), patch);
    synth.handle(EngineCommand::Panic);
    assert_eq!(synth.next_sample(), 0.0);
}

#[test]
fn command_parser_uses_stable_parameter_names() {
    assert_eq!(
        parse_synth_parameter_command("synth filter_cutoff 1200").unwrap(),
        (SynthParamId::FilterCutoff, SynthParamValue::Number(1200.0))
    );
    assert_eq!(
        parse_synth_parameter_command("synth oscillator triangle").unwrap(),
        (
            SynthParamId::Oscillator,
            SynthParamValue::Oscillator(Oscillator::Triangle)
        )
    );
    for command in [
        "synth",
        "synth output_gain NaN",
        "synth cutoff 100",
        "synth oscillator unknown",
        "synth output_gain 1 extra",
    ] {
        assert!(parse_synth_parameter_command(command).is_err());
    }
}

#[test]
fn held_notes_retune_without_resetting_the_envelope() {
    let mut patch = SynthPatch::legacy(Oscillator::Sine);
    patch.amp_env.attack_secs = 0.1;
    let mut voice = SynthVoice::with_patch(48_000.0, patch).unwrap();
    voice.note_on(100.0, 0.8);
    for _ in 0..6000 {
        voice.next_sample();
    }
    patch.octave = 1;
    assert!(voice.apply_patch(CompiledSynthPatch::new(48_000.0, patch).unwrap()));
    for _ in 0..240 {
        voice.next_sample();
    }
    let mut previous = voice.next_sample();
    let mut rising = 0;
    for _ in 0..48_000 {
        let sample = voice.next_sample();
        if previous <= 0.0 && sample > 0.0 {
            rising += 1;
        }
        previous = sample;
    }
    assert!(
        (199..=201).contains(&rising),
        "expected octave-up 200 Hz, got {rising}"
    );
    assert_eq!(voice.patch(), patch);
}

#[test]
fn changing_a_patch_during_release_does_not_restart_its_tail() {
    let mut patch = SynthPatch::legacy(Oscillator::Pulse);
    patch.amp_env.release_secs = 0.01;
    let mut voice = SynthVoice::with_patch(1_000.0, patch).unwrap();
    voice.note_on(1.0, 1.0);
    voice.next_sample();
    voice.note_off();
    for _ in 0..4 {
        voice.next_sample();
    }
    patch.amp_env.release_secs = 1.0;
    assert!(voice.apply_patch(CompiledSynthPatch::new(1_000.0, patch).unwrap()));
    for _ in 0..6 {
        voice.next_sample();
    }
    assert!(!voice.is_active());
    voice.note_on(1.0, 1.0);
    voice.note_off();
    for _ in 0..10 {
        voice.next_sample();
    }
    assert!(voice.is_active(), "new notes use the new release duration");
}

#[test]
fn live_filter_modulation_stays_finite_at_extremes() {
    for rate in [32_000.0, 44_100.0, 48_000.0, 96_000.0] {
        let mut voice = SynthVoice::with_patch(rate, SynthPatch::legacy(Oscillator::Saw)).unwrap();
        voice.note_on(440.0, 1.0);
        for mode in [
            shelloop::FilterMode::LowPass,
            shelloop::FilterMode::HighPass,
            shelloop::FilterMode::BandPass,
        ] {
            for resonance in [0.0, 1.0] {
                for cutoff in [20.0, shelloop::max_cutoff(rate)] {
                    let mut patch = voice.patch();
                    patch.filter.mode = mode;
                    patch.filter.cutoff_hz = cutoff;
                    patch.filter.resonance = resonance;
                    patch.filter_env_amount = 8.0;
                    patch.filter.key_tracking = 1.0;
                    assert!(voice.apply_patch(CompiledSynthPatch::new(rate, patch).unwrap()));
                    for _ in 0..1000 {
                        let sample = voice.next_sample();
                        assert!(sample.is_finite() && sample.abs() <= 1.0);
                    }
                }
            }
        }
    }
}

#[test]
fn live_patch_changes_are_isolated_to_the_addressed_track() {
    let mut project: shelloop::MultiTrackProject =
        serde_json::from_str(include_str!("../projects/example-multitrack.json")).unwrap();
    project.tracks.truncate(2);
    project.tracks[0].pan = -1.0;
    project.tracks[1].pan = 1.0;
    let mut engine = shelloop::MultiTrackEngine::from_snapshot(
        48_000,
        project.bpm,
        4,
        project.seed,
        8,
        project.to_snapshot().unwrap(),
    )
    .unwrap();
    for _ in 0..512 {
        engine.next_stereo_frame();
    }
    let mut reference = engine.clone();
    let mut patch = SynthPatch::legacy(Oscillator::Saw);
    patch.output_gain = 0.0;
    let compiled = CompiledSynthPatch::new(48_000.0, patch).unwrap();
    assert!(!engine.apply_synth_patch(shelloop::TrackId(999), compiled));
    assert!(!engine.apply_synth_patch(
        shelloop::TrackId(1),
        CompiledSynthPatch::new(44_100.0, patch).unwrap()
    ));
    assert_eq!(engine.next_stereo_frame(), reference.next_stereo_frame());
    assert!(engine.apply_synth_patch(shelloop::TrackId(1), compiled));
    let mut audible = false;
    for index in 0..512 {
        let actual = engine.next_stereo_frame();
        let expected = reference.next_stereo_frame();
        assert!((actual.1 - expected.1).abs() < 0.00001);
        if index >= 240 {
            assert!(actual.0.abs() < 0.00001);
        }
        audible |= actual.1.abs() > 0.001;
    }
    assert!(audible);
}

#[test]
fn every_numeric_parameter_has_a_valid_dispatch_path() {
    let patch = SynthPatch::legacy(Oscillator::Saw);
    for (id, field, nested, value) in [
        (SynthParamId::Octave, "octave", "", 1.0),
        (SynthParamId::Semitone, "semitone", "", 7.0),
        (SynthParamId::FineCents, "fine_cents", "", 15.0),
        (SynthParamId::PulseWidth, "pulse_width", "", 0.25),
        (SynthParamId::AmpAttack, "amp_env", "attack_secs", 0.25),
        (SynthParamId::AmpDecay, "amp_env", "decay_secs", 0.25),
        (SynthParamId::AmpSustain, "amp_env", "sustain", 0.25),
        (SynthParamId::AmpRelease, "amp_env", "release_secs", 0.25),
        (SynthParamId::FilterCutoff, "filter", "cutoff_hz", 1200.0),
        (SynthParamId::FilterResonance, "filter", "resonance", 0.25),
        (SynthParamId::FilterKeytrack, "filter", "key_tracking", 0.25),
        (
            SynthParamId::FilterAttack,
            "filter_env",
            "attack_secs",
            0.25,
        ),
        (SynthParamId::FilterDecay, "filter_env", "decay_secs", 0.25),
        (SynthParamId::FilterSustain, "filter_env", "sustain", 0.25),
        (
            SynthParamId::FilterRelease,
            "filter_env",
            "release_secs",
            0.25,
        ),
        (SynthParamId::FilterEnvAmount, "filter_env_amount", "", 0.25),
        (SynthParamId::OutputGain, "output_gain", "", 0.25),
    ] {
        let updated = patch
            .with_parameter(id, SynthParamValue::Number(value), 48_000.0)
            .unwrap();
        let json = serde_json::to_value(updated).unwrap();
        let actual = if nested.is_empty() {
            &json[field]
        } else {
            &json[field][nested]
        };
        assert_eq!(actual.as_f64().unwrap(), f64::from(value));
    }
}
