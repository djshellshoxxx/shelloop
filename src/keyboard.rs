#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceKey {
    Note(u8),
    OctaveDown,
    OctaveUp,
    TogglePlay,
    Panic,
    Quit,
}

pub fn shift_octave(current: i8, delta: i8) -> i8 {
    current.saturating_add(delta).clamp(-2, 3)
}

pub fn map_performance_key(key: char, octave: i8) -> Option<PerformanceKey> {
    let special = match key {
        '[' => Some(PerformanceKey::OctaveDown),
        ']' => Some(PerformanceKey::OctaveUp),
        ' ' => Some(PerformanceKey::TogglePlay),
        '!' => Some(PerformanceKey::Panic),
        '~' => Some(PerformanceKey::Quit),
        _ => None,
    };
    if special.is_some() {
        return special;
    }

    let offset = match key.to_ascii_lowercase() {
        'z' => 0,
        's' => 1,
        'x' => 2,
        'd' => 3,
        'c' => 4,
        'v' => 5,
        'g' => 6,
        'b' => 7,
        'h' => 8,
        'n' => 9,
        'j' => 10,
        'm' => 11,
        'q' => 12,
        '2' => 13,
        'w' => 14,
        '3' => 15,
        'e' => 16,
        'r' => 17,
        '5' => 18,
        't' => 19,
        '6' => 20,
        'y' => 21,
        '7' => 22,
        'u' => 23,
        _ => return None,
    };

    let base = 60_i16 + i16::from(octave.clamp(-2, 3)) * 12;
    let note = base + offset;
    (0..=127)
        .contains(&note)
        .then_some(PerformanceKey::Note(note as u8))
}
