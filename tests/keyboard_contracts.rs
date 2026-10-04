use shelloop::{map_performance_key, shift_octave, PerformanceKey};

#[test]
fn lower_keyboard_row_maps_chromatic_notes_from_middle_c() {
    assert_eq!(map_performance_key('z', 0), Some(PerformanceKey::Note(60)));
    assert_eq!(map_performance_key('s', 0), Some(PerformanceKey::Note(61)));
    assert_eq!(map_performance_key('x', 0), Some(PerformanceKey::Note(62)));
    assert_eq!(map_performance_key('m', 0), Some(PerformanceKey::Note(71)));
}

#[test]
fn upper_keyboard_row_maps_the_next_octave() {
    assert_eq!(map_performance_key('q', 0), Some(PerformanceKey::Note(72)));
    assert_eq!(map_performance_key('2', 0), Some(PerformanceKey::Note(73)));
    assert_eq!(map_performance_key('u', 0), Some(PerformanceKey::Note(83)));
}

#[test]
fn octave_shift_is_bounded_and_changes_note_mapping() {
    assert_eq!(shift_octave(0, 1), 1);
    assert_eq!(shift_octave(3, 1), 3);
    assert_eq!(shift_octave(-2, -1), -2);
    assert_eq!(map_performance_key('z', 1), Some(PerformanceKey::Note(72)));
    assert_eq!(map_performance_key('z', -2), Some(PerformanceKey::Note(36)));
}

#[test]
fn special_performance_keys_are_available_without_stealing_note_keys() {
    assert_eq!(
        map_performance_key('[', 0),
        Some(PerformanceKey::OctaveDown)
    );
    assert_eq!(map_performance_key(']', 0), Some(PerformanceKey::OctaveUp));
    assert_eq!(
        map_performance_key(' ', 0),
        Some(PerformanceKey::TogglePlay)
    );
    assert_eq!(map_performance_key('!', 0), Some(PerformanceKey::Panic));
    assert_eq!(map_performance_key('~', 0), Some(PerformanceKey::Quit));
    assert_eq!(map_performance_key('?', 0), None);
}
