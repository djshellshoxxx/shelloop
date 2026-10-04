use shelloop::{reconnect_decision, select_midi_port_index, MidiPortSelector, ReconnectDecision};

#[test]
fn midi_port_selection_supports_first_index_and_exact_name() {
    let names = vec![
        "Launchkey MIDI".to_string(),
        "USB Keyboard".to_string(),
        "Virtual Port".to_string(),
    ];

    assert_eq!(
        select_midi_port_index(&names, &MidiPortSelector::First).unwrap(),
        0
    );
    assert_eq!(
        select_midi_port_index(&names, &MidiPortSelector::Index(2)).unwrap(),
        2
    );
    assert_eq!(
        select_midi_port_index(&names, &MidiPortSelector::Name("usb keyboard".to_string()))
            .unwrap(),
        1
    );
}

#[test]
fn midi_port_selection_rejects_missing_or_out_of_range_targets() {
    let names = vec!["Only Port".to_string()];

    assert!(select_midi_port_index(&[], &MidiPortSelector::First).is_err());
    assert!(select_midi_port_index(&names, &MidiPortSelector::Index(1)).is_err());
    assert!(
        select_midi_port_index(&names, &MidiPortSelector::Name("missing".to_string())).is_err()
    );
}

#[test]
fn reconnect_policy_only_reconnects_when_the_target_is_available() {
    assert_eq!(reconnect_decision(true, true), ReconnectDecision::Keep);
    assert_eq!(reconnect_decision(true, false), ReconnectDecision::Wait);
    assert_eq!(reconnect_decision(false, false), ReconnectDecision::Wait);
    assert_eq!(
        reconnect_decision(false, true),
        ReconnectDecision::Reconnect
    );
}
