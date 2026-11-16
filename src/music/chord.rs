//! Chord voicings built from one root note. Pure data.

use crate::music::scale::note_name;

/// Largest chord this module produces (a dominant ninth is not supported;
/// the biggest voicing is a seventh with an octave-doubled root).
pub const MAX_CHORD_NOTES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordQuality {
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    Power,
    Major7,
    Minor7,
    Dominant7,
    Diminished7,
}

impl ChordQuality {
    pub const ALL: [ChordQuality; 11] = [
        Self::Major,
        Self::Minor,
        Self::Diminished,
        Self::Augmented,
        Self::Sus2,
        Self::Sus4,
        Self::Power,
        Self::Major7,
        Self::Minor7,
        Self::Dominant7,
        Self::Diminished7,
    ];

    /// Semitone offsets from the root, ascending.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Power => &[0, 7],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Diminished7 => &[0, 3, 6, 9],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Major => "maj",
            Self::Minor => "min",
            Self::Diminished => "dim",
            Self::Augmented => "aug",
            Self::Sus2 => "sus2",
            Self::Sus4 => "sus4",
            Self::Power => "5",
            Self::Major7 => "maj7",
            Self::Minor7 => "min7",
            Self::Dominant7 => "dom7",
            Self::Diminished7 => "dim7",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(match text.to_ascii_lowercase().as_str() {
            "maj" | "major" | "" => Self::Major,
            "min" | "minor" | "m" => Self::Minor,
            "dim" | "diminished" => Self::Diminished,
            "aug" | "augmented" => Self::Augmented,
            "sus2" => Self::Sus2,
            "sus4" => Self::Sus4,
            "5" | "power" => Self::Power,
            "maj7" | "major7" => Self::Major7,
            "min7" | "m7" | "minor7" => Self::Minor7,
            "dom7" | "7" => Self::Dominant7,
            "dim7" => Self::Diminished7,
            other => {
                return Err(format!(
                    "unknown chord quality: {other} (maj, min, dim, aug, sus2, sus4, 5, maj7, min7, dom7, dim7)"
                ))
            }
        })
    }
}

/// Notes of a chord voiced from `root` (a MIDI note), with `inversion`
/// lowest notes moved up an octave each. Ascending, all within 0..=127.
pub fn chord_notes(root: u8, quality: ChordQuality, inversion: u8) -> Result<Vec<u8>, String> {
    let intervals = quality.intervals();
    if usize::from(inversion) >= intervals.len() {
        return Err(format!(
            "inversion must be below {} for a {} chord",
            intervals.len(),
            quality.name()
        ));
    }
    let mut notes = Vec::with_capacity(intervals.len());
    for interval in intervals {
        let note = u16::from(root) + u16::from(*interval);
        if note > 127 {
            return Err(format!(
                "chord from {} runs above MIDI 127; choose a lower root",
                note_name(root)
            ));
        }
        notes.push(note as u8);
    }
    for _ in 0..inversion {
        let lowest = notes.remove(0);
        let raised = u16::from(lowest) + 12;
        if raised > 127 {
            return Err("inversion raises a note above MIDI 127".into());
        }
        notes.push(raised as u8);
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_major_triad_and_inversions() {
        assert_eq!(
            chord_notes(60, ChordQuality::Major, 0).unwrap(),
            [60, 64, 67]
        );
        assert_eq!(
            chord_notes(60, ChordQuality::Major, 1).unwrap(),
            [64, 67, 72]
        );
        assert_eq!(
            chord_notes(60, ChordQuality::Major, 2).unwrap(),
            [67, 72, 76]
        );
    }

    #[test]
    fn sevenths_have_four_notes_and_inversions_stay_ascending() {
        let notes = chord_notes(48, ChordQuality::Dominant7, 3).unwrap();
        assert_eq!(notes.len(), 4);
        assert!(notes.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(chord_notes(48, ChordQuality::Dominant7, 4).is_err());
    }

    #[test]
    fn range_is_checked() {
        assert!(chord_notes(126, ChordQuality::Major, 0).is_err());
        assert!(chord_notes(120, ChordQuality::Major7, 0).is_err());
        assert!(chord_notes(110, ChordQuality::Major, 2).is_ok()); // 117, 122, 126
    }

    #[test]
    fn quality_names_parse() {
        for quality in ChordQuality::ALL {
            assert_eq!(ChordQuality::parse(quality.name()).unwrap(), quality);
        }
        assert!(ChordQuality::parse("blorp").is_err());
    }
}
