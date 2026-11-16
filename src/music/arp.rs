//! Arpeggio sequences built from a chord. `note_at` is a pure function of
//! the note index, so the sequence is reproducible and random mode is
//! deterministic for a given seed.

use crate::music::humanize::splitmix64;

pub const MAX_ARP_OCTAVES: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
    Random,
}

impl ArpMode {
    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(match text.to_ascii_lowercase().as_str() {
            "up" => Self::Up,
            "down" => Self::Down,
            "updown" | "up-down" => Self::UpDown,
            "random" | "rand" => Self::Random,
            other => {
                return Err(format!(
                    "unknown arp mode: {other} (up, down, updown, random)"
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arpeggio {
    mode: ArpMode,
    seed: u64,
    /// One full cycle of notes (the pool for `Random`).
    cycle: Vec<u8>,
}

impl Arpeggio {
    /// Build an arpeggio over `chord` spread across `octaves` (1..=4).
    pub fn new(chord: &[u8], mode: ArpMode, octaves: u8, seed: u64) -> Result<Self, String> {
        if chord.is_empty() {
            return Err("arpeggio needs at least one note".into());
        }
        if !(1..=MAX_ARP_OCTAVES).contains(&octaves) {
            return Err(format!(
                "arp octaves must be between 1 and {MAX_ARP_OCTAVES}"
            ));
        }
        let mut pool = Vec::with_capacity(chord.len() * usize::from(octaves));
        for octave in 0..octaves {
            for note in chord {
                let shifted = u16::from(*note) + 12 * u16::from(octave);
                if shifted <= 127 {
                    pool.push(shifted as u8);
                }
            }
        }
        pool.sort_unstable();
        pool.dedup();
        let cycle = match mode {
            ArpMode::Up | ArpMode::Random => pool.clone(),
            ArpMode::Down => pool.iter().rev().copied().collect(),
            ArpMode::UpDown => {
                // Up, then back down without repeating the top and bottom
                // notes, so each cycle is a seamless loop.
                let mut cycle = pool.clone();
                if pool.len() > 2 {
                    cycle.extend(pool[1..pool.len() - 1].iter().rev());
                }
                cycle
            }
        };
        Ok(Self { mode, seed, cycle })
    }

    pub fn mode(&self) -> ArpMode {
        self.mode
    }

    /// Notes in one cycle (for display).
    pub fn cycle(&self) -> &[u8] {
        &self.cycle
    }

    /// The note played by the `index`-th arp step.
    pub fn note_at(&self, index: usize) -> u8 {
        match self.mode {
            ArpMode::Random => {
                let pick =
                    splitmix64(self.seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                self.cycle[(pick % self.cycle.len() as u64) as usize]
            }
            _ => self.cycle[index % self.cycle.len()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_down_and_updown_cycles() {
        let chord = [60, 64, 67];
        let up = Arpeggio::new(&chord, ArpMode::Up, 1, 0).unwrap();
        assert_eq!(up.cycle(), [60, 64, 67]);
        let down = Arpeggio::new(&chord, ArpMode::Down, 1, 0).unwrap();
        assert_eq!(down.cycle(), [67, 64, 60]);
        let updown = Arpeggio::new(&chord, ArpMode::UpDown, 1, 0).unwrap();
        assert_eq!(updown.cycle(), [60, 64, 67, 64]);
        assert_eq!(updown.note_at(4), 60);
    }

    #[test]
    fn octaves_extend_the_cycle_and_respect_midi_range() {
        let arp = Arpeggio::new(&[60, 64, 67], ArpMode::Up, 2, 0).unwrap();
        assert_eq!(arp.cycle(), [60, 64, 67, 72, 76, 79]);
        let high = Arpeggio::new(&[120, 124], ArpMode::Up, 4, 0).unwrap();
        assert_eq!(high.cycle(), [120, 124]);
        assert!(Arpeggio::new(&[60], ArpMode::Up, 0, 0).is_err());
        assert!(Arpeggio::new(&[60], ArpMode::Up, 5, 0).is_err());
        assert!(Arpeggio::new(&[], ArpMode::Up, 1, 0).is_err());
    }

    #[test]
    fn random_is_seeded_and_stays_in_the_pool() {
        let a = Arpeggio::new(&[60, 64, 67], ArpMode::Random, 2, 7).unwrap();
        let b = Arpeggio::new(&[60, 64, 67], ArpMode::Random, 2, 7).unwrap();
        let c = Arpeggio::new(&[60, 64, 67], ArpMode::Random, 2, 8).unwrap();
        let seq_a: Vec<u8> = (0..64).map(|i| a.note_at(i)).collect();
        let seq_b: Vec<u8> = (0..64).map(|i| b.note_at(i)).collect();
        let seq_c: Vec<u8> = (0..64).map(|i| c.note_at(i)).collect();
        assert_eq!(seq_a, seq_b);
        assert_ne!(seq_a, seq_c);
        assert!(seq_a.iter().all(|note| a.cycle().contains(note)));
    }

    #[test]
    fn mode_names_parse() {
        assert_eq!(ArpMode::parse("UpDown").unwrap(), ArpMode::UpDown);
        assert!(ArpMode::parse("sideways").is_err());
    }
}
