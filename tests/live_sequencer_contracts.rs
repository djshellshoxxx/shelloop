use shelloop::{
    parse_pattern_json, CompiledPattern, EngineCommand, LiveSequencer, Pattern, PatternStep,
};

fn simple_pattern() -> Pattern {
    Pattern::new(
        "pulse",
        7,
        0.0,
        9,
        vec![
            Some(PatternStep {
                note: 60,
                velocity: 0.75,
                gate: 0.5,
                probability: 1.0,
                ratchets: 1,
                microtiming_frames: 0,
            }),
            None,
            None,
            None,
        ],
    )
    .unwrap()
}

#[test]
fn live_sequencer_emits_exact_note_on_and_gate_note_off() {
    let mut sequencer = LiveSequencer::new(48_000, 120.0, 4, 123, simple_pattern()).unwrap();
    let mut commands = Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME);

    sequencer.fill_commands(&mut commands);
    assert_eq!(
        commands,
        vec![EngineCommand::NoteOn {
            channel: 9,
            note: 60,
            velocity: 0.75,
        }]
    );

    commands.clear();
    for _ in 1..3_000 {
        sequencer.fill_commands(&mut commands);
        assert!(commands.is_empty());
    }
    sequencer.fill_commands(&mut commands);
    assert_eq!(
        commands,
        vec![EngineCommand::NoteOff {
            channel: 9,
            note: 60,
        }]
    );
}

#[test]
fn pause_preserves_position_and_restart_returns_to_frame_zero() {
    let mut sequencer = LiveSequencer::new(44_100, 120.0, 4, 123, simple_pattern()).unwrap();
    let mut commands = Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME);

    for _ in 0..100 {
        sequencer.fill_commands(&mut commands);
    }
    assert_eq!(sequencer.position_frame(), 100);

    sequencer.set_playing(false);
    for _ in 0..100 {
        sequencer.fill_commands(&mut commands);
    }
    assert_eq!(sequencer.position_frame(), 100);

    sequencer.restart();
    assert_eq!(sequencer.position_frame(), 0);
    assert!(sequencer.is_playing());
}

#[test]
fn json_pattern_loading_validates_the_pattern() {
    let json = r#"{
        "name":"json beat",
        "seed":44,
        "swing":0.1,
        "channel":2,
        "steps":[
            {"note":36,"velocity":1.0,"gate":0.25,"probability":1.0,"ratchets":1,"microtiming_frames":0},
            null
        ]
    }"#;

    let pattern = parse_pattern_json(json).unwrap();
    assert_eq!(pattern.name, "json beat");
    assert_eq!(pattern.steps.len(), 2);

    let invalid = json.replace("\"ratchets\":1", "\"ratchets\":0");
    assert!(parse_pattern_json(&invalid)
        .unwrap_err()
        .contains("ratchets"));
}

#[test]
fn compiled_pattern_replacement_preserves_transport_and_uses_new_events() {
    let old = Pattern::new(
        "old",
        1,
        0.0,
        0,
        vec![Some(PatternStep {
            note: 60,
            velocity: 1.0,
            gate: 0.5,
            probability: 1.0,
            ratchets: 1,
            microtiming_frames: 0,
        })],
    )
    .unwrap();
    let new = Pattern::new(
        "new",
        2,
        0.0,
        0,
        vec![Some(PatternStep {
            note: 72,
            velocity: 1.0,
            gate: 0.5,
            probability: 1.0,
            ratchets: 1,
            microtiming_frames: 0,
        })],
    )
    .unwrap();

    let mut sequencer = LiveSequencer::new(100, 60.0, 1, 99, old).unwrap();
    let mut commands = Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME);
    sequencer.fill_commands(&mut commands);
    assert!(commands
        .iter()
        .any(|command| matches!(command, EngineCommand::NoteOn { note: 60, .. })));

    for _ in 1..100 {
        sequencer.fill_commands(&mut commands);
    }
    assert_eq!(sequencer.position_frame(), 100);

    let compiled = CompiledPattern::from_pattern(&new).unwrap();
    sequencer.replace_compiled_pattern(compiled);
    assert_eq!(sequencer.position_frame(), 100);

    sequencer.fill_commands(&mut commands);
    assert!(commands
        .iter()
        .any(|command| matches!(command, EngineCommand::NoteOn { note: 72, .. })));
    assert!(!commands
        .iter()
        .any(|command| matches!(command, EngineCommand::NoteOn { note: 60, .. })));
}
