use shelloop::{
    AdsrEnvelope, AdsrParams, EngineCommand, EnvelopeStage, FilterMode, FilterParams,
    Oscillator, RealtimeSynth, StateVariableFilter, SynthPatch, SynthVoice,
};

#[test]
fn adsr_envelope_moves_through_exact_linear_stages() {
    let params = AdsrParams::new(0.02, 0.02, 0.5, 0.02).unwrap();
    let mut env = AdsrEnvelope::new(100.0, params).unwrap();

    env.note_on();
    assert_eq!(env.stage(), EnvelopeStage::Attack);
    assert!((env.next_value() - 0.5).abs() < 0.0001);
    assert!((env.next_value() - 1.0).abs() < 0.0001);
    assert_eq!(env.stage(), EnvelopeStage::Decay);
    assert!((env.next_value() - 0.75).abs() < 0.0001);
    assert!((env.next_value() - 0.5).abs() < 0.0001);
    assert_eq!(env.stage(), EnvelopeStage::Sustain);

    env.note_off();
    assert_eq!(env.stage(), EnvelopeStage::Release);
    assert!((env.next_value() - 0.25).abs() < 0.0001);
    assert_eq!(env.next_value(), 0.0);
    assert_eq!(env.stage(), EnvelopeStage::Idle);
}

#[test]
fn adsr_zero_time_stages_and_mid_attack_release_are_safe() {
    let instant = AdsrParams::new(0.0, 0.0, 1.0, 0.0).unwrap();
    let mut env = AdsrEnvelope::new(48_000.0, instant).unwrap();
    env.note_on();
    assert_eq!(env.stage(), EnvelopeStage::Sustain);
    assert_eq!(env.next_value(), 1.0);
    env.note_off();
    assert_eq!(env.stage(), EnvelopeStage::Idle);
    assert_eq!(env.next_value(), 0.0);

    let params = AdsrParams::new(0.04, 0.0, 1.0, 0.02).unwrap();
    let mut env = AdsrEnvelope::new(100.0, params).unwrap();
    env.note_on();
    assert!((env.next_value() - 0.25).abs() < 0.0001);
    env.note_off();
    assert!((env.next_value() - 0.125).abs() < 0.0001);
    assert_eq!(env.next_value(), 0.0);
    assert!(!env.is_active());
}

#[test]
fn adsr_validation_rejects_nonfinite_and_out_of_range_values() {
    assert!(AdsrParams::new(-0.1, 0.1, 0.5, 0.1).is_err());
    assert!(AdsrParams::new(0.1, 31.0, 0.5, 0.1).is_err());
    assert!(AdsrParams::new(0.1, 0.1, 1.1, 0.1).is_err());
    assert!(AdsrParams::new(0.1, 0.1, 0.5, 61.0).is_err());
    assert!(AdsrParams::new(f32::NAN, 0.1, 0.5, 0.1).is_err());
}

#[test]
fn state_variable_filter_stays_finite_at_supported_extremes() {
    for mode in [
        FilterMode::LowPass,
        FilterMode::HighPass,
        FilterMode::BandPass,
    ] {
        for resonance in [0.0, 1.0] {
            let params = FilterParams {
                mode,
                cutoff_hz: 20_000.0,
                resonance,
                key_tracking: 0.0,
            };
            let mut filter = StateVariableFilter::new(48_000.0, params).unwrap();

            for index in 0..10_000 {
                let input = if index == 0 { 1.0 } else { 0.0 };
                let sample = filter.process(input);
                assert!(sample.is_finite());
                assert!(sample.abs() < 100.0);
            }
        }
    }
}

#[test]
fn filter_validation_enforces_nyquist_safe_cutoff_and_ranges() {
    let valid = FilterParams {
        mode: FilterMode::LowPass,
        cutoff_hz: 19_000.0,
        resonance: 1.0,
        key_tracking: 1.0,
    };
    assert!(valid.validate(48_000.0).is_ok());

    assert!(FilterParams {
        cutoff_hz: 10.0,
        ..valid
    }
    .validate(48_000.0)
    .is_err());
    assert!(FilterParams {
        cutoff_hz: 21_601.0,
        ..valid
    }
    .validate(48_000.0)
    .is_err());
    assert!(FilterParams {
        resonance: 1.1,
        ..valid
    }
    .validate(48_000.0)
    .is_err());
}

#[test]
fn synth_patch_validation_covers_tuning_pulse_width_and_gain() {
    let patch = SynthPatch::legacy(Oscillator::Saw);
    assert!(patch.validate(48_000.0).is_ok());

    assert!(SynthPatch {
        pulse_width: 0.0,
        ..patch
    }
    .validate(48_000.0)
    .is_err());
    assert!(SynthPatch {
        octave: 5,
        ..patch
    }
    .validate(48_000.0)
    .is_err());
    assert!(SynthPatch {
        fine_cents: 101.0,
        ..patch
    }
    .validate(48_000.0)
    .is_err());
}

#[test]
fn patched_synth_voice_has_release_tail_and_eventually_idles() {
    let mut patch = SynthPatch::legacy(Oscillator::Sine);
    patch.amp_env = AdsrParams::new(0.0, 0.0, 1.0, 0.01).unwrap();
    let mut voice = SynthVoice::with_patch(1_000.0, patch).unwrap();

    voice.note_on(100.0, 1.0);
    for _ in 0..5 {
        voice.next_sample();
    }
    voice.note_off();

    let release: Vec<f32> = (0..10).map(|_| voice.next_sample()).collect();
    assert!(release.iter().any(|sample| sample.abs() > 0.0001));
    assert!(!voice.is_active());
    assert_eq!(voice.next_sample(), 0.0);
}

#[test]
fn realtime_synth_continues_rendering_after_allocator_note_off_until_release_finishes() {
    let mut patch = SynthPatch::legacy(Oscillator::Sine);
    patch.amp_env = AdsrParams::new(0.0, 0.0, 1.0, 0.02).unwrap();
    let mut synth = RealtimeSynth::new_with_patch(1_000.0, patch, 4).unwrap();

    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 69,
        velocity: 1.0,
    });
    for _ in 0..5 {
        synth.next_sample();
    }

    synth.handle(EngineCommand::NoteOff {
        channel: 0,
        note: 69,
    });
    assert_eq!(synth.active_voice_count(), 0);

    let tail: Vec<f32> = (0..20).map(|_| synth.next_sample()).collect();
    assert!(tail.iter().any(|sample| sample.abs() > 0.0001));
    for _ in 0..10 {
        assert_eq!(synth.next_sample(), 0.0);
    }
}
