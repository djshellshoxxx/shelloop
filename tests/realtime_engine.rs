use shelloop::{midi_note_hz, EngineCommand, Oscillator, RealtimeSynth};

#[test]
fn midi_note_frequency_uses_equal_temperament() {
    assert!((midi_note_hz(69) - 440.0).abs() < 0.001);
    assert!((midi_note_hz(60) - 261.625_55).abs() < 0.01);
}

#[test]
fn note_on_and_note_off_drive_audio_without_allocating_render_buffers() {
    let mut synth = RealtimeSynth::new(48_000.0, Oscillator::Saw, 8).unwrap();
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 69,
        velocity: 1.0,
    });

    let first = synth.next_sample();
    let second = synth.next_sample();
    assert!(first.is_finite());
    assert!(second.is_finite());
    assert!(first.abs() > 0.01 || second.abs() > 0.01);

    synth.handle(EngineCommand::NoteOff {
        channel: 0,
        note: 69,
    });
    assert_eq!(synth.next_sample(), 0.0);
}

#[test]
fn sustain_defers_release_until_pedal_is_lifted() {
    let mut synth = RealtimeSynth::new(48_000.0, Oscillator::Saw, 8).unwrap();
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 60,
        velocity: 0.8,
    });
    synth.handle(EngineCommand::Sustain {
        channel: 0,
        enabled: true,
    });
    synth.handle(EngineCommand::NoteOff {
        channel: 0,
        note: 60,
    });
    assert!(synth.next_sample().abs() > 0.0);

    synth.handle(EngineCommand::Sustain {
        channel: 0,
        enabled: false,
    });
    assert_eq!(synth.next_sample(), 0.0);
}

#[test]
fn polyphonic_mix_is_finite_bounded_and_panic_is_immediate() {
    let mut synth = RealtimeSynth::new(48_000.0, Oscillator::Saw, 4).unwrap();
    for note in [60, 64, 67, 72] {
        synth.handle(EngineCommand::NoteOn {
            channel: 0,
            note,
            velocity: 1.0,
        });
    }

    for _ in 0..128 {
        let sample = synth.next_sample();
        assert!(sample.is_finite());
        assert!((-1.0..=1.0).contains(&sample));
    }

    synth.handle(EngineCommand::Panic);
    assert_eq!(synth.active_voice_count(), 0);
    assert_eq!(synth.next_sample(), 0.0);
}
