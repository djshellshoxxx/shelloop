use shelloop::{engine_command_from_midi, EngineCommand, MidiEvent};

#[test]
fn midi_note_events_route_to_realtime_synth_commands() {
    assert_eq!(
        engine_command_from_midi(MidiEvent::NoteOn {
            channel: 2,
            note: 64,
            velocity: 127,
        }),
        Some(EngineCommand::NoteOn {
            channel: 2,
            note: 64,
            velocity: 1.0,
        })
    );

    assert_eq!(
        engine_command_from_midi(MidiEvent::NoteOff {
            channel: 2,
            note: 64,
            velocity: 90,
        }),
        Some(EngineCommand::NoteOff {
            channel: 2,
            note: 64,
        })
    );
}

#[test]
fn midi_sustain_and_all_notes_off_route_to_engine_controls() {
    assert_eq!(
        engine_command_from_midi(MidiEvent::ControlChange {
            channel: 3,
            controller: 64,
            value: 127,
        }),
        Some(EngineCommand::Sustain {
            channel: 3,
            enabled: true,
        })
    );
    assert_eq!(
        engine_command_from_midi(MidiEvent::ControlChange {
            channel: 3,
            controller: 64,
            value: 0,
        }),
        Some(EngineCommand::Sustain {
            channel: 3,
            enabled: false,
        })
    );
    assert_eq!(
        engine_command_from_midi(MidiEvent::ControlChange {
            channel: 3,
            controller: 123,
            value: 0,
        }),
        Some(EngineCommand::Panic)
    );
    assert_eq!(
        engine_command_from_midi(MidiEvent::ControlChange {
            channel: 3,
            controller: 120,
            value: 0,
        }),
        Some(EngineCommand::Panic)
    );
}

#[test]
fn unsupported_live_midi_messages_are_ignored() {
    assert_eq!(
        engine_command_from_midi(MidiEvent::ControlChange {
            channel: 0,
            controller: 1,
            value: 100,
        }),
        None
    );
    assert_eq!(
        engine_command_from_midi(MidiEvent::PitchBend {
            channel: 0,
            value: 4096,
        }),
        None
    );
}
