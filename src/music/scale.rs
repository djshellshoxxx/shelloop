//! Scales and note quantisation. Pure data; `snap` maps any MIDI note onto
//! the nearest pitch in the scale (ties resolve downwards).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleKind {
    Chromatic,
    Major,
    Minor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    HarmonicMinor,
    Pentatonic,
    MinorPentatonic,
    Blues,
}

impl ScaleKind {
    pub const ALL: [ScaleKind; 12] = [
        Self::Chromatic,
        Self::Major,
        Self::Minor,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::Locrian,
        Self::HarmonicMinor,
        Self::Pentatonic,
        Self::MinorPentatonic,
        Self::Blues,
    ];

    /// Semitone offsets from the root, ascending, within one octave.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::Pentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Chromatic => "chromatic",
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Dorian => "dorian",
            Self::Phrygian => "phrygian",
            Self::Lydian => "lydian",
            Self::Mixolydian => "mixolydian",
            Self::Locrian => "locrian",
            Self::HarmonicMinor => "harmonic_minor",
            Self::Pentatonic => "pentatonic",
            Self::MinorPentatonic => "minor_pentatonic",
            Self::Blues => "blues",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(match text.to_ascii_lowercase().as_str() {
            "chromatic" => Self::Chromatic,
            "major" | "ionian" => Self::Major,
            "minor" | "aeolian" => Self::Minor,
            "dorian" => Self::Dorian,
            "phrygian" => Self::Phrygian,
            "lydian" => Self::Lydian,
            "mixolydian" => Self::Mixolydian,
            "locrian" => Self::Locrian,
            "harmonic_minor" | "harmonicminor" => Self::HarmonicMinor,
            "pentatonic" | "major_pentatonic" => Self::Pentatonic,
            "minor_pentatonic" => Self::MinorPentatonic,
            "blues" => Self::Blues,
            other => return Err(format!("unknown scale: {other} (type scale list)")),
        })
    }
}

impl fmt::Display for ScaleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A root pitch class (0 = C) and a scale kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    root: u8,
    kind: ScaleKind,
}

impl Scale {
    pub fn new(root: u8, kind: ScaleKind) -> Result<Self, String> {
        if root > 11 {
            return Err("scale root must be a pitch class 0..=11".into());
        }
        Ok(Self { root, kind })
    }

    pub fn root(&self) -> u8 {
        self.root
    }

    pub fn kind(&self) -> ScaleKind {
        self.kind
    }

    /// True when a MIDI note belongs to the scale (any octave).
    pub fn contains(&self, note: u8) -> bool {
        let pitch_class = (note % 12 + 12 - self.root) % 12;
        self.kind.intervals().contains(&pitch_class)
    }

    /// Nearest in-scale note. Ties go down. Notes stay within 0..=127.
    pub fn snap(&self, note: u8) -> u8 {
        let note = note.min(127);
        if self.contains(note) {
            return note;
        }
        // Every supported scale has a gap of at most 3 semitones, so a
        // search of six semitones always finds a member.
        for distance in 1..=6u8 {
            if note >= distance && self.contains(note - distance) {
                return note - distance;
            }
            if note + distance <= 127 && self.contains(note + distance) {
                return note + distance;
            }
        }
        note
    }
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Parse a pitch class: `0..=11` or a note name without octave (`C`, `F#`,
/// `Bb`).
pub fn parse_pitch_class(text: &str) -> Result<u8, String> {
    if let Ok(value) = text.parse::<u8>() {
        return if value <= 11 {
            Ok(value)
        } else {
            Err("pitch class must be 0..=11".into())
        };
    }
    let (letter, rest) = split_letter(text)?;
    let (accidental, rest) = split_accidental(rest);
    if !rest.is_empty() {
        return Err(format!("invalid pitch class: {text} (use C, F#, Bb or 0..11)"));
    }
    Ok((letter + accidental).rem_euclid(12) as u8)
}

/// Parse a MIDI note: `0..=127` or a name with octave, where `C4` is 60.
pub fn parse_note(text: &str) -> Result<u8, String> {
    if let Ok(value) = text.parse::<u8>() {
        return if value <= 127 {
            Ok(value)
        } else {
            Err("note must be 0..=127".into())
        };
    }
    let (letter, rest) = split_letter(text)?;
    let (accidental, rest) = split_accidental(rest);
    let octave: i32 = rest
        .parse()
        .map_err(|_| format!("invalid note: {text} (use C4, F#3 or 0..127)"))?;
    let midi = (octave + 1) * 12 + letter + accidental;
    if !(0..=127).contains(&midi) {
        return Err(format!("note {text} is outside 0..=127"));
    }
    Ok(midi as u8)
}

fn split_letter(text: &str) -> Result<(i32, &str), String> {
    let mut chars = text.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err("empty note name".into());
    };
    let semitone = match first.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(format!("invalid note name: {text}")),
    };
    Ok((semitone, &text[first.len_utf8()..]))
}

fn split_accidental(text: &str) -> (i32, &str) {
    if let Some(rest) = text.strip_prefix('#') {
        (1, rest)
    } else if let Some(rest) = text.strip_prefix('b') {
        (-1, rest)
    } else {
        (0, text)
    }
}

/// Display name for a MIDI note (`C4` = 60).
pub fn note_name(note: u8) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[usize::from(note % 12)],
        i32::from(note / 12) - 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_major_snaps_black_keys_down_and_up_by_ties() {
        let scale = Scale::new(0, ScaleKind::Major).unwrap();
        assert_eq!(scale.snap(61), 60); // C# between C and D: tie goes down
        assert_eq!(scale.snap(66), 65); // F# between F and G: down
        assert_eq!(scale.snap(60), 60);
        assert_eq!(scale.snap(127), 127 - 1); // G9 is out of range for G; 126 is F#
    }

    #[test]
    fn snap_stays_in_range_and_in_scale() {
        for kind in ScaleKind::ALL {
            for root in 0..12 {
                let scale = Scale::new(root, kind).unwrap();
                for note in 0..=127u8 {
                    let snapped = scale.snap(note);
                    assert!(snapped <= 127);
                    assert!(scale.contains(snapped), "{kind} root {root} note {note}");
                }
            }
        }
    }

    #[test]
    fn chromatic_is_identity() {
        let scale = Scale::new(3, ScaleKind::Chromatic).unwrap();
        for note in 0..=127u8 {
            assert_eq!(scale.snap(note), note);
        }
    }

    #[test]
    fn parses_names_and_numbers() {
        assert_eq!(parse_pitch_class("C").unwrap(), 0);
        assert_eq!(parse_pitch_class("F#").unwrap(), 6);
        assert_eq!(parse_pitch_class("Bb").unwrap(), 10);
        assert_eq!(parse_pitch_class("11").unwrap(), 11);
        assert!(parse_pitch_class("12").is_err());
        assert!(parse_pitch_class("H").is_err());
        assert_eq!(parse_note("C4").unwrap(), 60);
        assert_eq!(parse_note("A4").unwrap(), 69);
        assert_eq!(parse_note("Bb3").unwrap(), 58);
        assert_eq!(parse_note("C-1").unwrap(), 0);
        assert!(parse_note("G9").is_err());
        assert!(parse_note("128").is_err());
        assert_eq!(note_name(60), "C4");
    }

    #[test]
    fn scale_kind_names_round_trip() {
        for kind in ScaleKind::ALL {
            assert_eq!(ScaleKind::parse(kind.name()).unwrap(), kind);
        }
        assert!(ScaleKind::parse("klingon").is_err());
    }
}
